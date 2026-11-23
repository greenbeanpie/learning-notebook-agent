import SwiftUI

struct MainView: View {
    @EnvironmentObject var state: AppState
    @State private var chatInput = ""
    @State private var hoveredSourceId: String? = nil

    private var isLight: Bool { state.prefs.theme == "light" }
    private var canvasBg: Color { isLight ? Color(white: 0.96) : Color(red: 0x13/255, green: 0x13/255, blue: 0x14/255) }
    private var panelBg: Color { isLight ? Color.white : Color(red: 0x1E/255, green: 0x1F/255, blue: 0x20/255) }
    private var elevatedBg: Color { isLight ? Color(white: 0.94) : Color(red: 0x28/255, green: 0x2A/255, blue: 0x2C/255) }

    private var resolvedColorScheme: ColorScheme? {
        switch state.prefs.theme {
        case "light": return .light
        case "system": return nil
        default: return .dark
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            topBar
            GeometryReader { geo in
                HStack(spacing: 12) {
                    if !state.studioFullscreen {
                        sourcesPane
                            .frame(width: state.activeArtifact != nil ? 250 : 300)
                            .transition(.opacity.combined(with: .move(edge: .leading)))
                        chatPane
                            .frame(maxWidth: .infinity)
                            .transition(.opacity)
                    }
                    studioPane
                        .frame(
                            width: state.studioFullscreen
                                ? geo.size.width
                                : (state.activeArtifact != nil ? max(460, geo.size.width * 0.48) : 340)
                        )
                }
                .animation(state.prefs.fullscreenAnimation ? .spring(response: 0.22, dampingFraction: 0.86) : nil, value: state.studioFullscreen)
            }
            if !state.statusMessage.isEmpty {
                HStack {
                    Text(state.statusMessage)
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Spacer()
                    Button(state.t("settings.close")) { state.statusMessage = "" }
                        .buttonStyle(.plain)
                        .font(.caption)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(elevatedBg)
                .cornerRadius(10)
            }
        }
        .padding(14)
        .background(canvasBg)
        .preferredColorScheme(resolvedColorScheme)
        .sheet(isPresented: Binding(get: { pendingStudioKind != nil }, set: { if !$0 { pendingStudioKind = nil } })) {
            VStack(alignment: .leading, spacing: 12) {
                Text(state.isEnglish ? "Configure Studio Artifact" : "生成配置")
                    .font(.headline)
                Text(state.isEnglish ? "Prompt / Focus & Scope (Optional)" : "生成提示与范围要求（可选）")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                TextEditor(text: $studioPromptInput)
                    .frame(minHeight: 110, maxHeight: 180)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.gray.opacity(0.35)))
                if pendingStudioKind == "mindmap" || pendingStudioKind == "knowledge_graph" {
                    Picker(state.isEnglish ? "Detail Level" : "详尽程度", selection: $studioDetailLevel) {
                        Text(state.isEnglish ? "Concise (2-3 levels)" : "简洁（2~3 层概览）").tag("concise")
                        Text(state.isEnglish ? "Standard (3-4 levels)" : "标准（3~4 层均衡）").tag("standard")
                        Text(state.isEnglish ? "Detailed (4-5 levels)" : "详尽（4~5 层细分）").tag("detailed")
                    }
                    .pickerStyle(.segmented)
                } else if pendingStudioKind == "report" {
                    Picker(state.isEnglish ? "Depth" : "报告深度", selection: $studioDetailLevel) {
                        Text(state.isEnglish ? "Brief" : "简明摘要").tag("brief")
                        Text(state.isEnglish ? "Standard" : "标准报告").tag("standard")
                        Text(state.isEnglish ? "Deep" : "深度解析").tag("deep")
                    }
                    .pickerStyle(.segmented)
                }
                HStack {
                    Spacer()
                    Button(state.isEnglish ? "Cancel" : "取消") { pendingStudioKind = nil }
                    Button(state.isEnglish ? "Generate" : "开始生成") {
                        let k = pendingStudioKind ?? "mindmap"
                        let p = studioPromptInput.trimmingCharacters(in: .whitespacesAndNewlines)
                        let d = studioDetailLevel
                        pendingStudioKind = nil
                        Task { await state.createStudio(kind: k, topic: p, detail: d) }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding(20)
            .frame(width: 460)
        }
        .sheet(isPresented: $state.showSettingsSheet) {
            UnifiedSettingsSheet().environmentObject(state)
        }
    }

    private var topBar: some View {
        HStack(spacing: 12) {
            Text("Local Notebook Agent")
                .font(.headline)
            Picker("", selection: $state.activeNotebookId) {
                ForEach(state.notebooks) { nb in
                    Text(nb.title).tag(nb.id)
                }
            }
            .frame(width: 200)
            .onChange(of: state.activeNotebookId) { _, _ in
                Task { await state.refreshAll() }
            }
            Spacer()
            Button(state.t("nb.new")) {
                Task {
                    if let res = try? await state.core.call(method: "notebook.create", params: ["title": state.t("nb.new")]),
                       let id = res["id"] as? String {
                        state.activeNotebookId = id
                        await state.refreshAll()
                    }
                }
            }
            .buttonStyle(.bordered)

            Button {
                state.showSettingsSheet = true
            } label: {
                Label(state.t("settings.title"), systemImage: "gearshape")
            }
            .buttonStyle(.bordered)
        }
    }

    private var sourcesPane: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(state.t("sources.title")).font(.subheadline.bold())
            HStack(spacing: 8) {
                Menu(state.t("sources.add")) {
                    Button(state.t("sources.add_file")) { pickAndAddFile() }
                    Button(state.t("sources.add_paste")) {
                        Task {
                            _ = try? await state.core.call(method: "source.add_text", params: [
                                "notebook_id": state.activeNotebookId,
                                "title": "pasted-note.txt",
                                "text": "Sample source content."
                            ])
                            await state.refreshAll()
                        }
                    }
                }
                .menuStyle(.borderlessButton)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(elevatedBg)
                .cornerRadius(16)

                Button(state.t("sources.create_note")) {
                    Task {
                        _ = try? await state.core.call(method: "note.save", params: [
                            "notebook_id": state.activeNotebookId,
                            "title": "Note",
                            "body_md": ""
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(state.sources) { src in
                        HStack {
                            Image(systemName: "doc.text")
                                .foregroundColor(Color(red: 0x8A/255, green: 0xB4/255, blue: 0xF8/255))
                            Text(src.title).lineLimit(1).font(.callout)
                            Spacer()
                            Toggle("", isOn: Binding(
                                get: { src.isSelected },
                                set: { val in
                                    Task {
                                        _ = try? await state.core.call(method: "source.set_selected", params: ["source_id": src.id, "selected": val])
                                        await state.refreshAll()
                                    }
                                }
                            ))
                            .labelsHidden()
                        }
                        .padding(8)
                        .background(hoveredSourceId == src.id ? elevatedBg : Color.clear)
                        .cornerRadius(10)
                        .onHover { hovering in
                            withAnimation(.easeOut(duration: 0.12)) {
                                hoveredSourceId = hovering ? src.id : (hoveredSourceId == src.id ? nil : hoveredSourceId)
                            }
                        }
                        if src.recommendKnowledgeGraph {
                            HStack(spacing: 6) {
                                Text(state.t("kg.recommended"))
                                    .font(.caption2)
                                    .foregroundColor(Color(red: 0.83, green: 0.89, blue: 0.97))
                                Spacer()
                                Button(state.t("kg.build_btn")) {
                                    Task { await state.createKnowledgeGraph(sourceTitle: src.title) }
                                }
                                .buttonStyle(.borderedProminent)
                                .controlSize(.mini)
                            }
                            .padding(8)
                            .background(Color(red: 34/255, green: 52/255, blue: 68/255))
                            .cornerRadius(10)
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(panelBg)
        .cornerRadius(16)
    }

    private var chatPane: some View {
        VStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 8) {
                Text(state.summaryText)
                    .font(.system(size: state.prefs.fontSize))
                    .foregroundColor(.secondary)
                    .lineLimit(5)
                if state.prefs.showSuggestedQuestions && !state.suggestedQuestions.isEmpty {
                    WrapFlowLayout(spacing: 8) {
                        ForEach(state.suggestedQuestions.prefix(6), id: \.self) { q in
                            Button(q) {
                                Task { await state.sendChat(q) }
                            }
                            .buttonStyle(.bordered)
                            .font(.caption)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(elevatedBg)
            .cornerRadius(12)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(state.messages) { msg in
                        ChatMessageRow(message: msg, defaultExpanded: state.prefs.expandThoughtsByDefault, en: state.isEnglish, fontSize: state.prefs.fontSize)
                    }
                }
            }

            HStack(spacing: 8) {
                TextField(state.t("chat.placeholder"), text: $chatInput)
                    .textFieldStyle(.plain)
                    .font(.system(size: state.prefs.fontSize))
                    .onSubmit { submitChat() }
                Button(action: submitChat) {
                    Image(systemName: "arrow.up")
                        .foregroundColor(.black)
                        .frame(width: 32, height: 32)
                        .background(Color(red: 0xA8/255, green: 0xC7/255, blue: 0xFA/255))
                        .clipShape(Circle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(elevatedBg)
            .cornerRadius(24)
        }
        .padding(16)
        .background(panelBg)
        .cornerRadius(16)
    }

    private var studioPane: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let art = state.activeArtifact {
                ArtifactViewerView(artifact: art)
                    .environmentObject(state)
            } else {
                Text("Studio").font(.subheadline.bold())
                let tiles: [(String, String, String)] = [
                    ("audio", state.t("studio.audio"), "waveform"),
                    ("slides", state.t("studio.slides"), "rectangle.stack"),
                    ("mindmap", state.t("studio.mindmap"), "point.3.connected.trianglepath.dotted"),
                    ("report", state.t("studio.report"), "doc.richtext"),
                    ("flashcards", state.t("studio.flashcards"), "square.on.square"),
                    ("quiz", state.t("studio.quiz"), "checkmark.circle"),
                    ("infographic", state.t("studio.infographic"), "chart.bar.xaxis"),
                    ("knowledge_graph", state.t("studio.knowledge_graph"), "circle.hexagongrid"),
                ]
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                    ForEach(tiles, id: \.0) { kind, label, icon in
                        Button {
                            studioPromptInput = ""; studioDetailLevel = "standard"; pendingStudioKind = kind
                        } label: {
                            HStack(spacing: 8) {
                                Image(systemName: icon)
                                Text(label).font(.callout)
                                Spacer()
                            }
                            .padding(10)
                            .background(elevatedBg)
                            .cornerRadius(12)
                        }
                        .buttonStyle(.plain)
                    }
                }
                ScrollView {
                    VStack(spacing: 8) {
                        ForEach(state.artifacts) { art in
                            Button {
                                state.activeArtifact = art
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(art.title).font(.callout.weight(.medium))
                                        Text("\(state.t("studio.\(art.artifactType)")) · \(art.status)")
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                    Spacer()
                                }
                                .padding(10)
                                .background(elevatedBg)
                                .cornerRadius(12)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(panelBg)
        .cornerRadius(16)
    }

    private func submitChat() {
        let txt = chatInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !txt.isEmpty else { return }
        chatInput = ""
        Task { await state.sendChat(txt) }
    }

    private func pickAndAddFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                _ = try? await state.core.call(method: "source.add_files", params: [
                    "notebook_id": state.activeNotebookId,
                    "paths": [url.path]
                ])
                await state.refreshAll()
            }
        }
    }
}

struct ChatMessageRow: View {
    let message: ChatMessageItem
    let defaultExpanded: Bool
    let en: Bool
    let fontSize: Double
    @State private var expanded: Bool = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(message.role == "user" ? L10n.t("chat.you", en: en) : L10n.t("chat.assistant", en: en))
                .font(.caption)
                .foregroundColor(.secondary)

            if message.role == "assistant" && !message.thoughts.isEmpty {
                Button {
                    expanded.toggle()
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "circle")
                            .font(.system(size: 11, weight: .bold))
                        Text("Thoughts")
                            .font(.callout)
                        Image(systemName: expanded ? "chevron.up" : "chevron.down")
                            .font(.caption)
                    }
                    .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)

                if expanded {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(message.thoughts) { step in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 8) {
                                    Image(systemName: step.isTool ? "doc.text.magnifyingglass" : "brain")
                                        .foregroundColor(.secondary)
                                    Text(step.title)
                                        .font(.callout.weight(step.isTool ? .regular : .medium))
                                }
                                if !step.isTool && !step.detail.isEmpty {
                                    HStack(spacing: 10) {
                                        Rectangle()
                                            .fill(Color.secondary.opacity(0.35))
                                            .frame(width: 1)
                                            .padding(.leading, 7)
                                        Text(step.detail)
                                            .font(.callout)
                                            .foregroundColor(.secondary)
                                    }
                                }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }

            Text(message.content)
                .textSelection(.enabled)
                .font(.system(size: fontSize, weight: .regular))
                .lineSpacing(6)
        }
        .onAppear { expanded = defaultExpanded }
    }
}

struct WrapFlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var lineH: CGFloat = 0
        var maxLineW: CGFloat = 0
        for sub in subviews {
            let sz = sub.sizeThatFits(.unspecified)
            if x > 0 && x + spacing + sz.width > maxW {
                maxLineW = max(maxLineW, x)
                y += lineH + spacing
                x = sz.width
                lineH = sz.height
            } else {
                if x > 0 { x += spacing }
                x += sz.width
                lineH = max(lineH, sz.height)
            }
        }
        return CGSize(width: max(maxLineW, x), height: y + lineH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var lineH: CGFloat = 0
        for sub in subviews {
            let sz = sub.sizeThatFits(.unspecified)
            if x > bounds.minX && x + spacing + sz.width > bounds.maxX {
                x = bounds.minX
                y += lineH + spacing
                lineH = 0
            } else if x > bounds.minX {
                x += spacing
            }
            sub.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(sz))
            x += sz.width
            lineH = max(lineH, sz.height)
        }
    }
}
