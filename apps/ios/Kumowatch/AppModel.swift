import Foundation
import Observation
import UIKit
import UserNotifications

enum Screen {
    case signIn
    case pickSpace
    case chats
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
    var passwordPrompt: RoomItem?
    var openRequest: String?
    var pushError: String?
    let relay = RelayClient()
    @ObservationIgnored private var lastJoin: RoomItem?
    let call = CallModel()

    private var reloadScheduled = false

    init() {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let bridge = ListenerBridge()
        core = try! KumoApp(dataDir: dir.path, storage: KeychainStorage(), listener: bridge)
        core.setDevice(platform: .ios, timeZone: TimeZone.current.identifier)
        bridge.model = self
        call.app = self
        refreshSession()
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
            scheduleSpaceReload()
            call.onCallInfo(core.callInfo())
        case .notice:
            break
        case .spaceError(let code, let message):
            onSpaceError(code: code, message: message)
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
            call.onPeersChanged()
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
            if call.phase == .waiting, !call.isRemote { call.leave() }
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
            call.leave()
            return
        }
        try? core.joinRoomWithPassword(floorId: room.floorId, zoneId: room.zoneId, password: password)
    }

    func joinRoom(_ room: RoomItem) {
        lastJoin = room
        call.expectCall()
        do { try core.joinRoom(floorId: room.floorId, zoneId: room.zoneId) } catch { notice = error.localizedDescription }
    }

    func joinPerson(_ person: PersonItem) {
        call.expectCall()
        do { _ = try core.joinPerson(uid: person.uid) } catch { notice = error.localizedDescription }
    }

    func joinDesktopCall() async throws {
        _ = try await relay.media(audio: false, video: false)
        let desktopCall = try await relay.handoff()
        call.joinRemote(desktopCall)
    }

    func leaveCall() {
        let remote = call.isRemote
        call.leave()
        if !remote { try? core.leaveCall() }
    }

    func handle(url: URL) async {
        switch (url.scheme, url.host) {
        case ("kumospace", "login"):
            await signIn(link: url.absoluteString)
        case ("kumowatch", "chat"):
            openRequest = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first { $0.name == "channel" }?.value
        case ("kumowatch", "relay"):
            if relay.configure(from: url) {
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

    func didReceiveDeviceToken(_ token: String) {
        guard relay.isConfigured else { return }
        Task {
            do { try await relay.register(deviceToken: token, name: UIDevice.current.name) } catch { pushError = error.localizedDescription }
        }
    }

    func person(key: String) -> PersonItem? {
        guard let view = spaceView else { return nil }
        let all = view.floors.flatMap { f in f.rooms.flatMap(\.people) + f.commonArea } + view.away
        return all.first { $0.key == key }
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
