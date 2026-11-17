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
    @State private var full: UIImage?
    @State private var failed = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            Group {
                if let image = full ?? ImageStore.shared.cached(url, size: .full) ?? ImageStore.shared.cached(url, size: .preview) {
                    AnimatedImage(image: image)
                } else if failed {
                    Label("Image unavailable", systemImage: "photo").foregroundStyle(.white.opacity(0.7))
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ProgressView().tint(.white).frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
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
        .task(id: url) {
            let image = await ImageStore.shared.image(url, size: .full)
            if Task.isCancelled { return }
            full = image
            failed = image == nil
        }
    }
}
