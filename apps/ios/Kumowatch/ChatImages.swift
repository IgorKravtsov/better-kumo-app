import ImageIO
import SwiftUI
import UIKit

enum ImageSize: Sendable {
    case thumb
    case preview
    case full

    var maxPixel: CGFloat {
        switch self {
        case .thumb: 240
        case .preview: 800
        case .full: 4096
        }
    }
}

@MainActor
final class ImageStore {
    static let shared = ImageStore()

    private let images = NSCache<NSString, UIImage>()
    private let bytes = NSCache<NSString, NSData>()
    private var loading: [String: Task<UIImage?, Never>] = [:]

    private init() {
        images.totalCostLimit = 96 * 1024 * 1024
        bytes.totalCostLimit = 48 * 1024 * 1024
    }

    func cached(_ url: String, size: ImageSize) -> UIImage? {
        images.object(forKey: Self.key(url, size) as NSString)
    }

    func remember(_ image: UIImage, local name: String) {
        images.setObject(image, forKey: Self.key(Self.local(name), .preview) as NSString, cost: Self.cost(image))
    }

    func image(_ url: String, size: ImageSize) async -> UIImage? {
        let key = Self.key(url, size)
        if let hit = images.object(forKey: key as NSString) { return hit }
        if let running = loading[key] { return await running.value }
        guard !url.hasPrefix(Self.localScheme), let target = URL(string: url) else { return nil }
        let raw = bytes.object(forKey: Self.key(url, nil) as NSString) as Data?
        let task = Task<UIImage?, Never> { [weak self] in
            let data: Data
            if let raw {
                data = raw
            } else {
                guard let (fetched, response) = try? await URLSession.shared.data(from: target),
                      (response as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) ?? true
                else { return nil }
                data = fetched
                self?.bytes.setObject(fetched as NSData, forKey: Self.key(url, nil) as NSString, cost: fetched.count)
            }
            return await Task.detached(priority: .userInitiated) { Self.decode(data, maxPixel: size.maxPixel) }.value
        }
        loading[key] = task
        let image = await task.value
        loading[key] = nil
        if let image { images.setObject(image, forKey: key as NSString, cost: Self.cost(image)) }
        return image
    }

    static let localScheme = "local:"

    static func local(_ name: String) -> String { localScheme + name }

    nonisolated static func key(_ url: String, _ size: ImageSize?) -> String {
        var stable = url
        if var parts = URLComponents(string: url), let items = parts.queryItems {
            let kept = items.filter { $0.name != "auth" }
            parts.queryItems = kept.isEmpty ? nil : kept
            stable = parts.string ?? url
        }
        switch size {
        case .thumb: return "t|" + stable
        case .preview: return "p|" + stable
        case .full: return "f|" + stable
        case nil: return "d|" + stable
        }
    }

    nonisolated static func cost(_ image: UIImage) -> Int {
        let frames = max(1, image.images?.count ?? 1)
        return Int(image.size.width * image.size.height * image.scale * image.scale) * 4 * frames
    }

    nonisolated static func decode(_ data: Data, maxPixel: CGFloat) -> UIImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ] as CFDictionary
        let count = CGImageSourceGetCount(source)
        guard count > 1 else {
            return CGImageSourceCreateThumbnailAtIndex(source, 0, options).map { UIImage(cgImage: $0) }
        }
        var frames: [UIImage] = []
        var duration: Double = 0
        for i in 0..<min(count, 200) {
            guard let cg = CGImageSourceCreateThumbnailAtIndex(source, i, options) else { continue }
            let props = CGImageSourceCopyPropertiesAtIndex(source, i, nil) as? [CFString: Any]
            let gif = props?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
            let delay = (gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double)
                ?? (gif?[kCGImagePropertyGIFDelayTime] as? Double) ?? 0.1
            duration += max(delay, 0.02)
            frames.append(UIImage(cgImage: cg))
        }
        return UIImage.animatedImage(with: frames, duration: duration)
    }

    nonisolated static func jpeg(_ data: Data, limits: AttachmentLimits) -> Data? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: CGFloat(limits.imageMaxSide),
        ] as CFDictionary
        guard let cg = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else { return nil }
        return UIImage(cgImage: cg).jpegData(compressionQuality: CGFloat(limits.jpegQuality) / 100)
    }
}

struct ChatImage: View {
    let url: String
    var maxSide: CGFloat = 260
    var onTap: (() -> Void)?
    @State private var outcome: LoadOutcome?

    var body: some View {
        let mine = outcome?.url == url ? outcome : nil
        let image = mine?.image ?? ImageStore.shared.cached(url, size: .preview)
        let failed = mine != nil && mine?.image == nil
        Group {
            if let image {
                let fitted = fit(image.size)
                let shown = AnimatedImage(image: image, contentMode: .scaleAspectFill)
                    .frame(width: fitted.width, height: fitted.height)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .contentShape(RoundedRectangle(cornerRadius: 8))
                if let onTap {
                    shown.onTapGesture(perform: onTap)
                } else {
                    shown
                }
            } else if failed {
                Label("Image unavailable", systemImage: "photo")
                    .font(.caption).foregroundStyle(.secondary)
                    .padding(.horizontal, 12).padding(.vertical, 10)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color(.secondarySystemFill)))
            } else {
                RoundedRectangle(cornerRadius: 8).fill(Color(.secondarySystemFill))
                    .frame(width: maxSide * 0.8, height: maxSide * 0.6)
                    .overlay { ProgressView() }
            }
        }
        .task(id: url) {
            if url.hasPrefix(ImageStore.localScheme) || ImageStore.shared.cached(url, size: .preview) != nil { return }
            let image = await ImageStore.shared.image(url, size: .preview)
            if Task.isCancelled { return }
            outcome = LoadOutcome(url: url, image: image)
        }
    }

    private func fit(_ size: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0 else { return CGSize(width: maxSide, height: maxSide) }
        let scale = min(1, maxSide / max(size.width, size.height))
        return CGSize(width: max(40, size.width * scale), height: max(40, size.height * scale))
    }
}

private struct LoadOutcome {
    let url: String
    let image: UIImage?
}

struct AnimatedImage: UIViewRepresentable {
    let image: UIImage
    var contentMode: UIView.ContentMode = .scaleAspectFit

    func makeUIView(context: Context) -> UIImageView {
        let view = UIImageView()
        view.clipsToBounds = true
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .vertical)
        return view
    }

    func updateUIView(_ view: UIImageView, context: Context) {
        view.contentMode = contentMode
        if view.image !== image { view.image = image }
    }
}
