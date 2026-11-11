import ImageIO
import SwiftUI
import UIKit

struct PeopleList: View {
    let title: String
    let people: [MemberItem]
    let onPick: (MemberItem) -> Void
    @State private var query = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List(people
                .filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }
                .sorted { ($0.isOnline ? 0 : 1, $0.name.lowercased()) < ($1.isOnline ? 0 : 1, $1.name.lowercased()) },
                 id: \.uid) { p in
                Button {
                    dismiss()
                    onPick(p)
                } label: {
                    HStack {
                        AvatarView(name: p.name, url: p.avatarUrl, online: p.isOnline)
                        Text(p.name)
                    }
                }
                .foregroundStyle(.primary)
            }
            .searchable(text: $query)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
        }
    }
}

struct ImageViewer: View {
    let url: String
    @Environment(\.dismiss) private var dismiss
    @State private var scale: CGFloat = 1
    @State private var base: CGFloat = 1

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            RemoteImage(url: url)
                .scaleEffect(scale)
                .gesture(MagnifyGesture()
                    .onChanged { scale = max(1, min(6, base * $0.magnification)) }
                    .onEnded { _ in base = scale })
                .onTapGesture(count: 2) {
                    scale = scale > 1 ? 1 : 2.5
                    base = scale
                }
            Button { dismiss() } label: {
                Image(systemName: "xmark.circle.fill").font(.title).foregroundStyle(.white)
            }
            .padding()
        }
    }
}

struct RemoteImage: UIViewRepresentable {
    let url: String

    func makeUIView(context: Context) -> UIImageView {
        let view = UIImageView()
        view.contentMode = .scaleAspectFit
        view.clipsToBounds = true
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        return view
    }

    func updateUIView(_ view: UIImageView, context: Context) {
        guard context.coordinator.loaded != url, let target = URL(string: url) else { return }
        context.coordinator.loaded = url
        Task {
            guard let (data, _) = try? await URLSession.shared.data(from: target) else { return }
            let image = await Task.detached { Self.decode(data) }.value
            await MainActor.run { view.image = image }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var loaded: String?
    }

    nonisolated static func decode(_ data: Data) -> UIImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return UIImage(data: data) }
        let count = CGImageSourceGetCount(source)
        guard count > 1 else { return UIImage(data: data) }
        var frames: [UIImage] = []
        var duration: Double = 0
        for i in 0..<min(count, 200) {
            guard let cg = CGImageSourceCreateImageAtIndex(source, i, nil) else { continue }
            let props = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any]
            let gif = props?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
            let delay = (gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double)
                ?? (gif?[kCGImagePropertyGIFDelayTime] as? Double) ?? 0.1
            duration += max(delay, 0.02)
            frames.append(UIImage(cgImage: cg))
        }
        return UIImage.animatedImage(with: frames, duration: duration)
    }
}
