import Foundation
import Security
import WidgetKit

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

    var errorDescription: String? {
        switch self {
        case .notConfigured: "Relay is not set up"
        case .http(let code): code == 401 ? "Wrong relay secret" : "Relay returned \(code)"
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

    private let shared = UserDefaults(suiteName: "group.com.kravtsov.kumowatch")

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
