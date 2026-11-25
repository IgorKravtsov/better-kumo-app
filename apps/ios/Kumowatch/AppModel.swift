import Foundation
import Observation
import UIKit
import UserNotifications

enum Screen {
    case signIn
    case pickSpace
    case chats
}

enum OfficeRequest {
    case office
    case desktop
    case call
}

final class ListenerBridge: AppListener, @unchecked Sendable {
    weak var model: AppModel?

    func onEvent(event: AppEvent) {
        Task { @MainActor [weak model] in model?.handle(event) }
    }
}

@MainActor
@Observable
final class AppModel {
    let core: KumoApp
    var screen: Screen = .signIn
    var user: UserInfo?
    var space: SpaceInfo?
    var channels: [ChannelItem] = []
    var connection: ConnectionStatus = .offline
    var typing: [String: [String]] = [:]
    var messageRevision: [String: Int] = [:]
    var lastError: String?
    var signingIn = false
    var spaceView: SpaceView?
    var notice: String?
    var quickReactions: [String] = []
    var passwordPrompt: RoomItem?
    /// State, not an event: a call screen that opens after the rejection must still close.
    var joinFailed = false
    var openRequest: String?
    var officeRequest: OfficeRequest?
    var pushError: String?
    var pushUnsupported = false
    var desktop = DesktopSnapshot(state: nil, error: nil)
    let relay: RelayClient
    @ObservationIgnored private var lastJoin: RoomItem?
    @ObservationIgnored private(set) var visibleChannel: String?
    let media = DailyMedia()
    var call: CallView

    private var reloadScheduled = false

    init() {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let bridge = ListenerBridge()
        core = try! KumoApp(
            dataDir: dir.path,
            storage: KeychainStorage(),
            relayStorage: RelayPairingStorage(),
            callMedia: DailyMediaBridge(media: media),
            listener: bridge
        )
        relay = core.relayClient()
        call = core.call()
        core.setDevice(platform: .ios, timeZone: TimeZone.current.identifier)
        bridge.model = self
        refreshSession()
    }

    func channelAppeared(_ channelUrl: String) {
        visibleChannel = channelUrl
        core.openChannel(channelUrl: channelUrl)
    }

    func channelDisappeared(_ channelUrl: String) {
        if visibleChannel == channelUrl { visibleChannel = nil }
        core.closeChannel(channelUrl: channelUrl)
    }

    func handle(_ event: AppEvent) {
        switch event {
        case .channelsChanged:
            scheduleChannelsReload()
        case .messagesChanged(let url):
            messageRevision[url, default: 0] += 1
        case .connection(let status):
            connection = status
        case .typing(let url, let names):
            typing[url] = names
        case .error(let message):
            lastError = message
        case .alert:
            break
        case .spaceChanged:
            scheduleSpaceReload()
        case .callChanged:
            call = core.call()
            if call.phase != .idle { joinFailed = false }
        case .notice:
            break
        case .spaceError(let code, let message):
            onSpaceError(code: code, message: message)
        case .desktopChanged:
            desktop = relay.desktop()
        }
    }

    @ObservationIgnored private var spaceReloadScheduled = false

    private func scheduleSpaceReload() {
        guard !spaceReloadScheduled else { return }
        spaceReloadScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(100))
            spaceReloadScheduled = false
            spaceView = core.spaceView()
        }
    }

    func openSpace() {
        try? core.spaceOpen()
        scheduleSpaceReload()
    }

    func closeSpace() {
        try? core.spaceClose()
    }

    private func onSpaceError(code: String, message: String) {
        switch code {
        case "ROOM_PASSWORD_REQUIRED", "ROOM_PASSWORD_INCORRECT":
            passwordPrompt = lastJoin
            if code == "ROOM_PASSWORD_INCORRECT" { notice = "Wrong room password" }
        default:
            joinEnded()
            switch code {
            case "ZONE_CLOSED", "ROOM_CLOSED": notice = "This room is closed"
            case "FULL_CAPACITY": notice = "This room is full"
            default: notice = message.isEmpty ? code : message
            }
        }
    }

    func submitPassword(_ password: String?) {
        guard let room = passwordPrompt else { return }
        passwordPrompt = nil
        guard let password, !password.isEmpty else {
            joinEnded()
            return
        }
        joinFailed = false
        try? core.joinRoomWithPassword(floorId: room.floorId, zoneId: room.zoneId, password: password)
    }

    /// Any space error abandons a waiting call; only one that left no call ends the join.
    private func joinEnded() {
        if core.call().phase == .idle { joinFailed = true }
    }

    func joinRoom(_ room: RoomItem) {
        lastJoin = room
        joinFailed = false
        do { try core.joinRoom(floorId: room.floorId, zoneId: room.zoneId) } catch { notice = error.localizedDescription }
    }

    private func joined(_ join: PersonJoin?) -> Bool {
        guard let join else { return false }
        lastJoin = spaceView?.floors.lazy.flatMap(\.rooms).first { $0.zoneId == join.zoneId }
            ?? RoomItem(zoneId: join.zoneId, floorId: join.floorId, name: "Room", open: true, people: [])
        return true
    }

    func joinPerson(_ person: PersonItem) -> Bool {
        joinFailed = false
        do {
            if joined(try core.joinPerson(personId: person.key)) { return true }
            notice = "They are not in a room you can join"
        } catch {
            notice = error.localizedDescription
        }
        return false
    }

    func openPerson(_ personId: String, via: String?) {
        switch via {
        case "desktop":
            officeRequest = .desktop
            Task {
                do { try await relay.joinPerson(personId: personId) } catch { notice = describe(error) }
            }
        case "phone":
            officeRequest = .office
            Task {
                do {
                    joinFailed = false
                    if joined(try await core.joinPersonSoon(personId: personId)) {
                        officeRequest = .call
                    } else {
                        notice = "They are not in a room you can join"
                    }
                } catch {
                    notice = error.localizedDescription
                }
            }
        default:
            officeRequest = .office
        }
    }

    func joinDesktopCall() {
        joinFailed = false
        core.handoffCall()
    }

    func leaveCall() {
        core.leaveCall()
    }

    func toggleMic() {
        core.setMic(on: !call.micOn)
    }

    func toggleCamera() {
        core.setCamera(on: !call.cameraOn)
    }

    func handle(url: URL) async {
        switch (url.scheme, url.host) {
        case ("kumospace", "login"):
            await signIn(link: url.absoluteString)
        case ("kumowatch", "chat"):
            openRequest = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first { $0.name == "channel" }?.value
        case ("kumowatch", "person"):
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
            if let id = items.first(where: { $0.name == "id" })?.value {
                openPerson(id, via: items.first { $0.name == "via" }?.value)
            }
        case ("kumowatch", "relay"):
            if relay.pair(link: url.absoluteString) {
                notice = "Relay connected"
                registerForPush()
            }
        default:
            break
        }
    }

    func registerForPush() {
        Task {
            let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])) ?? false
            if granted { UIApplication.shared.registerForRemoteNotifications() }
        }
    }

    func pushRegistrationFailed(_ error: Error) {
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain && ns.code == 3000 || ns.localizedDescription.contains("aps-environment") {
            pushUnsupported = true
            pushError = nil
        } else {
            pushError = describe(error)
        }
    }

    func didReceiveDeviceToken(_ token: String) {
        guard relay.isPaired() else { return }
        Task {
            do { try await relay.registerDevice(token: token, name: UIDevice.current.name) } catch { pushError = describe(error) }
        }
    }

    private func scheduleChannelsReload() {
        guard !reloadScheduled else { return }
        reloadScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(50))
            reloadScheduled = false
            channels = core.channels()
        }
    }

    func refreshSession() {
        user = core.currentUser()
        guard user != nil else {
            screen = .signIn
            return
        }
        space = core.selectedSpace()
        guard space != nil else {
            screen = .pickSpace
            return
        }
        channels = core.channels()
        _ = try? core.start()
        screen = .chats
    }

    func signIn(link: String) async {
        signingIn = true
        defer { signingIn = false }
        do {
            _ = try await core.signInWithLink(link: link)
            refreshSession()
            if screen == .pickSpace, let space = try await core.defaultSpace() {
                select(space: space)
            }
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    func spaces() async throws -> [SpaceInfo] {
        try await core.spaces()
    }

    func select(space: SpaceInfo) {
        try? core.selectSpace(space: space)
        self.space = space
        channels = core.channels()
        screen = .chats
    }

    func transferLink() async throws -> String {
        try await core.transferLink()
    }

    func signOut() {
        try? core.signOut()
        channels = []
        refreshSession()
    }

    func onForeground() {
        if screen == .chats { _ = try? core.start() }
    }
}

extension CallView {
    var isActive: Bool {
        if case .idle = phase { return false }
        return true
    }

    var isDesktop: Bool { source == .desktop }

    var isOffice: Bool { source == .office }
}

extension AppModel {
    func refreshQuickReactions() {
        quickReactions = core.quickReactions()
    }
}
