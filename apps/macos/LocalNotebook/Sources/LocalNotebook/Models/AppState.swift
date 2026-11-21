import Foundation
import SwiftUI

struct NotebookItem: Identifiable, Hashable {
    let id: String
    let title: String
    let summaryMd: String
}

struct SourceItem: Identifiable {
    let id: String
    let title: String
    let kind: String
    let status: String
    var isSelected: Bool
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
    let draftMarkdown: String
    let payload: [String: Any]
}

@MainActor
final class AppState: ObservableObject {
    let core = CoreClient()

    @Published var notebooks: [NotebookItem] = []
    @Published var activeNotebookId: String = ""
    @Published var summaryText: String = "请添加来源或直接提问。"
    @Published var sources: [SourceItem] = []
    @Published var messages: [ChatMessageItem] = []
    @Published var artifacts: [StudioArtifactItem] = []
    @Published var activeArtifact: StudioArtifactItem?
    @Published var studioFullscreen: Bool = false
    @Published var statusMessage: String = ""
    @Published var showSettingsSheet: Bool = false
    @Published var showEmbeddingSheet: Bool = false

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
            Task { await loadInitialNotebook() }
        } catch {
            statusMessage = "核心启动失败: \(error.localizedDescription)"
        }
    }

    func loadInitialNotebook() async {
        do {
            let res = try await core.call(method: "notebook.list")
            let rawList = res["notebooks"] as? [[String: Any]] ?? []
            if let first = rawList.first, let id = first["id"] as? String {
                activeNotebookId = id
            } else {
                let created = try await core.call(method: "notebook.create", params: ["title": "未命名笔记本"])
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
                NotebookItem(id: $0["id"] as? String ?? "", title: $0["title"] as? String ?? "", summaryMd: $0["summary_md"] as? String ?? "")
            }
            if let cur = notebooks.first(where: { $0.id == activeNotebookId }), !cur.summaryMd.isEmpty {
                summaryText = cur.summaryMd
            }

            let srcRes = try await core.call(method: "source.list", params: ["notebook_id": activeNotebookId])
            sources = (srcRes["sources"] as? [[String: Any]] ?? []).map {
                SourceItem(
                    id: $0["id"] as? String ?? "",
                    title: $0["title"] as? String ?? "",
                    kind: $0["kind"] as? String ?? "txt",
                    status: $0["status"] as? String ?? "READY",
                    isSelected: $0["is_selected"] as? Bool ?? true
                )
            }

            let chatRes = try await core.call(method: "chat.history", params: ["notebook_id": activeNotebookId])
            messages = (chatRes["messages"] as? [[String: Any]] ?? []).map { m in
                let rawTh = m["thoughts"] as? [[String: Any]] ?? []
                let ths = rawTh.map {
                    ThoughtStepItem(
                        id: $0["step_id"] as? String ?? UUID().uuidString,
                        kind: $0["kind"] as? String ?? "thought_stage",
                        title: $0["title"] as? String ?? "",
                        detail: $0["detail"] as? String ?? ""
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
            artifacts = (artRes["artifacts"] as? [[String: Any]] ?? []).map { a in
                StudioArtifactItem(
                    id: a["id"] as? String ?? "",
                    artifactType: a["artifact_type"] as? String ?? "report",
                    title: a["title"] as? String ?? "产物",
                    status: a["status"] as? String ?? "PENDING",
                    sourceCount: (a["source_ids"] as? [Any])?.count ?? 0,
                    draftMarkdown: a["draft"] as? String ?? "",
                    payload: a["payload"] as? [String: Any] ?? [:]
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
            statusMessage = "对话失败: \(error.localizedDescription)"
        }
    }

    func createStudio(kind: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let cfg: [String: Any] = kind == "slides" ? ["pages": 12, "reviewOutline": true] : [:]
        do {
            _ = try await core.call(method: "studio.create", params: ["notebook_id": activeNotebookId, "artifact_type": kind, "config": cfg])
            statusMessage = "已提交生成任务。"
            await refreshAll()
        } catch {
            statusMessage = "生成失败: \(error.localizedDescription)"
        }
    }

    func approveDraft(artifactId: String, markdown: String) async {
        do {
            _ = try await core.call(method: "studio.approve_draft", params: ["artifact_id": artifactId, "draft": markdown])
            statusMessage = "已批准提纲，正在生成最终文稿。"
            await refreshAll()
        } catch {
            statusMessage = "批准失败: \(error.localizedDescription)"
        }
    }
}
