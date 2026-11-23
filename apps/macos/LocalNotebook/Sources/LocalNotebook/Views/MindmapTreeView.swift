import SwiftUI

private final class MindNodeItem: Identifiable {
    let id: String
    let label: String
    var depth: Int = 0
    var children: [MindNodeItem] = []
    var x: CGFloat = 0
    var y: CGFloat = 0
    var width: CGFloat = 120
    let height: CGFloat = 34

    init(id: String, label: String) {
        self.id = id
        self.label = label
        var units: CGFloat = 0
        for scalar in label.unicodeScalars {
            units += scalar.value > 127 ? 13.5 : 7.5
        }
        self.width = min(270, max(96, units + 26))
    }
}

struct MindmapTreeView: View {
    let payload: [String: Any]
    let en: Bool
    var onQuizConcept: ((String) -> Void)? = nil
    var onExpandMindmap: ((String) -> Void)? = nil
    var onAskConcept: ((String) -> Void)? = nil

    @State private var collapsedIds: Set<String> = []
    @State private var scale: CGFloat = 1.0
    @State private var allCollapsed: Bool = false
    @State private var panOffset: CGSize = .zero
    @GestureState private var dragDelta: CGSize = .zero

    private let rowGap: CGFloat = 14
    private let colGap: CGFloat = 66
    private let toggleSize: CGFloat = 22
    private let padX: CGFloat = 70
    private let padY: CGFloat = 30

    private var treeLayout: (roots: [MindNodeItem], visible: [MindNodeItem], width: CGFloat, height: CGFloat) {
        let rawNodes = payload["nodes"] as? [[String: Any]] ?? []
        var map: [String: MindNodeItem] = [:]
        var parentById: [String: String] = [:]
        var order: [MindNodeItem] = []

        for (i, n) in rawNodes.enumerated() {
            let id = (n["id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "n\(i)"
            let rawLabel = n["label"] as? String ?? ""
            let label = rawLabel.replacingOccurrences(of: #"\s*(?:\[\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*\]|【\s*\d+(?:\s*[,，\-–—、]\s*\d+)*\s*】)+"#, with: "", options: .regularExpression).trimmingCharacters(in: .whitespacesAndNewlines)
            let node = MindNodeItem(id: id, label: label)
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

        @discardableResult
        func layout(_ node: MindNodeItem, _ x: CGFloat) -> CGFloat {
            node.x = x
            visible.append(node)
            let isCol = collapsedIds.contains(node.id)
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
        return (roots, visible, maxX, maxY)
    }

    var body: some View {
        let laid = treeLayout
        return ZStack(alignment: .topLeading) {
            ScrollView([.horizontal, .vertical]) {
                ZStack(alignment: .topLeading) {
                    Canvas { ctx, _ in
                        func drawEdges(_ node: MindNodeItem) {
                            guard !collapsedIds.contains(node.id), !node.children.isEmpty else { return }
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
                        Menu {
                            Button(L10n.t("kg.action_quiz", en: en)) { onQuizConcept?(node.label) }
                            Button(L10n.t("kg.action_mindmap", en: en)) { onExpandMindmap?(node.label) }
                            Button(L10n.t("kg.action_chat", en: en)) { onAskConcept?(node.label) }
                        } label: {
                            Text(node.label)
                                .font(.system(size: 13, weight: node.depth == 0 ? .semibold : .regular))
                                .foregroundColor(Color(white: 0.93))
                                .lineLimit(1)
                                .padding(.horizontal, 11)
                                .frame(width: node.width, height: node.height, alignment: .leading)
                                .background(colors.bg)
                                .overlay(RoundedRectangle(cornerRadius: 8).stroke(colors.border, lineWidth: 1))
                                .cornerRadius(8)
                        }
                        .menuStyle(.borderlessButton)
                        .frame(width: node.width, height: node.height)
                        .position(x: node.x + node.width / 2, y: node.y + node.height / 2)

                        if !node.children.isEmpty {
                            let isCol = collapsedIds.contains(node.id)
                            Button {
                                withAnimation(.easeOut(duration: 0.16)) {
                                    if isCol { collapsedIds.remove(node.id) }
                                    else { collapsedIds.insert(node.id) }
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

            // Left floating vertical controls
            VStack(spacing: 8) {
                Button {
                    withAnimation(.easeOut(duration: 0.18)) {
                        allCollapsed.toggle()
                        collapsedIds.removeAll()
                        if allCollapsed {
                            for n in laid.visible where n.depth > 0 && !n.children.isEmpty {
                                collapsedIds.insert(n.id)
                            }
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
