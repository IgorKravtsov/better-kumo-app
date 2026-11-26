import SwiftUI
import UIKit
import UserNotifications

@MainActor
final class PushDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    weak var model: AppModel? {
        didSet { deliverPending() }
    }
    private var pending: ((AppModel) -> Void)?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories([
            UNNotificationCategory(
                identifier: PersonAction.category,
                actions: [
                    UNNotificationAction(identifier: PersonAction.desktop, title: "Go to them on Mac", options: [.foreground]),
                    UNNotificationAction(identifier: PersonAction.phone, title: "Join them on iPhone", options: [.foreground]),
                ],
                intentIdentifiers: []
            )
        ])
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        Task { @MainActor in self.model?.didReceiveDeviceToken(token) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in self.model?.pushRegistrationFailed(error) }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let url = notification.request.content.userInfo["channel_url"] as? String
        let visible = await MainActor.run { self.model?.visibleChannel }
        if let url, url == visible { return [] }
        return [.banner, .sound, .list]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        let url = info["channel_url"] as? String
        let person = info["person_id"] as? String
        let action = response.actionIdentifier
        await MainActor.run {
            self.pending = { model in
                if let person {
                    switch action {
                    case PersonAction.desktop: model.openPerson(person, via: "desktop")
                    case PersonAction.phone: model.openPerson(person, via: "phone")
                    default: model.openPerson(person, via: nil)
                    }
                } else {
                    model.openRequest = url
                }
            }
            self.deliverPending()
        }
    }

    private func deliverPending() {
        guard let model, let pending else { return }
        self.pending = nil
        pending(model)
    }
}
