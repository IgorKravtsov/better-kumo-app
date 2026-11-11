import Daily
import Foundation
import Observation

enum CallPhase {
    case idle, waiting, joining, joined, failed
}

struct CallTile: Identifiable, Equatable {
    let id: ParticipantID
    let key: String
    let name: String
    let isLocal: Bool
    let camera: VideoTrack?
    let screen: VideoTrack?
    let micOn: Bool

    static func == (a: CallTile, b: CallTile) -> Bool {
        a.id == b.id && a.name == b.name && a.micOn == b.micOn && (a.camera == nil) == (b.camera == nil)
            && (a.screen == nil) == (b.screen == nil)
    }
}

@MainActor
@Observable
final class CallModel: CallClientDelegate {
    private(set) var phase: CallPhase = .idle
    private(set) var roomName = ""
    private(set) var tiles: [CallTile] = []
    private(set) var speaking: Set<ParticipantID> = []
    private(set) var lastSpeaker: ParticipantID?
    private(set) var micOn = false
    private(set) var cameraOn = false
    private(set) var error: String?

    @ObservationIgnored private var client: CallClient?
    @ObservationIgnored private var joinedURL: String?
    @ObservationIgnored private var tokenRetries = 0
    @ObservationIgnored weak var app: AppModel?

    private nonisolated static let speakingLevel: Float = 0.02

    func expectCall() {
        if phase == .idle || phase == .failed { phase = .waiting }
        if let info = app?.core.callInfo() { onCallInfo(info) }
    }

    func onCallInfo(_ info: CallInfo?) {
        guard let info else {
            if phase == .joined || phase == .joining { leaveClient() }
            if phase != .idle { phase = .waiting }
            return
        }
        if !info.roomName.isEmpty { roomName = info.roomName }
        guard phase != .idle else { return }
        if joinedURL == info.dailyUrl, client != nil {
            onPeersChanged()
            return
        }
        Task { await join(info) }
    }

    private func join(_ info: CallInfo) async {
        leaveClient()
        guard let url = URL(string: info.dailyUrl) else { return }
        let c = CallClient()
        c.delegate = self
        client = c
        joinedURL = info.dailyUrl
        phase = .joining
        error = nil
        do {
            _ = try await c.updateSubscriptionProfiles(.set([.base: .set(.subscribed(false))]))
            let inputs = InputSettingsUpdate(
                camera: .set(.enabled(cameraOn)),
                microphone: .set(.enabled(micOn))
            )
            _ = try await c.join(
                url: url,
                token: MeetingToken(stringValue: info.dailyToken),
                settings: .init(inputs: .set(inputs))
            )
            tokenRetries = 0
            try? await c.setPreferredAudioDevice(.speakerphone)
            try? await c.startRemoteParticipantsAudioLevelObserver(intervalMs: 200)
            try? await c.startLocalAudioLevelObserver(intervalMs: 200)
            phase = .joined
            onPeersChanged()
        } catch {
            leaveClient()
            if tokenRetries < 3 {
                tokenRetries += 1
                phase = .waiting
                app?.core.refreshCallToken()
            } else {
                phase = .failed
                self.error = error.localizedDescription
            }
        }
    }

    func leave() {
        leaveClient()
        phase = .idle
        tiles = []
        speaking = []
        roomName = ""
    }

    private func leaveClient() {
        if let c = client {
            Task { try? await c.leave() }
        }
        client = nil
        joinedURL = nil
    }

    func toggleMic() {
        micOn.toggle()
        let on = micOn
        Task { try? await client?.setInputEnabled(.microphone, on) }
    }

    func toggleCamera() {
        cameraOn.toggle()
        let on = cameraOn
        Task { try? await client?.setInputEnabled(.camera, on) }
    }

    func onPeersChanged() {
        if let name = app?.core.callInfo()?.roomName, !name.isEmpty { roomName = name }
        guard let c = client, let app else { return }
        let peers = Set(app.core.callPeers())
        let remotes = c.participants.remote.values
        var updates: [ParticipantID: Update<SubscriptionSettingsUpdate>] = [:]
        for p in remotes {
            updates[p.id] = .set(.subscribed(peers.contains(p.info.userID ?? "")))
        }
        if !updates.isEmpty {
            Task { _ = try? await c.updateSubscriptions(forParticipants: .set(.init(updates))) }
        }
        refreshTiles()
    }

    private func refreshTiles() {
        guard let c = client, let app else { return }
        let peers = Set(app.core.callPeers())
        tiles = c.participants.all.values
            .filter { $0.info.isLocal || peers.contains($0.info.userID ?? "") }
            .map { p in
                let key = p.info.userID ?? ""
                return CallTile(
                    id: p.id,
                    key: key,
                    name: app.person(key: key)?.name ?? p.info.username ?? "Guest",
                    isLocal: p.info.isLocal,
                    camera: p.media?.camera.state == .playable ? p.media?.camera.track : nil,
                    screen: p.media?.screenVideo.state == .playable ? p.media?.screenVideo.track : nil,
                    micOn: p.media?.microphone.state == .playable
                )
            }
            .sorted { ($0.isLocal ? 0 : 1, $0.name) < ($1.isLocal ? 0 : 1, $1.name) }
    }

    nonisolated func callClient(_ callClient: CallClient, participantJoined participant: Participant) {
        MainActor.assumeIsolated { self.onPeersChanged() }
    }

    nonisolated func callClient(_ callClient: CallClient, participantUpdated participant: Participant) {
        MainActor.assumeIsolated { self.refreshTiles() }
    }

    nonisolated func callClient(_ callClient: CallClient, participantLeft participant: Participant, withReason reason: ParticipantLeftReason) {
        MainActor.assumeIsolated { self.refreshTiles() }
    }

    nonisolated func callClient(_ callClient: CallClient, remoteParticipantsAudioLevel participantsAudioLevel: [ParticipantID: Float]) {
        let loud = Set(participantsAudioLevel.filter { $0.value > Self.speakingLevel }.map { $0.key.uuid })
        MainActor.assumeIsolated {
            let remote = Set(self.tiles.filter { !$0.isLocal && loud.contains($0.id.uuid) }.map(\.id))
            let local = self.tiles.first { $0.isLocal }?.id
            var next = remote
            if let local, self.speaking.contains(local) { next.insert(local) }
            if next != self.speaking {
                self.speaking = next
                if let first = remote.first { self.lastSpeaker = first }
            }
        }
    }

    nonisolated func callClient(_ callClient: CallClient, localAudioLevel audioLevel: Float) {
        MainActor.assumeIsolated {
            guard let local = self.tiles.first(where: { $0.isLocal })?.id else { return }
            var next = self.speaking
            if self.micOn && audioLevel > Self.speakingLevel { next.insert(local) } else { next.remove(local) }
            if next != self.speaking { self.speaking = next }
        }
    }

    nonisolated func callClient(_ callClient: CallClient, error: CallClientError) {
        MainActor.assumeIsolated { self.error = error.localizedDescription }
    }
}
