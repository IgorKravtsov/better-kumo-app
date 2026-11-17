import SwiftUI

enum ChatFilter: String, CaseIterable {
    case all = "All", unread = "Unread", channels = "Channels", direct = "Direct"
}

struct ChatListView: View {
    @Environment(AppModel.self) private var model
    @State private var filter = ChatFilter.all
    @State private var path: [String] = []
    @State private var picking = false

    private var visible: [ChannelItem] {
        model.channels.filter {
            switch filter {
            case .all: true
            case .unread: $0.unread > 0
            case .channels: $0.channelType == .channel
            case .direct: $0.channelType == .direct || $0.channelType == .group
            }
        }
    }

    var body: some View {
        NavigationStack(path: $path) {
            List(visible, id: \.url) { channel in
                NavigationLink(value: channel.url) { ChannelRowView(channel: channel) }
            }
            .listStyle(.plain)
            .safeAreaInset(edge: .top) {
                Picker("Filter", selection: $filter) {
                    ForEach(ChatFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented).padding(.horizontal).padding(.bottom, 6).background(.bar)
            }
            .navigationTitle(model.space?.displayName ?? "Chat")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 0) {
                        Text(model.space?.displayName ?? "Chat").font(.headline)
                        if let label = connectionLabel(model.connection) {
                            Text(label).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    NavigationLink { ProfileView() } label: { Image(systemName: "person.crop.circle") }
                }
                ToolbarItem(placement: .topBarLeading) {
                    Button { picking = true } label: { Image(systemName: "square.and.pencil") }
                }
            }
            .navigationDestination(for: String.self) { url in ChannelView(channelUrl: url) }
            .sheet(isPresented: $picking) {
                PeopleList(title: "New message", people: model.core.spacePeople()) { p in
                    Task {
                        if let url = try? await model.core.openDirect(userId: p.uid) { path = [url] }
                    }
                }
            }
            .onChange(of: model.openRequest) { _, url in
                guard let url else { return }
                path = [url]
                model.openRequest = nil
            }
        }
    }

    private func connectionLabel(_ status: ConnectionStatus) -> String? {
        switch status {
        case .online: nil
        case .syncing: "Updating…"
        case .connecting: "Connecting…"
        case .offline: "Offline"
        }
    }
}

struct ChannelRowView: View {
    let channel: ChannelItem

    var body: some View {
        HStack(spacing: 12) {
            switch channel.channelType {
            case .channel: iconAvatar("number")
            case .group: iconAvatar("person.3.fill")
            default: AvatarView(name: channel.title, url: channel.avatarUrl, online: channel.isOnline)
            }
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(channel.title).font(.body.weight(channel.unread > 0 ? .bold : .regular)).lineLimit(1)
                    Spacer()
                    Text(shortTime(channel.lastMessageAt)).font(.caption).foregroundStyle(.secondary)
                }
                HStack {
                    Text(preview).font(.subheadline).foregroundStyle(channel.unread > 0 ? .primary : .secondary).lineLimit(1)
                    Spacer()
                    if channel.unread > 0 {
                        Text(channel.mentions > 0 ? "@" : "\(channel.unread)")
                            .font(.caption2.bold()).foregroundStyle(.white)
                            .padding(.horizontal, 7).padding(.vertical, 2).background(Capsule().fill(.tint))
                    }
                }
            }
        }
    }

    private var preview: String {
        guard channel.channelType != .direct, !channel.lastSenderName.isEmpty else { return channel.lastMessageText }
        return "\(channel.lastSenderName.split(separator: " ").first ?? ""): \(channel.lastMessageText)"
    }

    private func iconAvatar(_ symbol: String) -> some View {
        Image(systemName: symbol).frame(width: 40, height: 40).background(Circle().fill(Color(.secondarySystemFill)))
    }
}
