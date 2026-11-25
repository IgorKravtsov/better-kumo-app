import SwiftUI

private let allGroups = emojiGroups()

struct ReactionPicker: View {
    @Environment(\.dismiss) private var dismiss
    let recent: [String]
    let onPick: (String) -> Void
    @State private var query = ""
    @State private var results: [String] = []

    private let columns = [GridItem(.adaptive(minimum: 44), spacing: 4)]

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    Color.clear.frame(height: 0).id("top")
                    LazyVGrid(columns: columns, alignment: .leading, spacing: 4) {
                        if query.isEmpty {
                            if !recent.isEmpty { section("Recently used", recent) }
                            ForEach(allGroups, id: \.name) { group in section(group.name, group.emojis) }
                        } else {
                            ForEach(results, id: \.self, content: cell)
                        }
                    }
                    .padding(.horizontal, 12)
                    if !query.isEmpty && results.isEmpty {
                        Text("No emoji found").foregroundStyle(.secondary).padding(.top, 40)
                    }
                }
                .onChange(of: query) { proxy.scrollTo("top") }
            }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search emoji")
            .autocorrectionDisabled()
            .navigationTitle("Reactions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
            .onChange(of: query, initial: true) { _, q in results = searchEmoji(query: q) }
        }
        .presentationDetents([.medium, .large])
    }

    @ViewBuilder
    private func section(_ title: String, _ emojis: [String]) -> some View {
        Section {
            ForEach(emojis, id: \.self, content: cell)
        } header: {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 10)
        }
    }

    private func cell(_ emoji: String) -> some View {
        Button {
            onPick(emoji)
            dismiss()
        } label: {
            Text(emoji).font(.system(size: 30)).frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

struct FlowLayout: Layout {
    var spacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = rows(width: proposal.width ?? .infinity, subviews: subviews)
        let width = rows.map(\.width).max() ?? 0
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(rows.count - 1, 0))
        return CGSize(width: width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(width: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for index in row.indices {
                let size = subviews[index].sizeThatFits(ProposedViewSize(width: bounds.width, height: nil))
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var indices: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func rows(width: CGFloat, subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(ProposedViewSize(width: width, height: nil))
            if let last = rows.last, !last.indices.isEmpty, last.width + spacing + size.width > width {
                rows.append(Row())
            }
            var row = rows.removeLast()
            row.width += (row.indices.isEmpty ? 0 : spacing) + size.width
            row.height = max(row.height, size.height)
            row.indices.append(index)
            rows.append(row)
        }
        return rows.filter { !$0.indices.isEmpty }
    }
}
