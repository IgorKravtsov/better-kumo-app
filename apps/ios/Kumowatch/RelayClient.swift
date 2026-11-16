import Foundation
import Security
import WidgetKit

struct DeskZone: Decodable, Identifiable {
    let id: String
    let name: String
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let open: Bool
}

struct DeskPerson: Decodable, Identifiable {
    let id: String
    let uid: String
    let name: String
    let avatar: String
    let x: Double
    let y: Double
    let zoneId: String?
    let isSelf: Bool
    let availability: String
    let idle: Bool?
    let message: String?

    enum CodingKeys: String, CodingKey {
        case id, uid, name, avatar, x, y, zoneId, availability, idle, message
        case isSelf = "self"
    }
}

struct DeskFloor: Decodable, Identifiable {
    let id: String
    let name: String
}

struct DeskSelf: Decodable {
    let zoneId: String?
    let availability: String
    let message: String?
}

struct DeskState: Decodable {
    let floorId: String?
    let width: Double
    let height: Double
    let floors: [DeskFloor]
    let me: DeskSelf?
    let zones: [DeskZone]
    let people: [DeskPerson]
    let audio: Bool
    let video: Bool

    enum CodingKeys: String, CodingKey {
        case floorId, width, height, floors, zones, people, audio, video
        case me = "self"
    }
}

struct DeskCall: Decodable {
    let url: String
    let token: String
    let selfId: String
    let peers: [String]
    let roomName: String?
}

struct RelayStatus: Decodable {
    let enabled: Bool
    let online: Bool
    let ntfy: Bool
    let apns: Bool
    let devices: Int
}

enum RelayError: LocalizedError {
    case notConfigured
    case http(Int)
    case message(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: "Relay is not set up"
        case .http(let code): code == 401 ? "Wrong relay secret" : "Relay returned \(code)"
        case .message(let text): text
        }
    }
}

final class RelayClient: @unchecked Sendable {
    private let defaults = UserDefaults.standard
    private let urlKey = "relay.url"
    private let account = "relay-secret"
    private let service = "com.kravtsov.kumowatch"

    var url: String? {
        get { defaults.string(forKey: urlKey) }
        set { defaults.set(newValue, forKey: urlKey) }
    }

    var isConfigured: Bool { url != nil && secret != nil }

    var secret: String? {
        get {
            var item: CFTypeRef?
            let q: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: account,
                kSecReturnData as String: true,
            ]
            guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
            return String(data: data, encoding: .utf8)
        }
        set {
            let q: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: account,
            ]
            SecItemDelete(q as CFDictionary)
            guard let newValue else { return }
            var add = q
            add[kSecValueData as String] = Data(newValue.utf8)
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    private let shared = UserDefaults(suiteName: Bundle.main.object(forInfoDictionaryKey: "KumoAppGroup") as? String ?? "")

    func publish(enabled: Bool?) {
        shared?.set(url, forKey: "relay.url")
        shared?.set(secret, forKey: "relay.secret")
        if let enabled { shared?.set(enabled, forKey: "relay.enabled") }
        WidgetCenter.shared.reloadAllTimelines()
    }

    func configure(from link: URL) -> Bool {
        guard let items = URLComponents(url: link, resolvingAgainstBaseURL: false)?.queryItems,
              let url = items.first(where: { $0.name == "url" })?.value,
              let secret = items.first(where: { $0.name == "secret" })?.value else { return false }
        self.url = url
        self.secret = secret
        return true
    }

    func forget() {
        url = nil
        secret = nil
        shared?.removeObject(forKey: "relay.url")
        shared?.removeObject(forKey: "relay.secret")
        shared?.removeObject(forKey: "relay.enabled")
        WidgetCenter.shared.reloadAllTimelines()
    }

    private func raw(_ path: String, method: String, body: [String: Any]? = nil) async throws -> Data {
        guard let base = url, let secret, let endpoint = URL(string: base.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path) else {
            throw RelayError.notConfigured
        }
        var req = URLRequest(url: endpoint)
        req.httpMethod = method
        req.timeoutInterval = 10
        req.setValue("Bearer \(secret)", forHTTPHeaderField: "Authorization")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: req)
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(code) else {
            if let text = String(data: data, encoding: .utf8), !text.isEmpty, code != 401 { throw RelayError.message(text) }
            throw RelayError.http(code)
        }
        return data
    }

    private func desk(_ path: String, method: String = "POST", body: [String: Any]? = nil) async throws -> DeskState? {
        let data = try await raw(path, method: method, body: body)
        return try? JSONDecoder().decode(DeskState.self, from: data)
    }

    func desktop() async throws -> DeskState? { try await desk("/v1/desktop", method: "GET") }
    func move(x: Double, y: Double) async throws -> DeskState? { try await desk("/v1/desktop/move", body: ["x": x, "y": y]) }
    func zone(_ id: String) async throws -> DeskState? { try await desk("/v1/desktop/zone", body: ["zone_id": id]) }
    func floor(_ id: String) async throws -> DeskState? { try await desk("/v1/desktop/floor", body: ["floor_id": id]) }
    func status(_ availability: String, message: String) async throws -> DeskState? {
        try await desk("/v1/desktop/status", body: ["availability": availability, "message": message])
    }
    func media(audio: Bool? = nil, video: Bool? = nil) async throws -> DeskState? {
        try await desk("/v1/desktop/media", body: ["audio": audio as Any? ?? NSNull(), "video": video as Any? ?? NSNull()])
    }
    func desktopCall() async throws -> DeskCall {
        try JSONDecoder().decode(DeskCall.self, from: try await raw("/v1/desktop/call", method: "GET"))
    }
    func handoff() async throws -> DeskCall {
        try JSONDecoder().decode(DeskCall.self, from: try await raw("/v1/desktop/handoff", method: "POST", body: [:]))
    }
    func reclaim() async throws { _ = try await raw("/v1/desktop/reclaim", method: "POST", body: [:]) }
    func home() async throws -> DeskState? { try await desk("/v1/desktop/home", body: [:]) }
    func nudge(_ id: String) async throws { _ = try await raw("/v1/desktop/nudge", method: "POST", body: ["user_id": id]) }

    private func request(_ path: String, method: String, body: [String: Any]? = nil) async throws -> RelayStatus? {
        guard let base = url, let secret, let endpoint = URL(string: base.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path) else {
            throw RelayError.notConfigured
        }
        var req = URLRequest(url: endpoint)
        req.httpMethod = method
        req.timeoutInterval = 10
        req.setValue("Bearer \(secret)", forHTTPHeaderField: "Authorization")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: req)
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(code) else { throw RelayError.http(code) }
        let status = data.isEmpty ? nil : try? JSONDecoder().decode(RelayStatus.self, from: data)
        publish(enabled: status?.enabled)
        return status
    }

    func status() async throws -> RelayStatus? { try await request("/v1/status", method: "GET") }

    func setEnabled(_ enabled: Bool) async throws -> RelayStatus? {
        try await request("/v1/enabled", method: "PUT", body: ["enabled": enabled])
    }

    func test() async throws -> RelayStatus? { try await request("/v1/test", method: "POST") }

    func register(deviceToken: String, name: String) async throws {
        _ = try await request("/v1/devices/\(deviceToken)", method: "PUT", body: ["enabled": true, "name": name])
    }
}
