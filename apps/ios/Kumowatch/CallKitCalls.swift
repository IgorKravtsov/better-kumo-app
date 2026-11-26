import AVFoundation
import CallKit
import Foundation
import UIKit

/// iOS lets a background app open the microphone only inside a CallKit call; Daily still drives the audio session.
@MainActor
final class CallKitCalls: NSObject, CXProviderDelegate {
    private let provider: CXProvider
    private let controller = CXCallController()
    private var current: UUID?
    private var connected = false
    /// The core has shown a call since this UUID started; before that, an idle or failed phase is the previous call's.
    private var ownsCoreCall = false
    /// What the system call UI shows; kept in step with the core's microphone in both directions.
    private var systemMuted: Bool?
    /// Mute actions this class requested to mirror the core: their callbacks must not write back to the core.
    private var mirrored: Set<UUID> = []
    /// Runs once CallKit starts the call with this UUID; a newer attempt replaces it, so a stale one never runs.
    private var pendingStart: (id: UUID, run: () -> Void)?

    var onEnd: (() -> Void)?
    var onMute: ((Bool) -> Void)?
    /// CallKit refused a call while the iPhone app was in the background: the microphone would not start.
    var onRefused: (() -> Void)?

    override init() {
        let config = CXProviderConfiguration()
        config.supportsVideo = false
        config.maximumCallGroups = 1
        config.maximumCallsPerCallGroup = 1
        config.supportedHandleTypes = [.generic]
        config.includesCallsInRecents = false
        provider = CXProvider(configuration: config)
        super.init()
        provider.setDelegate(self, queue: .main)
    }

    func start(name: String, _ start: @escaping () -> Void) {
        if let id = current {
            if pendingStart?.id == id {
                pendingStart = (id, start)
            } else {
                start()
            }
            return
        }
        let id = UUID()
        let action = CXStartCallAction(call: id, handle: CXHandle(type: .generic, value: name))
        current = id
        connected = false
        ownsCoreCall = false
        systemMuted = nil
        pendingStart = (id, start)
        controller.request(CXTransaction(action: action)) { [weak self] error in
            guard error != nil else { return }
            Task { @MainActor in
                guard let self, self.current == id else { return }
                self.current = nil
                // Without CallKit a background app gets no microphone, so only a visible app joins anyway.
                if UIApplication.shared.applicationState == .active {
                    self.runPending(id)
                } else {
                    self.pendingStart = nil
                    self.onRefused?()
                }
            }
        }
    }

    func update(_ call: CallView) {
        guard let id = current else { return }
        switch call.phase {
        case .idle, .failed:
            // Explicit cancellation covers a start or lookup that never reached the core.
            guard ownsCoreCall else { return }
        default:
            ownsCoreCall = true
        }
        switch call.phase {
        case .joined:
            if !connected {
                connected = true
                provider.reportOutgoingCall(with: id, connectedAt: nil)
            }
            syncMute(id, muted: !call.micOn)
        case .idle:
            end(id, reason: .remoteEnded)
        case .failed:
            end(id, reason: .failed)
        case .waiting, .joining:
            break
        }
    }

    /// A lookup may join only while its CallKit call is still there, or a locked iPhone would get no microphone.
    var hasCall: Bool { current != nil }

    /// For a join that never produced a core call, so no phase change will end the CallKit call.
    func cancel() {
        guard let id = current else { return }
        end(id, reason: .failed)
    }

    func dropPending() {
        guard let pending = pendingStart else { return }
        pendingStart = nil
        if current == pending.id { end(pending.id, reason: .failed) }
    }

    /// The watch and the iPhone's own controls change the core directly; mirror that into the system call UI.
    private func syncMute(_ id: UUID, muted: Bool) {
        guard systemMuted != muted else { return }
        systemMuted = muted
        let action = CXSetMutedCallAction(call: id, muted: muted)
        let actionId = action.uuid
        mirrored.insert(actionId)
        controller.request(CXTransaction(action: action)) { [weak self] error in
            guard error != nil else { return }
            Task { @MainActor in
                guard let self else { return }
                self.mirrored.remove(actionId)
                // Unknown again, so the next update retries.
                if self.current == id { self.systemMuted = nil }
            }
        }
    }

    private func end(_ id: UUID, reason: CXCallEndedReason) {
        current = nil
        connected = false
        systemMuted = nil
        mirrored.removeAll()
        pendingStart = nil
        provider.reportCall(with: id, endedAt: nil, reason: reason)
    }

    private func runPending(_ id: UUID) {
        guard let pending = pendingStart, pending.id == id else { return }
        pendingStart = nil
        pending.run()
    }

    nonisolated func providerDidReset(_ provider: CXProvider) {
        MainActor.assumeIsolated {
            let hadCall = current != nil || pendingStart != nil
            current = nil
            connected = false
            pendingStart = nil
            // CallKit dropped every call it held; the media behind them must go too.
            if hadCall { onEnd?() }
        }
    }

    // The provider calls these on the main queue (`setDelegate(_:queue: .main)`).

    nonisolated func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        let id = action.callUUID
        // A start that a hang-up or a newer join replaced must not leave a system call with no media behind it.
        let started = MainActor.assumeIsolated { () -> Bool in
            guard current == id else { return false }
            runPending(id)
            guard current == id else { return false }
            self.provider.reportOutgoingCall(with: id, startedConnectingAt: nil)
            return true
        }
        if started { action.fulfill() } else { action.fail() }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        let id = action.callUUID
        MainActor.assumeIsolated {
            if current == id {
                current = nil
                connected = false
                onEnd?()
            }
        }
        action.fulfill()
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
        let id = action.callUUID
        let muted = action.isMuted
        let actionId = action.uuid
        // A queued action for a call that already ended must not touch the microphone of the next one, and a mirror
        // of the core's own state must not overwrite a newer change.
        let applied = MainActor.assumeIsolated { () -> Bool in
            guard current == id else { return false }
            if mirrored.remove(actionId) != nil { return true }
            systemMuted = muted
            onMute?(!muted)
            return true
        }
        if applied { action.fulfill() } else { action.fail() }
    }

    nonisolated func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {}

    nonisolated func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {}
}
