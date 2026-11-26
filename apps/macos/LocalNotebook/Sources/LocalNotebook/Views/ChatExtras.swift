import SwiftUI

/// Popover with the source excerpt behind a `[n]` marker (`chat.citation_detail`).
struct CitationDetail: Identifiable {
    let id = UUID()
    let no: Int
    let sourceTitle: String
    let sectionPath: String
    let page: Int
    let content: String
    let removed: Bool

    init(no: Int, _ d: [String: Any]) {
        self.no = no
        sourceTitle = d["source_title"] as? String ?? ""
        sectionPath = d["section_path"] as? String ?? ""
        page = (d["page_start"] as? NSNumber)?.intValue ?? 0
        content = d["content"] as? String ?? ""
        removed = d["source_removed"] as? Bool ?? false
    }

    /// OCR / dot-leader noise collapsed, as the Windows client does before showing an excerpt.
    var cleanExcerpt: String {
        var t = content
        t = t.replacingOccurrences(of: "[·.…•․]{3,}", with: " ", options: .regularExpression)
        t = t.replacingOccurrences(of: "(\\s*[.·]\\s*){4,}", with: " ", options: .regularExpression)
        t = t.replacingOccurrences(of: "\\s{2,}", with: " ", options: .regularExpression)
        return t.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Too little real text (figure page, garbled scan): the PDF page itself is the better evidence.
    var isUnclear: Bool {
        if content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return true }
        var meaningful = 0, noise = 0
        for ch in content.unicodeScalars {
            if CharacterSet.alphanumerics.contains(ch) || (0x4E00...0x9FFF).contains(ch.value) { meaningful += 1 }
            else if ".·…•_-".unicodeScalars.contains(ch) { noise += 1 }
        }
        if meaningful < 22 { return true }
        let total = meaningful + noise
        return total > 0 && Double(noise) / Double(total) > 0.32
    }
}

struct CitationPopover: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let detail: CitationDetail
    let onOpen: (SourcePreviewRequest) -> Void

    private var p: AppThemePalette { state.palette(scheme) }
    private var source: SourceItem? { state.sources.first(where: { $0.title == detail.sourceTitle }) }
    private var isPdf: Bool { detail.sourceTitle.lowercased().hasSuffix(".pdf") }

    private func request() -> SourcePreviewRequest? {
        guard let src = source else { return nil }
        return SourcePreviewRequest(
            sourceId: src.id,
            page: detail.page > 0 ? detail.page : nil,
            focusText: detail.isUnclear ? nil : detail.cleanExcerpt,
            title: detail.sourceTitle
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label(state.t("cite.header"), systemImage: "doc.text").font(.system(size: 14, weight: .semibold))
                Spacer()
                if detail.page > 0 {
                    Text(L10n.fmt("cite.page", en: state.isEnglish, detail.page))
                        .font(.system(size: 11.5)).foregroundColor(p.textSecondary)
                        .padding(.horizontal, 8).padding(.vertical, 2)
                        .background(p.panelBg).overlay(Capsule().stroke(p.strongBorder)).clipShape(Capsule())
                }
            }
            let title = detail.sectionPath.isEmpty ? detail.sourceTitle : "\(detail.sourceTitle) · \(detail.sectionPath)"
            if !detail.removed, let r = request() {
                Button { onOpen(r) } label: {
                    Text(title).font(.system(size: 13.5, weight: .semibold)).foregroundColor(p.accent).multilineTextAlignment(.leading)
                }
                .buttonStyle(.plain)
            } else {
                Text(title).font(.system(size: 13.5, weight: .semibold))
            }
            if detail.removed {
                Text(state.t("cite.removed")).font(.system(size: 12)).foregroundColor(Color(hex: 0xFFB478))
            }
            let excerpt = detail.cleanExcerpt
            let body = detail.isUnclear && isPdf && detail.page > 0
                ? L10n.fmt("cite.unclear_pdf", en: state.isEnglish, detail.page)
                : String(excerpt.prefix(260)) + (excerpt.count > 260 ? "…" : "")
            if !body.isEmpty {
                Text(body).font(.system(size: 13)).foregroundColor(p.textSecondary).lineSpacing(4)
                    .lineLimit(6).textSelection(.enabled)
            }
            if !detail.removed, let r = request() {
                HStack {
                    Spacer()
                    Button(detail.page > 0 ? L10n.fmt("cite.open_page", en: state.isEnglish, detail.page) : state.t("cite.open_source")) { onOpen(r) }
                        .buttonStyle(.bordered).controlSize(.small)
                }
            }
        }
        .padding(14)
        .frame(width: 360)
    }
}

/// What to do in an empty chat (shown instead of an empty list).
struct ChatEmptyHint: View {
    let state: AppState
    let palette: AppThemePalette

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(state.t("chat.empty_title")).font(.system(size: 16, weight: .semibold)).foregroundColor(palette.textPrimary)
            ForEach(["chat.empty_step1", "chat.empty_step2", "chat.empty_step3"], id: \.self) { k in
                Text(state.t(k)).font(.system(size: 13)).foregroundColor(palette.textSecondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 20)
    }
}

/// A Studio artifact the assistant generated with a tool call, shown inline in the chat.
struct ChatArtifactCardView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let raw: [String: Any]
    @State private var expanded = true
    @State private var importing = false

    private var art: StudioArtifactItem { AppState.parseArtifact(raw, en: state.isEnglish) }
    private var p: AppThemePalette { state.palette(scheme) }
    private var inStudio: Bool { (raw["in_studio"] as? Bool ?? false) || state.chatArtifactsInStudio.contains(art.id) }

    private func kindLabel(_ k: String) -> String {
        ["audio", "slides", "mindmap", "report", "flashcards", "quiz", "infographic", "lecture"].contains(k) ? state.t("studio.\(k)") : k
    }

    private func stageLabel(_ s: String) -> String {
        switch s {
        case "COLLECT": return state.t("studio.status_collect")
        case "PLAN": return state.t("studio.status_plan")
        case "GENERATE": return state.t("studio.status_generate")
        case "RENDER": return state.t("studio.status_render")
        default: return state.t("studio.status_processing")
        }
    }

    private var metaLine: String {
        let label = kindLabel(art.artifactType)
        switch art.status {
        case "PENDING", "RUNNING": return "\(label) · \(stageLabel(art.stage))"
        case "FAILED": return L10n.fmt("art.failed_fmt", en: state.isEnglish, label)
        case "CANCELLED": return L10n.fmt("art.cancelled_fmt", en: state.isEnglish, label)
        case "READY":
            let (prop, unit): (String, String) = {
                switch art.artifactType {
                case "quiz": return ("questions", "art.unit_questions")
                case "flashcards": return ("cards", "art.unit_cards")
                case "mindmap": return ("nodes", "art.unit_nodes")
                case "report": return ("sections", "art.unit_sections")
                case "slides", "lecture": return ("slides", "art.unit_slides")
                case "infographic": return ("blocks", "art.unit_blocks")
                case "audio": return ("turns", "art.unit_turns")
                default: return ("", "")
                }
            }()
            if !prop.isEmpty, let n = (art.payload[prop] as? [Any])?.count, n > 0 {
                return L10n.fmt("art.count_fmt", en: state.isEnglish, label, n, state.t(unit))
            }
            return L10n.fmt("art.interactive_preview", en: state.isEnglish, label)
        default: return label
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Image(systemName: icon(art.artifactType)).foregroundColor(Color.studioKindColor(art.artifactType)).frame(width: 20)
                VStack(alignment: .leading, spacing: 2) {
                    Text(art.title).font(.system(size: 14, weight: .semibold))
                    Text(metaLine).font(.system(size: 12)).foregroundColor(p.textMuted)
                }
                Spacer(minLength: 6)
                Button(state.t("art.view_process")) { state.activeArtifact = art }
                    .buttonStyle(.bordered).controlSize(.small).help(state.t("art.view_process_tip"))
                if art.status == "READY" {
                    Button(expanded ? state.t("art.collapse") : state.t("art.expand_preview")) { expanded.toggle() }
                        .buttonStyle(.bordered).controlSize(.small)
                    if inStudio {
                        Button(state.t("art.open_in_studio")) { state.activeArtifact = art }
                            .buttonStyle(.bordered).controlSize(.small).help(state.t("art.open_in_studio_tip"))
                    } else {
                        Button(importing ? state.t("art.importing_studio") : state.t("art.import_studio")) { Task { await importToStudio() } }
                            .buttonStyle(.borderedProminent).controlSize(.small).disabled(importing).help(state.t("art.import_studio_tip"))
                    }
                }
            }
            bodyView
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(p.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.subtleBorder))
        .cornerRadius(12)
    }

    @ViewBuilder
    private var bodyView: some View {
        switch art.status {
        case "PENDING", "RUNNING":
            VStack(alignment: .leading, spacing: 6) {
                Text(L10n.fmt("art.running_hint", en: state.isEnglish, kindLabel(art.artifactType), stageLabel(art.stage)))
                    .font(.system(size: 12)).foregroundColor(p.textSecondary)
                if art.progress > 0.01 { ProgressView(value: min(1, art.progress)) } else { ProgressView().progressViewStyle(.linear) }
                Button(state.t("chat.stop")) { Task { _ = await state.run("studio.cancel", ["artifact_id": art.id]) } }
                    .buttonStyle(.bordered).controlSize(.small)
            }
        case "FAILED":
            VStack(alignment: .leading, spacing: 6) {
                Text(L10n.advice(art.errorCode, en: state.isEnglish) ?? state.t("studio.failed"))
                    .font(.callout).foregroundColor(Color(hex: 0xFFA08C))
                Button(state.t("studio.retry")) { Task { await state.retryArtifact(artifactId: art.id) } }.buttonStyle(.bordered).controlSize(.small)
            }
        case "CANCELLED":
            VStack(alignment: .leading, spacing: 6) {
                Text(state.t("studio.cancelled")).font(.system(size: 12)).foregroundColor(p.textMuted)
                Button(state.t("studio.regenerate")) { Task { await state.retryArtifact(artifactId: art.id) } }.buttonStyle(.bordered).controlSize(.small)
            }
        default:
            if expanded { ArtifactReadyBody(artifact: art, compact: true, showFeedback: false).frame(maxHeight: 460) }
        }
    }

    private func importToStudio() async {
        importing = true
        defer { importing = false }
        guard await state.run("studio.import_to_studio", ["artifact_id": art.id], what: state.t("art.import_failed")) != nil else { return }
        state.chatArtifactsInStudio.insert(art.id)
        state.statusMessage = "\(state.t("art.imported_title"))\(L10n.t("common.colon", en: state.isEnglish))\(L10n.fmt("art.imported_msg", en: state.isEnglish, art.title))"
        await state.refreshAll()
    }

    private func icon(_ kind: String) -> String {
        switch kind {
        case "audio": return "mic.fill"
        case "slides": return "rectangle.stack.fill"
        case "mindmap": return "point.3.connected.trianglepath.dotted"
        case "report": return "doc.richtext.fill"
        case "flashcards": return "square.on.square"
        case "quiz": return "checkmark.circle.fill"
        case "lecture": return "graduationcap.fill"
        default: return "chart.bar.xaxis"
        }
    }
}
