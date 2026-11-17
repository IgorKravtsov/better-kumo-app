import AppIntents
import SwiftUI
import WidgetKit

struct ToggleRelayIntent: AppIntent {
    static let title: LocalizedStringResource = "Toggle Kumospace notifications"
    static let openAppWhenRun = false

    func perform() async throws -> some IntentResult {
        guard var relay = SharedRelay.load() else { return .result() }
        let next = !relay.enabled
        guard let reply = try? await relay.request("PUT", "/v1/enabled", body: ["enabled": next]) else {
            return .result()
        }
        relay.enabled = reply["enabled"] as? Bool ?? next
        relay.save()
        return .result()
    }
}

struct RelayEntry: TimelineEntry {
    let date: Date
    let enabled: Bool
    let configured: Bool
    var reachable = true
}

private final class Completion: @unchecked Sendable {
    let call: (Timeline<RelayEntry>) -> Void

    init(call: @escaping (Timeline<RelayEntry>) -> Void) {
        self.call = call
    }
}

struct RelayProvider: TimelineProvider {
    func placeholder(in context: Context) -> RelayEntry { RelayEntry(date: .now, enabled: true, configured: true) }

    func getSnapshot(in context: Context, completion: @escaping (RelayEntry) -> Void) {
        completion(Self.entry(SharedRelay.load()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<RelayEntry>) -> Void) {
        let done = Completion(call: completion)
        Task {
            let (relay, reachable) = await Self.refreshed(SharedRelay.load())
            var entry = Self.entry(relay)
            entry.reachable = reachable
            let next: TimeInterval = reachable ? 15 * 60 : 60
            done.call(Timeline(entries: [entry], policy: .after(.now.addingTimeInterval(next))))
        }
    }

    private static func refreshed(_ relay: SharedRelay?) async -> (SharedRelay?, Bool) {
        guard var relay else { return (nil, true) }
        guard let reply = try? await relay.request("GET", "/v1/status"),
              let enabled = reply["enabled"] as? Bool else { return (relay, false) }
        if enabled != relay.enabled {
            relay.enabled = enabled
            relay.save()
        }
        return (relay, true)
    }

    private static func entry(_ relay: SharedRelay?) -> RelayEntry {
        RelayEntry(date: .now, enabled: relay?.enabled ?? false, configured: relay != nil)
    }
}

struct RelayWidgetView: View {
    let entry: RelayEntry

    var body: some View {
        Group {
            if entry.configured {
                Button(intent: ToggleRelayIntent()) { content }
                    .buttonStyle(.plain)
            } else {
                content
            }
        }
        .containerBackground(entry.enabled ? Color.indigo.gradient : Color.gray.gradient, for: .widget)
    }

    private var content: some View {
        VStack(spacing: 6) {
            Image(systemName: !entry.reachable ? "wifi.exclamationmark" : entry.enabled ? "bell.badge.fill" : "bell.slash.fill")
                .font(.system(size: 34, weight: .semibold))
                .contentTransition(.symbolEffect(.replace))
            Text(entry.configured ? (entry.enabled ? "Alerts on" : "Alerts off") : "Pair a relay in the app")
                .font(.subheadline.bold())
                .multilineTextAlignment(.center)
            if entry.configured {
                Text(!entry.reachable ? "Relay unreachable · tap to retry" : entry.enabled ? "Tap to mute" : "Tap to turn on")
                    .font(.caption2)
                    .opacity(0.8)
            }
        }
        .foregroundStyle(.white)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct RelayWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "RelayWidget", provider: RelayProvider()) { entry in
            RelayWidgetView(entry: entry)
        }
        .configurationDisplayName("Kumospace notifications")
        .description("Turn Kumospace notifications on or off.")
        .supportedFamilies([.systemSmall])
    }
}

@main
struct KumowatchWidgets: WidgetBundle {
    var body: some Widget {
        RelayWidget()
    }
}
