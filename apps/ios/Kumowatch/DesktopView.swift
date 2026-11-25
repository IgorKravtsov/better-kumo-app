import SwiftUI

struct DesktopView: View {
    @Environment(AppModel.self) private var model
    @Binding var showCall: Bool
    @State private var statusDraft: String?
    @State private var query = ""
    @State private var searching = false

    var body: some View {
        Group {
            if let desk = model.desktop.state {
                content(desk)
            } else if let error = model.desktop.error {
                Text(error).padding()
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Desktop")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { model.relay.watchDesktop() }
        .onDisappear { model.relay.unwatchDesktop() }
    }

    private func act(_ block: @escaping @Sendable (RelayClient) async throws -> Void) {
        let relay = model.relay
        Task { try? await block(relay) }
    }

    private func content(_ d: DesktopState) -> some View {
        let term = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return List {
            if !searching {
                Section {
                    VStack(alignment: .leading) {
                        Text(d.place).font(.headline)
                        Text(d.floorName).font(.caption).foregroundStyle(.secondary)
                        if let error = model.desktop.error { Text(error).font(.caption).foregroundStyle(.red) }
                    }
                    FloorMapView(desk: d) { x, y in act { try await $0.tap(x: x, y: y) } }
                        .listRowInsets(EdgeInsets(top: 4, leading: 8, bottom: 4, trailing: 8))
                    HStack {
                        Button { act { try await $0.goHome() } } label: {
                            Image(systemName: "house.fill")
                        }
                        .buttonStyle(.bordered)
                        Button { act { try await $0.setMedia(audio: !d.audio, video: nil) } } label: {
                            Image(systemName: d.audio ? "mic.fill" : "mic.slash.fill")
                        }
                        .buttonStyle(.bordered)
                        Button { act { try await $0.setMedia(audio: nil, video: !d.video) } } label: {
                            Image(systemName: d.video ? "video.fill" : "video.slash.fill")
                        }
                        .buttonStyle(.bordered)
                        Picker("Status", selection: Binding(
                            get: { d.availability ?? .available },
                            set: { value in
                                let message = statusDraft ?? d.statusMessage
                                act { try await $0.setStatus(availability: value, message: message) }
                            }
                        )) {
                            ForEach(availabilityChoices(), id: \.self) { Text(availabilityLabel(availability: $0)).tag($0) }
                        }
                        .pickerStyle(.menu)
                    }
                    HStack {
                        TextField("Status message", text: Binding(
                            get: { statusDraft ?? d.statusMessage },
                            set: { statusDraft = String($0.prefix(100)) }
                        ))
                        .submitLabel(.done)
                        Button("Save") {
                            let text = statusDraft ?? ""
                            statusDraft = nil
                            act { try await $0.setStatusMessage(message: text) }
                        }
                        .disabled(statusDraft == nil || statusDraft == d.statusMessage)
                    }
                    if d.floors.count > 1 {
                        Picker("Floor", selection: Binding(
                            get: { d.floorId ?? "" },
                            set: { id in act { try await $0.switchFloor(floorId: id) } }
                        )) {
                            ForEach(d.floors) { Text($0.name).tag($0.id) }
                        }
                    }
                }
                Section {
                    let inCall = model.call.isDesktop
                    Button {
                        if !inCall { model.joinDesktopCall() }
                        showCall = true
                    } label: {
                        Label(inCall ? "Open call" : "Use phone for audio and video", systemImage: inCall ? "phone.fill" : "iphone.radiowaves.left.and.right")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.call.isOffice)
                    .alignmentGuide(.listRowSeparatorLeading) { _ in 0 }
                    NavigationLink {
                        NearbyChatView()
                    } label: {
                        Label("Nearby Chat in \(d.place)", systemImage: "bubble.left.and.bubble.right.fill")
                    }
                }
                Section {
                    StatusLegend()
                }
            }
            Section {
                PeopleSearchHeader(query: $query, searching: $searching)
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 0, trailing: 16))
                if searching, let error = model.desktop.error {
                    Text(error).font(.caption).foregroundStyle(.red)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 16))
                }
            }
            .listSectionSpacing(8)
            Section {
                let people = d.people.filter { p in
                    !p.isSelf && (term.isEmpty || p.name.localizedCaseInsensitiveContains(term) || p.place.localizedCaseInsensitiveContains(term))
                }
                if people.isEmpty, !term.isEmpty {
                    Text("No one matches \"\(term)\"").foregroundStyle(.secondary)
                }
                ForEach(people) { p in
                    HStack {
                        StatusAvatar(name: p.name, url: p.avatarUrl, presence: p.presence)
                        VStack(alignment: .leading) {
                            Text(p.name)
                            Text(p.statusLine)
                                .font(.caption).foregroundStyle(statusColor(p.presence)).lineLimit(1)
                            Text(p.place).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        HStack(spacing: 8) {
                            Button {
                                let id = p.id
                                act { try await $0.goTo(personId: id) }
                            } label: {
                                Image(systemName: "figure.walk")
                                    .frame(width: 22, height: 22)
                            }
                            .buttonStyle(.bordered)
                            .accessibilityLabel("Go to \(p.name)")
                            Button {
                                let id = p.id
                                act { try await $0.nudge(userId: id) }
                            } label: {
                                Image(systemName: "hand.wave.fill")
                                    .frame(width: 22, height: 22)
                            }
                            .buttonStyle(.bordered)
                            .tint(.orange)
                            .accessibilityLabel("Nudge \(p.name)")
                        }
                    }
                }
            }
        }
    }
}

private struct PeopleSearchHeader: View {
    @Binding var query: String
    @Binding var searching: Bool
    @FocusState private var focused: Bool
    @Namespace private var morph

    var body: some View {
        HStack(spacing: 8) {
            if searching {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(.secondary)
                        .matchedGeometryEffect(id: "icon", in: morph)
                    TextField("Find a person", text: $query)
                        .focused($focused)
                        .submitLabel(.search)
                        .autocorrectionDisabled()
                        .foregroundStyle(.primary)
                        .onAppear { focused = true }
                        .transition(Self.contentIn)
                    Button(action: close) {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                    }
                    .buttonStyle(.borderless)
                    .accessibilityLabel("Close search")
                    .transition(Self.contentIn)
                }
                .font(.body)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(minHeight: 40)
                .background(Capsule().fill(Color(.tertiarySystemFill)).matchedGeometryEffect(id: "background", in: morph))
                .transition(.identity)
            } else {
                Text("People on this floor")
                    .font(.headline)
                    .foregroundStyle(.secondary)
                    .accessibilityAddTraits(.isHeader)
                    .transition(.asymmetric(
                        insertion: .opacity.combined(with: .offset(x: -16)).animation(.easeOut(duration: 0.25).delay(0.1)),
                        removal: .opacity.combined(with: .offset(x: -16)).animation(.easeIn(duration: 0.12))
                    ))
                Spacer()
                Button(action: open) {
                    Image(systemName: "magnifyingglass")
                        .font(.body.weight(.semibold))
                        .matchedGeometryEffect(id: "icon", in: morph)
                        .frame(width: 34, height: 34)
                        .background(Circle().fill(Color(.tertiarySystemFill)).matchedGeometryEffect(id: "background", in: morph))
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Search people")
            }
        }
        .onChange(of: focused) {
            if !focused, query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { close() }
        }
    }

    private static let contentIn = AnyTransition.asymmetric(
        insertion: .opacity.animation(.easeOut(duration: 0.2).delay(0.15)),
        removal: .opacity.animation(.easeIn(duration: 0.1))
    )

    private func open() {
        withAnimation(.spring(duration: 0.4, bounce: 0.2)) { searching = true }
    }

    private func close() {
        focused = false
        withAnimation(.spring(duration: 0.4, bounce: 0.2)) {
            query = ""
            searching = false
        }
    }
}

struct FloorMapView: View {
    static let minZoom = 0.5
    static let maxZoom = 8.0

    let desk: DesktopState
    var onTap: ((Double, Double) -> Void)? = nil
    @State private var zoom = 1.0
    @State private var pinch = 1.0
    @State private var center: CGPoint?
    @State private var drag = CGSize.zero

    private var bounds: DesktopRect { desk.bounds }
    private var currentZoom: Double { min(max(zoom * pinch, Self.minZoom), Self.maxZoom) }
    private var floorCenter: CGPoint { CGPoint(x: bounds.x + bounds.w / 2, y: bounds.y + bounds.h / 2) }
    private var adjusted: Bool { zoom != 1 || center != nil }

    var body: some View {
        GeometryReader { geo in
            let z = currentZoom
            let rw = bounds.w / z
            let rh = bounds.h / z
            let scale = rw > 0 ? geo.size.width / rw : 1
            let c = clamp(CGPoint(x: (center ?? floorCenter).x - drag.width / scale, y: (center ?? floorCenter).y - drag.height / scale), zoom: z)
            let ox = c.x - rw / 2
            let oy = c.y - rh / 2
            let visible = CGRect(x: ox, y: oy, width: rw, height: rh).insetBy(dx: -100, dy: -100)
            ZStack(alignment: .topLeading) {
                Canvas { ctx, _ in
                    for zone in desk.zones {
                        let rect = CGRect(x: (zone.x - ox) * scale, y: (zone.y - oy) * scale, width: zone.w * scale, height: zone.h * scale)
                        ctx.fill(Path(rect), with: .color(zone.isMine ? Color.accentColor.opacity(0.3) : Color(.secondarySystemFill)))
                        ctx.stroke(Path(rect), with: .color(Color(.separator)), lineWidth: 0.5)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture { location in onTap?(location.x / scale + ox, location.y / scale + oy) }
                ForEach(desk.people.filter { visible.contains(CGPoint(x: $0.x, y: $0.y)) }) { p in
                    VStack(spacing: 1) {
                        StatusAvatar(name: p.name, url: p.avatarUrl, size: p.isSelf ? 26 : 20, presence: p.presence, emphasize: p.isSelf)
                        Text(p.isSelf ? "You" : String(p.name.split(separator: " ").first ?? ""))
                            .font(.system(size: 9, weight: .medium))
                            .lineLimit(1)
                            .padding(.horizontal, 3)
                            .background(Color(.systemBackground).opacity(0.8), in: RoundedRectangle(cornerRadius: 4))
                    }
                    .frame(width: 60)
                    .position(x: (p.x - ox) * scale, y: (p.y - oy) * scale + 6)
                    .allowsHitTesting(false)
                }
            }
            .gesture(
                DragGesture(minimumDistance: 8)
                    .onChanged { drag = $0.translation }
                    .onEnded { value in
                        let base = center ?? floorCenter
                        center = clamp(CGPoint(x: base.x - value.translation.width / scale, y: base.y - value.translation.height / scale), zoom: z)
                        drag = .zero
                    },
                including: z > 1 ? .all : .subviews
            )
        }
        .aspectRatio(bounds.h > 0 ? bounds.w / bounds.h : 1, contentMode: .fit)
        .background(Color(.tertiarySystemFill))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .simultaneousGesture(
            MagnifyGesture()
                .onChanged { pinch = $0.magnification }
                .onEnded { _ in
                    zoom = currentZoom
                    pinch = 1
                    if zoom <= 1 { center = nil }
                }
        )
        .overlay(alignment: .topTrailing) {
            if adjusted {
                Button {
                    withAnimation(.snappy) {
                        zoom = 1
                        center = nil
                    }
                } label: {
                    Image(systemName: "arrow.down.right.and.arrow.up.left")
                        .font(.caption.weight(.semibold))
                        .padding(7)
                        .background(.regularMaterial, in: Circle())
                }
                .padding(6)
                .accessibilityLabel("Show the whole floor")
            }
        }
    }

    private func clamp(_ point: CGPoint, zoom z: Double) -> CGPoint {
        guard z > 1 else { return floorCenter }
        let halfW = bounds.w / z / 2
        let halfH = bounds.h / z / 2
        return CGPoint(
            x: min(max(point.x, bounds.x + halfW), bounds.x + bounds.w - halfW),
            y: min(max(point.y, bounds.y + halfH), bounds.y + bounds.h - halfH)
        )
    }
}

struct DesktopCard: View {
    @Environment(AppModel.self) private var model
    let onOpen: () -> Void

    var body: some View {
        header
            .padding(.vertical, 8)
            .contentShape(Rectangle())
            .onTapGesture(perform: onOpen)
            .onAppear { model.relay.watchDesktop() }
            .onDisappear { model.relay.unwatchDesktop() }
    }

    private var inPhoneCall: Bool { model.call.isDesktop }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 3) {
                Text("YOUR DESKTOP").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                if let d = model.desktop.state {
                    Text(d.place).font(.title3.bold()).lineLimit(1)
                    HStack(spacing: 6) {
                        Circle().fill(statusColor(d.presence)).frame(width: 8, height: 8)
                        Text(presenceLabel(presence: d.presence))
                        if !d.floorName.isEmpty, d.floors.count > 1 {
                            Text("· \(d.floorName)").foregroundStyle(.secondary)
                        }
                    }
                    .font(.subheadline)
                } else {
                    Text(model.desktop.error == nil ? "Connecting…" : "Desktop not reachable").font(.title3.bold())
                }
            }
            Spacer()
            if let d = model.desktop.state {
                HStack(spacing: 8) {
                    if inPhoneCall {
                        Image(systemName: "iphone").foregroundStyle(.green)
                    } else {
                        Image(systemName: d.audio ? "mic.fill" : "mic.slash.fill").foregroundStyle(d.audio ? .green : .secondary)
                        Image(systemName: d.video ? "video.fill" : "video.slash.fill").foregroundStyle(d.video ? .green : .secondary)
                    }
                    Image(systemName: "chevron.right").foregroundStyle(.tertiary)
                }
                .font(.subheadline)
            }
        }
    }
}
