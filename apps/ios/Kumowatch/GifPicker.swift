import SwiftUI

struct GifPicker: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let onPick: (GifItem) -> Void
    @State private var query = ""
    @State private var gifs: [GifItem] = []
    @State private var nextOffset: UInt32 = 0
    @State private var reachedEnd = false
    @State private var loading = false
    @State private var failed = false
    // Bumped on every new query so late pages of an older query are dropped.
    @State private var generation = 0

    private let columns = [GridItem(.flexible(), spacing: 6), GridItem(.flexible(), spacing: 6)]

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVGrid(columns: columns, spacing: 6) {
                    ForEach(gifs, id: \.id) { gif in
                        GifCell(gif: gif)
                            .onTapGesture {
                                onPick(gif)
                                dismiss()
                            }
                            .onAppear { if gif.id == gifs.last?.id { Task { await loadMore() } } }
                    }
                }
                .padding(.horizontal, 8)
                if loading { ProgressView().padding() }
                if failed && gifs.isEmpty {
                    Text("Couldn't load GIFs").foregroundStyle(.secondary).padding(.top, 40)
                } else if !loading && gifs.isEmpty && !query.isEmpty {
                    Text("No GIFs found").foregroundStyle(.secondary).padding(.top, 40)
                }
                Text("Powered by GIPHY").font(.caption2).foregroundStyle(.secondary).padding(.vertical, 8)
            }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search GIPHY")
            .navigationTitle("GIFs")
            .navigationBarTitleDisplayMode(.inline)
            .task(id: query) {
                generation += 1
                let mine = generation
                gifs = []
                nextOffset = 0
                reachedEnd = false
                failed = false
                loading = true
                if !query.isEmpty { try? await Task.sleep(for: .milliseconds(350)) }
                if Task.isCancelled { return }
                await fetch(generation: mine)
            }
        }
    }

    private func loadMore() async {
        guard !loading, !reachedEnd, !gifs.isEmpty else { return }
        loading = true
        await fetch(generation: generation)
    }

    private func fetch(generation mine: Int) async {
        let page: GifPage?
        do {
            page = try await model.core.searchGifs(query: query, offset: nextOffset)
        } catch {
            page = nil
        }
        guard mine == generation else { return }
        loading = false
        guard let page else {
            failed = true
            reachedEnd = true
            return
        }
        let known = Set(gifs.map(\.id))
        gifs += page.gifs.filter { !known.contains($0.id) }
        nextOffset = page.nextOffset
        reachedEnd = page.reachedEnd
    }
}

private struct GifCell: View {
    let gif: GifItem
    @State private var image: UIImage?

    var body: some View {
        let ratio = gif.previewWidth > 0 && gif.previewHeight > 0
            ? CGFloat(gif.previewWidth) / CGFloat(gif.previewHeight) : 1
        Color(.secondarySystemFill)
            .aspectRatio(ratio, contentMode: .fit)
            .overlay {
                if let image = image ?? ImageStore.shared.cached(gif.previewUrl, size: .thumb) {
                    AnimatedImage(image: image, contentMode: .scaleAspectFill)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
            .accessibilityLabel(gif.title.isEmpty ? "GIF" : gif.title)
            .accessibilityAddTraits(.isButton)
            .task(id: gif.previewUrl) {
                if image == nil { image = await ImageStore.shared.image(gif.previewUrl, size: .thumb) }
            }
            .onDisappear { image = nil }
    }
}
