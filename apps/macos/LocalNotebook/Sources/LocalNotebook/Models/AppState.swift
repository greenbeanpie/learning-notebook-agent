import AppKit
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

struct NoteItem: Identifiable {
    let id: String
    let title: String
    let bodyMd: String
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
    /// Pictures, files and links the user attached to the message.
    var attachments: [[String: Any]] = []
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
    /// Folder with the generated files (a lecture's figures live under `figures/`).
    var outputDir: String = ""
    /// What the player saved, e.g. a lecture's position.
    var progressState: [String: Any] = [:]
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
    @Published var notes: [NoteItem] = []
    @Published var messages: [ChatMessageItem] = []
    @Published var artifacts: [StudioArtifactItem] = []
    @Published var activeArtifact: StudioArtifactItem?
    @Published var studioFullscreen: Bool = false
    @Published var statusMessage: String = ""
    @Published var showCreateNotebookSheet = false
    @Published var indexingSourceIds: Set<String> = []
    @Published var activeAiActivity: String = ""
    @Published var studioThoughts: [String: [ThoughtStepItem]] = [:]
    @Published var showSettingsSheet: Bool = false
    @Published var showPrivacySheet: Bool = false
    @Published var isGeneratingChat: Bool = false
    @Published var embeddingDownloadRunning: Bool = false
    @Published var embeddingDownloadProgress: Double = 0
    @Published var embeddingDownloadStatus: String = ""

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
                        if ev.event == "chat.finished" {
                            self?.isGeneratingChat = false
                        }
                        await self?.refreshAll()
                    } else if ev.event == "embedding.download_progress" {
                        let total = max(1, (ev.data["overall_total"] as? Int) ?? 1)
                        let done = (ev.data["overall_done"] as? Int) ?? 0
                        let file = ev.data["file"] as? String ?? ""
                        let fileIdx = ((ev.data["file_index"] as? Int) ?? 0) + 1
                        let fileCnt = (ev.data["file_count"] as? Int) ?? 1
                        self?.embeddingDownloadRunning = true
                        self?.embeddingDownloadProgress = Double(done) * 100.0 / Double(total)
                        let en = self?.isEnglish ?? true
                        self?.embeddingDownloadStatus = en
                            ? "Downloading \(file) (\(fileIdx)/\(fileCnt)) — \(done / 1_000_000) / \(total / 1_000_000) MB"
                            : "正在下载 \(file) (\(fileIdx)/\(fileCnt)) — \(done / 1_000_000) / \(total / 1_000_000) MB"
                    } else if ev.event == "embedding.download_done" {
                        self?.embeddingDownloadRunning = false
                        let ok = ev.data["ok"] as? Bool ?? false
                        let en = self?.isEnglish ?? true
                        if ok {
                            self?.embeddingDownloadProgress = 100
                            self?.embeddingDownloadStatus = en ? "Semantic search model enabled." : "语义检索模型已启用。"
                            self?.statusMessage = self?.embeddingDownloadStatus ?? ""
                        } else if let err = ev.data["error"] as? [String: Any] {
                            let code = err["code"] as? String ?? ""
                            let msg = err["message"] as? String ?? ""
                            if code != "CANCELLED" {
                                self?.embeddingDownloadStatus = msg.isEmpty ? (en ? "Download failed." : "下载失败。") : msg
                            } else {
                                self?.embeddingDownloadStatus = en ? "Download cancelled." : "已取消下载。"
                            }
                        }
                    } else if ev.event == "studio.thought" {
                        if let aid = ev.data["artifact_id"] as? String {
                            let kind = ev.data["kind"] as? String ?? "stage"
                            if kind != "retry" {
                                let seq = ev.data["seq"] as? Int ?? ((self?.studioThoughts[aid]?.count ?? 0) + 1)
                                let seqId = String(seq)
                                let item = ThoughtStepItem(
                                    id: seqId,
                                    kind: kind,
                                    title: ev.data["title"] as? String ?? "",
                                    detail: ev.data["body"] as? String ?? (ev.data["detail"] as? String ?? "")
                                )
                                var list = self?.studioThoughts[aid] ?? []
                                if let idx = list.firstIndex(where: { $0.id == seqId }) {
                                    list[idx] = item
                                } else {
                                    list.append(item)
                                }
                                self?.studioThoughts[aid] = list
                            }
                        }
                    } else if ev.event == "studio.thought_delta" {
                        if let aid = ev.data["artifact_id"] as? String,
                           let seq = ev.data["seq"] as? Int,
                           let delta = ev.data["delta"] as? String {
                            let seqId = String(seq)
                            var list = self?.studioThoughts[aid] ?? []
                            if let idx = list.firstIndex(where: { $0.id == seqId }) {
                                let cur = list[idx]
                                list[idx] = ThoughtStepItem(id: cur.id, kind: cur.kind, title: cur.title, detail: cur.detail + delta)
                                self?.studioThoughts[aid] = list
                            }
                        }
                    } else if ev.event == "ai.activity" {
                        let active = ev.data["active"] as? Bool ?? false
                        let title = ev.data["title"] as? String ?? ""
                        let body = ev.data["body"] as? String ?? ""
                        if (ev.data["scope"] as? String) == "source_index" {
                            // Spinner on the source row instead of a status banner.
                            let sid = ev.data["source_id"] as? String ?? ""
                            if active { self?.indexingSourceIds.insert(sid) } else {
                                self?.indexingSourceIds.remove(sid)
                                Task { await self?.refreshAll() }
                            }
                        } else if active && !title.isEmpty {
                            self?.activeAiActivity = body.isEmpty ? title : "\(title) — \(body)"
                            self?.statusMessage = self?.activeAiActivity ?? ""
                        } else if !active {
                            self?.activeAiActivity = ""
                            self?.statusMessage = ""
                        }
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
                await refreshAll()
            } else {
                enterEmptyNotebookState()
            }
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    /// No notebook exists: clear the workspace and ask the user to create one.
    func enterEmptyNotebookState() {
        activeNotebookId = ""
        notebooks = []
        summaryText = ""
        suggestedQuestions = []
        sources = []
        notes = []
        messages = []
        artifacts = []
        activeArtifact = nil
        showCreateNotebookSheet = true
    }

    func createNotebook(title: String, filePaths: [String]) async {
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        do {
            let created = try await core.call(method: "notebook.create", params: ["title": name])
            guard let id = created["id"] as? String else { return }
            activeNotebookId = id
            await refreshAll()
            if !filePaths.isEmpty {
                _ = try await core.call(method: "source.add_files", params: ["notebook_id": id, "paths": filePaths])
                await refreshAll()
            }
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func renameActiveNotebook(_ title: String) async {
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !activeNotebookId.isEmpty, !name.isEmpty,
              notebooks.first(where: { $0.id == activeNotebookId })?.title != name else { return }
        _ = try? await core.call(method: "notebook.rename", params: ["notebook_id": activeNotebookId, "title": name])
        await refreshAll()
    }

    func deleteActiveNotebook() async {
        guard !activeNotebookId.isEmpty else { return }
        _ = try? await core.call(method: "notebook.delete", params: ["notebook_id": activeNotebookId])
        let res = try? await core.call(method: "notebook.list")
        let rest = res?["notebooks"] as? [[String: Any]] ?? []
        if let first = rest.first, let id = first["id"] as? String {
            activeNotebookId = id
            await refreshAll()
        } else {
            enterEmptyNotebookState()
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

            if let noteRes = try? await core.call(method: "note.list", params: ["notebook_id": activeNotebookId]) {
                notes = (noteRes["notes"] as? [[String: Any]] ?? []).map {
                    NoteItem(
                        id: $0["id"] as? String ?? "",
                        title: $0["title"] as? String ?? "Note",
                        bodyMd: $0["body_md"] as? String ?? ""
                    )
                }
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
                    thoughts: ths,
                    attachments: m["attachments"] as? [[String: Any]] ?? []
                )
            }

            let artRes = try await core.call(method: "studio.list", params: ["notebook_id": activeNotebookId])
            artifacts = (artRes["artifacts"] as? [[String: Any]] ?? []).map { a in
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
                    errorLog: a["error_log"] as? String ?? "",
                    outputDir: a["output_dir"] as? String ?? "",
                    progressState: a["progress_state"] as? [String: Any] ?? [:]
                )
            }
            if let openId = activeArtifact?.id, let updated = artifacts.first(where: { $0.id == openId }) {
                activeArtifact = updated
            }
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    /// `extra` carries the attachments (`images`, `files`, `links`). Returns whether the message was sent.
    @discardableResult
    func sendChat(_ text: String, extra: [String: Any] = [:]) async -> Bool {
        guard !activeNotebookId.isEmpty, !text.isEmpty || !extra.isEmpty else { return false }
        isGeneratingChat = true
        var sent = false
        do {
            var params: [String: Any] = ["notebook_id": activeNotebookId, "text": text]
            params.merge(extra) { _, new in new }
            _ = try await core.call(method: "chat.send", params: params)
            sent = true
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
        isGeneratingChat = false
        return sent
    }

    /// Adds a stored attachment (file or link) of a message to the notebook's sources.
    func promoteAttachment(_ attachmentId: String) async {
        guard !activeNotebookId.isEmpty else { return }
        do {
            _ = try await core.call(
                method: "attachment.promote",
                params: ["notebook_id": activeNotebookId, "attachment_id": attachmentId]
            )
            statusMessage = t("ctx.promoted")
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
    }

    func cancelChat() async {
        guard !activeNotebookId.isEmpty else { return }
        _ = try? await core.call(method: "chat.cancel", params: ["notebook_id": activeNotebookId])
        isGeneratingChat = false
    }

    func regenerateChat() async {
        guard !activeNotebookId.isEmpty else { return }
        isGeneratingChat = true
        do {
            _ = try await core.call(method: "chat.regenerate", params: ["notebook_id": activeNotebookId])
            await refreshAll()
        } catch {
            statusMessage = error.localizedDescription
        }
        isGeneratingChat = false
    }

    func exportDiagnostics() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "diagnostics.json"
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                do {
                    _ = try await core.call(method: "diagnostics.export", params: ["out_path": url.path])
                    statusMessage = isEnglish ? "Diagnostics exported: \(url.lastPathComponent)" : "已导出诊断包：\(url.lastPathComponent)"
                } catch {
                    statusMessage = error.localizedDescription
                }
            }
        }
    }

    func createStudio(kind: String, topic: String = "", detail: String = "standard", extra: [String: Any] = [:], config: [String: Any] = [:]) async {
        guard !activeNotebookId.isEmpty else { return }
        let actualType = kind == "knowledge_graph" ? "mindmap" : kind
        var cfg: [String: Any] = kind == "slides"
            ? ["pages": 8, "reviewOutline": true]
            : (kind == "knowledge_graph" ? ["mode": "knowledge_graph"] : [:])
        if !topic.isEmpty {
            cfg["topic"] = topic
        }
        if !detail.isEmpty {
            cfg["detail"] = detail
        }
        if !prefs.defaultStudioLang.isEmpty {
            cfg["lang"] = prefs.defaultStudioLang
        }
        cfg.merge(config) { _, new in new }
        do {
            var params: [String: Any] = ["notebook_id": activeNotebookId, "artifact_type": actualType, "config": cfg]
            params.merge(extra) { _, new in new }
            _ = try await core.call(method: "studio.create", params: params)
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

    func reviseDraft(artifactId: String, instruction: String, currentDraft: String, extra: [String: Any] = [:]) async -> String? {
        guard !instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        var params: [String: Any] = ["artifact_id": artifactId, "instruction": instruction, "draft": currentDraft]
        params.merge(extra) { _, new in new }
        do {
            let res = try await core.call(method: "studio.revise_draft", params: params)
            if let newDraft = res["draft"] as? String {
                await refreshAll()
                return newDraft
            }
        } catch {
            statusMessage = error.localizedDescription
        }
        return nil
    }

    /// Splits a mind-map / knowledge-graph node into finer points; the core may decline atomic nodes.
    func refineNode(artifactId: String, nodeId: String, label: String) async {
        statusMessage = "\(t("kg.refining"))：\(label)"
        do {
            let res = try await core.call(
                method: "studio.refine_node",
                params: ["artifact_id": artifactId, "node_id": nodeId]
            )
            if res["refined"] as? Bool == true {
                let added = (res["added"] as? NSNumber)?.intValue ?? 0
                statusMessage = t("kg.refined_fmt")
                    .replacingOccurrences(of: "{0}", with: label)
                    .replacingOccurrences(of: "{1}", with: String(added))
            } else {
                let reason = (res["reason"] as? String) ?? ""
                statusMessage = t("kg.refine_declined").replacingOccurrences(of: "{0}", with: label)
                    + (reason.isEmpty ? "" : " — \(reason)")
            }
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

    func loadStudioThoughts(artifactId: String) async {
        guard let res = try? await core.call(method: "studio.get", params: ["artifact_id": artifactId]),
              let raw = res["thoughts"] as? [[String: Any]] else { return }
        let items: [ThoughtStepItem] = raw.enumerated().compactMap { idx, t in
            let k = t["kind"] as? String ?? "stage"
            if k == "retry" { return nil }
            return ThoughtStepItem(
                id: String(t["seq"] as? Int ?? (idx + 1)),
                kind: k,
                title: t["title"] as? String ?? "",
                detail: t["body"] as? String ?? (t["detail"] as? String ?? "")
            )
        }
        studioThoughts[artifactId] = items
    }
}
