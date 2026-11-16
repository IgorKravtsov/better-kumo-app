import Daily
import SwiftUI

enum CallLayout: String, CaseIterable {
    case speaker = "Speaker", screen = "Screen", grid = "Grid"
}

struct CallView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var layout = CallLayout.speaker

    private var call: CallModel { model.call }
    private var sharing: CallTile? { call.tiles.first { $0.screen != nil } }

    var body: some View {
        VStack(spacing: 8) {
            HStack {
                Button { dismiss() } label: { Image(systemName: "chevron.down").font(.title3) }
                VStack(alignment: .leading) {
                    Text(call.roomName.isEmpty ? "Room" : call.roomName).font(.headline)
                    Text(status).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(.horizontal)
            Picker("Layout", selection: $layout) {
                ForEach(CallLayout.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal)
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(.horizontal, 8)
            HStack(spacing: 32) {
                control(call.micOn ? "mic.fill" : "mic.slash.fill", tint: .accentColor) { call.toggleMic() }
                control(call.cameraOn ? "video.fill" : "video.slash.fill", tint: .accentColor) { call.toggleCamera() }
                control("phone.down.fill", tint: .red) {
                    model.leaveCall()
                    dismiss()
                }
            }
            .padding(.bottom, 8)
        }
        .foregroundStyle(.white)
        .background(Color(red: 0.07, green: 0.07, blue: 0.09).ignoresSafeArea())
        .onChange(of: sharing?.id) { _, id in
            if id != nil { layout = .screen } else if layout == .screen { layout = .speaker }
        }
    }

    private var status: String {
        switch call.phase {
        case .joined:
            "\(call.tiles.filter(\.inCall).count) in call" + (call.tiles.contains { !$0.inCall } ? " · \(call.tiles.filter { !$0.inCall }.count) in the room" : "")
        case .failed: call.error ?? "Could not join"
        case .idle: "Not in a call"
        default: "Connecting…"
        }
    }

    @ViewBuilder
    private var content: some View {
        if call.phase != .joined {
            ProgressView().tint(.white)
        } else if layout == .grid {
            ScrollView {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 6) {
                    ForEach(call.tiles) { tile in
                        TileView(tile: tile, speaking: call.speaking.contains(tile.id)).aspectRatio(0.8, contentMode: .fit)
                    }
                }
            }
        } else {
            let main: CallTile? = (layout == .screen && sharing != nil) ? nil :
                call.tiles.first { $0.id == call.lastSpeaker && !$0.isLocal } ?? call.tiles.first { !$0.isLocal } ?? call.tiles.first
            VStack(spacing: 6) {
                if main == nil, let sharing, let screen = sharing.screen {
                    ZStack(alignment: .bottomLeading) {
                        DailyVideo(track: screen, mirror: false, fit: true)
                        Text("\(sharing.name) is sharing").font(.caption).padding(4).background(.black.opacity(0.6)).padding(8)
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                } else if let main {
                    TileView(tile: main, speaking: call.speaking.contains(main.id))
                }
                let others = call.tiles.filter { $0.id != main?.id }
                if !others.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 6) {
                            ForEach(others) { tile in
                                TileView(tile: tile, speaking: call.speaking.contains(tile.id)).frame(width: 96, height: 120)
                            }
                        }
                    }
                    .frame(height: 120)
                }
            }
        }
    }

    private func control(_ symbol: String, tint: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).font(.title2).frame(width: 60, height: 60).background(Circle().fill(tint))
        }
    }
}

struct TileView: View {
    @Environment(AppModel.self) private var model
    let tile: CallTile
    let speaking: Bool

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            RoundedRectangle(cornerRadius: 12).fill(Color(white: 0.13))
            if let camera = tile.camera {
                DailyVideo(track: camera, mirror: tile.isLocal, fit: false)
            } else {
                VStack(spacing: 4) {
                    StatusAvatar(name: tile.name, url: tile.avatar, size: 64, availability: tile.availability)
                    if !tile.inCall {
                        Text(statusLabel(tile.availability) + " · not in call")
                            .font(.caption2).foregroundStyle(statusColor(tile.availability))
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            HStack(spacing: 4) {
                if !tile.micOn && tile.inCall { Image(systemName: "mic.slash.fill").foregroundStyle(.red).font(.caption2) }
                Text(tile.name + (tile.isLocal ? " (you)" : "")).font(.caption2)
            }
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(.black.opacity(0.6), in: RoundedRectangle(cornerRadius: 6))
            .padding(6)
        }
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(speaking ? Color.green : .clear, lineWidth: 3))
    }
}

struct DailyVideo: UIViewRepresentable {
    let track: VideoTrack
    let mirror: Bool
    let fit: Bool

    func makeUIView(context: Context) -> VideoView {
        VideoView()
    }

    func updateUIView(_ view: VideoView, context: Context) {
        view.track = track
        view.videoScaleMode = fit ? .fit : .fill
        view.transform = mirror ? CGAffineTransform(scaleX: -1, y: 1) : .identity
    }
}
