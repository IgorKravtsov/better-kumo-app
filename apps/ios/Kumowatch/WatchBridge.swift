import Foundation
import WatchConnectivity

@MainActor
final class WatchBridge: NSObject, WCSessionDelegate {
    private weak var model: AppModel?
    private let session: WCSession? = WCSession.isSupported() ? .default : nil
    private var pushScheduled = false
    private var lastPushed: WatchSnapshot?
    /// Counts iPhone app launches, so the watch can tell a newer process from an older one without trusting the clock.
    private let epoch: Int64 = {
        let next = Int64(UserDefaults.standard.integer(forKey: "watchEpoch")) + 1
        UserDefaults.standard.set(Int(next), forKey: "watchEpoch")
        return next
    }()
    private var revision = 0
    private var officeOpen = false
    private var officeLease: Task<Void, Never>?
    private var notice: String?
    private var noticeId = 0
    private var micOnJoin = false
    /// Answered from a live call: the mic waits for the media switch (`.joining`), so a rejected move never unmutes.
    private var micAfterSwitch = false
    /// The call was already switching when the nudge was answered: that `.joining` may still be the old room's.
    private var micSkipJoining = false
    private var micZone: String?
    private var joinGeneration = 0
    private var joinTask: Task<Void, Never>?
    private var forwardNoticesUntil = Date.distantPast
    private var watchChannel: String?
    private var fetchedChannels: Set<String> = []
    private var channelLease: Task<Void, Never>?
    let calls = CallKitCalls()

    init(model: AppModel) {
        self.model = model
        super.init()
        calls.onEnd = { [weak self] in self?.hangUp() }
        calls.onMute = { [weak model] on in model?.core.setMic(on: on) }
        calls.onRefused = { [weak self] in
            guard let self else { return }
            self.newJoin()
            self.noticed("Unlock the iPhone to start a call from the watch")
        }
        session?.delegate = self
        session?.activate()
    }

    func callChanged(_ call: CallView) {
        switch call.phase {
        case .idle, .failed:
            if !micAfterSwitch { micOnJoin = false }
        default:
            if micOnJoin, micReady(call) {
                micOnJoin = false
                micAfterSwitch = false
                model?.core.setMic(on: true)
            }
        }
        calls.update(call)
        schedulePush()
    }

    /// The iPhone's own notices stay on the iPhone; only those during or right after a watch request go to the watch.
    func modelNoticed(_ message: String) {
        if Date() < forwardNoticesUntil { noticed(message) }
    }

    /// From a live call the mic waits for the switch to the requested room, so a rejected move never unmutes the old one.
    private func micReady(_ call: CallView) -> Bool {
        guard micAfterSwitch else { return true }
        if call.phase == .joining, !micSkipJoining { return true }
        guard call.phase == .joined, let zone = micZone else { return false }
        return model?.core.spaceView().selfZoneId == zone
    }

    /// The server turned the room down: a pending nudge mic must not open in the call the user is still in.
    func joinRejected() {
        micOnJoin = false
        micAfterSwitch = false
    }

    /// The iPhone shows its own password prompt; a watch that asked for the room needs to know where to look.
    func passwordRequired() {
        if Date() < forwardNoticesUntil { noticed("This room needs a password: enter it on the iPhone") }
    }

    /// Sign-out or another workspace: nothing the watch held belongs to the new session.
    func sessionChanged() {
        newJoin()
        calls.dropPending()
        closeOffice()
        releaseChannel()
        lastPushed = nil
        schedulePush()
    }

    func noticed(_ message: String) {
        notice = message
        noticeId += 1
        schedulePush()
    }

    /// Throttled: the core can emit several changes a second while people move or talk.
    func schedulePush() {
        guard let session, session.activationState == .activated, session.isPaired, session.isWatchAppInstalled,
              !pushScheduled else { return }
        pushScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(500))
            pushScheduled = false
            push()
        }
    }

    private func push() {
        guard let session, session.activationState == .activated else { return }
        let content = content()
        guard content != lastPushed else { return }
        do {
            try session.updateApplicationContext(WatchLink.fitted(stamped(content)))
            lastPushed = content
        } catch {
            // Not paired: the watch still gets the snapshot in every reply.
        }
    }

    /// A reply shows the watch a state the push dedupe never saw, so the next push must go out even if it matches.
    func snapshot() -> WatchSnapshot {
        lastPushed = nil
        return stamped(content())
    }

    /// Replies and pushes travel separately, so the watch keeps only the newest by revision.
    private func stamped(_ content: WatchSnapshot) -> WatchSnapshot {
        revision += 1
        var snapshot = content
        snapshot.epoch = epoch
        snapshot.revision = revision
        return snapshot
    }

    private func content() -> WatchSnapshot {
        guard let model else { return .empty }
        var snapshot = WatchSnapshot.empty
        snapshot.notice = notice
        snapshot.noticeId = noticeId
        switch model.screen {
        case .signIn:
            return snapshot
        case .pickSpace:
            snapshot.setup = .pickSpace
            return snapshot
        case .chats:
            snapshot.setup = .ready
        }
        snapshot.spaceName = model.space.map { $0.displayName.isEmpty ? $0.name : $0.displayName } ?? ""
        snapshot.relayPaired = model.relay.isPaired()
        snapshot.call = watchCall(model.call)
        if officeOpen || model.call.isActive, let view = model.spaceView {
            snapshot.floors = view.floors.map { floor in
                WatchFloor(
                    id: floor.id,
                    name: floor.name,
                    rooms: floor.rooms.map { room in
                        WatchRoom(
                            zoneId: room.zoneId,
                            floorId: room.floorId,
                            name: room.name,
                            open: room.open,
                            isMine: room.zoneId == view.selfZoneId,
                            people: room.people.map(watchPerson)
                        )
                    },
                    commonArea: floor.commonArea.map(watchPerson)
                )
            }
        }
        snapshot.chats = model.channels
            .sorted { ($0.unread > 0 ? 1 : 0, $0.lastMessageAt) > ($1.unread > 0 ? 1 : 0, $1.lastMessageAt) }
            .prefix(WatchLink.chatLimit)
            .map { channel in
                WatchChat(
                    url: channel.url,
                    title: channel.title,
                    unread: channel.unread,
                    mentions: channel.mentions,
                    preview: String(
                        (channel.lastSenderName.isEmpty ? channel.lastMessageText
                            : "\(channel.lastSenderName): \(channel.lastMessageText)").prefix(80)
                    ),
                    lastMessageAt: channel.lastMessageAt
                )
            }
        return snapshot
    }

    private func watchPerson(_ person: PersonItem) -> WatchPerson {
        WatchPerson(id: person.key, name: person.name, isSelf: person.isSelf)
    }

    private func watchCall(_ call: CallView) -> WatchCall? {
        let phase: WatchCall.Phase
        var failure: String?
        switch call.phase {
        case .idle: return nil
        case .waiting: phase = .waiting
        case .joining: phase = .joining
        case .joined: phase = .joined
        case .failed(let reason):
            phase = .failed
            failure = callFailureLabel(failure: reason)
        }
        return WatchCall(
            phase: phase,
            fromDesktop: call.isDesktop,
            roomName: call.roomName,
            micOn: call.micOn,
            failure: failure,
            people: call.tiles.map {
                WatchCallPerson(
                    id: $0.key, name: $0.name, isLocal: $0.isLocal, inCall: $0.inCall, micOn: $0.micOn,
                    speaking: $0.speaking
                )
            }
        )
    }

    private func handle(_ request: WatchRequest) async -> WatchReply {
        guard let model else { return WatchReply(error: "The iPhone app is starting") }
        if model.screen != .chats, !isSnapshot(request) {
            return WatchReply(error: "Open Better Kumo on the iPhone and sign in", snapshot: snapshot())
        }
        var reply = WatchReply()
        if !isPassive(request) { forwardNoticesUntil = Date().addingTimeInterval(30) }
        switch request {
        case .snapshot:
            break
        case .openOffice:
            openOffice()
        case .closeOffice:
            closeOffice()
        case .joinRoom(let floorId, let zoneId):
            let generation = newJoin()
            let name = model.spaceView?.floors.lazy.flatMap(\.rooms).first { $0.zoneId == zoneId }?.name ?? "Room"
            calls.start(name: name) {
                guard generation == self.joinGeneration else { return }
                let room = RoomItem(zoneId: zoneId, floorId: floorId, name: name, open: true, people: [])
                // A failed switch must not end the CallKit call of the call that is still running.
                if !model.joinRoom(room, fromWatch: true), !model.call.isLive { self.calls.cancel() }
            }
        case .joinPerson(let personId, let mic):
            openOffice()
            let generation = newJoin()
            let name = model.spaceView.flatMap { view in
                (view.floors.flatMap { $0.rooms.flatMap(\.people) + $0.commonArea } + view.away)
                    .first { $0.key == personId }?.name
            } ?? "Kumospace"
            let wasInCall = model.call.isActive
            let zoneBefore = model.spaceView?.selfZoneId
            calls.start(name: name) {
                guard generation == self.joinGeneration else { return }
                self.joinTask = Task {
                    let joined = await self.joinPersonWhenFound(personId, generation: generation)
                    guard generation == self.joinGeneration, !Task.isCancelled else { return }
                    self.joinTask = nil
                    if joined {
                        // A nudge from someone in this room switches no media, so there is no `.joining` to wait for.
                        let sameRoom = zoneBefore != nil && model.lastJoin?.zoneId == zoneBefore
                        if mic { self.turnMicOnWhenJoined(fromLiveCall: wasInCall && !sameRoom) }
                    } else {
                        if !model.call.isLive { self.calls.cancel() }
                        self.noticed("They are not in a room you can join")
                    }
                }
            }
        case .joinPersonOnDesktop(let personId):
            do { try await model.relay.joinPerson(personId: personId) } catch { reply.error = describe(error) }
        case .takeDesktopCall:
            let generation = newJoin()
            calls.start(name: "Desktop call") {
                guard generation == self.joinGeneration else { return }
                model.joinDesktopCall(fromWatch: true)
            }
        case .setMic(let on):
            model.core.setMic(on: on)
        case .leaveCall:
            hangUp()
        case .messages(let url):
            await holdChannel(url)
            reply.messages = messages(url)
        case .closeChat(let url):
            if watchChannel == url { releaseChannel() }
        case .send(let url, let text):
            await holdChannel(url)
            let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                do {
                    try await model.core.sendMessage(channelUrl: url, text: text, files: [], parentId: nil)
                } catch {
                    reply.error = error.localizedDescription
                }
            }
            reply.messages = messages(url)
        }
        // Let a join or a mic change reach the core's view before answering.
        try? await Task.sleep(for: .milliseconds(150))
        reply.snapshot = snapshot()
        return reply
    }

    @discardableResult
    private func newJoin() -> Int {
        joinGeneration += 1
        joinTask?.cancel()
        joinTask = nil
        micOnJoin = false
        micAfterSwitch = false
        // A prompt for an earlier room would join that room, or its dismissal would cancel this join.
        model?.passwordPrompt = nil
        return joinGeneration
    }

    func invalidateJoin() {
        newJoin()
        calls.dropPending()
        if model?.call.isLive != true { calls.cancel() }
    }

    private func hangUp() {
        newJoin()
        model?.leaveCall(fromWatch: true)
        calls.cancel()
    }

    /// The core's own retrying lookup can't be cancelled and joins by itself, so the retry lives here: every attempt
    /// checks the generation first, and a hang-up or a newer join (from the watch or the iPhone) stops it before it can
    /// move the user. The iPhone's own Join on a nudge uses it too.
    func phoneJoinPerson(_ personId: String) async -> Bool? {
        let generation = newJoin()
        let joined = await joinPersonWhenFound(personId, generation: generation)
        return generation == joinGeneration ? joined : nil
    }

    private func joinPersonWhenFound(_ personId: String, generation: Int) async -> Bool {
        guard let model else { return false }
        let deadline = Date().addingTimeInterval(15)
        while generation == joinGeneration, !Task.isCancelled {
            let view = model.core.spaceView()
            if view.connected {
                switch model.joinPersonNow(personId) {
                case .some(true): return true
                case .some(false):
                    // Listed but not in a joinable room: waiting won't change that.
                    let people = view.floors.flatMap { $0.rooms.flatMap(\.people) + $0.commonArea } + view.away
                    if people.contains(where: { $0.key == personId || $0.uid == personId }) { return false }
                case .none: return false
                }
            }
            if Date() > deadline { return false }
            try? await Task.sleep(for: .milliseconds(500))
        }
        return false
    }

    private func turnMicOnWhenJoined(fromLiveCall: Bool) {
        guard let model else { return }
        if fromLiveCall {
            micOnJoin = true
            micAfterSwitch = true
            micSkipJoining = model.call.phase == .joining
            micZone = model.lastJoin?.zoneId
            return
        }
        switch model.call.phase {
        case .idle, .failed: micOnJoin = true
        default: model.core.setMic(on: true)
        }
    }

    /// The watch must renew this lease while it shows a chat.
    private func holdChannel(_ url: String) async {
        if watchChannel != url {
            watchChannel = url
            model?.setWatchChannel(url)
        }
        // Only one channel is open in the core; one the iPhone keeps open for its own screen gets no fetch from
        // the watch's lease, so load its latest page once.
        if let model, model.visibleChannel != nil, model.visibleChannel != url, !fetchedChannels.contains(url),
           (try? await model.core.loadOlder(channelUrl: url, beforeTs: Int64.max)) != nil {
            fetchedChannels.insert(url)
        }
        channelLease?.cancel()
        channelLease = Task { [weak self] in
            try? await Task.sleep(for: .seconds(20))
            guard !Task.isCancelled else { return }
            self?.releaseChannel()
        }
    }

    private func releaseChannel() {
        channelLease?.cancel()
        channelLease = nil
        guard watchChannel != nil else { return }
        watchChannel = nil
        fetchedChannels.removeAll()
        model?.setWatchChannel(nil)
    }

    private func isSnapshot(_ request: WatchRequest) -> Bool {
        if case .snapshot = request { return true }
        return false
    }

    /// Polls and leases the watch repeats on its own; only what the user did opens the notice window.
    private func isPassive(_ request: WatchRequest) -> Bool {
        switch request {
        case .snapshot, .openOffice, .closeOffice, .messages, .closeChat: return true
        default: return false
        }
    }

    private func messages(_ url: String) -> [WatchMessage] {
        guard let model else { return [] }
        return model.core.messages(channelUrl: url, limit: WatchLink.messageLimit)
            .filter { $0.parentId == 0 }
            .map { m in
                var text = m.text
                if !m.images.isEmpty || m.fileType.hasPrefix("image") {
                    text = text.isEmpty || text == m.fileName ? "📷 Photo" : "📷 \(text)"
                } else if m.kind == "FILE" {
                    text = "📎 \(m.fileName)"
                }
                return WatchMessage(
                    id: m.id, sender: m.senderName, isMine: m.isMine, text: text, createdAt: m.createdAt,
                    pending: m.state == .pending, failed: m.state == .failed
                )
            }
    }

    /// The watch renews the Office while it shows it; a watch that went away stops holding it open.
    private func openOffice() {
        if !officeOpen {
            officeOpen = true
            model?.openSpace(.watch)
        }
        officeLease?.cancel()
        officeLease = Task { [weak self] in
            try? await Task.sleep(for: .seconds(WatchLink.officeLease))
            guard !Task.isCancelled else { return }
            self?.closeOffice()
        }
    }

    private func closeOffice() {
        officeLease?.cancel()
        officeLease = nil
        guard officeOpen else { return }
        officeOpen = false
        model?.closeSpace(.watch)
        schedulePush()
    }

    nonisolated func session(_ session: WCSession, activationDidCompleteWith state: WCSessionActivationState, error: Error?) {
        Task { @MainActor in
            self.lastPushed = nil
            self.schedulePush()
        }
    }

    nonisolated func sessionDidBecomeInactive(_ session: WCSession) {}

    nonisolated func sessionDidDeactivate(_ session: WCSession) {
        session.activate()
    }

    nonisolated func sessionWatchStateDidChange(_ session: WCSession) {
        Task { @MainActor in
            self.lastPushed = nil
            self.schedulePush()
        }
    }

    nonisolated func session(
        _ session: WCSession,
        didReceiveMessage message: [String: Any],
        replyHandler: @escaping ([String: Any]) -> Void
    ) {
        let request = WatchLink.decode(WatchRequest.self, from: message)
        let reply = UncheckedReply(send: replyHandler)
        Task { @MainActor in
            guard let request else {
                reply.send(WatchLink.encode(WatchReply(error: "Update Better Kumo on the iPhone")))
                return
            }
            reply.send(WatchLink.fitted(await self.handle(request)))
        }
    }
}

private struct UncheckedReply: @unchecked Sendable {
    let send: ([String: Any]) -> Void
}
