import SwiftUI

/// Native Markdown + math rendering: SwiftUI `Text` with `AttributedString`, no WebView and no
/// extra packages. TeX is converted to Unicode math (`TexMath`); unmappable scripts are raised
/// or lowered with `baselineOffset`.
struct MarkdownView: View {
    let markdown: String
    var fontSize: CGFloat = 14
    var color: Color = .primary
    var mutedColor: Color = .secondary
    var accent: Color = .accentColor
    var lineSpacing: CGFloat = 4
    /// `[n]` markers that are clickable (the answer's known citations); they open `lnacite://n`.
    var citations: Set<Int> = []

    private var blocks: [MdParser.Block] { MdBlockCache.blocks(for: markdown) }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                blockView(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var style: MdInlineStyle {
        MdInlineStyle(fontSize: fontSize, color: color, accent: accent, citations: citations)
    }

    @ViewBuilder
    private func blockView(_ block: MdParser.Block) -> some View {
        switch block {
        case let .heading(level, text):
            let scale: CGFloat = level == 1 ? 1.35 : (level == 2 ? 1.2 : 1.08)
            Text(MdAttributed.inline(text, style.with(size: fontSize * scale, weight: .semibold)))
                .padding(.top, 4)
                .fixedSize(horizontal: false, vertical: true)
        case let .paragraph(text):
            Text(MdAttributed.inline(text, style))
                .lineSpacing(lineSpacing)
                .fixedSize(horizontal: false, vertical: true)
        case let .listItem(ordered, marker, depth, text):
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker)
                    .font(.system(size: fontSize))
                    .foregroundColor(mutedColor)
                    .frame(minWidth: ordered ? 20 : 12, alignment: .leading)
                Text(MdAttributed.inline(text, style))
                    .lineSpacing(lineSpacing)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, CGFloat(depth) * 16)
        case let .quote(text):
            HStack(alignment: .top, spacing: 10) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(mutedColor.opacity(0.5))
                    .frame(width: 3)
                Text(MdAttributed.inline(text, style.with(color: mutedColor)))
                    .lineSpacing(lineSpacing)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .fixedSize(horizontal: false, vertical: true)
        case let .code(text):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(.system(size: fontSize * 0.9, design: .monospaced))
                    .foregroundColor(color)
                    .padding(10)
            }
            .background(Color.gray.opacity(0.15))
            .cornerRadius(8)
        case let .math(tex):
            Text(MdAttributed.math(tex, style.with(size: fontSize * 1.1)))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 2)
        case .rule:
            Divider().padding(.vertical, 4)
        case let .table(header, rows):
            tableView(header: header, rows: rows)
        }
    }

    private func tableView(header: [String], rows: [[String]]) -> some View {
        let cols = max(header.count, rows.map(\.count).max() ?? 0)
        return Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
            GridRow {
                ForEach(0..<cols, id: \.self) { ci in
                    Text(MdAttributed.inline(ci < header.count ? header[ci] : "", style.with(size: fontSize * 0.92, weight: .semibold)))
                }
            }
            Divider()
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                GridRow {
                    ForEach(0..<cols, id: \.self) { ci in
                        Text(MdAttributed.inline(ci < row.count ? row[ci] : "", style.with(size: fontSize * 0.92)))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
        .padding(10)
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(mutedColor.opacity(0.35), lineWidth: 1))
    }
}

/// Single-line / inline Markdown (no block structure): quiz options, card faces, slide bullets.
struct MarkdownInlineText: View {
    let text: String
    var fontSize: CGFloat = 14
    var weight: Font.Weight = .regular
    var color: Color = .primary
    var accent: Color = .accentColor

    var body: some View {
        Text(MdAttributed.inline(text, MdInlineStyle(fontSize: fontSize, color: color, accent: accent, weight: weight)))
            .fixedSize(horizontal: false, vertical: true)
    }
}

struct MdInlineStyle {
    var fontSize: CGFloat
    var color: Color
    var accent: Color
    var weight: Font.Weight = .regular
    var citations: Set<Int> = []

    func with(size: CGFloat? = nil, weight: Font.Weight? = nil, color: Color? = nil) -> MdInlineStyle {
        var s = self
        if let size { s.fontSize = size }
        if let weight { s.weight = weight }
        if let color { s.color = color }
        return s
    }
}

enum MdAttributed {
    static func inline(_ text: String, _ st: MdInlineStyle) -> AttributedString {
        var result = AttributedString()
        for span in MdParser.parseInline(text) {
            switch span {
            case let .text(s, bold, italic, strike):
                var a = AttributedString(s)
                var font = Font.system(size: st.fontSize, weight: bold ? .semibold : st.weight)
                if italic { font = font.italic() }
                a.font = font
                a.foregroundColor = st.color
                if strike { a.strikethroughStyle = .single }
                result.append(a)
            case let .code(s):
                var a = AttributedString(s)
                a.font = .system(size: st.fontSize * 0.92, design: .monospaced)
                a.foregroundColor = Color(red: 0.63, green: 0.78, blue: 1.0)
                a.backgroundColor = Color.gray.opacity(0.2)
                result.append(a)
            case let .math(tex):
                result.append(math(tex, st))
            case let .link(label, url):
                var a = AttributedString(label)
                a.font = .system(size: st.fontSize, weight: st.weight)
                a.foregroundColor = st.accent
                a.underlineStyle = .single
                if let u = URL(string: url) { a.link = u }
                result.append(a)
            case let .cite(n):
                var a = AttributedString("[\(n)]")
                a.font = .system(size: st.fontSize * 0.72, weight: .semibold)
                a.foregroundColor = st.accent
                a.baselineOffset = st.fontSize * 0.38
                if st.citations.contains(n), let u = URL(string: "lnacite://\(n)") {
                    a.link = u
                    a.underlineStyle = .single
                }
                result.append(a)
            }
        }
        return result
    }

    static func math(_ tex: String, _ st: MdInlineStyle) -> AttributedString {
        var result = AttributedString()
        for run in TexMath.toRuns(tex) {
            var a = AttributedString(run.text)
            let size = run.shift == 0 ? st.fontSize : st.fontSize * 0.72
            a.font = .system(size: size, weight: st.weight, design: .serif)
            a.foregroundColor = st.color
            if run.shift > 0 { a.baselineOffset = st.fontSize * 0.38 }
            if run.shift < 0 { a.baselineOffset = -st.fontSize * 0.15 }
            result.append(a)
        }
        return result
    }
}

/// Parsed blocks keyed by source text, so SwiftUI re-renders don't re-parse unchanged messages.
private final class MdBlockBox {
    let blocks: [MdParser.Block]
    init(_ blocks: [MdParser.Block]) { self.blocks = blocks }
}

private enum MdBlockCache {
    static let cache: NSCache<NSString, MdBlockBox> = {
        let c = NSCache<NSString, MdBlockBox>()
        c.countLimit = 256
        return c
    }()

    static func blocks(for text: String) -> [MdParser.Block] {
        let key = text as NSString
        if let hit = cache.object(forKey: key) { return hit.blocks }
        let parsed = MdParser.parse(text)
        cache.setObject(MdBlockBox(parsed), forKey: key)
        return parsed
    }
}
