import Foundation
import Security
import WidgetKit

final class RelayPairingStorage: RelayStorage, @unchecked Sendable {
    private let defaults = UserDefaults.standard
    private let urlKey = "relay.url"
    private let service = "com.kravtsov.kumowatch"
    private let account = "relay-secret"

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    func load() -> RelayPairing? {
        guard let url = defaults.string(forKey: urlKey), let secret = loadSecret() else { return nil }
        let shared = SharedRelay.load()
        if shared?.url != url || shared?.secret != secret {
            publish(url: url, secret: secret, enabled: shared?.enabled ?? false)
        }
        return RelayPairing(url: url, secret: secret)
    }

    func save(pairing: RelayPairing) {
        defaults.set(pairing.url, forKey: urlKey)
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData as String] = Data(pairing.secret.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(add as CFDictionary, nil)
        publish(url: pairing.url, secret: pairing.secret, enabled: SharedRelay.load()?.enabled ?? false)
    }

    func clear() {
        defaults.removeObject(forKey: urlKey)
        SecItemDelete(query as CFDictionary)
        SharedRelay.clear()
        WidgetCenter.shared.reloadAllTimelines()
    }

    func saveMirroring(enabled: Bool) {
        guard var shared = SharedRelay.load(), shared.enabled != enabled else { return }
        shared.enabled = enabled
        shared.save()
        WidgetCenter.shared.reloadAllTimelines()
    }

    private func loadSecret() -> String? {
        var q = query
        q[kSecReturnData as String] = true
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private func publish(url: String, secret: String, enabled: Bool) {
        SharedRelay(url: url, secret: secret, enabled: enabled).save()
        WidgetCenter.shared.reloadAllTimelines()
    }
}
