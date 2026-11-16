import AppIntents
import SwiftUI
import WidgetKit

enum SharedRelay {
    static var suite: UserDefaults? { UserDefaults(suiteName: Bundle.main.object(forInfoDictionaryKey: "KumoAppGroup") as? String ?? "") }

    static var url: String? { suite?.string(forKey: "relay.url") }
    static var secret: String? { suite?.string(forKey: "relay.secret") }
    static var enabled: Bool {
        get { suite?.bool(forKey: "relay.enabled") ?? false }
        set { suite?.set(newValue, forKey: "relay.enabled") }
    }
}

struct ToggleRelayIntent: AppIntent {
    static let title: LocalizedStringResource = "Toggle Kumospace alerts"

    func perform() async throws -> some IntentResult {
        guard let base = SharedRelay.url, let secret = SharedRelay.secret,
              let url = URL(string: base.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + "/v1/enabled") else {
            return .result()
        }
        let next = !SharedRelay.enabled
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.timeoutInterval = 10
        req.setValue("Bearer \(secret)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: ["enabled": next])
        let (_, response) = try await URLSession.shared.data(for: req)
        if let code = (response as? HTTPURLResponse)?.statusCode, (200..<300).contains(code) {
            SharedRelay.enabled = next
        }
        return .result()
    }
}

struct RelayEntry: TimelineEntry {
    let date: Date
    let enabled: Bool
    let configured: Bool
}

struct RelayProvider: TimelineProvider {
    func placeholder(in context: Context) -> RelayEntry { RelayEntry(date: .now, enabled: true, configured: true) }

    func getSnapshot(in context: Context, completion: @escaping (RelayEntry) -> Void) {
        completion(current())
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<RelayEntry>) -> Void) {
        completion(Timeline(entries: [current()], policy: .never))
    }

    private func current() -> RelayEntry {
        RelayEntry(date: .now, enabled: SharedRelay.enabled, configured: SharedRelay.url != nil)
    }
}

struct RelayWidgetView: View {
    let entry: RelayEntry

    var body: some View {
        Button(intent: ToggleRelayIntent()) {
            HStack(spacing: 8) {
                Image(systemName: entry.enabled ? "bell.fill" : "bell.slash.fill")
                Text(entry.configured ? (entry.enabled ? "Kumo alerts: on" : "Kumo alerts: off") : "Set up relay")
                    .font(.subheadline.bold())
            }
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .buttonStyle(.plain)
        .disabled(!entry.configured)
        .containerBackground(entry.enabled ? Color.indigo : Color.gray, for: .widget)
    }
}

struct RelayWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "RelayWidget", provider: RelayProvider()) { entry in
            RelayWidgetView(entry: entry)
        }
        .configurationDisplayName("Kumospace alerts")
        .description("Turn mirrored Kumospace notifications on or off.")
        .supportedFamilies([.systemSmall])
    }
}

@main
struct KumowatchWidgets: WidgetBundle {
    var body: some Widget {
        RelayWidget()
    }
}
