import SwiftUI

struct MainView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var systemColorScheme

    @State private var chatInput = ""
    @State private var hoveredSourceId: String? = nil
    @State private var hoveredTileKind: String? = nil
    @State private var hoveredArtifactId: String? = nil
    @State private var pendingStudioKind: String? = nil
    @State private var studioPromptInput: String = ""
    @State private var studioDetailLevel: String = "standard"

    // Windows-synced helper dialogs
    @State private var titleDraft = ""
    @State private var titleHovered = false
    @FocusState private var titleFocused: Bool
    @State private var showAddUrlSheet = false
    @State private var addUrlInput = ""
    @State private var showPasteTextSheet = false
    @State private var pasteTitleInput = ""
    @State private var pasteBodyInput = ""
    @State private var renamingSource: SourceItem? = nil
    @State private var renameSourceInput = ""
    @State private var renamingArtifact: StudioArtifactItem? = nil
    @State private var renameArtifactInput = ""
    @State private var editingNote: NoteItem? = nil
    @State private var editNoteTitle = ""
    @State private var editNoteBody = ""

    private var themeId: AppThemeId { AppThemeId.resolve(state.prefs.theme) }
    private var palette: AppThemePalette { themeId.palette(systemScheme: systemColorScheme) }

    private var activeNotebookTitle: String {
        if let nb = state.notebooks.first(where: { $0.id == state.activeNotebookId }), !nb.title.isEmpty {
            return nb.title
        }
        return ""
    }

    var body: some View {
        VStack(spacing: 12) {
            topBar
            GeometryReader { geo in
                HStack(spacing: 14) {
                    if !state.studioFullscreen {
                        sourcesPane
                            .frame(width: state.activeArtifact != nil ? 250 : 320)
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
                HStack(spacing: 10) {
                    Image(systemName: "info.circle.fill")
                        .font(.system(size: 14))
                        .foregroundColor(palette.accent)
                    Text(state.statusMessage)
                        .font(.caption)
                        .foregroundColor(palette.textSecondary)
                    Spacer()
                    Button(state.t("settings.close")) { state.statusMessage = "" }
                        .buttonStyle(.plain)
                        .font(.caption.weight(.medium))
                        .foregroundColor(palette.textMuted)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(palette.elevatedBg)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(palette.strongBorder, lineWidth: 1))
                .cornerRadius(10)
            }
        }
        .padding(14)
        .background(palette.canvasBg)
        .foregroundColor(palette.textPrimary)
        .preferredColorScheme(themeId.preferredColorScheme)
        .sheet(isPresented: Binding(get: { pendingStudioKind != nil }, set: { if !$0 { pendingStudioKind = nil } })) {
            studioConfigSheet
        }
        .sheet(isPresented: $state.showSettingsSheet) {
            UnifiedSettingsSheet().environmentObject(state)
        }
        .sheet(isPresented: $state.showPrivacySheet) {
            PrivacyInfoSheet().environmentObject(state)
        }
        .sheet(isPresented: $state.showCreateNotebookSheet) {
            CreateNotebookSheet(palette: palette, preferredScheme: themeId.preferredColorScheme)
                .environmentObject(state)
        }
        .sheet(isPresented: $showAddUrlSheet) {
            addUrlSheet
        }
        .sheet(isPresented: $showPasteTextSheet) {
            pasteTextSheet
        }
        .sheet(item: $renamingSource) { src in
            renameSourceSheet(src)
        }
        .sheet(item: $renamingArtifact) { art in
            renameArtifactSheet(art)
        }
        .sheet(item: $editingNote) { note in
            editNoteSheet(note)
        }
    }

    // MARK: - Top Header Bar (synced with Windows MainWindow.xaml)

    private var topBar: some View {
        HStack(spacing: 12) {
            AppLogoView(size: 30, accent: palette.accent)

            // Click the title to rename in place (Return / focus loss commits, Esc reverts).
            TextField(state.t("nb.none_hint"), text: $titleDraft)
                .textFieldStyle(.plain)
                .font(.system(size: 18, weight: .semibold))
                .foregroundColor(palette.textPrimary)
                .lineLimit(1)
                .focused($titleFocused)
                .disabled(state.activeNotebookId.isEmpty)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .frame(minWidth: 120, maxWidth: 320)
                .fixedSize(horizontal: false, vertical: true)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(titleFocused ? palette.accent : (titleHovered ? palette.subtleBorder : Color.clear), lineWidth: 1)
                )
                .onHover { titleHovered = $0 }
                .help(state.t("nb.title_edit_hint"))
                .onAppear { titleDraft = activeNotebookTitle }
                .onChange(of: activeNotebookTitle) { _, new in if !titleFocused { titleDraft = new } }
                .onSubmit { titleFocused = false }
                .onExitCommand { titleDraft = activeNotebookTitle; titleFocused = false }
                .onChange(of: titleFocused) { _, focused in
                    if !focused {
                        let draft = titleDraft
                        if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { titleDraft = activeNotebookTitle }
                        else { Task { await state.renameActiveNotebook(draft) } }
                    }
                }

            // Notebook switcher styled consistently with "+ Add source" menu
            Menu {
                ForEach(state.notebooks) { nb in
                    Button {
                        state.activeNotebookId = nb.id
                        Task { await state.refreshAll() }
                    } label: {
                        if nb.id == state.activeNotebookId {
                            Label(nb.title, systemImage: "checkmark")
                        } else {
                            Text(nb.title)
                        }
                    }
                }
            } label: {
                HStack(spacing: 5) {
                    Text(activeNotebookTitle)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 10, weight: .semibold))
                }
                .foregroundColor(palette.textPrimary)
                .frame(height: 36)
                .padding(.horizontal, 12)
                .background(palette.elevatedBg)
                .overlay(Capsule().stroke(palette.subtleBorder, lineWidth: 1))
                .clipShape(Capsule())
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel(state.isEnglish ? "Switch notebook" : "切换笔记本")

            Spacer()

            PillButton(title: state.t("nb.new"), palette: palette) {
                state.showCreateNotebookSheet = true
            }

            PillButton(title: state.t("settings.title"), systemImage: "gearshape", palette: palette) {
                state.showSettingsSheet = true
            }

            Menu {
                Button(state.t("nb.rename")) {
                    titleFocused = true
                }
                Button(state.t("nb.export_lna")) { exportNotebook(format: "lna", ext: "lna") }
                Button(state.t("nb.export_md")) { exportNotebook(format: "markdown", ext: "md") }
                Button(state.t("nb.import_lna")) { importNotebook() }
                Divider()
                Button(state.t("nb.clear_chat")) {
                    Task {
                        _ = try? await state.core.call(method: "chat.clear", params: ["notebook_id": state.activeNotebookId])
                        await state.refreshAll()
                    }
                }
                Button(state.t("nb.delete"), role: .destructive) {
                    Task { await state.deleteActiveNotebook() }
                }
                Divider()
                Button(state.t("nb.export_diag")) { state.exportDiagnostics() }
                Button(state.t("nb.privacy")) { state.showPrivacySheet = true }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(palette.textPrimary)
                    .frame(width: 38, height: 38)
                    .background(palette.elevatedBg)
                    .overlay(Circle().stroke(palette.subtleBorder, lineWidth: 1))
                    .clipShape(Circle())
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel(state.t("nb.menu"))
        }
        .frame(height: 46)
        .padding(.horizontal, 4)
    }

    // MARK: - Left: Sources Pane

    private var sourcesPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(state.t("sources.title"))
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(palette.textPrimary)
                .padding(.leading, 2)

            HStack(spacing: 10) {
                Menu {
                    Button(state.t("sources.add_file")) { pickAndAddFile() }
                    Button(state.t("sources.add_url")) {
                        addUrlInput = ""
                        showAddUrlSheet = true
                    }
                    Button(state.t("sources.add_paste")) {
                        pasteTitleInput = ""
                        pasteBodyInput = ""
                        showPasteTextSheet = true
                    }
                } label: {
                    HStack(spacing: 5) {
                        Text(state.t("sources.add"))
                            .font(.system(size: 13, weight: .medium))
                            .lineLimit(1)
                        Image(systemName: "chevron.down")
                            .font(.system(size: 10, weight: .semibold))
                    }
                    .foregroundColor(palette.textPrimary)
                    .frame(maxWidth: .infinity)
                    .frame(height: 38)
                    .padding(.horizontal, 10)
                    .background(palette.elevatedBg)
                    .overlay(Capsule().stroke(palette.subtleBorder, lineWidth: 1))
                    .clipShape(Capsule())
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)

                PillButton(title: state.t("sources.create_note"), fillWidth: true, palette: palette) {
                    Task {
                        if let res = try? await state.core.call(method: "note.save", params: [
                            "notebook_id": state.activeNotebookId,
                            "title": state.isEnglish ? "Note" : "新笔记",
                            "body_md": ""
                        ]), let id = res["id"] as? String {
                            await state.refreshAll()
                            let title = state.isEnglish ? "Note" : "新笔记"
                            editNoteTitle = title
                            editNoteBody = ""
                            editingNote = NoteItem(id: id, title: title, bodyMd: "")
                        } else {
                            await state.refreshAll()
                        }
                    }
                }
            }

            if !state.sources.isEmpty {
                let allSelected = state.sources.allSatisfy { $0.isSelected }
                HStack {
                    Text(state.t("sources.select_all"))
                        .font(.system(size: 12))
                        .foregroundColor(palette.textMuted)
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { allSelected },
                        set: { newVal in
                            let ids = state.sources.map { $0.id }
                            Task {
                                for id in ids {
                                    _ = try? await state.core.call(method: "source.set_selected", params: ["source_id": id, "selected": newVal])
                                }
                                await state.refreshAll()
                            }
                        }
                    ))
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 2)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    if state.sources.isEmpty && state.notes.isEmpty {
                        Text(state.t("sources.empty_hint"))
                            .font(.system(size: 13))
                            .foregroundColor(palette.textMuted)
                            .padding(.horizontal, 4)
                            .padding(.vertical, 8)
                    } else {
                        ForEach(state.sources) { src in
                            sourceRow(src)
                        }

                        if !state.notes.isEmpty {
                            Divider().opacity(0.3).padding(.vertical, 6)
                            Text(state.t("sources.notes_header"))
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundColor(palette.textMuted)
                                .padding(.horizontal, 6)
                            ForEach(state.notes) { note in
                                Button {
                                    editNoteTitle = note.title
                                    editNoteBody = note.bodyMd
                                    editingNote = note
                                } label: {
                                    HStack(spacing: 10) {
                                        Image(systemName: "note.text")
                                            .font(.system(size: 14))
                                            .foregroundColor(Color(hex: 0xF5B942))
                                            .frame(width: 20)
                                        Text(note.title.isEmpty ? (state.isEnglish ? "Untitled Note" : "未命名笔记") : note.title)
                                            .font(.system(size: 13))
                                            .foregroundColor(palette.textPrimary)
                                            .lineLimit(1)
                                        Spacer(minLength: 4)
                                    }
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 8)
                                    .background(palette.elevatedBg.opacity(0.6))
                                    .cornerRadius(10)
                                }
                                .buttonStyle(.plain)
                                .contextMenu {
                                    Button(state.t("notes.convert_to_source")) {
                                        Task {
                                            _ = try? await state.core.call(method: "note.to_source", params: ["note_id": note.id])
                                            await state.refreshAll()
                                        }
                                    }
                                    Button(state.t("notes.delete"), role: .destructive) {
                                        Task {
                                            _ = try? await state.core.call(method: "note.delete", params: ["note_id": note.id])
                                            await state.refreshAll()
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(palette.panelBg)
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.subtleBorder, lineWidth: 1))
        .cornerRadius(16)
    }

    @ViewBuilder
    private func sourceRow(_ src: SourceItem) -> some View {
        let iconStyle = Color.sourceKindStyle(kind: src.kind, title: src.title)
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Image(systemName: iconStyle.symbol)
                    .font(.system(size: 15))
                    .foregroundColor(iconStyle.color)
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text(src.title)
                        .lineLimit(1)
                        .font(.system(size: 13))
                        .foregroundColor(palette.textPrimary)
                    if state.indexingSourceIds.contains(src.id) {
                        ProgressView().controlSize(.small)
                    } else if src.status != "READY" {
                        Text(sourceStatusLabel(src.status))
                            .font(.system(size: 11))
                            .foregroundColor(src.status == "FAILED" ? Color(hex: 0xFFA08C) : palette.textMuted)
                    }
                }

                Spacer(minLength: 4)

                Toggle("", isOn: Binding(
                    get: { src.isSelected },
                    set: { val in
                        Task {
                            _ = try? await state.core.call(method: "source.set_selected", params: ["source_id": src.id, "selected": val])
                            await state.refreshAll()
                        }
                    }
                ))
                .toggleStyle(.checkbox)
                .labelsHidden()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(hoveredSourceId == src.id ? palette.hoverBg : Color.clear)
            .cornerRadius(10)
            .onHover { hovering in
                withAnimation(.easeOut(duration: 0.12)) {
                    hoveredSourceId = hovering ? src.id : (hoveredSourceId == src.id ? nil : hoveredSourceId)
                }
            }
            .contextMenu {
                Button(state.t("sources.rename")) {
                    renameSourceInput = src.title
                    renamingSource = src
                }
                if src.status == "FAILED" {
                    Button(state.t("sources.retry")) {
                        Task {
                            _ = try? await state.core.call(method: "source.retry", params: ["source_id": src.id])
                            await state.refreshAll()
                        }
                    }
                }
                Button(state.t("sources.remove"), role: .destructive) {
                    Task {
                        _ = try? await state.core.call(method: "source.remove", params: ["source_id": src.id])
                        await state.refreshAll()
                    }
                }
            }

            if src.recommendKnowledgeGraph {
                HStack(spacing: 6) {
                    Text(state.t("kg.recommended"))
                        .font(.caption2)
                        .foregroundColor(palette.kgBannerText)
                    Spacer()
                    Button(state.t("kg.build_btn")) {
                        Task { await state.createKnowledgeGraph(sourceTitle: src.title) }
                    }
                    .buttonStyle(.plain)
                    .font(.caption2.weight(.semibold))
                    .foregroundColor(palette.accentText)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(palette.accent)
                    .clipShape(Capsule())
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(palette.kgBannerBg)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(palette.kgBannerBorder, lineWidth: 1))
                .cornerRadius(10)
            }
        }
    }

    private func sourceStatusLabel(_ status: String) -> String {
        switch status {
        case "QUEUED": return state.t("sources.status_queued")
        case "PARSING": return state.t("sources.status_parsing")
        case "CHUNKING": return state.t("sources.status_chunking")
        case "EMBEDDING": return state.t("sources.status_embedding")
        case "READY": return state.t("sources.status_ready")
        case "FAILED": return state.t("sources.status_failed")
        default: return status
        }
    }

    // MARK: - Center: Chat Pane

    private var chatPane: some View {
        VStack(spacing: 14) {
            VStack(alignment: .leading, spacing: 10) {
                MarkdownInlineText(text: state.summaryText, fontSize: state.prefs.fontSize, color: palette.textSecondary, accent: palette.accent)
                    .lineLimit(6)
                    .textSelection(.enabled)

                if state.prefs.showSuggestedQuestions && !state.suggestedQuestions.isEmpty {
                    WrapFlowLayout(spacing: 8) {
                        ForEach(state.suggestedQuestions.prefix(6), id: \.self) { q in
                            Button {
                                Task { await state.sendChat(q) }
                            } label: {
                                Text(q)
                                    .font(.system(size: 12.5))
                                    .foregroundColor(palette.textPrimary)
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 6)
                                    .background(palette.panelBg)
                                    .overlay(Capsule().stroke(palette.strongBorder, lineWidth: 1))
                                    .clipShape(Capsule())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(palette.elevatedBg)
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.subtleBorder, lineWidth: 1))
            .cornerRadius(12)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(state.messages) { msg in
                        ChatMessageRow(
                            message: msg,
                            defaultExpanded: state.prefs.expandThoughtsByDefault,
                            en: state.isEnglish,
                            fontSize: state.prefs.fontSize,
                            palette: palette,
                            onSaveToNote: {
                                Task {
                                    _ = try? await state.core.call(method: "note.from_message", params: ["message_id": msg.id])
                                    state.statusMessage = state.isEnglish ? "Saved response to Notes." : "已保存为笔记。"
                                    await state.refreshAll()
                                }
                            },
                            onRegenerate: {
                                Task { await state.regenerateChat() }
                            },
                            onFeedback: { val in
                                Task {
                                    _ = try? await state.core.call(method: "chat.set_feedback", params: ["message_id": msg.id, "feedback": val])
                                }
                            }
                        )
                    }
                }
            }

            // Windows-synced capsule composer (CornerRadius 26, MinHeight 52, 40x40 accent send / stop button)
            HStack(spacing: 10) {
                TextField(state.t("chat.placeholder"), text: $chatInput)
                    .textFieldStyle(.plain)
                    .font(.system(size: state.prefs.fontSize))
                    .foregroundColor(palette.textPrimary)
                    .onSubmit { submitChat() }

                if state.isGeneratingChat {
                    Button {
                        Task { await state.cancelChat() }
                    } label: {
                        Image(systemName: "stop.fill")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(palette.textPrimary)
                            .frame(width: 40, height: 40)
                            .background(palette.hoverBg)
                            .overlay(Circle().stroke(palette.strongBorder, lineWidth: 1))
                            .clipShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(state.t("chat.stop"))
                } else {
                    Button(action: submitChat) {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(palette.accentText)
                            .frame(width: 40, height: 40)
                            .background(palette.accent)
                            .clipShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Up")
                }
            }
            .padding(.leading, 16)
            .padding(.trailing, 8)
            .padding(.vertical, 6)
            .frame(minHeight: 52)
            .background(palette.elevatedBg)
            .overlay(RoundedRectangle(cornerRadius: 26).stroke(palette.strongBorder, lineWidth: 1))
            .cornerRadius(26)
        }
        .padding(18)
        .background(palette.panelBg)
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.subtleBorder, lineWidth: 1))
        .cornerRadius(16)
    }

    // MARK: - Right: Studio Pane

    private var studioPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let art = state.activeArtifact {
                ArtifactViewerView(artifact: art)
                    .environmentObject(state)
            } else {
                Text("Studio")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(palette.textPrimary)
                    .padding(.leading, 2)

                let tiles: [(String, String, String)] = [
                    ("audio", state.t("studio.audio"), "mic.fill"),
                    ("slides", state.t("studio.slides"), "rectangle.stack.fill"),
                    ("mindmap", state.t("studio.mindmap"), "point.3.connected.trianglepath.dotted"),
                    ("report", state.t("studio.report"), "doc.richtext.fill"),
                    ("flashcards", state.t("studio.flashcards"), "square.on.square"),
                    ("quiz", state.t("studio.quiz"), "checkmark.circle.fill"),
                    ("infographic", state.t("studio.infographic"), "chart.bar.xaxis"),
                    ("knowledge_graph", state.t("studio.knowledge_graph"), "circle.hexagongrid.fill"),
                ]

                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                    ForEach(tiles, id: \.0) { kind, label, icon in
                        Button {
                            studioPromptInput = ""
                            studioDetailLevel = "standard"
                            pendingStudioKind = kind
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: icon)
                                    .font(.system(size: 16))
                                    .foregroundColor(Color.studioKindColor(kind))
                                    .frame(width: 20)
                                Text(label)
                                    .font(.system(size: 12.5, weight: .medium))
                                    .foregroundColor(palette.textPrimary)
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 10)
                            .frame(minHeight: 54)
                            .background(hoveredTileKind == kind ? palette.hoverBg : palette.elevatedBg)
                            .overlay(RoundedRectangle(cornerRadius: 14).stroke(palette.subtleBorder, lineWidth: 1))
                            .cornerRadius(14)
                        }
                        .buttonStyle(.plain)
                        .onHover { hovering in
                            withAnimation(.easeOut(duration: 0.12)) {
                                hoveredTileKind = hovering ? kind : (hoveredTileKind == kind ? nil : hoveredTileKind)
                            }
                        }
                    }
                }

                ScrollView {
                    VStack(spacing: 10) {
                        if state.artifacts.isEmpty {
                            Text(state.t("studio.empty_hint"))
                                .font(.system(size: 13))
                                .foregroundColor(palette.textMuted)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 4)
                                .padding(.vertical, 6)
                        } else {
                            ForEach(state.artifacts) { art in
                                artifactCard(art)
                            }
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(palette.panelBg)
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.subtleBorder, lineWidth: 1))
        .cornerRadius(16)
    }

    @ViewBuilder
    private func artifactCard(_ art: StudioArtifactItem) -> some View {
        let iconName = studioSymbol(for: art.artifactType)
        Button {
            state.activeArtifact = art
        } label: {
            HStack(spacing: 10) {
                Image(systemName: iconName)
                    .font(.system(size: 16))
                    .foregroundColor(Color.studioKindColor(art.artifactType))
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text(art.title)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(palette.textPrimary)
                        .lineLimit(2)
                    Text("\(state.t("studio.\(art.artifactType)")) · \(art.status)")
                        .font(.system(size: 11))
                        .foregroundColor(palette.textMuted)
                }

                Spacer(minLength: 4)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 12)
            .frame(minHeight: 56)
            .background(hoveredArtifactId == art.id ? palette.hoverBg : palette.elevatedBg)
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.subtleBorder, lineWidth: 1))
            .cornerRadius(12)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            withAnimation(.easeOut(duration: 0.12)) {
                hoveredArtifactId = hovering ? art.id : (hoveredArtifactId == art.id ? nil : hoveredArtifactId)
            }
        }
        .contextMenu {
            Button(state.t("studio.rename")) {
                renameArtifactInput = art.title
                renamingArtifact = art
            }
            if art.status == "PENDING" || art.status == "RUNNING" {
                Button(state.t("studio.cancel")) {
                    Task {
                        _ = try? await state.core.call(method: "studio.cancel", params: ["artifact_id": art.id])
                        await state.refreshAll()
                    }
                }
            }
            if art.status == "FAILED" || art.status == "CANCELLED" {
                Button(state.t("studio.retry")) {
                    Task {
                        _ = try? await state.core.call(method: "studio.retry", params: ["artifact_id": art.id])
                        await state.refreshAll()
                    }
                }
            } else if art.status == "READY" {
                Button(state.t("studio.regenerate")) {
                    Task { await state.regenerateArtifact(art) }
                }
            }
            Divider()
            Button(state.t("studio.delete"), role: .destructive) {
                Task {
                    _ = try? await state.core.call(method: "studio.delete", params: ["artifact_id": art.id])
                    await state.refreshAll()
                }
            }
        }
    }

    private func studioSymbol(for kind: String) -> String {
        switch kind {
        case "audio": return "mic.fill"
        case "slides": return "rectangle.stack.fill"
        case "mindmap": return "point.3.connected.trianglepath.dotted"
        case "report": return "doc.richtext.fill"
        case "flashcards": return "square.on.square"
        case "quiz": return "checkmark.circle.fill"
        case "infographic": return "chart.bar.xaxis"
        case "knowledge_graph": return "circle.hexagongrid.fill"
        default: return "sparkles"
        }
    }

    // MARK: - Sheets

    private var studioConfigSheet: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(state.isEnglish ? "Configure Studio Artifact" : "生成配置")
                .font(.headline)
                .foregroundColor(palette.textPrimary)
            Text(state.isEnglish ? "Prompt / Focus & Scope (Optional)" : "生成提示与范围要求（可选）")
                .font(.subheadline)
                .foregroundColor(palette.textSecondary)
            TextEditor(text: $studioPromptInput)
                .frame(minHeight: 110, maxHeight: 180)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.strongBorder))
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
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    private var addUrlSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.t("sources.add_url"))
                .font(.headline)
            TextField("https://example.com/article", text: $addUrlInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(state.isEnglish ? "Cancel" : "取消") { showAddUrlSheet = false }
                Button(state.isEnglish ? "Add" : "添加") {
                    let url = addUrlInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    showAddUrlSheet = false
                    guard !url.isEmpty else { return }
                    Task {
                        _ = try? await state.core.call(method: "source.add_url", params: [
                            "notebook_id": state.activeNotebookId,
                            "url": url
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 420)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    private var pasteTextSheet: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(state.t("sources.add_paste"))
                .font(.headline)
            TextField(state.isEnglish ? "Title (optional)" : "标题（可选）", text: $pasteTitleInput)
                .textFieldStyle(.roundedBorder)
            TextEditor(text: $pasteBodyInput)
                .frame(minHeight: 140, maxHeight: 220)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.strongBorder))
            HStack {
                Spacer()
                Button(state.isEnglish ? "Cancel" : "取消") { showPasteTextSheet = false }
                Button(state.isEnglish ? "Add" : "添加") {
                    let body = pasteBodyInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    let title = pasteTitleInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    showPasteTextSheet = false
                    guard !body.isEmpty else { return }
                    Task {
                        _ = try? await state.core.call(method: "source.add_text", params: [
                            "notebook_id": state.activeNotebookId,
                            "title": title.isEmpty ? "pasted-note.txt" : title,
                            "text": body
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 460)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    private func renameSourceSheet(_ src: SourceItem) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.t("sources.rename"))
                .font(.headline)
            TextField(state.isEnglish ? "Source title" : "来源标题", text: $renameSourceInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(state.isEnglish ? "Cancel" : "取消") { renamingSource = nil }
                Button(state.t("settings.save")) {
                    let trimmed = renameSourceInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    renamingSource = nil
                    guard !trimmed.isEmpty else { return }
                    Task {
                        _ = try? await state.core.call(method: "source.rename", params: [
                            "source_id": src.id,
                            "title": trimmed
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 380)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    private func renameArtifactSheet(_ art: StudioArtifactItem) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.t("studio.rename"))
                .font(.headline)
            TextField(state.isEnglish ? "Artifact title" : "产物标题", text: $renameArtifactInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(state.isEnglish ? "Cancel" : "取消") { renamingArtifact = nil }
                Button(state.t("settings.save")) {
                    let trimmed = renameArtifactInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    renamingArtifact = nil
                    guard !trimmed.isEmpty else { return }
                    Task {
                        _ = try? await state.core.call(method: "studio.rename", params: [
                            "artifact_id": art.id,
                            "title": trimmed
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 380)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    private func editNoteSheet(_ note: NoteItem) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(state.t("sources.notes_header"))
                .font(.headline)
            TextField(state.isEnglish ? "Note title" : "笔记标题", text: $editNoteTitle)
                .textFieldStyle(.roundedBorder)
            TextEditor(text: $editNoteBody)
                .frame(minHeight: 200, maxHeight: 320)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.strongBorder))
            HStack(spacing: 8) {
                Button(state.t("notes.convert_to_source")) {
                    let nid = note.id
                    let t = editNoteTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                    let b = editNoteBody
                    editingNote = nil
                    Task {
                        _ = try? await state.core.call(method: "note.save", params: [
                            "notebook_id": state.activeNotebookId,
                            "note_id": nid,
                            "title": t.isEmpty ? "Note" : t,
                            "body_md": b
                        ])
                        _ = try? await state.core.call(method: "note.to_source", params: ["note_id": nid])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)

                Button(state.t("notes.delete"), role: .destructive) {
                    let nid = note.id
                    editingNote = nil
                    Task {
                        _ = try? await state.core.call(method: "note.delete", params: ["note_id": nid])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)

                Spacer()

                Button(state.isEnglish ? "Cancel" : "取消") { editingNote = nil }
                Button(state.t("settings.save")) {
                    let nid = note.id
                    let t = editNoteTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                    let b = editNoteBody
                    editingNote = nil
                    Task {
                        _ = try? await state.core.call(method: "note.save", params: [
                            "notebook_id": state.activeNotebookId,
                            "note_id": nid,
                            "title": t.isEmpty ? "Note" : t,
                            "body_md": b
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(palette.panelBg)
        .preferredColorScheme(themeId.preferredColorScheme)
    }

    // MARK: - Actions

    private func submitChat() {
        let txt = chatInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !txt.isEmpty else { return }
        chatInput = ""
        Task { await state.sendChat(txt) }
    }

    private func pickAndAddFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, !panel.urls.isEmpty {
            let paths = panel.urls.map(\.path)
            Task {
                _ = try? await state.core.call(method: "source.add_files", params: [
                    "notebook_id": state.activeNotebookId,
                    "paths": paths
                ])
                await state.refreshAll()
            }
        }
    }

    private func exportNotebook(format: String, ext: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(activeNotebookTitle).\(ext)"
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                do {
                    _ = try await state.core.call(method: "notebook.export", params: [
                        "notebook_id": state.activeNotebookId,
                        "format": format,
                        "out_path": url.path
                    ])
                    state.statusMessage = state.isEnglish ? "Exported: \(url.lastPathComponent)" : "已导出：\(url.lastPathComponent)"
                } catch {
                    state.statusMessage = error.localizedDescription
                }
            }
        }
    }

    private func importNotebook() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                do {
                    let res = try await state.core.call(method: "notebook.import", params: ["in_path": url.path])
                    if let newId = res["notebook_id"] as? String {
                        state.activeNotebookId = newId
                    }
                    state.statusMessage = state.isEnglish ? "Notebook imported." : "笔记本已导入。"
                    await state.refreshAll()
                } catch {
                    state.statusMessage = error.localizedDescription
                }
            }
        }
    }
}

// MARK: - Reusable Windows-Synced Pill Button

private struct PillButton: View {
    let title: String
    var systemImage: String? = nil
    var fillWidth: Bool = false
    let palette: AppThemePalette
    let action: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let systemImage {
                    Image(systemName: systemImage)
                        .font(.system(size: 13.5))
                }
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
            }
            .foregroundColor(palette.textPrimary)
            .frame(maxWidth: fillWidth ? .infinity : nil)
            .frame(height: 38)
            .padding(.horizontal, 14)
            .background(isHovered ? palette.hoverBg : palette.elevatedBg)
            .overlay(Capsule().stroke(palette.subtleBorder, lineWidth: 1))
            .clipShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            withAnimation(.easeOut(duration: 0.12)) {
                isHovered = hovering
            }
        }
    }
}

// MARK: - Chat Message Row

struct ChatMessageRow: View {
    let message: ChatMessageItem
    let defaultExpanded: Bool
    let en: Bool
    let fontSize: Double
    let palette: AppThemePalette
    var onSaveToNote: (() -> Void)? = nil
    var onRegenerate: (() -> Void)? = nil
    var onFeedback: ((Int) -> Void)? = nil
    @State private var expanded: Bool = false
    @State private var feedbackVal: Int = 0
    @State private var copied: Bool = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(message.role == "user" ? L10n.t("chat.you", en: en) : L10n.t("chat.assistant", en: en))
                .font(.caption.weight(.medium))
                .foregroundColor(palette.textMuted)

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
                    .foregroundColor(palette.textSecondary)
                }
                .buttonStyle(.plain)

                if expanded {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(message.thoughts) { step in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 8) {
                                    Image(systemName: step.isTool ? "doc.text.magnifyingglass" : "brain")
                                        .foregroundColor(palette.textMuted)
                                    Text(step.title)
                                        .font(.callout.weight(step.isTool ? .regular : .medium))
                                        .foregroundColor(palette.textPrimary)
                                }
                                if !step.isTool && !step.detail.isEmpty {
                                    HStack(spacing: 10) {
                                        Rectangle()
                                            .fill(palette.strongBorder)
                                            .frame(width: 1)
                                            .padding(.leading, 7)
                                        Text(step.detail)
                                            .font(.callout)
                                            .foregroundColor(palette.textSecondary)
                                    }
                                }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }

            if message.role == "user" {
                Text(message.content)
                    .textSelection(.enabled)
                    .font(.system(size: fontSize, weight: .regular))
                    .foregroundColor(palette.textPrimary)
                    .lineSpacing(6)
            } else {
                MarkdownView(
                    markdown: message.content,
                    fontSize: fontSize,
                    color: palette.textPrimary,
                    mutedColor: palette.textSecondary,
                    accent: palette.accent,
                    lineSpacing: 6
                )
                .textSelection(.enabled)
            }

            if message.role == "assistant" && !message.content.isEmpty {
                HStack(spacing: 10) {
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(message.content, forType: .string)
                        copied = true
                    } label: {
                        Label(copied ? (en ? "Copied" : "已复制") : L10n.t("chat.copy", en: en), systemImage: copied ? "checkmark" : "doc.on.doc")
                            .font(.caption)
                            .foregroundColor(palette.textMuted)
                    }
                    .buttonStyle(.plain)

                    if let onSaveToNote {
                        Button(action: onSaveToNote) {
                            Label(L10n.t("chat.save_note", en: en), systemImage: "note.text.badge.plus")
                                .font(.caption)
                                .foregroundColor(palette.textMuted)
                        }
                        .buttonStyle(.plain)
                    }

                    if let onRegenerate {
                        Button(action: onRegenerate) {
                            Label(L10n.t("chat.regenerate", en: en), systemImage: "arrow.clockwise")
                                .font(.caption)
                                .foregroundColor(palette.textMuted)
                        }
                        .buttonStyle(.plain)
                    }

                    if let onFeedback {
                        Button {
                            feedbackVal = 1
                            onFeedback(1)
                        } label: {
                            Image(systemName: feedbackVal == 1 ? "hand.thumbsup.fill" : "hand.thumbsup")
                                .font(.caption)
                                .foregroundColor(feedbackVal == 1 ? palette.accent : palette.textMuted)
                        }
                        .buttonStyle(.plain)

                        Button {
                            feedbackVal = -1
                            onFeedback(-1)
                        } label: {
                            Image(systemName: feedbackVal == -1 ? "hand.thumbsdown.fill" : "hand.thumbsdown")
                                .font(.caption)
                                .foregroundColor(feedbackVal == -1 ? Color(hex: 0xFFA08C) : palette.textMuted)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 2)
            }
        }
        .onAppear { expanded = defaultExpanded }
    }
}

// MARK: - Wrap Flow Layout

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
            if x + sz.width > maxW && x > 0 {
                x = 0
                y += lineH + spacing
                lineH = 0
            }
            lineH = max(lineH, sz.height)
            x += sz.width + spacing
            maxLineW = max(maxLineW, x)
        }
        return CGSize(width: min(maxW, maxLineW), height: y + lineH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var lineH: CGFloat = 0
        for sub in subviews {
            let sz = sub.sizeThatFits(.unspecified)
            if x + sz.width > bounds.maxX && x > bounds.minX {
                x = bounds.minX
                y += lineH + spacing
                lineH = 0
            }
            sub.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(sz))
            lineH = max(lineH, sz.height)
            x += sz.width + spacing
        }
    }
}
