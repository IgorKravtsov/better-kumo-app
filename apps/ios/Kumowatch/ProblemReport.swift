import Foundation

struct RelayDiagnostics: Sendable, Equatable {
    var version: String
    var os: String
    var log: String

    /// Older relays answer 404.
    static func fetch() async -> RelayDiagnostics? {
        guard let relay = SharedRelay.load(),
              let reply = try? await relay.request("GET", "/v1/diagnostics", timeout: 5),
              let version = reply["version"] as? String, let os = reply["os"] as? String,
              let log = reply["log"] as? String else { return nil }
        return RelayDiagnostics(version: version, os: os, log: log)
    }
}

enum ProblemReport {
    static let newIssue = "https://github.com/IgorKravtsov/better-kumo-app/issues/new"
    static let maxURLLength = 7000

    static func issueURL(title: String, description: String, phone: String, computer: String, log: String?) -> URL {
        let title = cappedTitle(title.trimmingCharacters(in: .whitespacesAndNewlines))
        let description = description.trimmingCharacters(in: .whitespacesAndNewlines)
        var lines = (log ?? "").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        while lines.last?.isEmpty == true { lines.removeLast() }
        func url(_ text: String) -> String {
            build(title: title.isEmpty ? "Problem: " : title, description: text, phone: phone, computer: computer, log: lines)
        }
        while url(description).count > maxURLLength, !lines.isEmpty { lines.removeFirst() }
        guard url(description).count > maxURLLength else { return URL(string: url(description))! }
        var low = 0, high = description.count
        while low < high {
            let mid = (low + high + 1) / 2
            if url(String(description.prefix(mid)) + "…").count <= maxURLLength { low = mid } else { high = mid - 1 }
        }
        return URL(string: url(String(description.prefix(low)) + "…"))!
    }

    /// Counted in encoded bytes, by scalar: one Character can hold thousands of combining marks.
    static func cappedTitle(_ title: String, budget: Int = 1500) -> String {
        var left = budget
        var kept = String.UnicodeScalarView()
        for scalar in title.unicodeScalars {
            let cost = encode(String(scalar)).utf8.count
            if cost > left { break }
            left -= cost
            kept.append(scalar)
        }
        return String(kept)
    }

    private static func build(title: String, description: String, phone: String, computer: String, log: [String]) -> String {
        var body = """
        **What happened**
        \(description)
        <!-- Add screenshots here: tap "Paste, drop, or click to add files" below. -->

        **Environment**
        - Phone: \(phone)
        - Computer: \(computer)

        """
        if !log.isEmpty {
            body += """

            <details><summary>Computer log (last \(log.count) lines, names and ids removed)</summary>

            ```text
            \(log.joined(separator: "\n"))
            ```
            </details>

            """
        }
        return "\(newIssue)?title=\(encode(title))&body=\(encode(body))"
    }

    private static let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")

    private static func encode(_ text: String) -> String {
        text.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
    }
}
