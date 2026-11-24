import SwiftUI
import AppKit

struct ArtifactViewerView: View {
    @EnvironmentObject var state: AppState
    let artifact: StudioArtifactItem
    @State private var infographicScale: CGFloat = 1.0
    @State private var draftText: String = ""
    @State private var revisePrompt: String = ""
    @State private var isRevisingDraft: Bool = false
    @State private var editingDraft: Bool = false
    @State private var selectedSectionId: Int = 0
    @State private var showPromptPopover: Bool = false
    @State private var showDiagLog: Bool = false
    @State private var showThoughts: Bool = false

    private var promptPillText: String {
        if artifact.sourceCount > 0 {
            return state.isEnglish
                ? "View prompt and \(artifact.sourceCount) sources"
                : "查看提示和 \(artifact.sourceCount) 个来源"
        }
        return state.isEnglish ? "View prompt" : "查看提示"
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
                    Label(state.isEnglish ? "Thoughts" : "思考过程", systemImage: "sparkles")
                        .font(.caption)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)

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

            if artifact.status == "PENDING" || artifact.status == "RUNNING" || showThoughts {
                thoughtsWorkflowBody
            } else if artifact.status == "FAILED" {
                failedBody
            } else if artifact.status == "DRAFT_REVIEW" {
                draftReviewBody
            } else if artifact.artifactType == "mindmap" {
                MindmapTreeView(
                    payload: artifact.payload,
                    en: state.isEnglish,
                    onQuizConcept: { c in Task { await state.createConceptLink(kind: "quiz", concept: c) } },
                    onExpandMindmap: { c in Task { await state.createConceptLink(kind: "mindmap", concept: c) } },
                    onAskConcept: { c in Task { await state.sendChat(c) } }
                )
                feedbackBar
            } else if artifact.artifactType == "slides" {
                SlideDeckView(payload: artifact.payload, en: state.isEnglish)
                feedbackBar
            } else if artifact.artifactType == "report" {
                reportBody
                feedbackBar
            } else if artifact.artifactType == "infographic" {
                infographicBody
            } else {
                ScrollView {
                    Text(prettyPayload(artifact.payload))
                        .font(.callout)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                feedbackBar
            }
        }
        .onAppear { draftText = artifact.draftMarkdown; Task { await state.loadStudioThoughts(artifactId: artifact.id) } }
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

    private var feedbackBar: some View {
        HStack(spacing: 10) {
            Button {
                Task { _ = try? await state.core.call(method: "studio.set_feedback", params: ["artifact_id": artifact.id, "feedback": 1]) }
            } label: {
                Label(state.t("studio.good"), systemImage: "hand.thumbsup")
            }
            .buttonStyle(.bordered)
            Button {
                Task { _ = try? await state.core.call(method: "studio.set_feedback", params: ["artifact_id": artifact.id, "feedback": -1]) }
            } label: {
                Label(state.t("studio.bad"), systemImage: "hand.thumbsdown")
            }
            .buttonStyle(.bordered)
            Spacer()
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
                    Text(LocalizedStringKey(draftText))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                }
                .background(Color(white: 0.16))
                .cornerRadius(12)
            }
            HStack(spacing: 8) {
                TextField(
                    state.isEnglish ? "Enter revision prompt to refine outline..." : "输入调整要求（如：拆分第3部分、补充案例），可反复调整…",
                    text: $revisePrompt
                )
                .textFieldStyle(.roundedBorder)
                Button(isRevisingDraft ? (state.isEnglish ? "Revising..." : "调整中…") : (state.isEnglish ? "Revise with AI" : "AI 调整提纲")) {
                    let instr = revisePrompt.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !instr.isEmpty else { return }
                    isRevisingDraft = true
                    Task {
                        if let updated = await state.reviseDraft(artifactId: artifact.id, instruction: instr, currentDraft: draftText) {
                            draftText = updated
                            revisePrompt = ""
                        }
                        isRevisingDraft = false
                    }
                }
                .disabled(isRevisingDraft || revisePrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            Button(state.t("studio.approve_draft")) {
                Task { await state.approveDraft(artifactId: artifact.id, markdown: draftText) }
            }
            .buttonStyle(.borderedProminent)
        }
    }

    private var reportBody: some View {
        let sections = artifact.payload["sections"] as? [[String: Any]] ?? []
        return HStack(alignment: .top, spacing: 12) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text(state.t("studio.toc")).font(.caption.bold()).foregroundColor(.secondary)
                    ForEach(Array(sections.enumerated()), id: \.offset) { idx, sec in
                        Button {
                            selectedSectionId = idx
                        } label: {
                            Text("\(idx + 1). \(sec["heading"] as? String ?? "")")
                                .font(.caption)
                                .multilineTextAlignment(.leading)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(6)
                                .background(selectedSectionId == idx ? Color.white.opacity(0.1) : Color.clear)
                                .cornerRadius(6)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(8)
            }
            .frame(width: 180)
            .background(Color(white: 0.16))
            .cornerRadius(10)

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(Array(sections.enumerated()), id: \.offset) { idx, sec in
                            VStack(alignment: .leading, spacing: 6) {
                                Text(sec["heading"] as? String ?? "")
                                    .font(.title3.bold())
                                    .id(idx)
                                Text(LocalizedStringKey(sec["body_md"] as? String ?? ""))
                                    .textSelection(.enabled)
                            }
                        }
                    }
                }
                .onChange(of: selectedSectionId) { _, target in
                    withAnimation { proxy.scrollTo(target, anchor: .top) }
                }
            }
        }
    }

    private var infographicBody: some View {
        let blocks = artifact.payload["blocks"] as? [[String: Any]] ?? []
        return VStack(spacing: 8) {
            ZStack(alignment: .bottomTrailing) {
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(artifact.payload["title"] as? String ?? artifact.title)
                            .font(.title2.bold())
                            .foregroundColor(Color(red: 23/255, green: 58/255, blue: 122/255))
                        Text(artifact.payload["subtitle"] as? String ?? "")
                            .font(.callout)
                            .foregroundColor(.gray)
                        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
                            ForEach(Array(blocks.enumerated()), id: \.offset) { _, blk in
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(blk["heading"] as? String ?? "").font(.headline).foregroundColor(Color(red: 23/255, green: 58/255, blue: 122/255))
                                    Text(blk["metric"] as? String ?? "").font(.title.bold()).foregroundColor(.blue)
                                    Text(blk["summary"] as? String ?? "").font(.caption).foregroundColor(.black.opacity(0.8))
                                }
                                .padding(12)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Color.white)
                                .cornerRadius(12)
                            }
                        }
                    }
                    .padding(18)
                    .background(Color(red: 245/255, green: 248/255, blue: 255/255))
                    .cornerRadius(14)
                    .scaleEffect(infographicScale, anchor: .topLeading)
                }

                VStack(spacing: 4) {
                    Button("+") { infographicScale = min(2.4, infographicScale + 0.15) }
                    Button("-") { infographicScale = max(0.5, infographicScale - 0.15) }
                }
                .padding(6)
                .background(Color(white: 0.16))
                .cornerRadius(14)
                .padding(12)
            }

            feedbackBar
        }
    }

    private func prettyPayload(_ obj: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted]),
              let s = String(data: data, encoding: .utf8) else { return "" }
        return s
    }

    private var thoughtsWorkflowBody: some View {
        let steps = state.studioThoughts[artifact.id] ?? []
        return VStack(alignment: .leading, spacing: 10) {
            if artifact.status == "PENDING" || artifact.status == "RUNNING" {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(state.isEnglish ? "AI workflow in progress..." : "AI 正在执行工具调用与思考…")
                        .font(.subheadline.bold())
                }
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if steps.isEmpty {
                        Text(state.isEnglish ? "Waiting for AI steps..." : "正在等待 AI 工作步骤与思考记录…")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    } else {
                        ForEach(steps) { step in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 6) {
                                    Circle()
                                        .fill(step.kind == "tool" ? Color.cyan : (step.kind == "reasoning" ? Color.blue : Color.green))
                                        .frame(width: 6, height: 6)
                                    Text(step.title)
                                        .font(.caption.bold())
                                }
                                if !step.detail.isEmpty {
                                    Text(step.detail)
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
                }
            }
        }
    }
}
