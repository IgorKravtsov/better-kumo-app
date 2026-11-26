import AVFoundation
import Daily
import Foundation
import Observation

struct AudioRoute: Hashable {
    let type: AudioDeviceType
    let label: String
}

struct CallTracks {
    let camera: VideoTrack?
    let screen: VideoTrack?
}

final class DailyMediaBridge: CallMedia, @unchecked Sendable {
    private let media: DailyMedia

    init(media: DailyMedia) {
        self.media = media
    }

    func join(attempt: UInt64, url: String, token: String, mic: Bool, camera: Bool, sink: CallMediaSink) {
        onMain { $0.join(attempt: attempt, url: url, token: token, mic: mic, camera: camera, sink: sink) }
    }

    func leave(attempt: UInt64) {
        onMain { $0.leave(attempt: attempt) }
    }

    func setInputs(attempt: UInt64, mic: Bool, camera: Bool) {
        onMain { $0.setInputs(attempt: attempt, mic: mic, camera: camera) }
    }

    func subscribe(attempt: UInt64, participantIds: [String]) {
        onMain { $0.subscribe(attempt: attempt, participantIds: Set(participantIds)) }
    }

    private func onMain(_ work: @escaping @MainActor (DailyMedia) -> Void) {
        let media = media
        DispatchQueue.main.async { MainActor.assumeIsolated { work(media) } }
    }
}

@MainActor
@Observable
final class DailyMedia: CallClientDelegate {
    private(set) var tracks: [String: CallTracks] = [:]
    private(set) var audioRoutes: [AudioRoute] = []
    private(set) var audioRoute: AudioDeviceType?

    @ObservationIgnored private var client: CallClient?
    @ObservationIgnored private var attempt: UInt64 = 0
    @ObservationIgnored private var sink: CallMediaSink?
    @ObservationIgnored private var chosenRoute: AudioDeviceType?
    @ObservationIgnored private var desiredRoute: AudioDeviceType?
    @ObservationIgnored private var routingClient: CallClient?
    @ObservationIgnored private var routeObserver: NSObjectProtocol?

    func join(attempt: UInt64, url: String, token: String, mic: Bool, camera: Bool, sink: CallMediaSink) {
        release()
        guard let address = URL(string: url) else {
            sink.joinFailed(code: "bad-url", message: "Bad call address")
            return
        }
        let c = CallClient()
        c.delegate = self
        client = c
        self.attempt = attempt
        // iOS can switch between already-connected outputs (Control Center) without a device-list update.
        routeObserver = NotificationCenter.default.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
            let reason = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt).flatMap(AVAudioSession.RouteChangeReason.init)
            MainActor.assumeIsolated { self?.routeChanged(reason) }
        }
        self.sink = sink
        Task {
            do {
                _ = try await c.updateSubscriptionProfiles(.set([.base: .set(.subscribed(false))]))
                let inputs = InputSettingsUpdate(camera: .set(.enabled(camera)), microphone: .set(.enabled(mic)))
                _ = try await c.join(url: address, token: MeetingToken(stringValue: token), settings: .init(inputs: .set(inputs)))
                if c === client { routeAudio() }
                try? await c.startRemoteParticipantsAudioLevelObserver(intervalMs: 200)
                try? await c.startLocalAudioLevelObserver(intervalMs: 200)
                sink.joined()
                if c === client { publish() }
            } catch {
                sink.joinFailed(code: "join", message: error.localizedDescription)
            }
        }
    }

    func leave(attempt: UInt64) {
        guard attempt == self.attempt else { return }
        release()
    }

    func setInputs(attempt: UInt64, mic: Bool, camera: Bool) {
        guard attempt == self.attempt, let c = client else { return }
        Task {
            try? await c.setInputEnabled(.microphone, mic)
            try? await c.setInputEnabled(.camera, camera)
        }
    }

    func subscribe(attempt: UInt64, participantIds: Set<String>) {
        guard attempt == self.attempt, let c = client else { return }
        var updates: [ParticipantID: Update<SubscriptionSettingsUpdate>] = [:]
        for p in c.participants.remote.values {
            updates[p.id] = .set(.subscribed(participantIds.contains(p.id.uuid.uuidString)))
        }
        guard !updates.isEmpty else { return }
        Task { _ = try? await c.updateSubscriptions(forParticipants: .set(.init(updates))) }
    }

    func setAudioRoute(_ type: AudioDeviceType) {
        chosenRoute = type
        routeAudio()
    }

    private func routeChanged(_ reason: AVAudioSession.RouteChangeReason?) {
        guard let c = client, routingClient !== c else { return }
        let active = c.audioDevice
        audioRoute = active
        // Plug and unplug go through routeAudio; any other switch to a listed output came from outside the app and becomes the choice.
        guard reason != .newDeviceAvailable, reason != .oldDeviceUnavailable, reason != .categoryChange,
              active != desiredRoute, audioRoutes.contains(where: { $0.type == active }) else { return }
        chosenRoute = active
        desiredRoute = active
    }

    private func routeAudio(_ devices: [(id: String, label: String)]? = nil) {
        guard let c = client else { return }
        let routes = (devices ?? c.availableDevices.audio.map { ($0.deviceID, $0.label) }).compactMap { d in
            AudioDeviceType(deviceID: d.id).map { AudioRoute(type: $0, label: d.label) }
        }
        let previous = Set(audioRoutes)
        // A headset that connects mid-call takes over, as it does for a phone call.
        if !previous.isEmpty, let fresh = [AudioDeviceType.wired, .bluetooth].first(where: { t in routes.contains { $0.type == t && !previous.contains($0) } }) {
            chosenRoute = fresh
        }
        let types = Set(routes.map(\.type))
        if let chosen = chosenRoute, !types.contains(chosen) { chosenRoute = nil }
        audioRoutes = routes
        desiredRoute = chosenRoute ?? autoRoute()
        applyRoute()
    }

    private func autoRoute() -> AudioDeviceType {
        let types = Set(audioRoutes.map(\.type))
        return [.wired, .bluetooth, .speakerphone].first { types.contains($0) } ?? .speakerphone
    }

    private func applyRoute() {
        guard let c = client, routingClient !== c else { return }
        routingClient = c
        Task {
            var applied: AudioDeviceType?
            while c === client, let wanted = desiredRoute, wanted != applied, c.audioDevice != wanted || c.preferredAudioDevice != wanted {
                applied = wanted
                do {
                    try await c.setPreferredAudioDevice(wanted)
                } catch {
                    guard c === client else { break }
                    guard desiredRoute == wanted else { continue }
                    guard chosenRoute == wanted else { break }
                    chosenRoute = nil
                    desiredRoute = autoRoute()
                }
            }
            if routingClient === c { routingClient = nil }
            if c === client { audioRoute = c.audioDevice }
        }
    }

    private func release() {
        if let c = client {
            c.delegate = nil
            Task { try? await c.leave() }
        }
        if let routeObserver { NotificationCenter.default.removeObserver(routeObserver) }
        routeObserver = nil
        client = nil
        sink = nil
        tracks = [:]
        audioRoutes = []
        audioRoute = nil
        chosenRoute = nil
        desiredRoute = nil
        routingClient = nil
    }

    private func publish() {
        guard let c = client, let sink else { return }
        let all = Array(c.participants.all.values)
        tracks = Dictionary(all.map { p in
            (p.id.uuid.uuidString, CallTracks(
                camera: p.media?.camera.state == .playable ? p.media?.camera.track : nil,
                screen: p.media?.screenVideo.state == .playable ? p.media?.screenVideo.track : nil
            ))
        }, uniquingKeysWith: { a, _ in a })
        sink.participants(participants: all.map { p in
            CallMediaParticipant(
                id: p.id.uuid.uuidString,
                userId: p.info.userID,
                userName: p.info.username,
                isLocal: p.info.isLocal,
                micOn: p.media?.microphone.state == .playable,
                hasCamera: p.media?.camera.state == .playable,
                hasScreen: p.media?.screenVideo.state == .playable
            )
        })
    }

    private func current(_ callClient: CallClient) -> CallMediaSink? {
        callClient === client ? sink : nil
    }

    nonisolated func callClient(_ callClient: CallClient, participantJoined participant: Participant) {
        MainActor.assumeIsolated { if current(callClient) != nil { publish() } }
    }

    nonisolated func callClient(_ callClient: CallClient, participantUpdated participant: Participant) {
        MainActor.assumeIsolated { if current(callClient) != nil { publish() } }
    }

    nonisolated func callClient(_ callClient: CallClient, participantLeft participant: Participant, withReason reason: ParticipantLeftReason) {
        MainActor.assumeIsolated { if current(callClient) != nil { publish() } }
    }

    nonisolated func callClient(_ callClient: CallClient, remoteParticipantsAudioLevel participantsAudioLevel: [ParticipantID: Float]) {
        let levels = participantsAudioLevel.map { CallAudioLevel(participantId: $0.key.uuid.uuidString, level: $0.value) }
        MainActor.assumeIsolated { current(callClient)?.audioLevels(levels: levels) }
    }

    nonisolated func callClient(_ callClient: CallClient, localAudioLevel audioLevel: Float) {
        MainActor.assumeIsolated {
            guard let sink = current(callClient) else { return }
            let id = callClient.participants.local.id.uuid.uuidString
            sink.audioLevels(levels: [CallAudioLevel(participantId: id, level: audioLevel)])
        }
    }

    nonisolated func callClient(_ callClient: CallClient, availableDevicesUpdated availableDevices: Devices) {
        let audio = availableDevices.audio.map { (id: $0.deviceID, label: $0.label) }
        MainActor.assumeIsolated { if current(callClient) != nil { routeAudio(audio) } }
    }

    nonisolated func callClient(_ callClient: CallClient, error: CallClientError) {
        let message = error.localizedDescription
        MainActor.assumeIsolated { current(callClient)?.error(message: message) }
    }

    nonisolated func callClient(_ callClient: CallClient, callStateUpdated state: CallState) {
        let left = state == .left
        MainActor.assumeIsolated { if left { current(callClient)?.left() } }
    }
}
