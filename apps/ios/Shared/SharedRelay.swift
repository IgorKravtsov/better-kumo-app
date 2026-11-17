import Foundation
import Security

struct SharedRelay: Codable, Equatable {
    var url: String
    var secret: String
    var enabled: Bool

    private static let account = "relay-shared"

    private static var query: [String: Any]? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "KumoKeychainGroup") as? String, !group.isEmpty else { return nil }
        return [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: group,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: group,
        ]
    }

    static func load() -> SharedRelay? {
        guard var q = query else { return nil }
        q[kSecReturnData as String] = true
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
        return try? JSONDecoder().decode(SharedRelay.self, from: data)
    }

    func save() {
        guard let q = Self.query, let data = try? JSONEncoder().encode(self) else { return }
        let update = [kSecValueData as String: data]
        if SecItemUpdate(q as CFDictionary, update as CFDictionary) == errSecItemNotFound {
            var add = q
            add[kSecValueData as String] = data
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    static func clear() {
        guard let q = query else { return }
        SecItemDelete(q as CFDictionary)
    }

    func request(_ method: String, _ path: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        guard let endpoint = URL(string: url.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path) else {
            throw URLError(.badURL)
        }
        var req = URLRequest(url: endpoint)
        req.httpMethod = method
        req.timeoutInterval = 8
        req.setValue("Bearer \(secret)", forHTTPHeaderField: "Authorization")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await URLSession.shared.data(for: req)
        guard let code = (response as? HTTPURLResponse)?.statusCode, (200..<300).contains(code) else {
            throw URLError(.badServerResponse)
        }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }
}
