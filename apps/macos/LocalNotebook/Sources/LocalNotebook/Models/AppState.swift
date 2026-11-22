import Foundation
import SwiftUI

struct NotebookItem: Identifiable, Hashable {
    let id: String
    let title: String
    let summaryMd: String
    let suggestedQuestions: [String]
}

struct SourceItem: Identifiable {
    let id: String
    let title: String
    let kind: String
    let status: String
    var isSelected: Bool
    let recommendKnowledgeGraph: Bool
}

struct ArtifactSourceRefItem: Identifiable {
    let id: String
    let title: String
    let kind: String
}

struct ThoughtStepItem: Identifiable {
    let id: String
    let kind: String
    let title: String
    let detail: String
    var isTool: Bool {
        ["retrieve", "retrieve_fallback", "tool_search", "tool_read_context", "tool_list_sources", "tool_call"].contains(kind)
    }
}

struct ChatMessageItem: Identifiable {
    let id: String
    let role: String
    var content: String
    var thoughts: [ThoughtStepItem]
}

struct StudioArtifactItem: Identifiable {
    let id: String
    let artifactType: String
    let title: String
    let status: String
    let sourceCount: Int
    let sourceRefs: [ArtifactSourceRefItem]
    let promptTopic: String
    let config: [String: Any]
    let draftMarkdown: String
    let payload: [String: Any]
    let errorCode: String
    let errorDetail: String
    let errorLog: String
}

@MainActor
final class AppState: ObservableObject {
    let core = CoreClient()

    @Published var prefs: UiPreferences = UiPreferences()
    @Published var notebooks: [NotebookItem] = []
    @Published var activeNotebookId: String = ""
    @Published var summaryText: String = ""
    @Published var suggestedQuestions: [String] = []
    @Published var sources: [SourceItem] = []
    @Published var messages: [ChatMessageItem] = []
    @Published var artifacts: [StudioArtifactItem] = []
    @Published var activeArtifact: StudioArtifactItem?
    @Published var studioFullscreen: Bool = false
    @Published var statusMessage: String = ""
    @Published var showSettingsSheet: Bool = false

    var isEnglish: Bool { prefs.isEnglish }
    func t(_ key: String) -> String { L10n.t(key, en: isEnglish) }

    func start() {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = appSupport.appendingPathComponent("LocalNotebook", isDirectory: true).path
        do {
            try core.open(dataDir: dir)
            core.onEvent = { [weak self] ev in
                Task { @MainActor in
                    if ev.event == "source.status" || ev.event == "studio.status" || ev.event == "chat.finished" {
                        await self?.refreshAll()
                    }
                }
            }
            Task {
                await loadPreferences()
                await loadInitialNotebook()
            }
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func loadPreferences() async {
        if let res = try? await core.call(method: "settings.get", params: ["key": "ui.preferences"]),
           let val = res["value"] as? [String: Any] {
            prefs = UiPreferences.from(val)
        }
        if summaryText.isEmpty {
            summaryText = t("chat.empty_summary")
        }
    }

    func savePreferences() async {
        _ = try? await core.call(method: "settings.set", params: [
            "key": "ui.preferences",
            "value": prefs.toDict()
        ])
        if !activeNotebookId.isEmpty {
            _ = try? await core.call(method: "i18n.translate_notebook_content", params: [
                "notebook_id": activeNotebookId,
                "target_lang": isEnglish ? "en-US" : "zh-CN"
            ])
            await refreshAll()
        }
        statusMessage = t("settings.saved")
    }

    func loadInitialNotebook() async {
        do {
            let res = try await core.call(method: "notebook.list")
            let rawList = res["notebooks"] as? [[String: Any]] ?? []
            if let first = rawList.first, let id = first["id"] as? String {
                activeNotebookId = id
            } else {
                let created = try await core.call(method: "notebook.create", params: ["title": t("nb.untitled")])
                activeNotebookId = created["id"] as? String ?? ""
            }
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func refreshAll() async {
        guard !activeNotebookId.isEmpty else { return }
        do {
            let nbList = try await core.call(method: "notebook.list")
            notebooks = (nbList["notebooks"] as? [[String: Any]] ?? []).map {
                NotebookItem(
                    id: $0["id"] as? String ?? "",
                    title: $0["title"] as? String ?? "",
                    summaryMd: $0["summary_md"] as? String ?? "",
                    suggestedQuestions: $0["suggested_questions"] as? [String] ?? []
                )
            }
            if let cur = notebooks.first(where: { $0.id == activeNotebookId }) {
                summaryText = cur.summaryMd.isEmpty ? t("chat.empty_summary") : cur.summaryMd
                suggestedQuestions = cur.suggestedQuestions
            }

            let srcRes = try await core.call(method: "source.list", params: ["notebook_id": activeNotebookId])
            sources = (srcRes["sources"] as? [[String: Any]] ?? []).map {
                SourceItem(
                    id: $0["id"] as? String ?? "",
                    title: $0["title"] as? String ?? "",
                    kind: $0["kind"] as? String ?? "txt",
                    status: $0["status"] as? String ?? "READY",
                    isSelected: $0["is_selected"] as? Bool ?? true,
                    recommendKnowledgeGraph: $0["recommend_knowledge_graph"] as? Bool ?? false
                )
            }

            let chatRes = try await core.call(method: "chat.history", params: ["notebook_id": activeNotebookId])
            messages = (chatRes["messages"] as? [[String: Any]] ?? []).map { m in
                let rawTh = m["thoughts"] as? [[String: Any]] ?? []
                let ths = rawTh.compactMap { th -> ThoughtStepItem? in
                    let kind = th["kind"] as? String ?? "thought_stage"
                    if kind == "retry" { return nil }
                    return ThoughtStepItem(
                        id: th["step_id"] as? String ?? UUID().uuidString,
                        kind: kind,
                        title: th["title"] as? String ?? "",
                        detail: th["detail"] as? String ?? ""
                    )
                }
                return ChatMessageItem(
                    id: m["id"] as? String ?? UUID().uuidString,
                    role: m["role"] as? String ?? "assistant",
                    content: m["content"] as? String ?? "",
                    thoughts: ths
                )
            }

            let artRes = try await core.call(method: "studio.list", params: ["notebook_id": activeNotebookId])
            artifacts = (artRes["artifacts"] as? [[String: Any]] ?? []).reversed().map { a in
                let cfg = a["config"] as? [String: Any] ?? [:]
                let rawRefs = a["source_refs"] as? [[String: Any]] ?? []
                let refs = rawRefs.map {
                    ArtifactSourceRefItem(
                        id: $0["id"] as? String ?? UUID().uuidString,
                        title: $0["title"] as? String ?? "",
                        kind: $0["kind"] as? String ?? "file"
                    )
                }
                let srcCount = refs.isEmpty ? ((a["source_ids"] as? [Any])?.count ?? 0) : refs.count
                return StudioArtifactItem(
                    id: a["id"] as? String ?? "",
                    artifactType: a["artifact_type"] as? String ?? "report",
                    title: a["title"] as? String ?? "Artifact",
                    status: a["status"] as? String ?? "PENDING",
                    sourceCount: srcCount,
                    sourceRefs: refs,
                    promptTopic: (cfg["topic"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines),
                    config: cfg,
                    draftMarkdown: a["draft"] as? String ?? "",
                    payload: a["payload"] as? [String: Any] ?? [:],
                    errorCode: a["error_code"] as? String ?? "",
                    errorDetail: a["error_detail"] as? String ?? "",
                    errorLog: a["error_log"] as? String ?? ""
                )
            }
            if let openId = activeArtifact?.id, let updated = artifacts.first(where: { $0.id == openId }) {
                activeArtifact = updated
            }
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func sendChat(_ text: String) async {
        guard !activeNotebookId.isEmpty, !text.isEmpty else { return }
        do {
            _ = try await core.call(method: "chat.send", params: ["notebook_id": activeNotebookId, "text": text])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func createStudio(kind: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let actualType = kind == "knowledge_graph" ? "mindmap" : kind
        var cfg: [String: Any] = kind == "slides"
            ? ["pages": 8, "reviewOutline": true]
            : (kind == "knowledge_graph" ? ["mode": "knowledge_graph"] : [:])
        if !prefs.defaultStudioLang.isEmpty {
            cfg["lang"] = prefs.defaultStudioLang
        }
        do {
            _ = try await core.call(method: "studio.create", params: ["notebook_id": activeNotebookId, "artifact_type": actualType, "config": cfg])
            statusMessage = t("studio.submitted")
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func createKnowledgeGraph(sourceTitle: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let title = isEnglish ? "\(sourceTitle) - Knowledge Graph" : "\(sourceTitle) · 知识点图谱"
        do {
            _ = try await core.call(method: "studio.create", params: [
                "notebook_id": activeNotebookId,
                "artifact_type": "mindmap",
                "title": title,
                "config": ["mode": "knowledge_graph", "topic": sourceTitle]
            ])
            statusMessage = t("studio.submitted")
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func createConceptLink(kind: String, concept: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let cfg: [String: Any] = kind == "quiz"
            ? ["topic": concept, "count": 5, "difficulty": "mixed"]
            : ["topic": concept]
        do {
            _ = try await core.call(method: "studio.create", params: [
                "notebook_id": activeNotebookId,
                "artifact_type": kind,
                "title": kind == "quiz" ? "Quiz: \(concept)" : "Mind Map: \(concept)",
                "config": cfg
            ])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func regenerateArtifact(_ art: StudioArtifactItem) async {
        do {
            _ = try await core.call(method: "studio.regenerate", params: [
                "artifact_id": art.id,
                "config": art.config,
                "use_selected_sources": true
            ])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func retryArtifact(artifactId: String) async {
        do {
            _ = try await core.call(method: "studio.retry", params: ["artifact_id": artifactId])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func approveDraft(artifactId: String, markdown: String) async {
        do {
            _ = try await core.call(method: "studio.approve_draft", params: ["artifact_id": artifactId, "draft": markdown])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }
}
