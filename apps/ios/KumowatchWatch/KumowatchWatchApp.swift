import SwiftUI
import UserNotifications
import WatchKit

@main
struct KumowatchWatchApp: App {
    @WKApplicationDelegateAdaptor(WatchAppDelegate.self) private var delegate
    @State private var model = WatchModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            HomeView()
                .environment(model)
                .onAppear { delegate.model = model }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { model.refresh() }
                }
        }
    }
}

@MainActor
final class WatchAppDelegate: NSObject, WKApplicationDelegate, UNUserNotificationCenterDelegate {
    weak var model: WatchModel? {
        didSet { deliverPending() }
    }
    private var pending: WatchRequest?

    func applicationDidFinishLaunching() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories([
            UNNotificationCategory(
                identifier: PersonAction.category,
                actions: [
                    UNNotificationAction(identifier: PersonAction.phone, title: "Join on iPhone", options: [.foreground]),
                    UNNotificationAction(identifier: PersonAction.desktop, title: "Go to them on Mac", options: [.foreground]),
                ],
                intentIdentifiers: []
            )
        ])
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound, .list]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let person = response.notification.request.content.userInfo["person_id"] as? String else { return }
        let request: WatchRequest? = switch response.actionIdentifier {
        case PersonAction.phone: .joinPerson(personId: person, mic: true)
        case PersonAction.desktop: .joinPersonOnDesktop(personId: person)
        default: nil
        }
        guard let request else { return }
        await MainActor.run {
            self.pending = request
            self.deliverPending()
        }
    }

    private func deliverPending() {
        guard let model, let pending else { return }
        self.pending = nil
        Task {
            // A cold launch from the notification may not be reachable yet.
            for _ in 0..<10 where !model.reachable {
                try? await Task.sleep(for: .milliseconds(300))
            }
            await model.send(pending)
        }
    }
}
