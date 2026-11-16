import AuthenticationServices
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
    @Environment(AppModel.self) private var model
    @State private var status: RelayStatus?
    @State private var error: String?
    @State private var busy = false

    var body: some View {
        Section {
            if model.relay.isConfigured {
                LabeledContent("Relay", value: model.relay.url ?? "")
                if let status {
                    Toggle("Mirror notifications", isOn: Binding(
                        get: { status.enabled },
                        set: { value in Task { await run { try await model.relay.setEnabled(value) } } }
                    ))
                    .disabled(busy)
                    LabeledContent("Chat connection", value: status.online ? "Online" : "Offline")
                    LabeledContent("Delivery", value: [status.ntfy ? "ntfy" : nil, status.apns ? "push" : nil].compactMap { $0 }.joined(separator: " + "))
                }
                Button("Send test notification") { Task { await run { try await model.relay.test() } } }
                Button("Forget relay", role: .destructive) {
                    model.relay.forget()
                    status = nil
                }
            } else {
                Text("Run `kumo-relay pair --url <address>` on your Mac and open the link here.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            if let message = error ?? model.pushError {
                Text(message).font(.footnote).foregroundStyle(.red)
            }
        } header: {
            Text("Notifications relay")
        }
        .task { await run { try await model.relay.status() } }
    }

    private func run(_ action: () async throws -> RelayStatus?) async {
        guard model.relay.isConfigured else { return }
        busy = true
        defer { busy = false }
        do {
            if let s = try await action() { status = s }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}
