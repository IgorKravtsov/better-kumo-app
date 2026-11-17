import SwiftUI

struct AvatarView: View {
    let name: String
    let url: String
    var size: CGFloat = 40
    var online = false

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            ZStack {
                Circle().fill(color(for: name))
                Text(initials(name)).font(.system(size: size * 0.38, weight: .medium)).foregroundStyle(.white)
                if let u = URL(string: url), !url.isEmpty {
                    AsyncImage(url: u) { image in
                        image.resizable().scaledToFill()
                    } placeholder: {
                        Color.clear
                    }
                }
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
            if online {
                Circle().fill(.green).frame(width: size / 4, height: size / 4)
                    .overlay(Circle().stroke(Color(.systemBackground), lineWidth: 2))
            }
        }
    }
}

private let palette: [Color] = [.indigo, .teal, .orange, .purple, .blue, .pink, .green, .brown]

func color(for name: String) -> Color {
    let hash = name.unicodeScalars.reduce(0) { ($0 &* 31) &+ Int($1.value) }
    return palette[Int(UInt(bitPattern: hash) % UInt(palette.count))]
}

func initials(_ name: String) -> String {
    let parts = name.split(whereSeparator: { " ._".contains($0) }).prefix(2)
    let value = parts.compactMap { $0.first.map { String($0).uppercased() } }.joined()
    return value.isEmpty ? "?" : value
}

func date(_ ms: Int64) -> Date { Date(timeIntervalSince1970: TimeInterval(ms) / 1000) }

func shortTime(_ ms: Int64) -> String {
    guard ms > 0 else { return "" }
    let d = date(ms)
    if Calendar.current.isDateInToday(d) { return d.formatted(date: .omitted, time: .shortened) }
    return d.formatted(.dateTime.day().month(.abbreviated))
}

func dayLabel(_ ms: Int64) -> String {
    let d = date(ms)
    if Calendar.current.isDateInToday(d) { return "Today" }
    if Calendar.current.isDateInYesterday(d) { return "Yesterday" }
    return d.formatted(.dateTime.weekday(.wide).day().month(.wide).year())
}

func sameDay(_ a: Int64, _ b: Int64) -> Bool {
    Calendar.current.isDate(date(a), inSameDayAs: date(b))
}

func statusColor(_ presence: Presence) -> Color {
    switch presence {
    case .available: .green
    case .away: .orange
    case .focusing: .purple
    case .idle, .offline: .gray
    }
}

func describe(_ error: Error) -> String {
    guard let relay = error as? RelayError else { return error.localizedDescription }
    switch relay {
    case .NotPaired(let message), .Unauthorized(let message), .Rejected(let message), .Unavailable(let message), .Failed(let message):
        return message
    }
}

extension DesktopFloor: Identifiable {}
extension DesktopZone: Identifiable {}
extension DesktopPerson: Identifiable {}

struct StatusAvatar: View {
    let name: String
    let url: String
    var size: CGFloat = 40
    let presence: Presence
    var emphasize = false

    var body: some View {
        AvatarView(name: name, url: url, size: size)
            .padding(2)
            .overlay(Circle().stroke(statusColor(presence), lineWidth: emphasize ? 3 : 2))
    }
}

struct StatusLegend: View {
    var body: some View {
        HStack(spacing: 12) {
            ForEach(presenceLegend(), id: \.self) { presence in
                HStack(spacing: 3) {
                    Circle().fill(statusColor(presence)).frame(width: 8, height: 8)
                    Text(presenceLabel(presence: presence))
                }
            }
        }
        .font(.caption2)
    }
}
