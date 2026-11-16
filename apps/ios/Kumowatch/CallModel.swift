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
    var inCall = true
    var avatar = ""
    var availability = ""

    static func == (a: CallTile, b: CallTile) -> Bool {
        a.id == b.id && a.name == b.name && a.micOn == b.micOn && (a.camera == nil) == (b.camera == nil)
            && (a.screen == nil) == (b.screen == nil) && a.inCall == b.inCall && a.availability == b.availability
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
    @ObservationIgnored private var leavingOnPurpose = false
    @ObservationIgnored weak var app: AppModel?
    @ObservationIgnored private var remotePeers: Set<String>?
    @ObservationIgnored private var remotePresence: [String: DeskPerson] = [:]
    @ObservationIgnored private var peerWatcher: Task<Void, Never>?
    private(set) var isRemote = false

    private nonisolated static let speakingLevel: Float = 0.02

    func expectCall() {
        if phase == .idle || phase == .failed { phase = .waiting }
        if let info = app?.core.callInfo() { onCallInfo(info) }
    }

    func joinRemote(_ call: DeskCall) {
        remotePeers = Set(call.peers)
        isRemote = true
        tokenRetries = 0
        roomName = call.roomName ?? ""
        phase = .waiting
        peerWatcher?.cancel()
        peerWatcher = Task { [weak self] in
            var tick = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                tick += 1
                guard let self, let relay = self.app?.relay else { continue }
                if tick % 10 == 0 { _ = try? await relay.handoff() }
                guard let d = try? await relay.desktop() else { continue }
                let zone = d.me?.zoneId
                let mates = d.people.filter { !$0.isSelf && zone != nil && $0.zoneId == zone }
                self.remotePresence = Dictionary(mates.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
                self.setRemotePeers(Set(mates.map(\.id)), roomName: d.zones.first { $0.id == zone }?.name ?? "")
            }
        }
        Task { await join(url: call.url, token: call.token) }
    }

    private func setRemotePeers(_ peers: Set<String>, roomName name: String) {
        guard remotePeers != nil else { return }
        if !name.isEmpty { roomName = name }
        if peers != remotePeers {
            remotePeers = peers
            onPeersChanged()
        } else {
            refreshTiles()
        }
    }

    private func peers() -> Set<String> {
        remotePeers ?? Set(app?.core.callPeers() ?? [])
    }

    private func presence(_ key: String) -> (name: String, avatar: String, availability: String)? {
        if let p = remotePresence[key] {
            return (p.name, p.avatar, (p.idle ?? false) ? "away" : p.availability)
        }
        guard let p = app?.person(key: key) else { return nil }
        return (p.name, p.avatarUrl, p.availability)
    }

    func onCallInfo(_ info: CallInfo?) {
        guard remotePeers == nil else { return }
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
        Task { await join(url: info.dailyUrl, token: info.dailyToken) }
    }

    private func join(url dailyURL: String, token: String) async {
        leaveClient()
        guard let url = URL(string: dailyURL) else { return }
        let c = CallClient()
        c.delegate = self
        client = c
        joinedURL = dailyURL
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
                token: MeetingToken(stringValue: token),
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
            recover(error.localizedDescription)
        }
    }

    func leave() {
        if isRemote, let relay = app?.relay {
            Task { try? await relay.reclaim() }
        }
        leavingOnPurpose = true
        leaveClient()
        leavingOnPurpose = false
        phase = .idle
        tiles = []
        speaking = []
        roomName = ""
        peerWatcher?.cancel()
        peerWatcher = nil
        remotePeers = nil
        remotePresence = [:]
        isRemote = false
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
        if remotePeers == nil, let name = app?.core.callInfo()?.roomName, !name.isEmpty { roomName = name }
        guard let c = client else { return }
        let peers = peers()
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
        guard let c = client else { return }
        let peers = peers()
        let inDaily = c.participants.all.values.filter { $0.info.isLocal || peers.contains($0.info.userID ?? "") }
        let dailyKeys = Set(inDaily.compactMap { $0.info.userID })
        var result: [CallTile] = inDaily.map { p in
            let key = p.info.userID ?? ""
            let person = presence(key)
            return CallTile(
                id: p.id,
                key: key,
                name: person?.name ?? p.info.username ?? "Guest",
                isLocal: p.info.isLocal,
                camera: p.media?.camera.state == .playable ? p.media?.camera.track : nil,
                screen: p.media?.screenVideo.state == .playable ? p.media?.screenVideo.track : nil,
                micOn: p.media?.microphone.state == .playable,
                avatar: person?.avatar ?? "",
                availability: person?.availability ?? ""
            )
        }
        for key in peers.subtracting(dailyKeys) {
            guard let person = presence(key), let uuid = UUID(uuidString: key) else { continue }
            result.append(CallTile(
                id: ParticipantID(uuid: uuid), key: key, name: person.name, isLocal: false,
                camera: nil, screen: nil, micOn: false, inCall: false,
                avatar: person.avatar, availability: person.availability
            ))
        }
        tiles = result.sorted { ($0.isLocal ? 0 : 1, $0.inCall ? 0 : 1, $0.name) < ($1.isLocal ? 0 : 1, $1.inCall ? 0 : 1, $1.name) }
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

    private func recover(_ reason: String) {
        guard tokenRetries < 3 else {
            phase = .failed
            error = reason
            return
        }
        tokenRetries += 1
        phase = .waiting
        let delay = tokenRetries
        Task {
            try? await Task.sleep(for: .seconds(delay))
            guard self.phase == .waiting else { return }
            if self.remotePeers != nil {
                do {
                    guard let relay = self.app?.relay else { return }
                    let fresh = try await relay.handoff()
                    self.remotePeers = Set(fresh.peers)
                    await self.join(url: fresh.url, token: fresh.token)
                } catch {
                    self.recover(error.localizedDescription)
                }
            } else {
                self.app?.core.refreshCallToken()
            }
        }
    }

    private func isTokenProblem(_ message: String) -> Bool {
        let m = message.lowercased()
        return m.contains("exp-token") || m.contains("unauthorized") || (m.contains("token") && m.contains("invalid"))
    }

    nonisolated func callClient(_ callClient: CallClient, error: CallClientError) {
        let message = error.localizedDescription
        MainActor.assumeIsolated {
            if self.isTokenProblem(message), !self.leavingOnPurpose {
                self.leaveClient()
                self.recover(message)
            } else {
                self.error = message
            }
        }
    }

    nonisolated func callClient(_ callClient: CallClient, callStateUpdated state: CallState) {
        let left = state == .left
        MainActor.assumeIsolated {
            if left, self.phase == .joined, !self.leavingOnPurpose {
                self.leaveClient()
                self.recover("Call dropped")
            }
        }
    }
}
