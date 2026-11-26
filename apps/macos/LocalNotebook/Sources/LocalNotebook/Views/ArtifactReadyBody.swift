import SwiftUI

/// The finished content of a Studio artifact. Shared by the Studio pane and the inline chat cards.
struct ArtifactReadyBody: View {
    @EnvironmentObject var state: AppState
    let artifact: StudioArtifactItem
    var compact: Bool = false
    var showFeedback: Bool = true

    @State private var infographicScale: CGFloat = 1.0
    @State private var selectedSectionId: Int = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            content
            if showFeedback && artifact.artifactType != "infographic" { feedbackBar }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch artifact.artifactType {
        case "mindmap":
            MindmapTreeView(
                payload: artifact.payload,
                en: state.isEnglish,
                artifactId: artifact.id,
                isKnowledgeGraph: artifact.config["mode"] as? String == "knowledge_graph",
                progressState: artifact.progressState,
                onSaveProgress: { prog in
                    Task { _ = await state.run("studio.save_progress", ["artifact_id": artifact.id, "state": prog]) }
                },
                onQuizConcept: { c in Task { await state.createConceptLink(kind: "quiz", concept: c) } },
                onExpandMindmap: { c in Task { await state.createConceptLink(kind: "mindmap", concept: c) } },
                onAskConcept: { c in Task { await state.sendChat(L10n.fmt("kg.explain_prompt", en: state.isEnglish, c)) } },
                onRefineNode: { nodeId, label in
                    Task { await state.refineNode(artifactId: artifact.id, nodeId: nodeId, label: label) }
                },
                onDownloadSvg: { state.exportArtifact(artifact, format: "svg", ext: "svg") }
            )
            .frame(minHeight: compact ? 320 : 0)
        case "slides":
            SlideDeckView(payload: artifact.payload, en: state.isEnglish)
                .frame(minHeight: compact ? 320 : 0)
        case "report":
            reportBody.frame(minHeight: compact ? 320 : 0)
        case "infographic":
            infographicBody
        case "quiz":
            QuizView(artifactId: artifact.id, notebookId: artifact.notebookId, payload: artifact.payload, compact: compact)
                .id(artifact.id)
                .frame(minHeight: compact ? 280 : 0)
        case "flashcards":
            FlashcardsView(payload: artifact.payload, compact: compact)
                .id(artifact.id)
                .frame(minHeight: compact ? 280 : 0)
        case "audio":
            AudioOverviewView(artifact: artifact)
                .id(artifact.id)
                .frame(minHeight: compact ? 220 : 0)
        case "lecture":
            LecturePlayerView(artifact: artifact, core: state.core, en: state.isEnglish)
                .id(artifact.id)
        default:
            ScrollView {
                Text(prettyPayload(artifact.payload))
                    .font(.callout)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private var feedbackBar: some View {
        HStack(spacing: 10) {
            Button {
                Task {
                    if await state.run("studio.set_feedback", ["artifact_id": artifact.id, "feedback": 1], what: state.t("chat.op_feedback")) != nil {
                        state.statusMessage = state.t("studio.feedback_received")
                    }
                }
            } label: {
                Label(state.t("studio.good"), systemImage: "hand.thumbsup")
            }
            .buttonStyle(.bordered)
            Button {
                Task {
                    if await state.run("studio.set_feedback", ["artifact_id": artifact.id, "feedback": -1], what: state.t("chat.op_feedback")) != nil {
                        state.statusMessage = state.t("studio.feedback_received")
                    }
                }
            } label: {
                Label(state.t("studio.bad"), systemImage: "hand.thumbsdown")
            }
            .buttonStyle(.bordered)
            Spacer()
        }
    }

    private var reportBody: some View {
        let sections = artifact.payload["sections"] as? [[String: Any]] ?? []
        return HStack(alignment: .top, spacing: 12) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(state.t("report.toc")) · \(sections.count)").font(.caption.bold()).foregroundColor(.secondary)
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
            .frame(width: compact ? 130 : 180)
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
                                ReportSectionView(section: sec, en: state.isEnglish)
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
                                    MarkdownInlineText(text: blk["summary"] as? String ?? "", fontSize: 11, color: .black.opacity(0.8))
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
            if showFeedback { feedbackBar }
        }
        .frame(minHeight: compact ? 320 : 0)
    }

    private func prettyPayload(_ obj: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted]),
              let s = String(data: data, encoding: .utf8) else { return "" }
        return s
    }
}
