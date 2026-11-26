import SwiftUI

/// Lightweight, read-only rendering of unvalidated in-progress artifact payloads
/// (everything except mind maps, which reuse `MindmapTreeView` in read-only mode).
struct ArtifactPreviewView: View {
    let stage: String
    let artifactType: String
    let payload: [String: Any]
    let en: Bool

    private let maxItems = 400
    private let maxBodyChars = 700

    var body: some View {
        if stage == "mindmap_passes" || artifactType == "mindmap" || artifactType == "knowledge_graph" {
            MindmapTreeView(payload: payload, en: en, readOnly: true)
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    content
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
                .padding(.bottom, 8)
            }
        }
    }

    // MARK: helpers

    private func str(_ d: [String: Any], _ key: String) -> String {
        (d[key] as? String) ?? ""
    }

    private func dicts(_ d: [String: Any], _ key: String) -> [[String: Any]] {
        let raw = d[key] as? [Any] ?? []
        return raw.prefix(maxItems).compactMap { $0 as? [String: Any] }
    }

    private func itemText(_ v: Any) -> String {
        if let s = v as? String { return s }
        if let n = v as? NSNumber { return n.stringValue }
        if let d = v as? [String: Any] {
            for k in ["text", "label", "title", "heading", "value", "content"] {
                if let s = d[k] as? String, !s.isEmpty { return s }
            }
        }
        return ""
    }

    private func clip(_ s: String, _ max: Int) -> String {
        s.count <= max ? s : String(s.prefix(max)) + "…"
    }

    private func kind() -> String {
        if ["quiz", "slides", "report", "flashcards", "infographic", "audio"].contains(artifactType) {
            return artifactType
        }
        if payload["questions"] != nil { return "quiz" }
        if payload["slides"] != nil { return "slides" }
        if payload["cards"] != nil { return "flashcards" }
        if payload["blocks"] != nil { return "infographic" }
        if payload["turns"] != nil { return "audio" }
        if payload["sections"] != nil { return "report" }
        return ""
    }

    private func line(_ text: String, size: CGFloat = 12.5, bold: Bool = false, muted: Bool = false, left: CGFloat = 0) -> some View {
        Text(text)
            .font(.system(size: size, weight: bold ? .semibold : .regular))
            .foregroundColor(muted ? Color.secondary.opacity(0.8) : Color.secondary)
            .textSelection(.enabled)
            .padding(.leading, left)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func card<Content: View>(@ViewBuilder _ inner: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            inner()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.05))
        .cornerRadius(10)
    }

    @ViewBuilder
    private var titleBlock: some View {
        if !str(payload, "title").isEmpty {
            line(str(payload, "title"), size: 15, bold: true)
        }
        if !str(payload, "subtitle").isEmpty {
            line(str(payload, "subtitle"), size: 12, muted: true)
        }
    }

    // MARK: per type

    @ViewBuilder
    private var content: some View {
        if stage == "lecture_passes" || artifactType == "lecture" || payload["outline"] != nil {
            lecture
        } else {
            switch kind() {
            case "quiz": quiz
            case "slides": slides
            case "report": report
            case "flashcards": flashcards
            case "infographic": infographic
            case "audio": audio
            default: fallback
            }
        }
    }

    @ViewBuilder
    private var quiz: some View {
        titleBlock
        let qs = dicts(payload, "questions")
        ForEach(Array(qs.enumerated()), id: \.offset) { idx, q in
            card {
                line("\(idx + 1). \(str(q, "prompt"))", size: 13, bold: true)
                let opts = q["options"] as? [Any] ?? []
                ForEach(Array(opts.enumerated()), id: \.offset) { oi, o in
                    let letter = String(Character(UnicodeScalar(UInt8(65 + min(oi, 25)))))
                    line("\(letter). \(itemText(o))", left: 14)
                }
            }
        }
    }

    @ViewBuilder
    private var slides: some View {
        titleBlock
        let items = dicts(payload, "slides")
        ForEach(Array(items.enumerated()), id: \.offset) { idx, s in
            card {
                let t = str(s, "title")
                line("\(idx + 1). \(t.isEmpty ? str(s, "layout") : t)", size: 13, bold: true)
                if !str(s, "subtitle").isEmpty { line(str(s, "subtitle"), size: 12, muted: true) }
                let bullets = (s["bullets"] as? [Any] ?? []).map { itemText($0) }.filter { !$0.isEmpty }
                ForEach(Array(bullets.enumerated()), id: \.offset) { _, b in
                    line("• " + b, left: 8)
                }
                if !str(s, "callout").isEmpty { line(str(s, "callout"), size: 12, muted: true, left: 8) }
            }
        }
    }

    @ViewBuilder
    private var report: some View {
        titleBlock
        let items = dicts(payload, "sections")
        ForEach(Array(items.enumerated()), id: \.offset) { _, s in
            card {
                line(str(s, "heading"), size: 13, bold: true)
                if !str(s, "body_md").isEmpty { line(clip(str(s, "body_md"), maxBodyChars)) }
                let takeaways = (s["key_takeaways"] as? [Any] ?? []).map { itemText($0) }.filter { !$0.isEmpty }
                ForEach(Array(takeaways.enumerated()), id: \.offset) { _, k in
                    line("• " + k, left: 8)
                }
            }
        }
    }

    @ViewBuilder
    private var flashcards: some View {
        titleBlock
        let items = dicts(payload, "cards")
        ForEach(Array(items.enumerated()), id: \.offset) { idx, c in
            card {
                line("\(idx + 1). \(str(c, "front"))", size: 13, bold: true)
                if !str(c, "back").isEmpty { line(str(c, "back"), left: 14) }
            }
        }
    }

    @ViewBuilder
    private var infographic: some View {
        titleBlock
        let items = dicts(payload, "blocks")
        ForEach(Array(items.enumerated()), id: \.offset) { _, b in
            card {
                let head = str(b, "heading")
                let metric = str(b, "metric")
                line(metric.isEmpty ? head : "\(head)  ·  \(metric)", size: 13, bold: true)
                if !str(b, "summary").isEmpty { line(str(b, "summary")) }
            }
        }
    }

    @ViewBuilder
    private var audio: some View {
        titleBlock
        let items = dicts(payload, "turns")
        ForEach(Array(items.enumerated()), id: \.offset) { _, t in
            let sp = str(t, "speaker")
            line(sp.isEmpty ? str(t, "text") : "\(sp): \(str(t, "text"))")
        }
    }

    @ViewBuilder
    private var lecture: some View {
        let outline = payload["outline"] as? [String: Any] ?? [:]
        let done = payload["done"] as? [String: Any] ?? [:]
        if !str(outline, "title").isEmpty { line(str(outline, "title"), size: 15, bold: true) }
        let sections = dicts(outline, "sections")
        ForEach(Array(sections.enumerated()), id: \.offset) { idx, sec in
            let title = str(sec, "title")
            if let content = done["s\(idx + 1)"] as? [String: Any] {
                DisclosureGroup {
                    VStack(alignment: .leading, spacing: 6) {
                        let slideItems = dicts(content, "slides")
                        ForEach(Array(slideItems.enumerated()), id: \.offset) { sn, slide in
                            line("\(sn + 1). \(str(slide, "title"))", bold: true)
                            let narration = (slide["narration"] as? [Any] ?? [])
                                .compactMap { ($0 as? [String: Any])?["text"] as? String }
                                .filter { !$0.isEmpty }
                                .joined(separator: " ")
                            if !narration.isEmpty { line(clip(narration, maxBodyChars), size: 12, muted: true, left: 10) }
                        }
                    }
                    .padding(.top, 4)
                } label: {
                    Text("✓ \(idx + 1). \(title)")
                        .font(.system(size: 13, weight: .semibold))
                }
            } else {
                line("\(idx + 1). \(title)", size: 13, muted: true)
            }
        }
    }

    @ViewBuilder
    private var fallback: some View {
        let json: String = {
            guard let data = try? JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted]),
                  let s = String(data: data, encoding: .utf8) else { return "" }
            return s
        }()
        Text(clip(json, 4000))
            .font(.system(size: 11.5, design: .monospaced))
            .foregroundColor(.secondary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
