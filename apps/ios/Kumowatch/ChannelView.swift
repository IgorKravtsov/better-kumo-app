import PhotosUI
import SwiftUI
import UIKit

private let page: UInt32 = 50
private let quickReactions = ["👍", "🙌", "😂", "❤️", "🔥", "👀"]

struct ChannelView: View {
    @Environment(AppModel.self) private var model
    let channelUrl: String
    var thread: MessageItem?
    @State private var limit: UInt32 = page
    @State private var messages: [MessageItem] = []
    @State private var ready = false
    @State private var loadingOlder = false
    @State private var reachedStart = false
    @State private var draft = ""
    @State private var atBottom = true
    @State private var mentions: [String: String] = [:]
    @State private var editing: MessageItem?
    @State private var openThread: MessageItem?
    @State private var photo: PhotosPickerItem?
    @State private var uploading = false
    @State private var members: [MemberItem] = []
    @State private var initialScrollDone = false
    @State private var initialUnread: Int?
    @State private var viewer: String?
    @State private var showDetails = false

    private var channel: ChannelItem? { model.channels.first { $0.url == channelUrl } }
    private var parentId: Int64? { thread?.id }

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        if let thread {
                            MessageRowView(message: thread, grouped: false, onReaction: { _ in }, onReplies: {})
                            Divider().padding(.vertical, 6)
                        } else {
                            Color.clear.frame(height: 1).onAppear {
                                if initialScrollDone { Task { await loadOlder() } }
                            }
                            if loadingOlder { ProgressView().frame(maxWidth: .infinity) }
                        }
                        ForEach(Array(messages.enumerated()), id: \.element.id) { index, message in
                            row(index: index, message: message)
                        }
                        Color.clear.frame(height: 1).id("bottom")
                            .onAppear { atBottom = true }
                            .onDisappear { atBottom = false }
                    }
                    .padding(.horizontal, 12)
                }
                .bottomAnchored()
                .overlay(alignment: .bottomTrailing) {
                    if !atBottom {
                        Button { withAnimation { proxy.scrollTo("bottom", anchor: .bottom) } } label: {
                            Image(systemName: "chevron.down").padding(10).background(Circle().fill(.regularMaterial))
                        }
                        .padding(12)
                    }
                }
                .overlay { if !ready { ProgressView() } }
                .onChange(of: messages.last?.id) {
                    if atBottom || !initialScrollDone { proxy.scrollTo("bottom", anchor: .bottom) }
                }
                .task(id: messages.isEmpty) {
                    guard !messages.isEmpty, !initialScrollDone else { return }
                    try? await Task.sleep(for: .milliseconds(50))
                    proxy.scrollTo("bottom", anchor: .bottom)
                    try? await Task.sleep(for: .milliseconds(400))
                    initialScrollDone = true
                }
            }
            suggestions
            if editing != nil {
                HStack {
                    Text("Editing message").font(.caption)
                    Spacer()
                    Button("Cancel") {
                        editing = nil
                        draft = ""
                    }
                    .font(.caption)
                }
                .padding(.horizontal, 12).padding(.vertical, 6)
                .background(Color(.secondarySystemBackground))
            }
            composer
        }
        .toolbar(.hidden, for: .tabBar)
        .navigationTitle(thread != nil ? "Thread" : (channel?.title ?? ""))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 0) {
                    Text(thread != nil ? "Thread" : (channel?.title ?? "")).font(.headline).lineLimit(1)
                        .onTapGesture { if thread == nil { showDetails = true } }
                    if let names = model.typing[channelUrl], !names.isEmpty {
                        Text(names.joined(separator: ", ") + (names.count == 1 ? " is typing…" : " are typing…"))
                            .font(.caption2).foregroundStyle(.tint)
                    } else if thread != nil {
                        Text(channel?.title ?? "").font(.caption2).foregroundStyle(.secondary)
                    } else if let count = channel?.memberCount, count > 2 {
                        Text("\(count) members").font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .navigationDestination(isPresented: Binding(get: { openThread != nil }, set: { if !$0 { openThread = nil } })) {
            if let openThread { ChannelView(channelUrl: channelUrl, thread: openThread) }
        }
        .fullScreenCover(isPresented: Binding(get: { viewer != nil }, set: { if !$0 { viewer = nil } })) {
            if let viewer { ImageViewer(url: viewer) }
        }
        .sheet(isPresented: $showDetails) {
            PeopleList(title: "\(channel?.title ?? "") · \(members.count + 1) members", people: members) { member in
                Task {
                    if let url = try? await model.core.openDirect(userId: member.uid), url != channelUrl {
                        model.openRequest = url
                    }
                }
            }
        }
        .onAppear {
            if initialUnread == nil { initialUnread = thread == nil ? Int(channel?.unread ?? 0) : 0 }
            members = model.core.channelMembers(channelUrl: channelUrl)
            reload()
            if thread == nil { model.core.openChannel(channelUrl: channelUrl) }
        }
        .onDisappear { if thread == nil { model.core.closeChannel(channelUrl: channelUrl) } }
        .onChange(of: model.messageRevision[channelUrl]) { reload() }
        .task {
            if let parentId {
                try? await model.core.loadThread(channelUrl: channelUrl, parentId: parentId)
                reload()
                ready = true
            }
            try? await Task.sleep(for: .seconds(8))
            ready = true
        }
        .onChange(of: photo) { _, item in
            guard let item else { return }
            photo = nil
            Task { await upload(item) }
        }
    }

    @ViewBuilder
    private func row(index: Int, message: MessageItem) -> some View {
        let previous: MessageItem? = index > 0 ? messages[index - 1] : nil
        if previous == nil || !sameDay(previous!.createdAt, message.createdAt) {
            Text(dayLabel(message.createdAt)).font(.caption)
                .padding(.horizontal, 10).padding(.vertical, 3)
                .background(Capsule().fill(Color(.secondarySystemFill)))
                .frame(maxWidth: .infinity).padding(.vertical, 6)
        }
        if let unread = initialUnread, unread > 0, index == messages.count - unread {
            UnreadDivider()
        }
        MessageRowView(
            message: message,
            grouped: grouped(previous, message),
            onReaction: { key in react(message, key) },
            onReplies: { if thread == nil { openThread = message } },
            onImage: { viewer = $0 }
        )
        .id(message.id)
        .contentShape(Rectangle())
        .onTapGesture {
            if message.state == .failed {
                Task { try? await model.core.retryMessage(channelUrl: channelUrl, localId: message.id) }
            }
        }
        .contextMenu { if message.state == .sent { menu(for: message) } }
    }

    @ViewBuilder
    private func menu(for message: MessageItem) -> some View {
        ControlGroup {
            ForEach(quickReactions, id: \.self) { key in
                Button(key) { react(message, key) }
            }
        }
        if thread == nil {
            Button { openThread = message } label: { Label("Reply in thread", systemImage: "bubble.left.and.bubble.right") }
        }
        if !message.text.isEmpty {
            Button { UIPasteboard.general.string = message.text } label: { Label("Copy text", systemImage: "doc.on.doc") }
        }
        if message.isMine && message.kind == "MESG" {
            Button {
                let (raw, map) = decodeMentions(message)
                mentions = map
                draft = raw
                editing = message
            } label: { Label("Edit", systemImage: "pencil") }
        }
        if message.isMine {
            Button(role: .destructive) {
                Task { try? await model.core.deleteMessage(channelUrl: channelUrl, messageId: message.id) }
            } label: { Label("Delete", systemImage: "trash") }
        }
    }

    private var mentionQuery: String? {
        guard let at = draft.lastIndex(of: "@") else { return nil }
        if at > draft.startIndex, !draft[draft.index(before: at)].isWhitespace { return nil }
        let query = String(draft[draft.index(after: at)...])
        return query.count <= 30 && !query.contains("\n") ? query : nil
    }

    @ViewBuilder
    private var suggestions: some View {
        if let query = mentionQuery {
            let matches = members.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }.prefix(6)
            if !matches.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(matches), id: \.uid) { m in
                        Button {
                            if let at = draft.lastIndex(of: "@") {
                                draft = String(draft[..<at]) + "@\(m.name) "
                                mentions[m.name] = m.uid
                            }
                        } label: {
                            HStack {
                                AvatarView(name: m.name, url: m.avatarUrl, size: 28, online: m.isOnline)
                                Text(m.name)
                                Spacer()
                            }
                            .padding(.horizontal, 12).padding(.vertical, 6)
                        }
                        .foregroundStyle(.primary)
                    }
                }
                .background(Color(.secondarySystemBackground))
            }
        }
    }

    private var composer: some View {
        let frozen = channel?.frozen == true
        let enabled = ready && !frozen
        return HStack(alignment: .bottom) {
            PhotosPicker(selection: $photo, matching: .images) {
                if uploading { ProgressView() } else { Image(systemName: "photo.badge.plus").font(.title3) }
            }
            .disabled(!enabled || uploading)
            TextField(
                frozen ? "Chat closed" : (enabled ? (thread != nil ? "Reply in thread" : "Message \(channel?.title ?? "")") : "Loading…"),
                text: $draft,
                axis: .vertical
            )
            .lineLimit(1...6)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 18).stroke(Color(.separator)))
            .disabled(!enabled)
            .onChange(of: draft) { old, new in
                if old.isEmpty != new.isEmpty {
                    Task { await model.core.setTyping(channelUrl: channelUrl, typing: !new.isEmpty) }
                }
            }
            Button(action: send) {
                Image(systemName: "arrow.up.circle.fill").font(.title)
            }
            .disabled(!enabled || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .padding(8)
        .background(.bar)
    }

    private func send() {
        var text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        for (name, uid) in mentions.sorted(by: { $0.key.count > $1.key.count }) {
            text = text.replacingOccurrences(of: "@\(name)", with: "@mention(type=user&id=\(uid))")
        }
        let target = editing
        draft = ""
        editing = nil
        mentions = [:]
        Task {
            await model.core.setTyping(channelUrl: channelUrl, typing: false)
            if let target {
                try? await model.core.editMessage(channelUrl: channelUrl, messageId: target.id, text: text)
            } else {
                try? await model.core.sendText(channelUrl: channelUrl, text: text, parentId: parentId)
            }
        }
    }

    private func react(_ message: MessageItem, _ key: String) {
        Task { try? await model.core.toggleReaction(channelUrl: channelUrl, messageId: message.id, key: key) }
    }

    private func upload(_ item: PhotosPickerItem) async {
        uploading = true
        defer { uploading = false }
        guard let data = try? await item.loadTransferable(type: Data.self), let image = UIImage(data: data) else { return }
        let maxSide: CGFloat = 1600
        let scale = min(1, maxSide / max(image.size.width, image.size.height))
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let resized = UIGraphicsImageRenderer(size: size).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        guard let jpeg = resized.jpegData(compressionQuality: 0.85) else { return }
        try? await model.core.sendFile(file: OutgoingFile(
            channelUrl: channelUrl,
            name: "photo-\(Int(Date().timeIntervalSince1970)).jpg",
            mimeType: "image/jpeg",
            data: jpeg,
            parentId: parentId
        ))
    }

    private func decodeMentions(_ message: MessageItem) -> (String, [String: String]) {
        var map: [String: String] = [:]
        let text = message.spans.map { span -> String in
            if let uid = span.mentionUserId, !uid.isEmpty, span.text.hasPrefix("@") {
                map[String(span.text.dropFirst())] = uid
            }
            return span.text
        }.joined()
        return (text, map)
    }

    private func grouped(_ previous: MessageItem?, _ message: MessageItem) -> Bool {
        guard let previous else { return false }
        return previous.senderId == message.senderId && message.createdAt - previous.createdAt < 5 * 60_000
            && sameDay(previous.createdAt, message.createdAt)
    }

    private func reload() {
        if let parentId {
            messages = model.core.thread(channelUrl: channelUrl, parentId: parentId)
        } else {
            messages = model.core.messages(channelUrl: channelUrl, limit: limit)
        }
        if !messages.isEmpty { ready = true }
    }

    private func loadOlder() async {
        guard thread == nil, !loadingOlder, !reachedStart, let oldest = messages.first?.createdAt else { return }
        loadingOlder = true
        defer { loadingOlder = false }
        let local = model.core.messagesBefore(channelUrl: channelUrl, beforeTs: oldest, limit: page)
        if local.count < Int(page) {
            let fetched = (try? await model.core.loadOlder(channelUrl: channelUrl, beforeTs: oldest)) ?? 0
            if fetched == 0 { reachedStart = true }
        }
        limit += page
        reload()
    }
}

struct MessageRowView: View {
    let message: MessageItem
    let grouped: Bool
    var onReaction: (String) -> Void = { _ in }
    var onReplies: () -> Void = {}
    var onImage: (String) -> Void = { _ in }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if grouped {
                Color.clear.frame(width: 36, height: 1)
            } else {
                AvatarView(name: message.senderName, url: message.senderAvatar, size: 36)
            }
            VStack(alignment: .leading, spacing: 3) {
                if !grouped {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(message.senderName).font(.subheadline.weight(.semibold))
                        Text(date(message.createdAt).formatted(date: .omitted, time: .shortened))
                            .font(.caption2).foregroundStyle(.secondary)
                        if message.updatedAt > message.createdAt + 1000 && message.kind == "MESG" {
                            Text("· edited").font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }
                if message.kind == "FILE" { fileContent }
                if !message.spans.isEmpty { Text(attributed).textSelection(.enabled) }
                ForEach(message.images, id: \.self) { url in
                    RemoteImage(url: url)
                        .frame(width: 240, height: 180, alignment: .leading)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .onTapGesture { onImage(url) }
                }
                if !message.reactions.isEmpty {
                    HStack(spacing: 4) {
                        ForEach(message.reactions, id: \.key) { r in
                            Button { onReaction(r.key) } label: {
                                Text("\(r.key) \(r.count)").font(.caption)
                                    .padding(.horizontal, 6).padding(.vertical, 2)
                                    .background(Capsule().fill(r.mine ? Color.accentColor.opacity(0.2) : Color(.secondarySystemFill)))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                if message.replyCount > 0 {
                    Button(action: onReplies) {
                        Text("\(message.replyCount) \(message.replyCount == 1 ? "reply" : "replies")").font(.caption.weight(.medium))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.tint)
                }
                switch message.state {
                case .pending: Text("Sending…").font(.caption2).foregroundStyle(.secondary)
                case .failed: Text("Not sent. Tap to retry").font(.caption2).foregroundStyle(.red)
                case .sent: EmptyView()
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.top, grouped ? 0 : 8)
    }

    @ViewBuilder
    private var fileContent: some View {
        if message.fileType.hasPrefix("image/"), let url = URL(string: message.fileUrl) {
            AsyncImage(url: url) { $0.resizable().scaledToFit() } placeholder: { Color(.secondarySystemFill) }
                .frame(maxWidth: 260, maxHeight: 260, alignment: .leading)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .onTapGesture { onImage(message.fileUrl) }
        } else {
            Label(message.fileName.isEmpty ? "File" : message.fileName, systemImage: "paperclip").foregroundStyle(.tint)
        }
    }

    private var attributed: AttributedString {
        var result = AttributedString()
        for span in message.spans {
            var part = AttributedString(span.text)
            var font = Font.body
            if span.code { font = .body.monospaced() }
            if span.bold || span.mentionUserId != nil { font = font.weight(.semibold) }
            if span.italic { font = font.italic() }
            part.font = font
            if span.strike { part.strikethroughStyle = .single }
            if span.code { part.backgroundColor = Color(.secondarySystemFill) }
            if span.mentionUserId != nil { part.foregroundColor = .accentColor }
            if let link = span.link, let url = URL(string: link) { part.link = url }
            result += part
        }
        return result
    }
}

private extension View {
    @ViewBuilder
    func bottomAnchored() -> some View {
        if #available(iOS 18.0, *) {
            self.defaultScrollAnchor(.bottom, for: .initialOffset)
                .defaultScrollAnchor(.bottom, for: .sizeChanges)
        } else {
            self.defaultScrollAnchor(.bottom)
        }
    }
}

struct UnreadDivider: View {
    var body: some View {
        HStack {
            Rectangle().fill(.red).frame(height: 1)
            Text("New messages").font(.caption2).foregroundStyle(.red)
            Rectangle().fill(.red).frame(height: 1)
        }
        .padding(.vertical, 6)
    }
}
