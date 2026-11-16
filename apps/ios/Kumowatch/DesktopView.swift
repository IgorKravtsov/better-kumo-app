import SwiftUI

struct DesktopView: View {
    @Environment(AppModel.self) private var model
    @Binding var showCall: Bool
    @State private var desk: DeskState?
    @State private var error: String?
    @State private var statusDraft: String?

    var body: some View {
        Group {
            if let desk {
                content(desk)
            } else if let error {
                Text(error).padding()
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Desktop")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            while !Task.isCancelled {
                await act { try await model.relay.desktop() }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func act(_ block: () async throws -> DeskState?) async {
        do {
            if let next = try await block() { desk = next }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func content(_ d: DeskState) -> some View {
        List {
            Section {
                VStack(alignment: .leading) {
                    Text(d.zones.first { $0.id == d.me?.zoneId }?.name ?? "Common area").font(.headline)
                    Text(d.floors.first { $0.id == d.floorId }?.name ?? "").font(.caption).foregroundStyle(.secondary)
                    if let error { Text(error).font(.caption).foregroundStyle(.red) }
                }
                FloorMapView(desk: d) { x, y in
                    let zone = d.zones.first { x >= $0.x && x <= $0.x + $0.w && y >= $0.y && y <= $0.y + $0.h }
                    Task {
                        await act {
                            if let zone, zone.id != d.me?.zoneId { try await model.relay.zone(zone.id) } else { try await model.relay.move(x: x, y: y) }
                        }
                    }
                }
                .listRowInsets(EdgeInsets(top: 4, leading: 8, bottom: 4, trailing: 8))
                HStack {
                    Button { Task { await act { try await model.relay.home() } } } label: {
                        Image(systemName: "house.fill")
                    }
                    .buttonStyle(.bordered)
                    Button { Task { await act { try await model.relay.media(audio: !d.audio) } } } label: {
                        Image(systemName: d.audio ? "mic.fill" : "mic.slash.fill")
                    }
                    .buttonStyle(.bordered)
                    Button { Task { await act { try await model.relay.media(video: !d.video) } } } label: {
                        Image(systemName: d.video ? "video.fill" : "video.slash.fill")
                    }
                    .buttonStyle(.bordered)
                    Picker("Status", selection: Binding(
                        get: { d.me?.availability ?? "available" },
                        set: { value in
                            let message = statusDraft ?? d.me?.message ?? ""
                            Task { await act { try await model.relay.status(value, message: message) } }
                        }
                    )) {
                        Text("Available").tag("available")
                        Text("Away").tag("away")
                        Text("Focusing").tag("focusing")
                    }
                    .pickerStyle(.menu)
                }
                HStack {
                    TextField("Status message", text: Binding(
                        get: { statusDraft ?? d.me?.message ?? "" },
                        set: { statusDraft = String($0.prefix(100)) }
                    ))
                    .submitLabel(.done)
                    Button("Save") {
                        let text = (statusDraft ?? "").trimmingCharacters(in: .whitespaces)
                        let availability = d.me?.availability ?? "available"
                        statusDraft = nil
                        Task { await act { try await model.relay.status(availability.isEmpty ? "available" : availability, message: text) } }
                    }
                    .disabled(statusDraft == nil || statusDraft == d.me?.message)
                }
                if d.floors.count > 1 {
                    Picker("Floor", selection: Binding(
                        get: { d.floorId ?? "" },
                        set: { id in Task { await act { try await model.relay.floor(id) } } }
                    )) {
                        ForEach(d.floors) { Text($0.name).tag($0.id) }
                    }
                }
            }
            Section {
                let inCall = model.call.isRemote && model.call.phase != .idle
                Button {
                    if inCall {
                        showCall = true
                    } else {
                        Task {
                            do {
                                try await model.joinDesktopCall()
                                showCall = true
                            } catch {
                                self.error = error.localizedDescription
                            }
                        }
                    }
                } label: {
                    Label(inCall ? "Open call" : "Use phone for audio and video", systemImage: inCall ? "phone.fill" : "iphone.radiowaves.left.and.right")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.call.phase != .idle && !model.call.isRemote)
            }
            Section {
                StatusLegend()
            }
            Section("People on this floor") {
                ForEach(d.people.filter { !$0.isSelf }.sorted { $0.name < $1.name }) { p in
                    HStack {
                        StatusAvatar(name: p.name, url: p.avatar, availability: p.availability, idle: p.idle ?? false)
                        VStack(alignment: .leading) {
                            Text(p.name)
                            let idle = p.idle ?? false
                            let message = (p.message ?? "").isEmpty || idle ? "" : " · \(p.message ?? "")"
                            Text(statusLabel(p.availability, idle: idle) + message)
                                .font(.caption).foregroundStyle(statusColor(p.availability, idle: idle)).lineLimit(1)
                            Text(d.zones.first { $0.id == p.zoneId }?.name ?? "Common area").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        HStack(spacing: 8) {
                            Button {
                                Task {
                                    await act {
                                        if let zone = p.zoneId { try await model.relay.zone(zone) } else { try await model.relay.move(x: p.x, y: p.y) }
                                    }
                                }
                            } label: {
                                Image(systemName: "figure.walk")
                                    .frame(width: 22, height: 22)
                            }
                            .buttonStyle(.bordered)
                            .accessibilityLabel("Go to \(p.name)")
                            Button {
                                Task { try? await model.relay.nudge(p.id) }
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

extension DeskState {
    var focus: CGRect {
        let full = CGRect(x: 0, y: 0, width: width, height: height)
        let me = people.first { $0.isSelf }
        let zone = zones.first { $0.id == self.me?.zoneId }
        var rect: CGRect
        if let zone {
            rect = CGRect(x: zone.x, y: zone.y, width: zone.w, height: zone.h).insetBy(dx: -zone.w * 0.25, dy: -zone.h * 0.25)
        } else if let me {
            rect = CGRect(x: me.x - 700, y: me.y - 450, width: 1400, height: 900)
        } else {
            return full
        }
        let minW = max(rect.width, rect.height * 1.6)
        rect = rect.insetBy(dx: -(minW - rect.width) / 2, dy: 0)
        rect.size.height = rect.width / 1.6
        if let zone { rect.origin.y = zone.y + zone.h / 2 - rect.height / 2 }
        return rect.intersection(full).isNull ? full : rect
    }
}

struct FloorMapView: View {
    let desk: DeskState
    var viewport: CGRect? = nil
    var onTap: ((Double, Double) -> Void)? = nil

    var body: some View {
        let region = viewport ?? CGRect(x: 0, y: 0, width: desk.width, height: desk.height)
        GeometryReader { geo in
            let scale = region.width > 0 ? geo.size.width / region.width : 1
            let ox = region.minX
            let oy = region.minY
            ZStack(alignment: .topLeading) {
                Canvas { ctx, _ in
                    for z in desk.zones {
                        let rect = CGRect(x: (z.x - ox) * scale, y: (z.y - oy) * scale, width: z.w * scale, height: z.h * scale)
                        ctx.fill(Path(rect), with: .color(z.id == desk.me?.zoneId ? Color.accentColor.opacity(0.3) : Color(.secondarySystemFill)))
                        ctx.stroke(Path(rect), with: .color(Color(.separator)), lineWidth: 0.5)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture { location in onTap?(location.x / scale + ox, location.y / scale + oy) }
                ForEach(desk.people.filter { region.insetBy(dx: -100, dy: -100).contains(CGPoint(x: $0.x, y: $0.y)) }.sorted { !$0.isSelf && $1.isSelf }) { p in
                    VStack(spacing: 1) {
                        StatusAvatar(name: p.name, url: p.avatar, size: p.isSelf ? 26 : 20, availability: p.availability, idle: p.idle ?? false, emphasize: p.isSelf)
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
        }
        .aspectRatio(region.height > 0 ? region.width / region.height : 1, contentMode: .fit)
        .background(Color(.tertiarySystemFill))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }
}

struct DesktopCard: View {
    @Environment(AppModel.self) private var model
    let onOpen: () -> Void
    @State private var desk: DeskState?
    @State private var error: String?

    var body: some View {
        header
            .padding(.vertical, 8)
            .contentShape(Rectangle())
            .onTapGesture(perform: onOpen)
            .task {
                while !Task.isCancelled {
                    do {
                        if let next = try await model.relay.desktop() { desk = next }
                        error = nil
                    } catch {
                        self.error = error.localizedDescription
                    }
                    try? await Task.sleep(for: .seconds(3))
                }
            }
    }

    private var inPhoneCall: Bool { model.call.isRemote && model.call.phase != .idle }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 3) {
                Text("YOUR DESKTOP").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                if let d = desk {
                    Text(d.zones.first { $0.id == d.me?.zoneId }?.name ?? "Common area")
                        .font(.title3.bold()).lineLimit(1)
                    let availability = d.me?.availability ?? ""
                    HStack(spacing: 6) {
                        Circle().fill(statusColor(availability, idle: false)).frame(width: 8, height: 8)
                        Text(statusLabel(availability, idle: false))
                        if let floor = d.floors.first(where: { $0.id == d.floorId })?.name, !floor.isEmpty, d.floors.count > 1 {
                            Text("· \(floor)").foregroundStyle(.secondary)
                        }
                    }
                    .font(.subheadline)
                } else {
                    Text(error == nil ? "Connecting…" : "Desktop not reachable").font(.title3.bold())
                }
            }
            Spacer()
            if let d = desk {
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
