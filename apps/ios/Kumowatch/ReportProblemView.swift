import SwiftUI
import UIKit

struct ReportProblemView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var details = ""
    @State private var includeLog = true
    @State private var loading = true
    @State private var diagnostics: RelayDiagnostics?
    @State private var logFile: URL?
    @State private var openFailed = false
    @State private var opened = false
    @State private var attempt = 0

    var body: some View {
        let paired = model.relay.isPaired()
        NavigationStack {
            Form {
                Section {
                    Text("Describe the problem here. GitHub opens with it filled in: add screenshots there and tap Submit. The computer's recent log goes along unless you switch it off. Names, ids and paths it recognizes are removed, and GitHub gets the draft as soon as it opens, so look it over first.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("Title") {
                    TextField("Short summary", text: $title)
                }
                Section("What happened") {
                    TextField("What did you do, what did you expect, what happened instead?", text: $details, axis: .vertical)
                        .lineLimit(5...12)
                }
                Section {
                    Toggle("Include the computer's log", isOn: $includeLog)
                        .disabled(!paired)
                    if !paired {
                        Text("Pair a computer to include its log.")
                            .font(.footnote).foregroundStyle(.secondary)
                    } else if loading {
                        ProgressView()
                    } else if diagnostics?.log.isEmpty != false {
                        Text("The computer didn't send its log. It may be asleep or offline, or Better Kumo there may need an update.")
                            .font(.footnote).foregroundStyle(.secondary)
                        Button("Try again to get the log") { attempt += 1 }
                    }
                }
                Section {
                    Button("Open GitHub") {
                        // The sheet stays: GitHub may ask to sign in, and the log file is shared from here.
                        openURL(issueURL(paired: paired)) { accepted in
                            openFailed = !accepted
                            opened = accepted
                        }
                    }
                    .disabled((paired && loading && includeLog) || !hasText)
                    if opened {
                        Text("GitHub opened with your report. Attach screenshots and the log file there, then tap Submit. Your text stays here until you close this.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    if openFailed {
                        Text("iOS couldn't open GitHub. Your report is still here: try again.")
                            .font(.footnote).foregroundStyle(.red)
                    }
                    if let logFile {
                        ShareLink("Share the log file…", item: logFile)
                    }
                }
            }
            .navigationTitle("Report a problem")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(opened ? "Done" : "Cancel") { dismiss() } }
            }
        }
        .task(id: attempt) {
            guard paired else { loading = false; return }
            loading = true
            diagnostics = await RelayDiagnostics.fetch()
            loading = false
            if let log = diagnostics?.log, !log.isEmpty {
                let file = FileManager.default.temporaryDirectory.appendingPathComponent("better-kumo-log.txt")
                if (try? log.write(to: file, atomically: true, encoding: .utf8)) != nil { logFile = file }
            }
        }
    }

    private var hasText: Bool {
        !(title + details).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func issueURL(paired: Bool) -> URL {
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        let phone = "Better Kumo iOS \(version) (\(build)), \(Self.deviceModel), iOS \(UIDevice.current.systemVersion)"
        let computer = if let diagnostics {
            "Better Kumo \(diagnostics.version) on \(diagnostics.os)"
        } else {
            paired ? "log not available" : "not paired"
        }
        return ProblemReport.issueURL(title: title, description: details, phone: phone, computer: computer, log: includeLog ? diagnostics?.log : nil)
    }

    /// The model identifier, such as `iPhone17,1`; the simulator reports the one it simulates.
    private static var deviceModel: String {
        if let simulated = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] { return simulated }
        var system = utsname()
        uname(&system)
        return withUnsafeBytes(of: &system.machine) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
    }
}
