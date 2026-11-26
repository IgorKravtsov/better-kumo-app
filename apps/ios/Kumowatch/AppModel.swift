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
    var notice: String? {
        didSet { if let notice { watch?.modelNoticed(notice) } }
    }
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
    @ObservationIgnored private(set) var lastJoin: RoomItem?
    @ObservationIgnored private(set) var visibleChannel: String?
    @ObservationIgnored private var watchChannel: String?
    /// The core keeps one channel open: the iPhone's screen wins, the watch's lease gets it otherwise.
    @ObservationIgnored private var openChannel: String?
    let media = DailyMedia()
    var call: CallView
    @ObservationIgnored private var watch: WatchBridge?
    @ObservationIgnored private var officeHolders: Set<OfficeHolder> = []

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
        watch = WatchBridge(model: self)
    }

    func channelAppeared(_ channelUrl: String) {
        visibleChannel = channelUrl
        applyOpenChannel()
    }

    func channelDisappeared(_ channelUrl: String) {
        if visibleChannel == channelUrl { visibleChannel = nil }
        applyOpenChannel()
    }

    func setWatchChannel(_ channelUrl: String?) {
        watchChannel = channelUrl
        applyOpenChannel()
    }

    private func applyOpenChannel() {
        let wanted = visibleChannel ?? watchChannel
        guard wanted != openChannel else { return }
        if let open = openChannel { core.closeChannel(channelUrl: open) }
        if let wanted { core.openChannel(channelUrl: wanted) }
        openChannel = wanted
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
            watch?.callChanged(call)
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
            watch?.schedulePush()
        }
    }

    /// The Office tab and the watch release their interest independently; a set keeps an unpaired appear from leaking.
    func openSpace(_ holder: OfficeHolder) {
        officeHolders.insert(holder)
        try? core.spaceOpen()
        scheduleSpaceReload()
    }

    func closeSpace(_ holder: OfficeHolder) {
        guard officeHolders.remove(holder) != nil, officeHolders.isEmpty else { return }
        try? core.spaceClose()
    }

    private func onSpaceError(code: String, message: String) {
        watch?.joinRejected()
        // A rejected waiting join leaves the Office wanted in the core; leaving releases it.
        if case .idle = call.phase { core.leaveCall() }
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
        watch?.invalidateJoin()
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

    @discardableResult
    func joinRoom(_ room: RoomItem, fromWatch: Bool = false) -> Bool {
        if !fromWatch { watch?.invalidateJoin() }
        lastJoin = room
        joinFailed = false
        do {
            try core.joinRoom(floorId: room.floorId, zoneId: room.zoneId)
            return true
        } catch {
            notice = error.localizedDescription
            return false
        }
    }

    /// Record watch joins so a password prompt targets the requested room.
    func joinPersonNow(_ personId: String) -> Bool? {
        joinFailed = false
        do {
            return joined(try core.joinPerson(personId: personId))
        } catch {
            return nil
        }
    }

    private func joined(_ join: PersonJoin?) -> Bool {
        guard let join else { return false }
        lastJoin = spaceView?.floors.lazy.flatMap(\.rooms).first { $0.zoneId == join.zoneId }
            ?? RoomItem(zoneId: join.zoneId, floorId: join.floorId, name: "Room", open: true, people: [])
        return true
    }

    func joinPerson(_ person: PersonItem) -> Bool {
        watch?.invalidateJoin()
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
            watch?.invalidateJoin()
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

    func joinDesktopCall(fromWatch: Bool = false) {
        if !fromWatch { watch?.invalidateJoin() }
        joinFailed = false
        core.handoffCall()
    }

    /// The bridge passes `fromWatch` from its own hang-up, which has already stopped its pending join.
    func leaveCall(fromWatch: Bool = false) {
        if !fromWatch { watch?.invalidateJoin() }
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
            watch?.schedulePush()
        }
    }

    func refreshSession() {
        defer { watch?.schedulePush() }
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
        watch?.schedulePush()
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

    /// Joining or joined; a failed call that wasn't dismissed doesn't count.
    var isLive: Bool {
        switch phase {
        case .idle, .failed: return false
        default: return true
        }
    }

    var isDesktop: Bool { source == .desktop }

    var isOffice: Bool { source == .office }
}

extension AppModel {
    func refreshQuickReactions() {
        quickReactions = core.quickReactions()
    }
}

enum OfficeHolder {
    case tab
    case watch
}
