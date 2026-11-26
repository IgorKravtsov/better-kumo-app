import Foundation
import Observation
import WatchConnectivity

@MainActor
@Observable
final class WatchModel: NSObject, WCSessionDelegate {
    var snapshot: WatchSnapshot
    var reachable = false
    var heardFromPhone = false
    var busy = false
    var alert: String?
    /// A join the iPhone accepted but that has no call yet: the call screen shows "Connecting…" instead of "Not in a call".
    var joinPending = false
    @ObservationIgnored private var joinPendingTask: Task<Void, Never>?
    @ObservationIgnored private var officeHolds = 0
    @ObservationIgnored private var officeLease: Task<Void, Never>?
    @ObservationIgnored private var seenNoticeId: Int
    @ObservationIgnored private var seenNoticeEpoch: Int64
    @ObservationIgnored private var appliedEpoch: Int64
    @ObservationIgnored private var appliedRevision: Int
    @ObservationIgnored private let session = WCSession.default

    override init() {
        let context = WatchLink.decode(WatchSnapshot.self, from: WCSession.default.receivedApplicationContext)
        snapshot = context ?? .empty
        heardFromPhone = context != nil
        seenNoticeId = context?.noticeId ?? 0
        seenNoticeEpoch = context?.epoch ?? 0
        appliedEpoch = context?.epoch ?? 0
        appliedRevision = context?.revision ?? 0
        super.init()
        session.delegate = self
        session.activate()
    }

    @discardableResult
    func send(_ request: WatchRequest, quiet: Bool = false) async -> WatchReply? {
        guard session.activationState == .activated else {
            if !quiet { alert = "Still connecting to the iPhone" }
            return nil
        }
        guard session.isReachable else {
            if !quiet { alert = "The iPhone isn't reachable" }
            return nil
        }
        if !quiet { busy = true }
        defer { if !quiet { busy = false } }
        let payload = WatchLink.encode(request)
        let reply: WatchReply? = await withCheckedContinuation { continuation in
            let once = Once(continuation)
            session.sendMessage(payload) { dictionary in
                once.resume(WatchLink.decode(WatchReply.self, from: dictionary))
            } errorHandler: { error in
                once.resume(WatchReply(error: error.localizedDescription))
            }
        }
        guard let reply else {
            if !quiet { alert = "Update Better Kumo on the iPhone" }
            return nil
        }
        if let snapshot = reply.snapshot { apply(snapshot) }
        if let error = reply.error, !quiet { alert = error }
        return reply
    }

    /// Sends the join; true once the iPhone took it. The call screen then waits up to 15 s for the call to appear.
    /// Pending is set before sending, so a rejection notice already in the reply clears it rather than being overwritten.
    func join(_ request: WatchRequest) async -> Bool {
        joinPending = true
        joinPendingTask?.cancel()
        joinPendingTask = Task {
            try? await Task.sleep(for: .seconds(15))
            if !Task.isCancelled { joinPending = false }
        }
        guard let reply = await send(request), reply.error == nil else {
            joinPending = false
            return false
        }
        return true
    }

    /// Every Office screen holds the iPhone's Office open while it shows; it closes a moment after the last one goes,
    /// so moving between them doesn't close it.
    func holdOffice() async {
        officeHolds += 1
        if officeLease == nil {
            officeLease = Task {
                while !Task.isCancelled {
                    await send(.openOffice, quiet: true)
                    try? await Task.sleep(for: .seconds(WatchLink.officeLease / 3))
                }
            }
        }
        while !Task.isCancelled { try? await Task.sleep(for: .seconds(3600)) }
        officeHolds -= 1
        // This task is cancelled, so the grace period runs in a new one.
        Task {
            try? await Task.sleep(for: .seconds(2))
            guard officeHolds == 0, let lease = officeLease else { return }
            lease.cancel()
            officeLease = nil
            await send(.closeOffice, quiet: true)
        }
    }

    func refresh() {
        Task { await send(.snapshot, quiet: true) }
    }

    func act(_ request: WatchRequest) {
        Task { await send(request) }
    }

    private func apply(_ new: WatchSnapshot) {
        // Replies and application contexts arrive independently; an older one must not undo a newer one.
        guard new.epoch > appliedEpoch || (new.epoch == appliedEpoch && new.revision > appliedRevision) else { return }
        appliedEpoch = new.epoch
        appliedRevision = new.revision
        heardFromPhone = true
        snapshot = new
        if new.call != nil { joinPending = false }
        // Notice ids restart with each iPhone app process.
        if new.noticeId != seenNoticeId || new.epoch != seenNoticeEpoch {
            seenNoticeId = new.noticeId
            seenNoticeEpoch = new.epoch
            if let notice = new.notice {
                alert = notice
                // A notice after a join is its rejection: stop waiting for the call.
                joinPending = false
            }
        }
    }

    nonisolated func session(_ session: WCSession, activationDidCompleteWith state: WCSessionActivationState, error: Error?) {
        let reachable = session.isReachable
        Task { @MainActor in
            self.reachable = reachable
            if reachable { self.refresh() }
        }
    }

    nonisolated func sessionReachabilityDidChange(_ session: WCSession) {
        let reachable = session.isReachable
        Task { @MainActor in
            self.reachable = reachable
            if reachable { self.refresh() }
        }
    }

    nonisolated func session(_ session: WCSession, didReceiveApplicationContext applicationContext: [String: Any]) {
        guard let snapshot = WatchLink.decode(WatchSnapshot.self, from: applicationContext) else { return }
        Task { @MainActor in self.apply(snapshot) }
    }
}

/// WatchConnectivity may call both the reply and the error handler; the continuation must resume once.
private final class Once: @unchecked Sendable {
    private var continuation: CheckedContinuation<WatchReply?, Never>?
    private let lock = NSLock()

    init(_ continuation: CheckedContinuation<WatchReply?, Never>) {
        self.continuation = continuation
    }

    func resume(_ value: WatchReply?) {
        lock.lock()
        let c = continuation
        continuation = nil
        lock.unlock()
        c?.resume(returning: value)
    }
}
