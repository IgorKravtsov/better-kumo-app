import SwiftUI

struct NearbyChatView: View {
    @Environment(AppModel.self) private var model
    @State private var chat: NearbyChat?
    @State private var error: String?
    @State private var draft = ""
    @State private var sending = false

    private var canSend: Bool {
        chat != nil && !sending && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                List {
                    if let chat, chat.messages.isEmpty {
                        Text("No messages in \(chat.zoneName) yet. Nearby Chat is not saved.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .listRowSeparator(.hidden)
                    }
                    ForEach(chat?.messages ?? []) { message in
                        NearbyBubble(message: message)
                            .id(message.id)
                            .listRowSeparator(.hidden)
                    }
                }
                .listStyle(.plain)
                .onChange(of: chat?.messages.last?.id) { _, last in
                    guard let last else { return }
                    withAnimation { proxy.scrollTo(last, anchor: .bottom) }
                }
            }
            if let error {
                Text(error).font(.caption).foregroundStyle(.red).padding(.horizontal)
            }
            HStack(alignment: .bottom) {
                TextField("Message \(chat?.zoneName ?? "Nearby")", text: $draft, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(1...5)
                Button(action: send) {
                    Image(systemName: "arrow.up.circle.fill").font(.title)
                }
                .disabled(!canSend)
                .accessibilityLabel("Send")
            }
            .padding()
        }
        .navigationTitle(chat?.zoneName ?? "Nearby Chat")
        .navigationBarTitleDisplayMode(.inline)
        .task { await poll() }
    }

    private func poll() async {
        while !Task.isCancelled {
            do {
                chat = try await model.relay.nearbyChat()
                error = nil
            } catch {
                self.error = describe(error)
            }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private func send() {
        let text = draft
        sending = true
        Task {
            do {
                try await model.relay.sendNearby(message: text)
                if draft == text { draft = "" }
                error = nil
                if let fresh = try? await model.relay.nearbyChat() { chat = fresh }
            } catch {
                self.error = describe(error)
            }
            sending = false
        }
    }
}

private struct NearbyBubble: View {
    let message: NearbyMessage

    var body: some View {
        HStack {
            if message.mine { Spacer(minLength: 48) }
            VStack(alignment: message.mine ? .trailing : .leading, spacing: 2) {
                if !message.mine {
                    Text(message.senderName).font(.caption).foregroundStyle(color(for: message.senderName))
                }
                Text(message.text)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(message.mine ? Color.accentColor : Color(.secondarySystemBackground))
                    .foregroundStyle(message.mine ? .white : .primary)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                Text(shortTime(message.sentAtMs)).font(.caption2).foregroundStyle(.secondary)
            }
            if !message.mine { Spacer(minLength: 48) }
        }
    }
}
