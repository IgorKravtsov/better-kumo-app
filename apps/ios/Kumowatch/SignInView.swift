import AuthenticationServices
import AVFoundation
import SwiftUI

struct SignInView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.webAuthenticationSession) private var webAuth
    @State private var link = ""

    var body: some View {
        VStack(spacing: 16) {
            Spacer()
            Text("Better Kumo").font(.largeTitle.bold())
            Text("Sign in on kumospace.com, then tap “Open the desktop app”.")
                .font(.subheadline).multilineTextAlignment(.center).foregroundStyle(.secondary)
            Button {
                Task { await browserSignIn() }
            } label: {
                Text("Sign in with browser").frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent).controlSize(.large)
            .disabled(model.signingIn)

            Text("Or paste a sign-in link from another device").font(.footnote).padding(.top, 24)
            TextField("kumospace://login?token=…", text: $link)
                .textFieldStyle(.roundedBorder).textInputAutocapitalization(.never).autocorrectionDisabled()
            Button("Sign in with link") { Task { await model.signIn(link: link) } }
                .disabled(link.isEmpty || model.signingIn)
            if model.signingIn { ProgressView() }
            if let error = model.lastError { Text(error).foregroundStyle(.red).font(.footnote) }
            Spacer()
        }
        .padding(24)
    }

    private func browserSignIn() async {
        guard let url = URL(string: model.core.browserLoginUrl()) else { return }
        do {
            let callback = try await webAuth.authenticate(using: url, callbackURLScheme: "kumospace")
            await model.signIn(link: callback.absoluteString)
        } catch {
            if (error as? ASWebAuthenticationSessionError)?.code != .canceledLogin {
                model.lastError = error.localizedDescription
            }
        }
    }
}

struct SpacesView: View {
    @Environment(AppModel.self) private var model
    @State private var spaces: [SpaceInfo]?
    @State private var error: String?
    var onDone: (() -> Void)?

    var body: some View {
        Group {
            if let spaces {
                List(spaces, id: \.uid) { space in
                    Button {
                        model.select(space: space)
                        onDone?()
                    } label: {
                        HStack {
                            AvatarView(name: space.displayName, url: "")
                            VStack(alignment: .leading) {
                                Text(space.displayName)
                                Text("\(space.name) · \(space.memberCount) members").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    .foregroundStyle(.primary)
                }
            } else if let error {
                Text(error)
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Choose a space")
        .task {
            do { spaces = try await model.spaces() } catch { self.error = error.localizedDescription }
        }
    }
}

struct ProfileView: View {
    @Environment(AppModel.self) private var model
    @State private var shareLink: String?
    @State private var error: String?

    var body: some View {
        List {
            Section {
                HStack {
                    AvatarView(name: model.user?.displayName ?? "?", url: "")
                    VStack(alignment: .leading) {
                        Text(model.user?.displayName ?? "")
                        Text(model.user?.email ?? "").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            Section {
                NavigationLink("Space: \(model.space?.displayName ?? "-")") { SpacesView() }
                Button("Sign in on another device") {
                    Task {
                        do { shareLink = try await model.transferLink() } catch { self.error = error.localizedDescription }
                    }
                }
                if let shareLink {
                    ShareLink("Share one-time sign-in link", item: shareLink)
                }
                if let error { Text(error).foregroundStyle(.red) }
            }
            RelaySection()
            Section {
                Button("Sign out", role: .destructive) { model.signOut() }
            }
            Section { Text("Core \(model.core.coreVersion())").font(.caption2).foregroundStyle(.secondary) }
        }
        .navigationTitle("You")
    }
}

struct RelaySection: View {
    private static let kinds: [(label: String, key: WritableKeyPath<NotificationKinds, Bool>)] = [
        ("Messages", \.messages),
        ("@Mentions", \.mentions),
        ("Nudges", \.nudges),
        ("Knocks", \.knocks),
        ("Broadcasts", \.broadcasts),
        ("Public Chat", \.publicChat),
        ("Nearby Chat", \.nearbyChat),
        ("Joined the Floor", \.floorEntries),
    ]

    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @State private var status: RelayStatus?
    @State private var error: String?
    @State private var notice: String?
    @State private var busy = false
    @State private var scanning = false
    @State private var topic = ""
    @State private var pickedStatus: MyStatus?

    var body: some View {
        Section {
            if model.relay.isPaired() {
                LabeledContent("Relay", value: model.relay.pairedUrl() ?? "")
                if let status {
                    Toggle("Notifications", isOn: Binding(
                        get: { status.mirroring },
                        set: { value in Task { await run { try await model.relay.setMirroring(enabled: value) } } }
                    ))
                    .disabled(busy)
                    Text("The home screen widget flips this same switch.")
                        .font(.footnote).foregroundStyle(.secondary)
                    if status.mirroring {
                        let selected = pickedStatus ?? status.myStatus
                        let kinds = profileKinds(profile: status.notifications, status: selected)
                        Picker("Status", selection: Binding(get: { selected }, set: { pickedStatus = $0 })) {
                            ForEach(myStatusChoices(), id: \.self) { choice in
                                Text(myStatusLabel(status: choice)).tag(choice)
                            }
                        }
                        .pickerStyle(.segmented)
                        Text("Now: \(myStatusLabel(status: status.myStatus))")
                            .font(.footnote).foregroundStyle(.secondary)
                        Toggle("All for \(myStatusLabel(status: selected))", isOn: Binding(
                            get: { Self.kinds.allSatisfy { kinds[keyPath: $0.key] } },
                            set: { value in
                                var changed = kinds
                                for kind in Self.kinds { changed[keyPath: kind.key] = value }
                                Task { await run { try await model.relay.setNotificationKinds(status: selected, kinds: changed) } }
                            }
                        ))
                        .bold()
                        .disabled(busy)
                        ForEach(Self.kinds, id: \.label) { kind in
                            Toggle(kind.label, isOn: Binding(
                                get: { kinds[keyPath: kind.key] },
                                set: { value in
                                    var changed = kinds
                                    changed[keyPath: kind.key] = value
                                    Task { await run { try await model.relay.setNotificationKinds(status: selected, kinds: changed) } }
                                }
                            ))
                            .padding(.leading)
                            .disabled(busy)
                        }
                        Text("Unknown applies when the Desktop app isn't running, or you're idle or offline.")
                            .font(.footnote).foregroundStyle(.secondary)
                        Text("Nudges, Knocks, Broadcasts, Public Chat and Nearby Chat need the Desktop app running on your Mac.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    LabeledContent("Chat connection", value: status.chatOnline ? "Online" : "Offline")
                    Picker("Delivery", selection: .constant("ntfy")) {
                        Text("ntfy").tag("ntfy")
                        Text("This phone").tag("phone")
                    }
                    .pickerStyle(.segmented)
                    .disabled(true)
                    Text("This phone can't receive notifications on iOS yet, so they come through ntfy.")
                        .font(.footnote).foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("ntfy topic").font(.subheadline)
                        HStack {
                            TextField("Enter a topic", text: $topic)
                                .textFieldStyle(.roundedBorder)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .submitLabel(.done)
                                .onSubmit(saveTopic)
                            if topic.trimmingCharacters(in: .whitespaces) != (status.ntfyTopic ?? "") {
                                Button("Save", action: saveTopic)
                                    .buttonStyle(.borderedProminent)
                                    .disabled(busy)
                            }
                        }
                        Text(status.ntfyTopic == nil
                            ? "Nothing is sent until you enter a topic. Subscribe to the same topic in the ntfy app."
                            : "Subscribe to this topic in the ntfy app. Clear it to stop ntfy.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    if let name = status.ntfyTopic, let server = status.ntfyServer, let url = URL(string: "\(server)/\(name)") {
                        Link("Open topic in ntfy", destination: url)
                    }
                }
                Button("Send test notification") {
                    Task {
                        notice = nil
                        if await run({ try await model.relay.sendTestAlert() }) { notice = "Test notification sent" }
                    }
                }
                .disabled(busy)
                Button("Forget relay", role: .destructive) {
                    model.relay.forget()
                    status = nil
                    notice = nil
                }
            } else {
                Button {
                    Task { await openScanner() }
                } label: {
                    Label("Scan relay QR code", systemImage: "qrcode.viewfinder")
                }
                Text("Run `kumo-relay pair --url <address>` on your Mac, then scan its QR code or open the link here.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            if let notice {
                Text(notice).font(.footnote).foregroundStyle(.green)
            }
            if let message = error ?? model.pushError {
                Text(message).font(.footnote).foregroundStyle(.red)
            }
        } header: {
            Text("Notifications relay")
        }
        .task { await run { try await model.relay.status() } }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { Task { await run { try await model.relay.status() } } }
        }
        .onChange(of: status?.ntfyTopic, initial: true) { _, value in topic = value ?? "" }
        .sheet(isPresented: $scanning) {
            NavigationStack {
                QRScannerView { code in
                    scanning = false
                    Task { await pair(code) }
                }
                .ignoresSafeArea()
                .navigationTitle("Scan relay QR code")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { scanning = false } }
                }
            }
        }
    }

    private func saveTopic() {
        let value = topic.trimmingCharacters(in: .whitespaces)
        Task {
            notice = nil
            if await run({ try await model.relay.setNtfyTopic(topic: value.isEmpty ? nil : value) }) {
                notice = value.isEmpty ? "ntfy turned off" : "ntfy topic saved"
            }
        }
    }

    private func openScanner() async {
        error = nil
        guard QRScannerView.isSupported else {
            error = "This device can't scan QR codes. Open the pairing link instead."
            return
        }
        guard await AVCaptureDevice.requestAccess(for: .video) else {
            error = "Allow camera access in Settings to scan the QR code."
            return
        }
        scanning = true
    }

    private func pair(_ code: String) async {
        guard let url = URL(string: code) else { return }
        await model.handle(url: url)
        if model.relay.isPaired() {
            await run { try await model.relay.status() }
        } else {
            error = "That QR code isn't a relay pairing link."
        }
    }

    @discardableResult
    private func run(_ action: () async throws -> RelayStatus?) async -> Bool {
        guard model.relay.isPaired() else { return false }
        busy = true
        defer { busy = false }
        do {
            if let s = try await action() { status = s }
            error = nil
            return true
        } catch {
            self.error = describe(error)
            return false
        }
    }
}
