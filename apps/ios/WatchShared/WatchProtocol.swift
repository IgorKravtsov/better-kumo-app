import Foundation

enum WatchRequest: Codable, Sendable {
    case snapshot
    /// The watch shows the Office; repeat at least every `WatchLink.officeLease` seconds to keep it open.
    case openOffice
    case closeOffice
    case joinRoom(floorId: String, zoneId: String)
    /// `mic`: answering a nudge talks right away; picking someone from the Office joins muted.
    case joinPerson(personId: String, mic: Bool)
    case joinPersonOnDesktop(personId: String)
    case takeDesktopCall
    case setMic(on: Bool)
    case leaveCall
    /// The watch shows this chat; repeat while it does. `closeChat` releases it when the watch leaves.
    case messages(channelUrl: String)
    case closeChat(channelUrl: String)
    case send(channelUrl: String, text: String)
}

struct WatchReply: Codable, Sendable {
    var error: String?
    var snapshot: WatchSnapshot?
    var messages: [WatchMessage]?
}

struct WatchSnapshot: Codable, Equatable, Sendable {
    enum Setup: String, Codable, Sendable {
        case ready
        case signIn
        case pickSpace
    }

    var setup: Setup
    var spaceName: String
    var relayPaired: Bool
    var call: WatchCall?
    var floors: [WatchFloor]
    var chats: [WatchChat]
    var notice: String?
    /// Changes with each notice, so the same text can show twice.
    var noticeId: Int
    /// The iPhone app's launch count: a newer process wins, a delayed one from an older process loses.
    var epoch: Int64 = 0
    var revision = 0

    static let empty = WatchSnapshot(
        setup: .signIn, spaceName: "", relayPaired: false, call: nil, floors: [], chats: [], notice: nil, noticeId: 0
    )
}

struct WatchCall: Codable, Equatable, Sendable {
    enum Phase: String, Codable, Sendable {
        case waiting, joining, joined, failed
    }

    var phase: Phase
    var fromDesktop: Bool
    var roomName: String
    var micOn: Bool
    var failure: String?
    var people: [WatchCallPerson]
}

struct WatchCallPerson: Codable, Equatable, Identifiable, Sendable {
    var id: String
    var name: String
    var isLocal: Bool
    var inCall: Bool
    var micOn: Bool
    var speaking: Bool
}

struct WatchFloor: Codable, Equatable, Identifiable, Sendable {
    var id: String
    var name: String
    var rooms: [WatchRoom]
    var commonArea: [WatchPerson]
}

struct WatchRoom: Codable, Equatable, Identifiable, Sendable {
    var id: String { zoneId }
    var zoneId: String
    var floorId: String
    var name: String
    var open: Bool
    var isMine: Bool
    var people: [WatchPerson]
}

struct WatchPerson: Codable, Equatable, Identifiable, Sendable {
    var id: String
    var name: String
    var isSelf: Bool
}

struct WatchChat: Codable, Equatable, Identifiable, Sendable {
    var id: String { url }
    var url: String
    var title: String
    var unread: UInt64
    var mentions: UInt64
    var preview: String
    var lastMessageAt: Int64
}

struct WatchMessage: Codable, Equatable, Identifiable, Sendable {
    var id: Int64
    var sender: String
    var isMine: Bool
    var text: String
    var createdAt: Int64
    var pending: Bool
    var failed: Bool? = nil
}

enum WatchLink {
    static let key = "kumo"
    static let officeLease: TimeInterval = 90
    static let messageLimit: UInt32 = 20
    static let chatLimit = 25
    static let quickReplies = ["👍", "On my way", "Give me a minute", "Can't talk now, I'll text you"]

    static func encode<T: Encodable>(_ value: T) -> [String: Any] {
        guard let data = try? JSONEncoder().encode(value) else { return [:] }
        return [key: data]
    }

    /// WatchConnectivity refuses payloads over about 64 KB; a huge Office drops its rooms first so the call, chats
    /// and any error still get through.
    static func fitted(_ reply: WatchReply) -> [String: Any] {
        var reply = reply
        if size(reply) > budget, reply.snapshot != nil { reply.snapshot?.floors = [] }
        if size(reply) > budget, let messages = reply.messages { reply.messages = Array(messages.suffix(5)) }
        return encode(reply)
    }

    static func fitted(_ snapshot: WatchSnapshot) -> [String: Any] {
        var snapshot = snapshot
        if size(snapshot) > budget { snapshot.floors = [] }
        return encode(snapshot)
    }

    private static let budget = 60_000

    private static func size<T: Encodable>(_ value: T) -> Int {
        (try? JSONEncoder().encode(value).count) ?? 0
    }

    static func decode<T: Decodable>(_ type: T.Type, from dictionary: [String: Any]) -> T? {
        guard let data = dictionary[key] as? Data else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }
}

/// Must match the APNs category in `kumo-relay/src/push.rs`.
enum PersonAction {
    static let category = "PERSON"
    static let desktop = "PERSON_DESKTOP"
    static let phone = "PERSON_PHONE"
}
