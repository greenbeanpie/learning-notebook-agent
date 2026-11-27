import SwiftUI
import AppKit

struct ArtifactViewerView: View {
    @EnvironmentObject var state: AppState
    let artifact: StudioArtifactItem
    @State private var draftText: String = ""
    @State private var revisePrompt: String = ""
    @StateObject private var reviseAttach = AttachmentStore()
    @State private var isRevisingDraft: Bool = false
    @State private var editingDraft: Bool = false
    @State private var showPromptPopover: Bool = false
    @State private var showDiagLog: Bool = false
    @State private var showThoughts: Bool = false
    @State private var showPreviewTab: Bool = false

    private var promptPillText: String {
        if artifact.sourceCount > 0 {
            return L10n.fmt("studio.view_prompt_sources", en: state.isEnglish, artifact.sourceCount)
        }
        return L10n.t("studio.view_prompt", en: state.isEnglish)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(artifact.title).font(.headline)
                    Button {
                        showPromptPopover.toggle()
                    } label: {
                        Text(promptPillText)
                            .font(.caption)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(Color.white.opacity(0.08))
                            .overlay(Capsule().stroke(Color.white.opacity(0.14), lineWidth: 1))
                            .clipShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .popover(isPresented: $showPromptPopover, arrowEdge: .bottom) {
                        promptAndSourcesPopover
                    }
                }
                Spacer()
                Button {
                    showThoughts.toggle()
                } label: {
                    Label(L10n.t("studio.thoughts", en: state.isEnglish), systemImage: "sparkles")
                        .font(.caption)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)

                if artifact.status == "READY" { exportMenu }

                Button {
                    if state.prefs.fullscreenAnimation {
                        withAnimation(.spring(response: 0.22, dampingFraction: 0.86)) {
                            state.studioFullscreen.toggle()
                        }
                    } else {
                        state.studioFullscreen.toggle()
                    }
                } label: {
                    Image(systemName: state.studioFullscreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                }
                .buttonStyle(.plain)
                .help(state.t("studio.fullscreen"))

                Button {
                    if state.prefs.fullscreenAnimation {
                        withAnimation(.easeOut(duration: 0.18)) {
                            state.studioFullscreen = false
                            state.activeArtifact = nil
                        }
                    } else {
                        state.studioFullscreen = false
                        state.activeArtifact = nil
                    }
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.plain)
                .help(state.t("studio.close"))
            }

            Divider()

            if artifact.status == "PENDING" || artifact.status == "RUNNING" {
                generatingBody
            } else if showThoughts {
                thoughtsWorkflowBody
            } else if artifact.status == "FAILED" {
                failedBody
            } else if artifact.status == "CANCELLED" {
                cancelledBody
            } else if artifact.status == "DRAFT_REVIEW" {
                draftReviewBody
            } else {
                ArtifactReadyBody(artifact: artifact)
            }
        }
        .onAppear { draftText = artifact.draftMarkdown; Task { await state.loadStudioThoughts(artifactId: artifact.id); await state.loadStudioPreview(artifactId: artifact.id) } }
    }

    private var promptAndSourcesPopover: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label(state.t("studio.prompt_header"), systemImage: "sparkles")
                    .font(.subheadline.bold())
                Spacer()
                Button {
                    showPromptPopover = false
                    Task { await state.regenerateArtifact(artifact) }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.plain)
                .help(state.t("studio.regenerate"))

                Button {
                    let txt = artifact.promptTopic.isEmpty ? artifact.title : artifact.promptTopic
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(txt, forType: .string)
                } label: {
                    Image(systemName: "doc.on.doc")
                }
                .buttonStyle(.plain)
                .help(state.t("studio.copy_prompt"))
            }

            Text(artifact.promptTopic.isEmpty ? state.t("studio.prompt_empty") : artifact.promptTopic)
                .font(.callout)
                .foregroundColor(artifact.promptTopic.isEmpty ? .secondary : .primary)
                .textSelection(.enabled)

            if !artifact.imagePaths.isEmpty || !artifact.fileNames.isEmpty {
                Label(state.t("studio.attachments_header"), systemImage: "photo")
                    .font(.caption.bold())
                    .foregroundColor(.secondary)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(artifact.imagePaths, id: \.self) { p in
                            if let img = NSImage(contentsOfFile: p) {
                                Image(nsImage: img)
                                    .resizable()
                                    .aspectRatio(contentMode: .fill)
                                    .frame(width: 96, height: 96)
                                    .clipShape(RoundedRectangle(cornerRadius: 8))
                                    .help((p as NSString).lastPathComponent)
                            }
                        }
                    }
                }
                ForEach(artifact.fileNames, id: \.self) { n in
                    Text("📄 " + n).font(.caption).lineLimit(1)
                }
            }

            Label(state.t("studio.sources_header"), systemImage: "folder")
                .font(.caption.bold())
                .foregroundColor(.secondary)

            if artifact.sourceRefs.isEmpty {
                Text(state.t("studio.sources_empty"))
                    .font(.caption)
                    .foregroundColor(.secondary)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 8)], alignment: .leading, spacing: 8) {
                    ForEach(artifact.sourceRefs) { src in
                        HStack(spacing: 6) {
                            Text(src.kind.uppercased().prefix(4))
                                .font(.system(size: 9, weight: .bold))
                                .padding(.horizontal, 4)
                                .padding(.vertical, 2)
                                .background(src.kind.lowercased() == "pdf" ? Color.red.opacity(0.85) : Color.blue.opacity(0.8))
                                .foregroundColor(.white)
                                .cornerRadius(4)
                            Text(src.title)
                                .font(.caption)
                                .lineLimit(1)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .background(Color.white.opacity(0.06))
                        .overlay(Capsule().stroke(Color.white.opacity(0.14), lineWidth: 1))
                        .clipShape(Capsule())
                    }
                }
            }
        }
        .padding(16)
        .frame(width: 380)
    }

    private var failedBody: some View {
        let msg = L10n.advice(artifact.errorCode, en: state.isEnglish) ?? state.t("studio.failed")
        let logText = artifact.errorLog.isEmpty
            ? "ArtifactId: \(artifact.id)\nErrorCode: \(artifact.errorCode)\nDetail: \(artifact.errorDetail)"
            : artifact.errorLog
        return ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text(msg)
                    .font(.callout.weight(.medium))
                    .foregroundColor(Color(red: 1.0, green: 0.63, blue: 0.55))

                Button(state.t("studio.retry")) {
                    Task { await state.retryArtifact(artifactId: artifact.id) }
                }
                .buttonStyle(.borderedProminent)

                DisclosureGroup(state.t("studio.diag_log"), isExpanded: $showDiagLog) {
                    VStack(alignment: .leading, spacing: 8) {
                        Button(state.t("studio.copy_log")) {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(logText, forType: .string)
                        }
                        .buttonStyle(.bordered)
                        ScrollView {
                            Text(logText)
                                .font(.system(size: 11, design: .monospaced))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: 220)
                        .padding(8)
                        .background(Color.black.opacity(0.25))
                        .cornerRadius(8)
                    }
                    .padding(.top, 6)
                }
            }
            .padding(14)
            .background(Color.white.opacity(0.05))
            .cornerRadius(12)
        }
    }

    private var draftReviewBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button(state.t("studio.preview_draft")) { editingDraft = false }
                    .buttonStyle(.borderedProminent)
                Button(state.t("studio.edit_draft")) { editingDraft = true }
                    .buttonStyle(.bordered)
            }
            if editingDraft {
                TextEditor(text: $draftText)
                    .font(.body)
                    .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    MarkdownView(markdown: draftText)
                        .textSelection(.enabled)
                        .padding(12)
                }
                .background(Color(white: 0.16))
                .cornerRadius(12)
            }
            AttachChips(store: reviseAttach, en: state.isEnglish)
            HStack(spacing: 8) {
                AttachMenuButton(store: reviseAttach, en: state.isEnglish)
                TextField(
                    L10n.t("art.enter_revision_prompt_to_refine", en: state.isEnglish),
                    text: $revisePrompt
                )
                .textFieldStyle(.roundedBorder)
                Button(isRevisingDraft ? (L10n.t("art.revising", en: state.isEnglish)) : (L10n.t("art.outline_revise_btn", en: state.isEnglish))) {
                    var instr = revisePrompt.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !instr.isEmpty || !reviseAttach.isEmpty else { return }
                    if instr.isEmpty { instr = L10n.t("ctx.revise_default", en: state.isEnglish) }
                    var extra: [String: Any] = [:]
                    reviseAttach.merge(into: &extra)
                    isRevisingDraft = true
                    Task {
                        if let updated = await state.reviseDraft(artifactId: artifact.id, instruction: instr, currentDraft: draftText, extra: extra) {
                            draftText = updated
                            revisePrompt = ""
                            reviseAttach.clear()
                        }
                        isRevisingDraft = false
                    }
                }
                .disabled(isRevisingDraft || (revisePrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && reviseAttach.isEmpty))
            }
            .attachDrop(reviseAttach, en: state.isEnglish)
            Button(state.t("studio.approve_draft")) {
                Task { await state.approveDraft(artifactId: artifact.id, markdown: draftText) }
            }
            .buttonStyle(.borderedProminent)
        }
    }

    private func stageLabel(_ stage: String) -> String {
        switch stage {
        case "COLLECT": return state.t("studio.status_collect")
        case "PLAN": return state.t("studio.status_plan")
        case "GENERATE": return state.t("studio.status_generate")
        case "RENDER": return state.t("studio.status_render")
        default: return state.t("studio.status_processing")
        }
    }

    private var runningBar: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(L10n.fmt("art.running_hint", en: state.isEnglish, state.t("studio.\(artifact.artifactType)"), stageLabel(artifact.stage)))
                    .font(.caption).foregroundColor(.secondary)
                Spacer()
                Button(state.t("studio.pause")) {
                    Task { _ = await state.run("studio.pause", ["artifact_id": artifact.id]); await state.refreshAll() }
                }
                .buttonStyle(.bordered).controlSize(.small)
                Button(state.t("art.cancel_gen")) {
                    Task { _ = await state.run("studio.cancel", ["artifact_id": artifact.id]); await state.refreshAll() }
                }
                .buttonStyle(.bordered).controlSize(.small)
            }
            if artifact.progress > 0.01 {
                ProgressView(value: min(1, artifact.progress))
            } else {
                ProgressView().progressViewStyle(.linear)
            }
        }
    }

    private var cancelledBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            let paused = artifact.stage == "PAUSED"
            Text(state.t(paused ? "studio.paused" : "studio.cancelled")).font(.callout).foregroundColor(.secondary)
            Button(state.t(paused ? "studio.resume" : "studio.regenerate")) { Task { await state.retryArtifact(artifactId: artifact.id) } }
                .buttonStyle(.bordered)
        }
    }

    private var exportMenu: some View {
        let formats: [(String, String, String)] = {
            switch artifact.artifactType {
            case "quiz": return [("Markdown", "markdown", "md"), ("JSON", "json", "json")]
            case "flashcards": return [("CSV", "csv", "csv"), ("Markdown", "markdown", "md")]
            case "mindmap": return [("Markdown", "markdown", "md"), ("SVG", "svg", "svg")]
            case "report": return [("Markdown", "markdown", "md"), ("HTML", "html", "html")]
            case "slides": return [("PPTX", "pptx", "pptx"), ("Markdown", "markdown", "md")]
            case "lecture": return [("PPTX", "pptx", "pptx"), ("Markdown", "markdown", "md"), ("JSON", "json", "json")]
            case "infographic": return [("SVG", "svg", "svg"), ("Markdown", "markdown", "md")]
            case "audio": return [("WAV", "wav", "wav"), (state.t("art.transcript"), "markdown", "md")]
            default: return [("Markdown", "markdown", "md")]
            }
        }()
        return Menu {
            ForEach(formats, id: \.1) { label, fmt, ext in
                Button(L10n.fmt("studio.export_as", en: state.isEnglish, label)) { export(format: fmt, ext: ext) }
            }
        } label: {
            Label(state.t("studio.export"), systemImage: "square.and.arrow.up").font(.caption)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }

    private func export(format: String, ext: String) {
        state.exportArtifact(artifact, format: format, ext: ext)
    }

    /// Generating screen: process view, plus an unvalidated preview tab once a preview exists.
    @ViewBuilder
    private var generatingBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            runningBar
            generatingTabs
        }
    }

    @ViewBuilder
    private var generatingTabs: some View {
        if let preview = state.studioPreviews[artifact.id] {
            VStack(alignment: .leading, spacing: 10) {
                Picker("", selection: $showPreviewTab) {
                    Text(state.t("art.tab_process")).tag(false)
                    Text(state.t("art.tab_preview")).tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(maxWidth: 220)
                if showPreviewTab {
                    Text(state.t("art.preview_unvalidated"))
                        .font(.caption)
                        .foregroundColor(.secondary)
                    ArtifactPreviewView(
                        stage: preview.stage,
                        artifactType: artifact.artifactType,
                        payload: preview.payload,
                        en: state.isEnglish
                    )
                } else {
                    thoughtsWorkflowBody
                }
            }
        } else {
            thoughtsWorkflowBody
        }
    }

    private var thoughtsWorkflowBody: some View {
        let steps = state.studioThoughts[artifact.id] ?? []
        let running = artifact.status == "PENDING" || artifact.status == "RUNNING"
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                if running { ProgressView().controlSize(.small) }
                Text(state.t("art.thoughts_header"))
                    .font(.subheadline.bold())
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if !running && !steps.contains(where: { $0.kind == "reasoning" }) {
                        Text(state.t("art.no_reasoning_note"))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                    if steps.isEmpty {
                        Text(L10n.t("art.waiting_for_ai_steps", en: state.isEnglish))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    } else {
                        ForEach(steps) { step in
                            ThoughtCardView(step: step, omittedText: state.t("art.reasoning_omitted"))
                        }
                    }
                }
            }
        }
    }
}

/// One card of the process view. Reasoning cards start collapsed and only show the last
/// `tailChars` characters of very long text until the user asks for all of it.
private struct ThoughtCardView: View {
    let step: ThoughtStepItem
    let omittedText: String
    @State private var expanded: Bool = false
    @State private var showFull: Bool = false

    private let tailChars = 8000

    private var collapsible: Bool { step.kind == "reasoning" }

    private var dotColor: Color {
        switch step.kind {
        case "tool": return Color.cyan
        case "reasoning": return Color.blue
        case "error": return Color.red
        default: return Color.green
        }
    }

    private var shownDetail: String {
        if showFull || step.detail.count <= tailChars { return step.detail }
        return String(step.detail.suffix(tailChars))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle()
                    .fill(dotColor)
                    .frame(width: 6, height: 6)
                Text(step.title.isEmpty ? step.kind : step.title)
                    .font(.caption.bold())
                if collapsible {
                    Text(expanded ? "▾" : "▸")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                if collapsible { expanded.toggle() }
            }
            if (!collapsible || expanded) && !step.detail.isEmpty {
                if !showFull && step.detail.count > tailChars {
                    Button(omittedText) { showFull = true }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
                Text(shownDetail)
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .textSelection(.enabled)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.05))
        .cornerRadius(8)
    }
}
