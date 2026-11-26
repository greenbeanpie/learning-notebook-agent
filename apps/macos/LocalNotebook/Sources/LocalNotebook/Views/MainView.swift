import SwiftUI
import AppKit

struct MainView: View {
    @EnvironmentObject var state: AppState
    @Environment(\.colorScheme) private var systemColorScheme

    @State private var chatInput = ""
    // Chat follows the latest message until the user scrolls up (see chatScrollArea).
    @State private var chatFollowing = true
    @State private var chatHovered = false
    @State private var chatScrollMonitor: Any? = nil
    @StateObject private var chatAttach = AttachmentStore()
    /// Unsent chat text and attachments of the notebooks that are not open.
    @State private var chatDrafts: [String: (text: String, items: [AttachedItem])] = [:]
    @State private var hoveredSourceId: String? = nil
    @State private var hoveredTileKind: String? = nil
    @State private var hoveredArtifactId: String? = nil
    @State private var studioConfig: StudioConfigRequest? = nil

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
                // Every pane gets an explicit width, so their sum is the window width: a pane whose content
                // wants more room is clipped inside its own frame instead of pushing the Studio pane off-screen.
                let gap: CGFloat = 14
                let total = geo.size.width
                let srcW = sourcesWidth(total)
                let minChat: CGFloat = 300
                let studioWanted: CGFloat = state.studioFullscreen
                    ? total
                    : (state.activeArtifact != nil ? max(460, total * 0.48) : 340)
                let studioW: CGFloat = state.studioFullscreen
                    ? total
                    : min(studioWanted, max(320, total - srcW - gap * 2 - minChat))
                let chatW = max(0, total - srcW - studioW - gap * 2)
                HStack(spacing: gap) {
                    if !state.studioFullscreen {
                        sourcesPane
                            .frame(width: srcW)
                            .clipped()
                            .transition(.opacity.combined(with: .move(edge: .leading)))
                        if !state.sourceFullscreen {
                            chatPane
                                .frame(width: chatW)
                                .clipped()
                                .transition(.opacity)
                        }
                    }
                    if !state.sourceFullscreen {
                        studioPane
                            .frame(width: studioW)
                            .clipped()
                    }
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
        .sheet(item: $studioConfig) { req in
            StudioConfigSheet(
                request: req,
                onSubmit: { cfg, extra in
                    studioConfig = nil
                    Task {
                        if req.isUpdate, let art = state.artifacts.first(where: { $0.id == req.artifactId }) {
                            await state.regenerateArtifact(art, config: cfg, extra: extra)
                        } else {
                            await state.createStudio(kind: req.kind, config: cfg, extra: extra)
                        }
                    }
                },
                onCancel: { studioConfig = nil }
            )
            .environmentObject(state)
        }
        .onChange(of: state.activeNotebookId) { oldId, newId in
            swapChatDraft(from: oldId, to: newId)
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

    /// Width that fits the current title text (a flexible `.frame(maxWidth:)` would always grow to the maximum).
    private var titleFieldWidth: CGFloat {
        let font = NSFont.systemFont(ofSize: 18, weight: .semibold)
        let text = titleDraft.isEmpty ? state.t("nb.none_hint") : titleDraft
        let textWidth = (text as NSString).size(withAttributes: [.font: font]).width
        return min(360, max(120, ceil(textWidth) + 16 + 10)) // 8pt padding each side + caret room
    }

    private var topBar: some View {
        HStack(spacing: 12) {
            AppLogoView(size: 30, accent: palette.accent)

            // Click the title to rename in place (Return / focus loss commits, Esc reverts).
            // The field is as wide as its text, clamped to 120...360 pt.
            TextField(state.t("nb.none_hint"), text: $titleDraft)
                .textFieldStyle(.plain)
                .font(.system(size: 18, weight: .semibold))
                .foregroundColor(palette.textPrimary)
                .lineLimit(1)
                .focused($titleFocused)
                .disabled(state.activeNotebookId.isEmpty)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .frame(width: titleFieldWidth)
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
                Divider()
                Button {
                    state.showCreateNotebookSheet = true
                } label: {
                    Label(state.t("nb.new"), systemImage: "plus")
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
            .accessibilityLabel(L10n.t("nb.switch", en: state.isEnglish))

            Spacer()

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
                    guard Confirm.ask(title: state.t("nb.clear_chat"), message: state.t("chat.clear_confirm"),
                                      primary: state.t("chat.clear_btn"), cancel: state.t("common.cancel"), destructive: true) else { return }
                    Task {
                        _ = await state.run("chat.clear", ["notebook_id": state.activeNotebookId], what: state.t("nb.clear_chat"))
                        await state.refreshAll()
                    }
                }
                Button(state.t("nb.delete"), role: .destructive) {
                    guard Confirm.ask(title: state.t("nb.delete"), message: L10n.fmt("nb.delete_confirm", en: state.isEnglish, activeNotebookTitle),
                                      primary: state.t("common.delete"), cancel: state.t("common.cancel"), destructive: true) else { return }
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

    private func sourcesWidth(_ total: CGFloat) -> CGFloat {
        if state.sourceFullscreen { return total }
        if state.previewSource != nil { return max(420, total * 0.4) }
        return state.activeArtifact != nil ? 250 : 320
    }

    @ViewBuilder
    private var sourcesPane: some View {
        if let req = state.previewSource {
            SourcePreviewPane(request: req)
                .padding(14)
                .background(palette.panelBg)
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.subtleBorder, lineWidth: 1))
                .cornerRadius(16)
        } else {
            sourcesListPane
        }
    }

    private var sourcesListPane: some View {
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
                        if let res = await state.run("note.save", [
                            "notebook_id": state.activeNotebookId,
                            "title": L10n.t("main.note", en: state.isEnglish),
                            "body_md": ""
                        ]), let id = res["id"] as? String {
                            await state.refreshAll()
                            let title = L10n.t("main.note", en: state.isEnglish)
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
                            Task {
                                _ = await state.run("source.set_all_selected", ["notebook_id": state.activeNotebookId, "selected": newVal], what: state.t("sources.op_select"))
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
                                        Text(note.title.isEmpty ? (L10n.t("main.untitled_note", en: state.isEnglish)) : note.title)
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
                                            _ = await state.run("note.to_source", ["note_id": note.id])
                                            await state.refreshAll()
                                        }
                                    }
                                    Button(state.t("notes.delete"), role: .destructive) {
                                        guard Confirm.ask(title: state.t("note.delete_btn"), message: state.t("note.delete_confirm"),
                                                          primary: state.t("common.delete"), cancel: state.t("common.cancel"), destructive: true) else { return }
                                        Task {
                                            _ = await state.run("note.delete", ["note_id": note.id])
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
        // Sub-line: why it failed, or that only keyword search works, or the import stage.
        let sub: (text: String, color: Color)? = {
            if src.status == "FAILED" {
                let advice = L10n.advice(src.errorCode, en: state.isEnglish) ?? ""
                let detail = advice.isEmpty ? src.errorDetail : advice
                return (detail.isEmpty ? state.t("sources.status_failed") : L10n.fmt("sources.status_failed_detail", en: state.isEnglish, detail), Color(hex: 0xFFA08C))
            }
            if src.status != "READY" { return (sourceStatusLabel(src.status), palette.textMuted) }
            if src.errorCode == "EMBEDDING_INCOMPLETE" { return (state.t("sources.keyword_only"), Color(hex: 0xFFC878)) }
            return nil
        }()
        HStack(spacing: 8) {
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
                } else if let sub {
                    Text(sub.text).font(.system(size: 11)).foregroundColor(sub.color).lineLimit(1)
                }
            }
            .help(src.status != "READY"
                ? "\(src.title)\n\(sub?.text ?? "")\n\(state.t("sources.not_ready_hint"))"
                : (sub.map { "\(src.title)\n\($0.text)" } ?? "\(src.title) (\(state.t("sources.click_to_view")))"))

            Spacer(minLength: 4)

            Menu {
                // A source that is not fully parsed (index and guide included) can only be deleted
                // (or retried after a failed import).
                if src.status == "READY" {
                    Button(state.t("sources.rename")) {
                        renameSourceInput = src.title
                        renamingSource = src
                    }
                }
                if src.status == "FAILED" {
                    Button(state.t("sources.retry_import")) {
                        Task {
                            _ = await state.run("source.retry", ["source_id": src.id], what: state.t("sources.retry_import"))
                            await state.refreshAll()
                        }
                    }
                }
                Button(state.t("sources.delete"), role: .destructive) { deleteSource(src) }
            } label: {
                Image(systemName: "ellipsis").font(.system(size: 13)).foregroundColor(palette.textSecondary)
                    .frame(width: 28, height: 28).contentShape(Rectangle())
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help(state.t("sources.more_actions_tip"))
            .accessibilityLabel(L10n.fmt("sources.more_actions_for", en: state.isEnglish, src.title))

            Toggle("", isOn: Binding(
                get: { src.isSelected },
                set: { val in
                    Task {
                        _ = await state.run("source.set_selected", ["source_id": src.id, "selected": val], what: state.t("sources.op_select"))
                        await state.refreshAll()
                    }
                }
            ))
            .toggleStyle(.checkbox)
            .labelsHidden()
            .disabled(src.status != "READY")
            .help(state.t("sources.cite_source_tip"))
            .accessibilityLabel(L10n.fmt("sources.cite_source", en: state.isEnglish, src.title))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(minHeight: 44)
        .background(hoveredSourceId == src.id ? palette.hoverBg : Color.clear)
        .cornerRadius(10)
        .contentShape(Rectangle())
        .opacity(src.status == "READY" ? 1 : 0.7)
        .onTapGesture {
            guard src.status == "READY" else { return }
            state.openSourcePreview(SourcePreviewRequest(sourceId: src.id, title: src.title))
        }
        .onHover { hovering in
            guard src.status == "READY" else { return }
            withAnimation(.easeOut(duration: 0.12)) {
                hoveredSourceId = hovering ? src.id : (hoveredSourceId == src.id ? nil : hoveredSourceId)
            }
        }
    }

    /// The NSScrollView under a mouse event (SwiftUI's ScrollView is one underneath).
    private static func chatScrollView(at ev: NSEvent) -> NSScrollView? {
        var v = ev.window?.contentView?.hitTest(ev.locationInWindow)
        while let c = v {
            if let sv = c as? NSScrollView { return sv }
            v = c.superview
        }
        return nil
    }

    private static func isScroller(at ev: NSEvent) -> Bool {
        var v = ev.window?.contentView?.hitTest(ev.locationInWindow)
        while let c = v {
            if c is NSScroller { return true }
            v = c.superview
        }
        return false
    }

    private static func isAtBottom(_ sv: NSScrollView) -> Bool {
        guard let doc = sv.documentView else { return true }
        let visible = sv.contentView.bounds
        let gap = doc.isFlipped ? doc.bounds.maxY - visible.maxY : visible.minY - doc.bounds.minY
        return gap <= 36
    }

    private func deleteSource(_ src: SourceItem) {
        guard Confirm.ask(title: state.t("sources.delete"), message: L10n.fmt("sources.delete_confirm", en: state.isEnglish, src.title),
                          primary: state.t("common.delete"), cancel: state.t("common.cancel"), destructive: true) else { return }
        Task {
            _ = await state.run("source.remove", ["source_id": src.id], what: state.t("sources.delete"))
            if state.previewSource?.sourceId == src.id { state.closeSourcePreview() }
            await state.refreshAll()
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

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        if state.messages.isEmpty {
                            ChatEmptyHint(state: state, palette: palette)
                        }
                        ForEach(Array(state.messages.enumerated()), id: \.element.id) { idx, msg in
                            ChatMessageRow(
                                message: msg,
                                isLast: idx == state.messages.count - 1,
                                defaultExpanded: state.prefs.expandThoughtsByDefault,
                                en: state.isEnglish,
                                fontSize: state.prefs.fontSize,
                                palette: palette,
                                onPromoteAttachment: { id in
                                    Task { await state.promoteAttachment(id) }
                                },
                                onSaveToNote: {
                                    Task {
                                        if await state.run("note.from_message", ["message_id": msg.id], what: state.t("note.op_create")) != nil {
                                            state.statusMessage = "\(state.t("chat.saved_title"))\(L10n.t("common.colon", en: state.isEnglish))\(state.t("chat.saved_msg"))"
                                            await state.refreshAll()
                                        }
                                    }
                                },
                                onRegenerate: {
                                    Task { await state.regenerateChat() }
                                },
                                onFeedback: { val in
                                    Task { _ = await state.run("chat.set_feedback", ["message_id": msg.id, "feedback": val], what: state.t("chat.op_feedback")) }
                                },
                                onFollowup: { q in
                                    Task { await state.sendChat(q) }
                                },
                                onOpenSource: { req in state.openSourcePreview(req) }
                            )
                            .id(msg.id)
                        }
                        Color.clear.frame(height: 1).id("chat-bottom")
                            // back at the very bottom: following resumes
                            .onAppear { chatFollowing = true }
                    }
                }
                .onHover { chatHovered = $0 }
                .onAppear {
                    // Only the user's own scrolling decides whether the chat follows: a wheel /
                    // trackpad scroll up, grabbing the scroll bar, or a scrolling key (up arrow,
                    // Page Up, Home) over the chat stops following. Content growth never does;
                    // being back at the bottom resumes it.
                    if chatScrollMonitor == nil {
                        chatScrollMonitor = NSEvent.addLocalMonitorForEvents(
                            matching: [.scrollWheel, .leftMouseDown, .leftMouseUp, .keyDown]
                        ) { ev in
                            switch ev.type {
                            case .scrollWheel:
                                if chatHovered && ev.scrollingDeltaY > 0.5 { chatFollowing = false }
                            case .leftMouseDown:
                                if chatHovered, Self.isScroller(at: ev) { chatFollowing = false }
                            case .leftMouseUp:
                                // a scroll bar drag that ended at the bottom resumes following
                                if chatHovered, let sv = Self.chatScrollView(at: ev), Self.isAtBottom(sv) { chatFollowing = true }
                            case .keyDown:
                                let typing = NSApp.keyWindow?.firstResponder is NSTextView
                                // 126 up arrow, 116 Page Up, 115 Home
                                if chatHovered && !typing && [126, 116, 115].contains(Int(ev.keyCode)) {
                                    chatFollowing = false
                                }
                            default:
                                break
                            }
                            return ev
                        }
                    }
                }
                .onDisappear {
                    if let m = chatScrollMonitor { NSEvent.removeMonitor(m); chatScrollMonitor = nil }
                }
                .onChange(of: state.messages.count) { _, _ in
                    // a new message (sending, a reply starting, switching notebook) jumps to the end
                    chatFollowing = true
                    DispatchQueue.main.async { proxy.scrollTo("chat-bottom", anchor: .bottom) }
                }
                .onChange(of: state.messages.last?.content ?? "") { _, _ in
                    guard chatFollowing else { return }
                    // after the layout pass that placed the new text
                    DispatchQueue.main.async { proxy.scrollTo("chat-bottom", anchor: .bottom) }
                }
                .onChange(of: state.messages.last?.thoughts.count ?? 0) { _, _ in
                    guard chatFollowing else { return }
                    DispatchQueue.main.async { proxy.scrollTo("chat-bottom", anchor: .bottom) }
                }
            }

            AttachChips(store: chatAttach, en: state.isEnglish)

            // Windows-synced capsule composer (CornerRadius 26, MinHeight 52, 40x40 accent send / stop button)
            HStack(spacing: 10) {
                AttachMenuButton(store: chatAttach, en: state.isEnglish, tint: palette.textSecondary)
                TextField(state.t("chat.placeholder"), text: $chatInput)
                    .textFieldStyle(.plain)
                    .font(.system(size: state.prefs.fontSize))
                    .foregroundColor(palette.textPrimary)
                    .onSubmit { submitChat() }
                    .disabled(state.isGeneratingChat)

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
                    .accessibilityLabel(state.t("chat.send"))
                }
            }
            .padding(.leading, 16)
            .padding(.trailing, 8)
            .padding(.vertical, 6)
            .frame(minHeight: 52)
            .background(palette.elevatedBg)
            .overlay(RoundedRectangle(cornerRadius: 26).stroke(palette.strongBorder, lineWidth: 1))
            .cornerRadius(26)
            .attachDrop(chatAttach, en: state.isEnglish)
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
                Text(state.t("studio.title"))
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
                    ("lecture", state.t("studio.lecture"), "graduationcap.fill"),
                ]

                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                    ForEach(tiles, id: \.0) { kind, label, icon in
                        Button {
                            studioConfig = StudioConfigRequest(kind: kind)
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

    private func artifactStatusLabel(_ art: StudioArtifactItem) -> String {
        switch art.status {
        case "PENDING": return state.t("studio.status_pending")
        case "RUNNING":
            switch art.stage {
            case "COLLECT": return state.t("studio.status_collect")
            case "PLAN": return state.t("studio.status_plan")
            case "GENERATE": return state.t("studio.status_generate")
            case "RENDER": return state.t("studio.status_render")
            default: return state.t("studio.status_processing")
            }
        case "DRAFT_REVIEW": return state.t("studio.status_draft")
        case "READY": return state.t("studio.status_ready")
        case "FAILED": return state.t("studio.status_failed")
        case "CANCELLED": return state.t("studio.status_cancelled")
        default: return art.status
        }
    }

    /// "3 minutes ago" style age of a timestamp (unix ms).
    private func timeAgo(_ ms: Int64) -> String {
        guard ms > 0 else { return "" }
        let secs = max(0, Int64(Date().timeIntervalSince1970) - ms / 1000)
        if secs < 60 { return state.t("time.just_now") }
        let mins = secs / 60
        if mins < 60 { return L10n.fmt("time.minutes", en: state.isEnglish, mins) }
        let hours = mins / 60
        if hours < 24 { return L10n.fmt("time.hours", en: state.isEnglish, hours) }
        let days = hours / 24
        if days < 30 { return L10n.fmt("time.days", en: state.isEnglish, days) }
        if days < 365 { return L10n.fmt("time.months", en: state.isEnglish, days / 30) }
        return L10n.fmt("time.years", en: state.isEnglish, days / 365)
    }

    @ViewBuilder
    private func artifactCard(_ art: StudioArtifactItem) -> some View {
        let iconName = studioSymbol(for: art.artifactType)
        let running = art.status == "PENDING" || art.status == "RUNNING"
        let hint = running ? state.t("studio.click_thoughts") : (art.status == "FAILED" ? state.t("studio.click_error_log") : "")
        let age = timeAgo(art.createdAt)
        Button {
            state.activeArtifact = art
        } label: {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 10) {
                    Image(systemName: iconName)
                        .font(.system(size: 16))
                        .foregroundColor(Color.studioKindColor(art.artifactType))
                        .frame(width: 20)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(art.title.isEmpty ? state.t("studio.\(art.artifactType)") : art.title)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(palette.textPrimary)
                            .lineLimit(2)
                        Text("\(state.t("studio.\(art.artifactType)")) · \(artifactStatusLabel(art))\(age.isEmpty ? "" : " · " + age)\(hint)")
                            .font(.system(size: 11))
                            .foregroundColor(palette.textMuted)
                    }

                    Spacer(minLength: 4)
                }
                if running {
                    ProgressView(value: min(1, art.progress))
                    if let last = state.studioThoughts[art.id]?.last, !last.title.isEmpty {
                        Text(last.title).font(.system(size: 11)).foregroundColor(palette.textSecondary).lineLimit(2)
                    }
                }
                if art.status == "FAILED" {
                    let advice = L10n.advice(art.errorCode, en: state.isEnglish) ?? ""
                    Text(advice.isEmpty ? art.errorDetail : advice)
                        .font(.system(size: 12)).foregroundColor(Color(hex: 0xFFA08C)).lineLimit(3)
                }
                if art.status == "DRAFT_REVIEW" {
                    Text(state.t("studio.review_outline"))
                        .font(.system(size: 12)).foregroundColor(palette.textPrimary)
                        .padding(.horizontal, 14).padding(.vertical, 4)
                        .background(palette.hoverBg)
                        .overlay(Capsule().stroke(palette.strongBorder))
                        .clipShape(Capsule())
                }
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
                        _ = await state.run("studio.cancel", ["artifact_id": art.id])
                        await state.refreshAll()
                    }
                }
            }
            if art.status == "FAILED" || art.status == "CANCELLED" {
                Button(state.t("studio.retry")) {
                    Task {
                        _ = await state.run("studio.retry", ["artifact_id": art.id])
                        await state.refreshAll()
                    }
                }
            } else if art.status == "READY" {
                Button(state.t("studio.update_from_sources")) {
                    studioConfig = StudioConfigRequest(kind: art.artifactType, initial: art.config, isUpdate: true, artifactId: art.id)
                }
            }
            Divider()
            Button(state.t("common.delete"), role: .destructive) {
                guard Confirm.ask(title: state.t("studio.delete_title"), message: L10n.fmt("studio.delete_confirm", en: state.isEnglish, art.title),
                                  primary: state.t("common.delete"), cancel: state.t("common.cancel"), destructive: true) else { return }
                Task {
                    _ = await state.run("studio.delete", ["artifact_id": art.id])
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
        case "lecture": return "graduationcap.fill"
        case "infographic": return "chart.bar.xaxis"
        case "knowledge_graph": return "circle.hexagongrid.fill"
        default: return "sparkles"
        }
    }

    // MARK: - Sheets

    private var addUrlSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.t("sources.add_url_title"))
                .font(.headline)
            Text(state.t("sources.add_url_label")).font(.caption).foregroundColor(palette.textMuted)
            TextField("https://", text: $addUrlInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(L10n.t("common.cancel", en: state.isEnglish)) { showAddUrlSheet = false }
                Button(L10n.t("common.add", en: state.isEnglish)) {
                    let url = addUrlInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    showAddUrlSheet = false
                    guard !url.isEmpty else { return }
                    Task {
                        state.statusMessage = "\(state.t("sources.fetching_url"))\(L10n.t("common.colon", en: state.isEnglish))\(url)"
                        if await state.run("source.add_url", [
                            "notebook_id": state.activeNotebookId,
                            "url": url
                        ], what: state.t("sources.add_url_title")) != nil {
                            state.statusMessage = ""
                        }
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
            Text(state.t("sources.paste_dialog_title"))
                .font(.headline)
            TextField(state.t("common.title"), text: $pasteTitleInput)
                .textFieldStyle(.roundedBorder)
            TextEditor(text: $pasteBodyInput)
                .frame(minHeight: 140, maxHeight: 220)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(palette.strongBorder))
            HStack {
                Spacer()
                Button(L10n.t("common.cancel", en: state.isEnglish)) { showPasteTextSheet = false }
                Button(L10n.t("common.add", en: state.isEnglish)) {
                    let body = pasteBodyInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    let title = pasteTitleInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    showPasteTextSheet = false
                    guard !body.isEmpty else { return }
                    Task {
                        let res = await state.run("source.add_text", [
                            "notebook_id": state.activeNotebookId,
                            "title": title.isEmpty ? state.t("sources.paste_title_default") : title,
                            "text": body
                        ], what: state.t("sources.op_add_text"))
                        if res?["duplicate"] as? Bool == true {
                            state.statusMessage = "\(state.t("sources.dup_title"))\(L10n.t("common.colon", en: state.isEnglish))\(state.t("sources.dup_msg"))"
                        }
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
            TextField(L10n.t("main.source_title", en: state.isEnglish), text: $renameSourceInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(L10n.t("common.cancel", en: state.isEnglish)) { renamingSource = nil }
                Button(state.t("settings.save")) {
                    let trimmed = renameSourceInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    renamingSource = nil
                    guard !trimmed.isEmpty else { return }
                    Task {
                        _ = await state.run("source.rename", [
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
            TextField(L10n.t("main.artifact_title", en: state.isEnglish), text: $renameArtifactInput)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(L10n.t("common.cancel", en: state.isEnglish)) { renamingArtifact = nil }
                Button(state.t("settings.save")) {
                    let trimmed = renameArtifactInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    renamingArtifact = nil
                    guard !trimmed.isEmpty else { return }
                    Task {
                        _ = await state.run("studio.rename", [
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
            TextField(L10n.t("main.note_title", en: state.isEnglish), text: $editNoteTitle)
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
                        _ = await state.run("note.save", [
                            "notebook_id": state.activeNotebookId,
                            "note_id": nid,
                            "title": t.isEmpty ? "Note" : t,
                            "body_md": b
                        ])
                        _ = await state.run("note.to_source", ["note_id": nid])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)

                Button(state.t("notes.delete"), role: .destructive) {
                    guard Confirm.ask(title: state.t("note.delete_btn"), message: state.t("note.delete_confirm"),
                                      primary: state.t("common.delete"), cancel: state.t("common.cancel"), destructive: true) else { return }
                    let nid = note.id
                    editingNote = nil
                    Task {
                        _ = await state.run("note.delete", ["note_id": nid])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)

                Spacer()

                Button(L10n.t("common.cancel", en: state.isEnglish)) { editingNote = nil }
                Button(state.t("settings.save")) {
                    let nid = note.id
                    let t = editNoteTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                    let b = editNoteBody
                    editingNote = nil
                    Task {
                        _ = await state.run("note.save", [
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
        guard !txt.isEmpty || !chatAttach.isEmpty else { return }
        var extra: [String: Any] = [:]
        chatAttach.merge(into: &extra)
        let sentFrom = state.activeNotebookId
        Task {
            // the box is only cleared once the message is sent, so a failed send keeps its attachments
            if await state.sendChat(txt, extra: extra), state.activeNotebookId == sentFrom {
                chatInput = ""
                chatAttach.clear()
            }
        }
    }

    /// Keeps the unsent chat text and attachments per notebook.
    private func swapChatDraft(from oldId: String, to newId: String) {
        if !oldId.isEmpty {
            if chatInput.isEmpty && chatAttach.isEmpty {
                chatDrafts.removeValue(forKey: oldId)
            } else {
                chatDrafts[oldId] = (chatInput, chatAttach.items)
            }
        }
        if let d = chatDrafts.removeValue(forKey: newId) {
            chatInput = d.text
            chatAttach.restore(d.items)
        } else {
            chatInput = ""
            chatAttach.clear()
        }
    }

    private func pickAndAddFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, !panel.urls.isEmpty {
            let paths = panel.urls.map(\.path)
            Task {
                _ = await state.run("source.add_files", [
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
                    state.statusMessage = L10n.fmt("main.exported", en: state.isEnglish, url.lastPathComponent)
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
                    state.statusMessage = "\(state.t("nb.imported"))\(L10n.t("common.colon", en: state.isEnglish))\(state.t("nb.imported_msg"))"
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
    @EnvironmentObject var state: AppState
    let message: ChatMessageItem
    var isLast: Bool = false
    let defaultExpanded: Bool
    let en: Bool
    let fontSize: Double
    let palette: AppThemePalette
    var onPromoteAttachment: ((String) -> Void)? = nil
    var onSaveToNote: (() -> Void)? = nil
    var onRegenerate: (() -> Void)? = nil
    var onFeedback: ((Int) -> Void)? = nil
    var onFollowup: ((String) -> Void)? = nil
    var onOpenSource: ((SourcePreviewRequest) -> Void)? = nil
    @State private var expanded: Bool = false
    @State private var feedbackVal: Int = 0
    @State private var copied: Bool = false
    @State private var citation: CitationDetail? = nil

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
                        Text(L10n.t("studio.thoughts", en: en))
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

            if !message.attachments.isEmpty {
                MessageAttachments(attachments: message.attachments, en: en, onPromote: onPromoteAttachment)
            }

            if message.role == "user" {
                Text(message.content)
                    .textSelection(.enabled)
                    .font(.system(size: fontSize, weight: .regular))
                    .foregroundColor(palette.textPrimary)
                    .lineSpacing(6)
            } else {
                if message.isStreaming && message.content.isEmpty {
                    ProgressView().controlSize(.small)
                }
                MarkdownView(
                    markdown: message.content,
                    fontSize: fontSize,
                    color: palette.textPrimary,
                    mutedColor: palette.textSecondary,
                    accent: palette.accent,
                    lineSpacing: 6,
                    citations: Set(message.citations.keys)
                )
                .textSelection(.enabled)
                .environment(\.openURL, OpenURLAction { url in
                    guard url.scheme == "lnacite", let no = Int(url.host ?? "") else { return .systemAction }
                    Task { await showCitation(no) }
                    return .handled
                })
                .popover(item: $citation, arrowEdge: .bottom) { d in
                    CitationPopover(detail: d) { req in
                        citation = nil
                        onOpenSource?(req)
                    }
                    .environmentObject(state)
                }

                ForEach(Array(message.artifacts.enumerated()), id: \.offset) { _, art in
                    ChatArtifactCardView(raw: art)
                }

                if message.status == "INCOMPLETE" || message.status == "FAILED" || message.status == "CANCELLED" {
                    let why = message.status == "CANCELLED"
                        ? L10n.t("studio.cancelled", en: en)
                        : (L10n.advice(message.errorCode, en: en) ?? L10n.t("chat.incomplete", en: en))
                    Text(why).font(.callout).foregroundColor(Color(hex: 0xFFB478))
                }
            }

            if message.role == "assistant" && !message.content.isEmpty && !message.isStreaming {
                HStack(spacing: 10) {
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(message.content, forType: .string)
                        copied = true
                    } label: {
                        Label(copied ? (L10n.t("chat.copied_title", en: en)) : L10n.t("chat.copy", en: en), systemImage: copied ? "checkmark" : "doc.on.doc")
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

                    if isLast, let onRegenerate {
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

            if isLast, message.role == "assistant", !message.isStreaming, !message.followups.isEmpty {
                WrapFlowLayout(spacing: 8) {
                    ForEach(message.followups, id: \.self) { q in
                        Button { onFollowup?(q) } label: {
                            Text(q)
                                .font(.system(size: 13))
                                .foregroundColor(palette.textSecondary)
                                .multilineTextAlignment(.leading)
                                .padding(.horizontal, 14).padding(.vertical, 9)
                                .background(palette.panelBg)
                                .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.strongBorder, lineWidth: 1))
                                .cornerRadius(12)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 4)
            }
        }
        .onAppear { expanded = defaultExpanded; feedbackVal = message.feedback }
    }

    private func showCitation(_ no: Int) async {
        guard let res = await state.run("chat.citation_detail", ["message_id": message.id, "citation_no": no], what: state.t("cite.header")) else { return }
        let d = CitationDetail(no: no, res)
        // A figure-only or garbled PDF excerpt is useless: go straight to the page.
        if d.isUnclear, d.sourceTitle.lowercased().hasSuffix(".pdf"), !d.removed, d.page > 0,
           let src = state.sources.first(where: { $0.title == d.sourceTitle }) {
            onOpenSource?(SourcePreviewRequest(sourceId: src.id, page: d.page, focusText: nil, title: d.sourceTitle))
        }
        citation = d
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
