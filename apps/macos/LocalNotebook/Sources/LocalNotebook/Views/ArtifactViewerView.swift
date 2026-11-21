import SwiftUI

struct ArtifactViewerView: View {
    @EnvironmentObject var state: AppState
    let artifact: StudioArtifactItem
    @State private var infographicScale: CGFloat = 1.0
    @State private var draftText: String = ""
    @State private var editingDraft: Bool = false
    @State private var selectedSectionId: Int = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            // Header with Fullscreen toggle + Close button (Requirement 3)
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(artifact.title).font(.headline)
                    if artifact.sourceCount > 0 {
                        Text("基于 \(artifact.sourceCount) 个来源")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
                Spacer()
                Button {
                    state.studioFullscreen.toggle()
                } label: {
                    Image(systemName: state.studioFullscreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                }
                .buttonStyle(.plain)
                .help("全屏显示 / 还原")

                Button {
                    state.studioFullscreen = false
                    state.activeArtifact = nil
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.plain)
                .help("关闭详情")
            }

            Divider()

            if artifact.status == "DRAFT_REVIEW" {
                draftReviewBody
            } else if artifact.artifactType == "report" {
                reportBody
            } else if artifact.artifactType == "infographic" {
                infographicBody
            } else {
                ScrollView {
                    Text(prettyPayload(artifact.payload))
                        .font(.callout)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .onAppear { draftText = artifact.draftMarkdown }
    }

    // Requirement 5: Rendered Markdown outline preview & editor (NEVER raw JSON)
    private var draftReviewBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button("预览提纲") { editingDraft = false }
                    .buttonStyle(.borderedProminent)
                Button("编辑提纲") { editingDraft = true }
                    .buttonStyle(.bordered)
            }
            if editingDraft {
                TextEditor(text: $draftText)
                    .font(.body)
                    .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    Text(LocalizedStringKey(draftText.isEmpty ? "（暂无提纲内容）" : draftText))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                }
                .background(Color(white: 0.16))
                .cornerRadius(12)
            }
            Button("批准并生成演示文稿") {
                Task { await state.approveDraft(artifactId: artifact.id, markdown: draftText) }
            }
            .buttonStyle(.borderedProminent)
        }
    }

    // Requirement 4: Report TOC + Content split view
    private var reportBody: some View {
        let sections = artifact.payload["sections"] as? [[String: Any]] ?? []
        return HStack(alignment: .top, spacing: 12) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("目录").font(.caption.bold()).foregroundColor(.secondary)
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

    // Requirement 4: Infographic zoomable card canvas + bottom feedback bar
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

            HStack(spacing: 10) {
                Button {
                    Task { _ = try? await state.core.call(method: "studio.set_feedback", params: ["artifact_id": artifact.id, "feedback": 1]) }
                } label: {
                    Label("优质内容", systemImage: "hand.thumbsup")
                }
                .buttonStyle(.bordered)
                Button {
                    Task { _ = try? await state.core.call(method: "studio.set_feedback", params: ["artifact_id": artifact.id, "feedback": -1]) }
                } label: {
                    Label("劣质内容", systemImage: "hand.thumbsdown")
                }
                .buttonStyle(.bordered)
                Spacer()
            }
        }
    }

    private func prettyPayload(_ obj: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted]),
              let s = String(data: data, encoding: .utf8) else { return "" }
        return s
    }
}
