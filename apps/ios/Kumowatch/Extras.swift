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
    @State private var full: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            Group {
                if let image = full ?? ImageStore.shared.cached(url, size: .full) ?? ImageStore.shared.cached(url, size: .preview) {
                    ZoomableImage(image: image).ignoresSafeArea()
                } else if failed {
                    Label("Image unavailable", systemImage: "photo").foregroundStyle(.white.opacity(0.7))
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ProgressView().tint(.white).frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            Button { dismiss() } label: {
                Image(systemName: "xmark.circle.fill").font(.title).foregroundStyle(.white)
            }
            .padding()
        }
        .task(id: url) {
            let image = await ImageStore.shared.image(url, size: .full)
            if Task.isCancelled { return }
            full = image
            failed = image == nil
        }
    }
}

/// Pinch-to-zoom around the fingers, pan while zoomed, double-tap to zoom into the tapped spot.
struct ZoomableImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> ZoomScrollView { ZoomScrollView() }

    func updateUIView(_ view: ZoomScrollView, context: Context) { view.image = image }
}

final class ZoomScrollView: UIScrollView, UIScrollViewDelegate {
    private let imageView = UIImageView()
    private var laidOutSize: CGSize = .zero

    var image: UIImage? {
        get { imageView.image }
        set {
            guard newValue !== imageView.image else { return }
            imageView.image = newValue
            fit(previousViewport: bounds.size)
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        minimumZoomScale = 1
        maximumZoomScale = 8
        bouncesZoom = true
        showsHorizontalScrollIndicator = false
        showsVerticalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        decelerationRate = .fast
        imageView.contentMode = .scaleAspectFit
        addSubview(imageView)
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.size != laidOutSize {
            let previous = laidOutSize
            laidOutSize = bounds.size
            fit(previousViewport: previous)
        }
    }

    /// Sizes the image to fit the screen, keeping the zoom and the image point at the
    /// viewport centre (a sharper image replacing its preview, a window resize, a rotation).
    private func fit(previousViewport: CGSize) {
        guard let size = imageView.image?.size, size.width > 0, size.height > 0,
              bounds.width > 0, bounds.height > 0 else { return }
        let scale = zoomScale
        var focus = CGPoint(x: 0.5, y: 0.5)
        let frame = imageView.frame
        if scale > minimumZoomScale, frame.width > 0, frame.height > 0 {
            focus = CGPoint(x: (contentOffset.x + previousViewport.width / 2) / frame.width,
                            y: (contentOffset.y + previousViewport.height / 2) / frame.height)
        }
        let ratio = min(bounds.width / size.width, bounds.height / size.height)
        zoomScale = 1
        imageView.frame = CGRect(x: 0, y: 0, width: size.width * ratio, height: size.height * ratio)
        contentSize = imageView.frame.size
        zoomScale = min(max(scale, minimumZoomScale), maximumZoomScale)
        center()
        let x = focus.x * contentSize.width - bounds.width / 2
        let y = focus.y * contentSize.height - bounds.height / 2
        contentOffset = CGPoint(
            x: min(max(x, -contentInset.left), max(-contentInset.left, contentSize.width - bounds.width + contentInset.right)),
            y: min(max(y, -contentInset.top), max(-contentInset.top, contentSize.height - bounds.height + contentInset.bottom)))
    }

    private func center() {
        let dx = max(0, (bounds.width - contentSize.width) / 2)
        let dy = max(0, (bounds.height - contentSize.height) / 2)
        contentInset = UIEdgeInsets(top: dy, left: dx, bottom: dy, right: dx)
    }

    @objc private func doubleTapped(_ gesture: UITapGestureRecognizer) {
        if zoomScale > minimumZoomScale {
            setZoomScale(minimumZoomScale, animated: true)
            return
        }
        let point = gesture.location(in: imageView)
        let scale: CGFloat = 3
        let width = bounds.width / scale, height = bounds.height / scale
        zoom(to: CGRect(x: point.x - width / 2, y: point.y - height / 2, width: width, height: height), animated: true)
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }

    func scrollViewDidZoom(_ scrollView: UIScrollView) { center() }
}
