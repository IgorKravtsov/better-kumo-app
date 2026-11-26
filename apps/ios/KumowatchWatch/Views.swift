import SwiftUI

struct HomeView: View {
    @Environment(WatchModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            List {
                if !model.reachable {
                    Label(
                        model.heardFromPhone ? "iPhone not reachable" : "Waiting for the iPhone",
                        systemImage: "iphone.slash"
                    )
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                }
                switch model.snapshot.setup {
                case .signIn where model.heardFromPhone:
                    Text("Sign in to Better Kumo on the iPhone").font(.footnote)
                case .pickSpace:
                    Text("Pick a space in Better Kumo on the iPhone").font(.footnote)
                case .signIn:
                    Text("Open Better Kumo on the iPhone once to connect").font(.footnote)
                case .ready:
                    ready
                }
            }
            .navigationTitle(model.snapshot.spaceName.isEmpty ? "Better Kumo" : model.snapshot.spaceName)
        }
        .alert(model.alert ?? "", isPresented: Binding(
            get: { model.alert != nil },
            set: { if !$0 { model.alert = nil } }
        )) {
            Button("OK", role: .cancel) {}
        }
    }

    @ViewBuilder
    private var ready: some View {
        if let call = model.snapshot.call {
            NavigationLink {
                CallView()
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Label(call.roomName.isEmpty ? "Call" : call.roomName, systemImage: "phone.fill")
                        .font(.headline)
                    Text(callStatus(call)).font(.footnote).foregroundStyle(.secondary)
                }
            }
            .listItemTint(.green)
        } else if model.snapshot.relayPaired {
            Button {
                model.act(.takeDesktopCall)
            } label: {
                Label("Take desktop call", systemImage: "desktopcomputer")
            }
        }
        NavigationLink {
            OfficeView()
        } label: {
            Label("Office", systemImage: "building.2")
        }
        NavigationLink {
            ChatsView()
        } label: {
            HStack {
                Label("Chats", systemImage: "bubble.left.and.bubble.right")
                Spacer()
                let unread = model.snapshot.chats.filter { $0.unread > 0 }.count
                if unread > 0 { Badge(count: unread) }
            }
        }
    }
}

func callStatus(_ call: WatchCall) -> String {
    switch call.phase {
    case .waiting, .joining: return "Connecting…"
    case .failed: return call.failure ?? "Call failed"
    case .joined:
        let inCall = call.people.filter(\.inCall).count
        return "\(inCall) in call" + (call.fromDesktop ? " · from desktop" : "")
    }
}

struct Badge: View {
    let count: Int

    var body: some View {
        Text("\(count)")
            .font(.caption2.bold())
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(.red))
    }
}

struct CallView: View {
    @Environment(WatchModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        if let call = model.snapshot.call {
            List {
                Section {
                    HStack {
                        Button {
                            model.act(.setMic(on: !call.micOn))
                        } label: {
                            Image(systemName: call.micOn ? "mic.fill" : "mic.slash.fill")
                                .frame(maxWidth: .infinity)
                        }
                        .tint(call.micOn ? .blue : .gray)
                        Button(role: .destructive) {
                            model.act(.leaveCall)
                            dismiss()
                        } label: {
                            Image(systemName: "phone.down.fill").frame(maxWidth: .infinity)
                        }
                        .tint(.red)
                    }
                    .buttonStyle(.borderedProminent)
                    .listRowBackground(Color.clear)
                } footer: {
                    Text(callStatus(call) + " · audio on iPhone")
                }
                Section("People") {
                    ForEach(call.people) { person in
                        HStack {
                            Circle()
                                .fill(person.speaking ? Color.green : Color.clear)
                                .frame(width: 8, height: 8)
                            Text(person.isLocal ? "\(person.name) (you)" : person.name)
                                .foregroundStyle(person.inCall ? .primary : .secondary)
                                .lineLimit(1)
                            Spacer()
                            if !person.micOn && person.inCall {
                                Image(systemName: "mic.slash").font(.caption2).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            .navigationTitle(call.roomName.isEmpty ? "Call" : call.roomName)
        } else {
            Text("Not in a call")
                .foregroundStyle(.secondary)
                .navigationTitle("Call")
        }
    }
}

struct OfficeView: View {
    @Environment(WatchModel.self) private var model
    @State private var joinedCall = false

    var body: some View {
        List {
            if model.snapshot.floors.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            }
            ForEach(model.snapshot.floors) { floor in
                Section(floor.name) {
                    ForEach(floor.rooms.filter { !$0.people.isEmpty || $0.isMine }) { room in
                        NavigationLink {
                            RoomView(room: room, joined: $joinedCall)
                        } label: {
                            RoomRow(room: room)
                        }
                    }
                    if !floor.commonArea.isEmpty {
                        ForEach(floor.commonArea) { person in
                            Button {
                                join(.joinPerson(personId: person.id, mic: false))
                            } label: {
                                Label(person.name, systemImage: "figure.walk")
                            }
                            .disabled(person.isSelf)
                        }
                    }
                    let empty = floor.rooms.filter { $0.people.isEmpty && !$0.isMine && $0.open }
                    if !empty.isEmpty {
                        NavigationLink("\(empty.count) empty rooms") {
                            List(empty) { room in
                                Button(room.name) { join(.joinRoom(floorId: room.floorId, zoneId: room.zoneId)) }
                            }
                            .navigationTitle(floor.name)
                        }
                        .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .navigationTitle("Office")
        .navigationDestination(isPresented: $joinedCall) { CallView() }
        .task {
            while !Task.isCancelled {
                await model.send(.openOffice, quiet: true)
                try? await Task.sleep(for: .seconds(WatchLink.officeLease / 3))
            }
        }
        .onDisappear { Task { await model.send(.closeOffice, quiet: true) } }
    }

    private func join(_ request: WatchRequest) {
        Task {
            if let reply = await model.send(request), reply.error == nil { joinedCall = true }
        }
    }
}

struct RoomRow: View {
    let room: WatchRoom

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(room.name).lineLimit(1)
                if !room.open { Image(systemName: "lock.fill").font(.caption2) }
                if room.isMine { Image(systemName: "person.fill.checkmark").font(.caption2).foregroundStyle(.green) }
            }
            if !room.people.isEmpty {
                Text(room.people.map(\.name).joined(separator: ", "))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
    }
}

struct RoomView: View {
    @Environment(WatchModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let room: WatchRoom
    @Binding var joined: Bool

    var body: some View {
        List {
            Button {
                Task {
                    if let reply = await model.send(.joinRoom(floorId: room.floorId, zoneId: room.zoneId)),
                       reply.error == nil {
                        dismiss()
                        joined = true
                    }
                }
            } label: {
                Label("Join room", systemImage: "phone.fill")
            }
            .listItemTint(.green)
            .disabled(model.busy)
            ForEach(room.people) { person in
                Text(person.isSelf ? "\(person.name) (you)" : person.name)
            }
        }
        .navigationTitle(room.name)
    }
}

struct ChatsView: View {
    @Environment(WatchModel.self) private var model

    var body: some View {
        List {
            if model.snapshot.chats.isEmpty {
                Text("No chats yet").foregroundStyle(.secondary)
            }
            ForEach(model.snapshot.chats) { chat in
                NavigationLink {
                    ChannelView(chat: chat)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack {
                            Text(chat.title).font(.headline).lineLimit(1)
                            Spacer()
                            if chat.mentions > 0 {
                                Text("@").font(.caption2.bold()).foregroundStyle(.orange)
                            }
                            if chat.unread > 0 { Badge(count: Int(chat.unread)) }
                        }
                        Text(chat.preview).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
            }
        }
        .navigationTitle("Chats")
    }
}

struct ChannelView: View {
    @Environment(WatchModel.self) private var model
    let chat: WatchChat
    @State private var messages: [WatchMessage] = []
    @State private var loaded = false
    @State private var draft = ""

    var body: some View {
        ScrollViewReader { proxy in
            List {
                if !loaded {
                    ProgressView().frame(maxWidth: .infinity)
                }
                ForEach(messages) { message in
                    VStack(alignment: message.isMine ? .trailing : .leading, spacing: 2) {
                        if !message.isMine {
                            Text(message.sender).font(.caption2).foregroundStyle(.secondary)
                        }
                        Text(message.text)
                            .font(.footnote)
                            .opacity(message.pending ? 0.6 : 1)
                    }
                    .frame(maxWidth: .infinity, alignment: message.isMine ? .trailing : .leading)
                    .listRowBackground(
                        RoundedRectangle(cornerRadius: 10).fill(message.isMine ? Color.blue.opacity(0.35) : Color.gray.opacity(0.2))
                    )
                    .id(message.id)
                }
                Section {
                    TextField("Reply", text: $draft)
                        .onSubmit { send(draft) }
                    ForEach(WatchLink.quickReplies, id: \.self) { reply in
                        Button(reply) { send(reply) }
                    }
                }
                .id("reply")
            }
            .onChange(of: messages.last?.id) { _, id in
                if let id { proxy.scrollTo(id, anchor: .bottom) }
            }
        }
        .navigationTitle(chat.title)
        .task {
            while !Task.isCancelled {
                if let reply = await model.send(.messages(channelUrl: chat.url), quiet: true), let new = reply.messages {
                    messages = new
                }
                loaded = true
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    private func send(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        Task {
            if let reply = await model.send(.send(channelUrl: chat.url, text: text)), let new = reply.messages {
                messages = new
            }
        }
    }
}
