import SwiftUI

private final class MindNodeItem: Identifiable {
    let id: String
    let label: String
    var summary: String = ""
    var facets: [String] = []
    var atomic: Bool = false
    var expandable: Bool = false
    var depth: Int = 0
    var children: [MindNodeItem] = []
    var x: CGFloat = 0
    var y: CGFloat = 0
    var width: CGFloat = 120
    let height: CGFloat = 34

    init(id: String, label: String, summary: String = "", facets: [String] = [], atomic: Bool = false) {
        self.id = id
        self.label = label
        self.summary = summary
        self.facets = facets
        self.atomic = atomic
        var units: CGFloat = 0
        for scalar in label.unicodeScalars {
            units += scalar.value > 127 ? 13.5 : 7.5
        }
        self.width = min(294, max(96, units + 26 + (facets.isEmpty ? 0 : 24)))
    }
}

struct MindmapTreeView: View {
    let payload: [String: Any]
    let en: Bool
    var onQuizConcept: ((String) -> Void)? = nil
    var onExpandMindmap: ((String) -> Void)? = nil
    var onAskConcept: ((String) -> Void)? = nil
    /// (nodeId, label): split the node into finer knowledge points.
    var onRefineNode: ((String, String) -> Void)? = nil
    /// Read-only (preview) mode: no node menu, a tap only expands / collapses locally.
    var readOnly: Bool = false

    /// Explicit user choices (id -> collapsed). Nodes without an entry use the default:
    /// only the first two levels are shown, every deeper branch starts collapsed.
    @State private var collapseOverrides: [String: Bool] = [:]
    @State private var scale: CGFloat = 1.0
    @State private var allExpanded: Bool = false
    @State private var panOffset: CGSize = .zero
    @GestureState private var dragDelta: CGSize = .zero

    private let rowGap: CGFloat = 14
    private let colGap: CGFloat = 66
    private let toggleSize: CGFloat = 22
    private let padX: CGFloat = 70
    private let padY: CGFloat = 30

    private var treeLayout: (roots: [MindNodeItem], visible: [MindNodeItem], collapsed: Set<String>, parents: [String], total: Int, width: CGFloat, height: CGFloat) {
        let rawNodes = payload["nodes"] as? [[String: Any]] ?? []
        var map: [String: MindNodeItem] = [:]
        var parentById: [String: String] = [:]
        var order: [MindNodeItem] = []

        for (i, n) in rawNodes.enumerated() {
            let id = (n["id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "n\(i)"
            let rawLabel = n["label"] as? String ?? ""
            let label = rawLabel.replacingOccurrences(of: #"\s*(?:\[\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*\]|【\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*】)+"#, with: "", options: .regularExpression).trimmingCharacters(in: .whitespacesAndNewlines)
            let facets = (n["facets"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            let node = MindNodeItem(
                id: id,
                label: label,
                summary: n["summary"] as? String ?? "",
                facets: facets,
                atomic: n["atomic"] as? Bool ?? false
            )
            node.expandable = n["expandable"] as? Bool ?? false
            map[id] = node
            parentById[id] = n["parent_id"] as? String ?? ""
            order.append(node)
        }

        var roots: [MindNodeItem] = []
        for node in order {
            let pid = parentById[node.id] ?? ""
            if !pid.isEmpty, pid != node.id, let parent = map[pid] {
                parent.children.append(node)
            } else {
                roots.append(node)
            }
        }
        if roots.isEmpty, let first = order.first {
            roots = [first]
        }

        func setDepth(_ node: MindNodeItem, _ d: Int) {
            node.depth = d
            for c in node.children { setDepth(c, d + 1) }
        }
        for r in roots { setDepth(r, 0) }

        var cursorY: CGFloat = padY
        var maxX: CGFloat = 440
        var maxY: CGFloat = 320
        var visible: [MindNodeItem] = []
        var collapsedSet: Set<String> = []

        @discardableResult
        func layout(_ node: MindNodeItem, _ x: CGFloat) -> CGFloat {
            node.x = x
            visible.append(node)
            let isCol = !node.children.isEmpty && (collapseOverrides[node.id] ?? (node.depth >= 1))
            if isCol { collapsedSet.insert(node.id) }
            if node.children.isEmpty || isCol {
                node.y = cursorY
                cursorY += node.height + rowGap
            } else {
                let childX = x + node.width + colGap
                let firstY = layout(node.children[0], childX)
                var lastY = firstY
                for child in node.children.dropFirst() {
                    lastY = layout(child, childX)
                }
                node.y = (firstY + lastY) / 2.0
            }
            maxX = max(maxX, node.x + node.width + 64)
            maxY = max(maxY, node.y + node.height + 48)
            return node.y
        }

        for r in roots {
            layout(r, padX)
            cursorY += rowGap
        }
        let parents = order.filter { !$0.children.isEmpty && $0.depth >= 1 }.map { $0.id }
        return (roots, visible, collapsedSet, parents, order.count, maxX, maxY)
    }

    var body: some View {
        let laid = treeLayout
        return ZStack(alignment: .topLeading) {
            ScrollView([.horizontal, .vertical]) {
                ZStack(alignment: .topLeading) {
                    Canvas { ctx, _ in
                        func drawEdges(_ node: MindNodeItem) {
                            guard !laid.collapsed.contains(node.id), !node.children.isEmpty else { return }
                            let sx = node.x + node.width + toggleSize + 6
                            let sy = node.y + node.height / 2
                            let strokeColor = edgeColor(node.depth)
                            for child in node.children {
                                let ex = child.x
                                let ey = child.y + child.height / 2
                                let mx = (sx + ex) / 2
                                var path = Path()
                                path.move(to: CGPoint(x: sx, y: sy))
                                path.addCurve(
                                    to: CGPoint(x: ex, y: ey),
                                    control1: CGPoint(x: mx, y: sy),
                                    control2: CGPoint(x: mx, y: ey)
                                )
                                ctx.stroke(path, with: .color(strokeColor), lineWidth: 1.6)
                                drawEdges(child)
                            }
                        }
                        for r in laid.roots { drawEdges(r) }
                    }

                    ForEach(laid.visible) { node in
                        let colors = nodeColors(node.depth)
                        nodeView(node, colors: colors, collapsed: laid.collapsed.contains(node.id))
                        .frame(width: node.width, height: node.height)
                        .position(x: node.x + node.width / 2, y: node.y + node.height / 2)

                        if !node.children.isEmpty {
                            let isCol = laid.collapsed.contains(node.id)
                            Button {
                                withAnimation(.easeOut(duration: 0.16)) {
                                    collapseOverrides[node.id] = !isCol
                                }
                            } label: {
                                Text(isCol ? ">" : "<")
                                    .font(.system(size: 10, weight: .semibold))
                                    .foregroundColor(Color(white: 0.9))
                                    .frame(width: toggleSize, height: toggleSize)
                                    .background(Color(red: 52/255, green: 56/255, blue: 68/255))
                                    .overlay(Circle().stroke(Color(red: 86/255, green: 92/255, blue: 112/255), lineWidth: 1))
                                    .clipShape(Circle())
                            }
                            .buttonStyle(.plain)
                            .position(x: node.x + node.width + 6 + toggleSize / 2, y: node.y + node.height / 2)
                        }
                    }
                }
                .frame(width: laid.width, height: laid.height)
                .scaleEffect(scale, anchor: .topLeading)
                .offset(x: panOffset.width + dragDelta.width, y: panOffset.height + dragDelta.height)
                .gesture(
                    DragGesture(minimumDistance: 4)
                        .updating($dragDelta) { value, state, _ in state = value.translation }
                        .onEnded { value in
                            panOffset.width += value.translation.width
                            panOffset.height += value.translation.height
                        }
                )
            }
            .background(Color(red: 29/255, green: 31/255, blue: 36/255))
            .cornerRadius(12)

            if laid.total > 400 {
                VStack {
                    Spacer()
                    Text(L10n.t("mindmap.large_hint_fmt", en: en).replacingOccurrences(of: "{0}", with: "\(laid.total)"))
                        .font(.system(size: 11.5))
                        .foregroundColor(Color(white: 0.9))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .background(Color(red: 30/255, green: 42/255, blue: 56/255).opacity(0.86))
                        .cornerRadius(8)
                        .padding(.bottom, 12)
                }
                .frame(maxWidth: .infinity)
                .allowsHitTesting(false)
            }

            // Left floating vertical controls
            VStack(spacing: 8) {
                Button {
                    withAnimation(.easeOut(duration: 0.18)) {
                        allExpanded.toggle()
                        collapseOverrides.removeAll()
                        if allExpanded {
                            for id in laid.parents { collapseOverrides[id] = false }
                        }
                    }
                } label: {
                    Image(systemName: "arrow.up.and.down")
                        .frame(width: 34, height: 34)
                }
                .buttonStyle(.plain)
                .background(Color(red: 38/255, green: 41/255, blue: 48/255).opacity(0.92))
                .clipShape(Circle())
                .help(en ? "Expand / Collapse all" : "全部展开 / 收起")

                VStack(spacing: 2) {
                    Button { scale = min(2.2, scale + 0.15) } label: {
                        Image(systemName: "plus").frame(width: 34, height: 32)
                    }.buttonStyle(.plain)
                    Button { scale = max(0.45, scale - 0.15) } label: {
                        Image(systemName: "minus").frame(width: 34, height: 32)
                    }.buttonStyle(.plain)
                }
                .background(Color(red: 38/255, green: 41/255, blue: 48/255).opacity(0.92))
                .cornerRadius(17)
            }
            .padding(.leading, 14)
            .padding(.top, 16)
        }
    }

    private func expandNode(_ node: MindNodeItem) {
        withAnimation(.easeOut(duration: 0.16)) {
            collapseOverrides[node.id] = false
        }
    }

    private func nodeLabel(_ node: MindNodeItem, colors: (bg: Color, border: Color), collapsed: Bool) -> some View {
        let needsPlus = (collapsed && !node.children.isEmpty) || (node.children.isEmpty && node.expandable)
        return HStack(spacing: 5) {
            Text(node.label)
                .font(.system(size: 13, weight: node.depth == 0 ? .semibold : .regular))
                .foregroundColor(Color(white: 0.93))
                .lineLimit(1)
            if !node.facets.isEmpty {
                Text("\(node.facets.count)")
                    .font(.system(size: 10.5))
                    .foregroundColor(Color(white: 0.86))
                    .padding(.horizontal, 5)
                    .background(Color(white: 0.4, opacity: 0.6))
                    .cornerRadius(7)
            }
            if needsPlus {
                Text("+")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundColor(Color(red: 253/255, green: 214/255, blue: 99/255))
            }
        }
        .padding(.horizontal, 11)
        .frame(width: node.width, height: node.height, alignment: .leading)
        .background(colors.bg)
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(colors.border, lineWidth: 1))
        .cornerRadius(8)
    }

    @ViewBuilder
    private func nodeMenuItems(_ node: MindNodeItem) -> some View {
        if !node.summary.isEmpty {
            Text(node.summary)
        }
        if !node.facets.isEmpty {
            Menu("\(L10n.t("kg.facets", en: en)) (\(node.facets.count))") {
                ForEach(node.facets, id: \.self) { facet in
                    Button(facet) { onAskConcept?("\(node.label)：\(facet)") }
                }
            }
        }
        if let refine = onRefineNode, node.depth > 0 {
            Button(L10n.t(node.atomic ? "kg.atomic" : "kg.action_refine", en: en)) {
                refine(node.id, node.label)
            }
            .disabled(node.atomic)
        }
        Divider()
        Button(L10n.t("kg.action_quiz", en: en)) { onQuizConcept?(node.label) }
        Button(L10n.t("kg.action_mindmap", en: en)) { onExpandMindmap?(node.label) }
        Button(L10n.t("kg.action_chat", en: en)) { onAskConcept?(node.label) }
    }

    @ViewBuilder
    private func nodeView(_ node: MindNodeItem, colors: (bg: Color, border: Color), collapsed: Bool) -> some View {
        let label = nodeLabel(node, colors: colors, collapsed: collapsed)
        if collapsed && !node.children.isEmpty {
            // Click on a collapsed node expands it locally; the action menu stays on right-click.
            if readOnly {
                Button { expandNode(node) } label: { label }
                    .buttonStyle(.plain)
            } else {
                Button { expandNode(node) } label: { label }
                    .buttonStyle(.plain)
                    .contextMenu { nodeMenuItems(node) }
            }
        } else if readOnly {
            label
        } else {
            Menu {
                nodeMenuItems(node)
            } label: {
                label
            }
            .menuStyle(.borderlessButton)
        }
    }

    private func nodeColors(_ depth: Int) -> (bg: Color, border: Color) {
        switch depth {
        case 0: return (Color(red: 67/255, green: 70/255, blue: 99/255), Color(red: 102/255, green: 108/255, blue: 152/255))
        case 1: return (Color(red: 43/255, green: 50/255, blue: 69/255), Color(red: 68/255, green: 80/255, blue: 108/255))
        case 2: return (Color(red: 37/255, green: 55/255, blue: 58/255), Color(red: 56/255, green: 84/255, blue: 90/255))
        default: return (Color(red: 39/255, green: 56/255, blue: 46/255), Color(red: 60/255, green: 88/255, blue: 72/255))
        }
    }

    private func edgeColor(_ depth: Int) -> Color {
        switch depth {
        case 0: return Color(red: 132/255, green: 140/255, blue: 210/255).opacity(0.85)
        case 1: return Color(red: 122/255, green: 166/255, blue: 198/255).opacity(0.85)
        default: return Color(red: 115/255, green: 178/255, blue: 156/255).opacity(0.85)
        }
    }
}
