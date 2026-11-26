import SwiftUI

struct OfficeView: View {
    @Environment(AppModel.self) private var model
    @Binding var showCall: Bool
    @State private var query = ""
    @State private var expanded: Set<String> = []
    @State private var openDesktop = false

    var body: some View {
        NavigationStack {
            Group {
                if let view = model.spaceView, view.connected || !view.floors.isEmpty {
                    list(view)
                } else {
                    ProgressView()
                }
            }
            .navigationDestination(isPresented: $openDesktop) { DesktopView(showCall: $showCall) }
            .navigationTitle("Office")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    NavigationLink { ProfileView() } label: { Image(systemName: "person.crop.circle") }
                }
            }
            .searchable(text: $query, prompt: "Find a person")
        }
        .onAppear {
            model.openSpace(.tab)
            openRequestedDesktop()
        }
        .onDisappear { model.closeSpace(.tab) }
        .onChange(of: model.officeRequest) { openRequestedDesktop() }
    }

    private func openRequestedDesktop() {
        guard model.officeRequest == .desktop else { return }
        model.officeRequest = nil
        openDesktop = true
    }

    private func list(_ view: SpaceView) -> some View {
        List {
            if model.relay.isPaired() {
                Section {
                    DesktopCard { openDesktop = true }
                }
            }
            if model.call.isActive {
                Section {
                    HStack {
                        VStack(alignment: .leading) {
                            Text("In call").font(.caption).foregroundStyle(.secondary)
                            Text(model.call.roomName.isEmpty ? "Connecting…" : model.call.roomName).font(.headline)
                        }
                        Spacer()
                        Button("Open") { showCall = true }
                        Button("Leave", role: .destructive) { model.leaveCall() }
                    }
                    .buttonStyle(.borderless)
                }
            }
            if !query.isEmpty {
                let everyone = view.floors.flatMap { f in
                    f.rooms.flatMap { r in r.people.map { ($0, "\(f.name) · \(r.name)") } } + f.commonArea.map { ($0, f.name) }
                } + view.away.map { ($0, "Not in the office") }
                ForEach(everyone.filter { $0.0.name.localizedCaseInsensitiveContains(query) }, id: \.0.key) { person, place in
                    Button {
                        if model.joinPerson(person) { showCall = true }
                    } label: {
                        HStack {
                            StatusAvatar(name: person.name, url: person.avatarUrl, presence: person.presence)
                            VStack(alignment: .leading) {
                                Text(person.name + (person.isSelf ? " (you)" : ""))
                                Text(place).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    .foregroundStyle(.primary)
                    .disabled(person.isSelf)
                }
            } else {
                ForEach(view.floors, id: \.id) { floor in
                    Section(floor.name) {
                        ForEach(floor.rooms.filter { !$0.people.isEmpty }, id: \.zoneId) { room in
                            roomRow(room, mine: room.zoneId == view.selfZoneId)
                        }
                        if !floor.commonArea.isEmpty {
                            VStack(alignment: .leading) {
                                Text("Common area").font(.subheadline.weight(.semibold))
                                people(floor.commonArea)
                            }
                        }
                        let empty = floor.rooms.filter { $0.people.isEmpty && $0.open }
                        if !empty.isEmpty {
                            DisclosureGroup("\(empty.count) empty rooms", isExpanded: Binding(
                                get: { expanded.contains(floor.id) },
                                set: { if $0 { expanded.insert(floor.id) } else { expanded.remove(floor.id) } }
                            )) {
                                ForEach(empty, id: \.zoneId) { room in
                                    Button(room.name) {
                                        model.joinRoom(room)
                                        showCall = true
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func roomRow(_ room: RoomItem, mine: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(room.name).font(.headline)
                Spacer()
                if mine {
                    Text("You are here").font(.caption).foregroundStyle(.tint)
                } else {
                    Button(room.open ? "Join" : "Closed") {
                        model.joinRoom(room)
                        showCall = true
                    }
                    .buttonStyle(.bordered)
                    .disabled(!room.open)
                }
            }
            people(room.people)
        }
        .padding(.vertical, 4)
    }

    private func people(_ list: [PersonItem]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 12) {
                ForEach(list, id: \.key) { p in
                    VStack(spacing: 2) {
                        StatusAvatar(name: p.name, url: p.avatarUrl, presence: p.presence)
                        Text((p.name.split(separator: " ").first.map(String.init) ?? p.name) + (p.isSelf ? " (you)" : ""))
                            .font(.caption2).lineLimit(1)
                    }
                    .frame(width: 64)
                }
            }
        }
    }
}
