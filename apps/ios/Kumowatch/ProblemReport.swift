import Foundation

struct RelayDiagnostics: Sendable, Equatable {
    var version: String
    var os: String
    var log: String

    /// Older relays answer 404.
    /// Gives up after 8 s in all: the request's own timeout only covers a silent connection.
    static func fetch() async -> RelayDiagnostics? {
        await withTaskGroup(of: RelayDiagnostics?.self) { group in
            group.addTask { await request() }
            group.addTask { try? await Task.sleep(for: .seconds(8)); return nil }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
    }

    private static func request() async -> RelayDiagnostics? {
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
        // Capped, so title and environment can never fill the link on their own.
        let phone = cappedTitle(phone, budget: 1000, characters: 300)
        let computer = cappedTitle(computer, budget: 1000, characters: 300)
        var lines = (log ?? "").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        while lines.last?.isEmpty == true { lines.removeLast() }
        func url(_ text: String) -> String {
            build(title: title.isEmpty ? "Problem: " : title, description: text, phone: phone, computer: computer, log: lines)
        }
        // Too long without any log: no line can fit, so skip trying each one.
        let all = lines
        lines = []
        if url(description).count <= maxURLLength { lines = all }
        while url(description).count > maxURLLength, !lines.isEmpty { lines.removeFirst() }
        guard url(description).count > maxURLLength else { return URL(string: url(description))! }
        var low = 0, high = description.count
        while low < high {
            let mid = (low + high + 1) / 2
            if url(String(description.prefix(mid)) + "…").count <= maxURLLength { low = mid } else { high = mid - 1 }
        }
        return URL(string: url(String(description.prefix(low)) + "…"))!
    }

    /// GitHub keeps 256 characters of a title; the link budget is counted in encoded bytes, by
    /// scalar: one Character can hold thousands of combining marks.
    static func cappedTitle(_ title: String, budget: Int = 1500, characters: Int = 256) -> String {
        var left = budget
        var kept = String.UnicodeScalarView()
        for (index, scalar) in title.unicodeScalars.enumerated() {
            let cost = encode(String(scalar)).utf8.count
            if cost > left || index >= characters { break }
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

            <details><summary>Computer log (last \(log.count) lines, redacted, check before submitting)</summary>

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
