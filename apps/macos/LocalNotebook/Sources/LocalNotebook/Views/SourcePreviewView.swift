import AVKit
import PDFKit
import SwiftUI
import WebKit

/// The left pane while a source is open: header, "source guide" card and the original file
/// (or its index / table of contents). Mirrors the Windows inline source preview.
struct SourcePreviewPane: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var scheme
    let request: SourcePreviewRequest

    @State private var data: [String: Any]? = nil
    @State private var kg: [String: Any]? = nil
    @State private var showingIndex = false
    @State private var jumpPage: Int? = nil
    @State private var guideExpanded = true
    @State private var guideBusy = false
    @State private var loading = true
    @State private var pollTask: Task<Void, Never>? = nil

    private var p: AppThemePalette { state.palette(scheme) }
    private var title: String {
        if let t = request.title, !t.isEmpty { return t }
        if let t = data?["title"] as? String, !t.isEmpty { return t }
        return state.t("sources.material_fallback")
    }
    private var kind: String { data?["kind"] as? String ?? "" }
    private var blobPath: String { data?["blob_path"] as? String ?? "" }
    private var previewHtmlPath: String { data?["preview_html_path"] as? String ?? "" }
    private var pageCount: Int { (data?["page_count"] as? NSNumber)?.intValue ?? 0 }
    private var chunks: [[String: Any]] { data?["chunks"] as? [[String: Any]] ?? [] }
    private var indexNodes: [[String: Any]] { data?["index_nodes"] as? [[String: Any]] ?? [] }
    private var guideSummary: String { data?["guide_summary_md"] as? String ?? "" }
    private var guideTopics: [String] { (data?["guide_topics"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty } }
    private var metaLine: String {
        var meta: [String] = []
        if !kind.isEmpty { meta.append(kind.uppercased()) }
        if pageCount > 0 { meta.append(L10n.fmt("sources.pages_summary", en: state.isEnglish, pageCount)) }
        if !indexNodes.isEmpty { meta.append(L10n.fmt("sources.ai_nodes_summary", en: state.isEnglish, indexNodes.count)) }
        return meta.isEmpty ? state.t("sources.original_preview") : meta.joined(separator: " · ")
    }
    private var hasBlob: Bool { !blobPath.isEmpty && FileManager.default.fileExists(atPath: blobPath) }
    private var kgStatus: String { kg?["status"] as? String ?? "" }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            if data != nil {
                guideCard
                content.frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if loading {
                ProgressView().frame(maxWidth: .infinity).padding(.top, 30)
                Spacer()
            } else {
                Spacer()
            }
        }
        .task(id: request) { await load() }
        .onDisappear { pollTask?.cancel() }
    }

    // MARK: Loading

    private func load() async {
        loading = true
        showingIndex = false
        jumpPage = request.page
        let res = await state.run("source.preview", ["source_id": request.sourceId], what: state.t("sources.op_preview"))
        guard let res else {
            loading = false
            state.closeSourcePreview()
            return
        }
        data = res
        await loadKnowledgeGraph()
        loading = false
        await autoGenerateGuideIfMissing()
    }

    private func loadKnowledgeGraph() async {
        let r = try? await state.core.call(method: "source.knowledge_graph", params: ["source_id": request.sourceId])
        kg = r?["artifact"] as? [String: Any]
        pollTask?.cancel()
        if kgStatus == "PENDING" || kgStatus == "RUNNING" {
            pollTask = Task {
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 2_000_000_000)
                    if Task.isCancelled { return }
                    let r = try? await state.core.call(method: "source.knowledge_graph", params: ["source_id": request.sourceId])
                    let next = r?["artifact"] as? [String: Any]
                    await MainActor.run { kg = next }
                    let st = next?["status"] as? String ?? ""
                    if st != "PENDING" && st != "RUNNING" {
                        await state.refreshAll()
                        return
                    }
                }
            }
        }
    }

    /// A source without a guide gets one generated once (when it has been indexed); the button retries.
    private func autoGenerateGuideIfMissing() async {
        guard guideSummary.isEmpty, guideTopics.isEmpty, !state.guideAutoTried.contains(request.sourceId),
              !state.indexingSourceIds.contains(request.sourceId) else { return }
        state.guideAutoTried.insert(request.sourceId)
        await generateGuide()
    }

    private func generateGuide() async {
        guideBusy = true
        defer { guideBusy = false }
        if await state.run("source.generate_guide", ["source_id": request.sourceId], what: state.t("sources.op_gen_guide")) != nil {
            if let res = await state.run("source.preview", ["source_id": request.sourceId], what: state.t("sources.op_preview")) { data = res }
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundColor(p.textPrimary).lineLimit(2)
                Text(metaLine).font(.system(size: 11)).foregroundColor(p.textMuted)
            }
            Spacer(minLength: 4)
            if data != nil {
                Button(showingIndex ? state.t("sources.original_file") : state.t("sources.index_toc")) {
                    showingIndex.toggle()
                }
                .buttonStyle(.bordered).controlSize(.small).help(state.t("sources.toggle_mode_tip"))
                if kgStatus == "READY" {
                    Button(state.t("kg.title")) { openKnowledgeGraph() }.buttonStyle(.bordered).controlSize(.small)
                }
                if hasBlob {
                    iconButton("arrow.up.right.square", state.t("sources.open_external_tip")) {
                        NSWorkspace.shared.open(URL(fileURLWithPath: blobPath))
                    }
                }
            }
            iconButton(state.sourceFullscreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right", state.t("studio.fullscreen_toggle")) {
                state.sourceFullscreen.toggle()
            }
            iconButton("xmark", state.t("sources.back_to_list")) { state.closeSourcePreview() }
        }
    }

    private func iconButton(_ symbol: String, _ tip: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).font(.system(size: 13)).foregroundColor(p.textSecondary).frame(width: 28, height: 28)
        }
        .buttonStyle(.plain)
        .help(tip)
    }

    // MARK: Guide card

    private func openKnowledgeGraph() {
        guard let kg else { return }
        state.activeArtifact = AppState.parseArtifact(kg, en: state.isEnglish)
    }

    private func masterySummary() -> (mastered: Int, total: Int)? {
        let payload = kg?["payload"] as? [String: Any] ?? [:]
        let nodes = payload["nodes"] as? [[String: Any]] ?? []
        guard !nodes.isEmpty else { return nil }
        let roots = Set(nodes.filter { ($0["parent_id"] as? String ?? "").isEmpty }.compactMap { $0["id"] as? String })
        let concepts = nodes.count > 1 ? nodes.compactMap { $0["id"] as? String }.filter { !roots.contains($0) } : nodes.compactMap { $0["id"] as? String }
        let m = (kg?["progress_state"] as? [String: Any])?["mastery"] as? [String: Any] ?? [:]
        let mastered = concepts.filter { ((m[$0] as? [String: Any])?["status"] as? String) == "mastered" }.count
        return (mastered, concepts.count)
    }

    private var guideCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label(state.t("sources.guide_title"), systemImage: "sparkles").font(.system(size: 14, weight: .semibold))
                Spacer()
                iconButton(guideExpanded ? "chevron.up" : "chevron.down", state.t("sources.guide_toggle_tip")) { guideExpanded.toggle() }
            }
            if guideExpanded {
                if !guideSummary.isEmpty {
                    MarkdownView(markdown: guideSummary, fontSize: 13, color: p.textSecondary, mutedColor: p.textMuted, accent: p.accent, lineSpacing: 4)
                        .textSelection(.enabled)
                }
                if !guideTopics.isEmpty {
                    WrapFlowLayout(spacing: 6) {
                        ForEach(guideTopics, id: \.self) { topic in
                            Button {
                                Task { await state.sendChat(L10n.fmt("sources.guide_ask_prompt", en: state.isEnglish, title, topic)) }
                            } label: {
                                Text(topic).font(.system(size: 12)).foregroundColor(p.textPrimary)
                                    .padding(.horizontal, 10).padding(.vertical, 5)
                                    .background(p.panelBg).overlay(Capsule().stroke(p.strongBorder)).clipShape(Capsule())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                if guideSummary.isEmpty && guideTopics.isEmpty {
                    HStack(spacing: 8) {
                        if guideBusy || state.indexingSourceIds.contains(request.sourceId) {
                            ProgressView().controlSize(.small)
                            Text(state.t("sources.guide_generating")).font(.caption).foregroundColor(p.textMuted)
                        } else {
                            Text(state.t("sources.guide_missing")).font(.caption).foregroundColor(p.textMuted)
                            Button(state.t("sources.gen_guide")) { Task { await generateGuide() } }.buttonStyle(.bordered).controlSize(.small)
                        }
                    }
                }
                knowledgeGraphSection
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(p.elevatedBg)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(p.subtleBorder))
        .cornerRadius(12)
    }

    @ViewBuilder
    private var knowledgeGraphSection: some View {
        let recommend = (state.sources.first(where: { $0.id == request.sourceId })?.recommendKnowledgeGraph ?? false) || guideTopics.count >= 3
        if kgStatus == "READY" {
            VStack(alignment: .leading, spacing: 6) {
                if let m = masterySummary() {
                    let pct = m.total > 0 ? Int((Double(m.mastered) * 100 / Double(m.total)).rounded()) : 0
                    Text("\(state.t("kg.title")) · \(L10n.fmt("kg.progress_summary_fmt", en: state.isEnglish, m.mastered, m.total, pct))")
                        .font(.system(size: 12)).foregroundColor(p.textSecondary)
                    ProgressView(value: Double(m.mastered), total: Double(max(m.total, 1)))
                }
                HStack(spacing: 8) {
                    Button(state.t("kg.open_btn")) { openKnowledgeGraph() }.buttonStyle(.borderedProminent).controlSize(.small)
                    Button(state.t("kg.rebuild_btn")) {
                        Task {
                            await state.createKnowledgeGraph(sourceId: request.sourceId, sourceTitle: title)
                            await loadKnowledgeGraph()
                        }
                    }
                    .buttonStyle(.bordered).controlSize(.small)
                }
            }
        } else if kgStatus == "PENDING" || kgStatus == "RUNNING" {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(state.t("kg.building_inline")).font(.system(size: 12)).foregroundColor(p.textSecondary)
                Spacer()
                Button(state.t("art.view_process")) { openKnowledgeGraph() }.buttonStyle(.bordered).controlSize(.small)
            }
        } else if recommend {
            HStack(spacing: 8) {
                Text(state.t(kgStatus.isEmpty ? "kg.recommended_banner" : "kg.title"))
                    .font(.system(size: 12)).foregroundColor(p.kgBannerText)
                Spacer()
                Button(state.t("kg.build_btn")) {
                    Task {
                        await state.createKnowledgeGraph(sourceId: request.sourceId, sourceTitle: title)
                        await loadKnowledgeGraph()
                    }
                }
                .buttonStyle(.borderedProminent).controlSize(.small)
            }
            .padding(10)
            .background(p.kgBannerBg)
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(p.kgBannerBorder))
            .cornerRadius(10)
        }
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        if showingIndex {
            indexTree
        } else {
            originalView
        }
    }

    private var ext: String { (blobPath as NSString).pathExtension.lowercased() }

    @ViewBuilder
    private var originalView: some View {
        if kind == "pdf" && hasBlob {
            PDFFileView(path: blobPath, page: jumpPage, focus: request.focusText)
        } else if kind == "pdf" {
            Text(state.t("sources.original_missing")).font(.callout).foregroundColor(p.textMuted).padding(4)
        } else if !previewHtmlPath.isEmpty && FileManager.default.fileExists(atPath: previewHtmlPath) {
            WebFileView(path: previewHtmlPath, fragment: jumpPage.map { "page-\($0)" })
                .cornerRadius(8)
        } else if (kind == "web" || ["html", "htm", "xhtml"].contains(ext)) && hasBlob {
            WebFileView(path: blobPath, fragment: nil).cornerRadius(8)
        } else if kind == "image" && hasBlob, let img = NSImage(contentsOfFile: blobPath) {
            ScrollView([.horizontal, .vertical]) {
                Image(nsImage: img).resizable().scaledToFit().frame(maxWidth: .infinity)
            }
            .background(Color(hex: 0x141720)).cornerRadius(8)
        } else if (kind == "audio" || kind == "video") && hasBlob {
            VStack(alignment: .leading, spacing: 10) {
                VideoPlayer(player: AVPlayer(url: URL(fileURLWithPath: blobPath)))
                    .frame(height: kind == "video" ? 260 : 60)
                    .cornerRadius(8)
                ScrollView {
                    Text(chunks.compactMap { $0["content"] as? String }.joined(separator: "\n\n"))
                        .font(.system(size: 13.5)).lineSpacing(5).foregroundColor(p.textPrimary)
                        .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        } else if (["md", "note"].contains(kind) || ["md", "markdown", "mdx"].contains(ext)) && hasBlob,
                  let raw = try? String(contentsOfFile: blobPath, encoding: .utf8) {
            ScrollView {
                MarkdownView(markdown: raw, fontSize: 14, color: p.textPrimary, mutedColor: p.textSecondary, accent: p.accent, lineSpacing: 5)
                    .textSelection(.enabled).padding(.trailing, 8).padding(.bottom, 20)
            }
        } else if hasBlob, let raw = try? String(contentsOfFile: blobPath, encoding: .utf8) {
            let code = ["py", "rs", "js", "ts", "jsx", "tsx", "c", "cpp", "h", "hpp", "cs", "java", "go", "sql", "json", "jsonl", "yaml", "yml", "toml", "xml", "ini", "cfg", "sh", "ps1", "bat"].contains(ext)
            ScrollView {
                Text(raw)
                    .font(code ? .system(size: 12.5, design: .monospaced) : .system(size: 13.5))
                    .lineSpacing(5).foregroundColor(p.textPrimary)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(.trailing, 8).padding(.bottom, 20)
            }
        } else {
            chunksFallback
        }
    }

    /// When the original file is gone, the indexed text chunks are what is left.
    private var chunksFallback: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if chunks.isEmpty {
                    Text(state.t("sources.empty_content")).foregroundColor(p.textMuted)
                }
                ForEach(Array(chunks.enumerated()), id: \.offset) { _, c in
                    let page = (c["page_start"] as? NSNumber)?.intValue ?? 0
                    let section = c["section_path"] as? String ?? ""
                    let idx = ((c["chunk_index"] as? NSNumber)?.intValue ?? 0) + 1
                    VStack(alignment: .leading, spacing: 4) {
                        Text(L10n.fmt("sources.chunk_meta", en: state.isEnglish, idx)
                             + (page > 0 ? L10n.fmt("sources.chunk_page", en: state.isEnglish, page) : "")
                             + (section.isEmpty ? "" : " · \(section)"))
                            .font(.system(size: 11)).foregroundColor(p.textMuted)
                        Text(c["content"] as? String ?? "").font(.system(size: 13.5)).lineSpacing(5)
                            .foregroundColor(p.textPrimary).textSelection(.enabled)
                    }
                    .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                    .background(p.elevatedBg).cornerRadius(8)
                }
            }
        }
    }

    /// The AI-built table of contents of the source; a node jumps to its page in the original.
    private var indexTree: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                if indexNodes.isEmpty {
                    Text(state.t("sources.empty_content")).foregroundColor(p.textMuted)
                }
                ForEach(Array(indexNodes.enumerated()), id: \.offset) { _, n in
                    let ps = (n["page_start"] as? NSNumber)?.intValue ?? 0
                    let pe = (n["page_end"] as? NSNumber)?.intValue ?? 0
                    let level = max(0, ((n["level"] as? NSNumber)?.intValue ?? 1) - 1)
                    let label = ps > 0 ? (pe > ps ? L10n.fmt("sources.page_range", en: state.isEnglish, ps, pe) : L10n.fmt("sources.page_single", en: state.isEnglish, ps)) : ""
                    Button {
                        if ps > 0 { jumpPage = ps }
                        showingIndex = false
                    } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            HStack {
                                Text(n["title"] as? String ?? "").font(.system(size: 13, weight: .semibold)).foregroundColor(p.textPrimary)
                                Spacer(minLength: 4)
                                if !label.isEmpty {
                                    Text(label).font(.system(size: 11)).foregroundColor(p.textSecondary)
                                        .padding(.horizontal, 7).padding(.vertical, 1).background(p.panelBg).clipShape(Capsule())
                                }
                            }
                            if let s = n["summary"] as? String, !s.isEmpty {
                                Text(s).font(.system(size: 12)).foregroundColor(p.textSecondary).multilineTextAlignment(.leading)
                            }
                        }
                        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                        .background(p.elevatedBg).cornerRadius(8)
                        .padding(.leading, CGFloat(level) * 14)
                    }
                    .buttonStyle(.plain)
                    .help(ps > 0 ? L10n.fmt("sources.jump_page_tip", en: state.isEnglish, ps) : "")
                }
            }
        }
    }
}

/// A PDF with page jump (PDFKit), highlighting the cited excerpt when it can be found.
struct PDFFileView: NSViewRepresentable {
    let path: String
    let page: Int?
    let focus: String?

    func makeNSView(context: Context) -> PDFView {
        let v = PDFView()
        v.autoScales = true
        v.displayMode = .singlePageContinuous
        v.displaysPageBreaks = true
        v.wantsLayer = true
        v.layer?.cornerRadius = 8
        return v
    }

    func updateNSView(_ v: PDFView, context: Context) {
        if v.document?.documentURL?.path != path {
            v.document = PDFDocument(url: URL(fileURLWithPath: path))
        }
        guard let doc = v.document else { return }
        var jumped = false
        if let focus, !focus.isEmpty {
            let needle = String(focus.prefix(40))
            if let hit = doc.findString(needle, withOptions: [.caseInsensitive]).first(where: { sel in
                guard let pg = sel.pages.first else { return false }
                return page == nil || doc.index(for: pg) + 1 == page
            }) {
                hit.color = NSColor.systemYellow.withAlphaComponent(0.6)
                v.setCurrentSelection(hit, animate: true)
                v.go(to: hit)
                jumped = true
            }
        }
        if !jumped, let page, page > 0, let pg = doc.page(at: min(page, doc.pageCount) - 1) {
            v.go(to: PDFDestination(page: pg, at: NSPoint(x: 0, y: pg.bounds(for: .mediaBox).height)))
        }
    }
}

/// A local HTML file in a web view (Office/EPUB/CSV previews, saved web pages).
struct WebFileView: NSViewRepresentable {
    let path: String
    let fragment: String?

    func makeNSView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        cfg.defaultWebpagePreferences.allowsContentJavaScript = false
        let v = WKWebView(frame: .zero, configuration: cfg)
        v.setValue(false, forKey: "drawsBackground")
        return v
    }

    func updateNSView(_ v: WKWebView, context: Context) {
        let url = URL(fileURLWithPath: path)
        guard context.coordinator.loaded != path + (fragment ?? "") else { return }
        context.coordinator.loaded = path + (fragment ?? "")
        var target = url
        if let fragment, !fragment.isEmpty, var comps = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            comps.fragment = fragment
            target = comps.url ?? url
        }
        v.loadFileURL(target, allowingReadAccessTo: url.deletingLastPathComponent())
    }

    func makeCoordinator() -> Coordinator { Coordinator() }
    final class Coordinator { var loaded = "" }
}
