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
    var errorCode: String = ""
    var errorDetail: String = ""
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
        ["retrieve", "retrieve_fallback", "tool_search", "tool_read_context", "tool_list_sources", "tool_call", "tool_studio"].contains(kind)
    }
}

/// Unvalidated in-progress payload of a running artifact (event `studio.preview`).
struct StudioPreviewItem {
    let stage: String
    let payload: [String: Any]
}

/// A source excerpt a chat answer cites with `[n]`.
struct CitationInfo {
    let no: Int
    let title: String
    let page: Int
    let quote: String
}

struct ChatMessageItem: Identifiable {
    let id: String
    let role: String
    var content: String
    var thoughts: [ThoughtStepItem]
    /// Pictures, files and links the user attached to the message.
    var attachments: [[String: Any]] = []
    var citations: [Int: CitationInfo] = [:]
    /// COMPLETE | INCOMPLETE | FAILED | CANCELLED
    var status: String = "COMPLETE"
    var errorCode: String = ""
    var feedback: Int = 0
    var followups: [String] = []
    /// Studio artifacts the assistant generated through tool calls (shown as inline cards).
    var artifacts: [[String: Any]] = []
    var isStreaming: Bool = false
}

struct StudioArtifactItem: Identifiable {
    let id: String
    var notebookId: String = ""
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
    /// COLLECT | PLAN | GENERATE | RENDER while running; `progress` is 0…1.
    var stage: String = ""
    var progress: Double = 0
    /// Unix milliseconds.
    var createdAt: Int64 = 0
    var updatedAt: Int64 = 0
    /// The user's own pictures (local paths) and file names given with the request.
    var imagePaths: [String] = []
    var fileNames: [String] = []
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
    /// artifact id -> latest unvalidated preview; dropped once the artifact leaves PENDING/RUNNING.
    @Published var studioPreviews: [String: StudioPreviewItem] = [:]
    @Published var showSettingsSheet: Bool = false
    @Published var showPrivacySheet: Bool = false
    @Published var isGeneratingChat: Bool = false
    /// Source whose preview is open in the left pane (nil = source list).
    @Published var previewSource: SourcePreviewRequest? = nil
    @Published var sourceFullscreen: Bool = false
    /// Ids of chat artifacts the user saved to Studio from the chat card.
    @Published var chatArtifactsInStudio: Set<String> = []
    /// Sources whose study guide was already generated automatically once this session.
    var guideAutoTried: Set<String> = []
    private var lastSavedEnglish = true
    private var streamBuffer = ""
    private var streamFlushScheduled = false
    @Published var embeddingDownloadRunning: Bool = false
    @Published var embeddingDownloadProgress: Double = 0
    @Published var embeddingDownloadStatus: String = ""
    /// ffmpeg download (`ffmpeg.install`), driven by the `ffmpeg` core events.
    @Published var ffmpegDownloadRunning: Bool = false
    @Published var ffmpegDownloadedBytes: Int = 0
    @Published var ffmpegDownloadTotalBytes: Int? = nil
    @Published var ffmpegDownloadError: String = ""
    /// Bumped on every core `profile.updated` event (a profile's capability probe or test finished).
    @Published var profilesRevision: Int = 0

    var isEnglish: Bool { prefs.isEnglish }
    func t(_ key: String) -> String { L10n.t(key, en: isEnglish) }

    /// Safety net: while any artifact is still PENDING/RUNNING, re-read the list every few seconds, so a missed
    /// or reordered status event can never leave a finished artifact showing "in progress".
    private func startRunningArtifactWatchdog() {
        Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                guard let self else { return }
                let running = self.artifacts.contains { $0.status == "PENDING" || $0.status == "RUNNING" }
                if running { await self.refreshAll() }
            }
        }
    }

    func start() {
        startRunningArtifactWatchdog()
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = appSupport.appendingPathComponent("LocalNotebook", isDirectory: true).path
        do {
            try core.open(dataDir: dir)
            core.onEvent = { [weak self] ev in
                Task { @MainActor in
                    if ev.event.hasPrefix("chat.") {
                        self?.handleChatEvent(ev)
                    } else if ev.event == "source.status" || ev.event == "studio.status" {
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
                        self?.embeddingDownloadStatus = L10n.fmt("app.downloading_mb", en: en, file, fileIdx, fileCnt, done / 1_000_000, total / 1_000_000)
                    } else if ev.event == "embedding.download_done" {
                        self?.embeddingDownloadRunning = false
                        let ok = ev.data["ok"] as? Bool ?? false
                        let en = self?.isEnglish ?? true
                        if ok {
                            self?.embeddingDownloadProgress = 100
                            self?.embeddingDownloadStatus = L10n.t("app.semantic_search_model_enabled", en: en)
                            self?.statusMessage = self?.embeddingDownloadStatus ?? ""
                        } else if let err = ev.data["error"] as? [String: Any] {
                            let code = err["code"] as? String ?? ""
                            let msg = err["message"] as? String ?? ""
                            if code != "CANCELLED" {
                                self?.embeddingDownloadStatus = msg.isEmpty ? (L10n.t("app.download_failed", en: en)) : msg
                            } else {
                                self?.embeddingDownloadStatus = L10n.t("app.download_cancelled", en: en)
                            }
                        }
                    } else if ev.event == "ffmpeg.download_progress" {
                        self?.ffmpegDownloadRunning = true
                        self?.ffmpegDownloadedBytes = (ev.data["downloaded"] as? Int) ?? 0
                        self?.ffmpegDownloadTotalBytes = ev.data["total"] as? Int
                    } else if ev.event == "ffmpeg.download_done" {
                        self?.ffmpegDownloadRunning = false
                        if ev.data["ok"] as? Bool ?? false {
                            self?.ffmpegDownloadError = ""
                        } else {
                            let en = self?.isEnglish ?? true
                            let err = ev.data["error"] as? [String: Any]
                            let msg = err?["message"] as? String ?? ""
                            self?.ffmpegDownloadError = msg.isEmpty ? L10n.t("app.download_failed", en: en) : msg
                        }
                    } else if ev.event == "profile.updated" {
                        // the core finished probing a profile's inputs (or a test): views re-read their capability tags
                        self?.profilesRevision += 1
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
                    } else if ev.event == "studio.preview" {
                        if let aid = ev.data["artifact_id"] as? String,
                           let payload = ev.data["payload"] as? [String: Any] {
                            self?.studioPreviews[aid] = StudioPreviewItem(
                                stage: ev.data["stage"] as? String ?? "",
                                payload: payload
                            )
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
            report(error)
        }
    }

    func loadPreferences() async {
        if let res = try? await core.call(method: "settings.get", params: ["key": "app.preferences"]),
           let val = res["value"] as? [String: Any] {
            prefs = UiPreferences.from(val)
        }
        lastSavedEnglish = isEnglish
        if summaryText.isEmpty {
            summaryText = t("chat.empty_summary")
        }
    }

    func savePreferences() async {
        _ = try? await core.call(method: "settings.set", params: [
            "key": "app.preferences",
            "value": prefs.toDict()
        ])
        // Only a language change needs the notebook's generated text translated.
        let languageChanged = isEnglish != lastSavedEnglish
        lastSavedEnglish = isEnglish
        if !activeNotebookId.isEmpty && languageChanged {
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
            await afterStartup()
        } catch {
            report(error)
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
            report(error)
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

    private var refreshInFlight = false
    private var refreshAgain = false

    /// Reloads everything. Refreshes never overlap: core replies can arrive out of order, so a slow refresh that
    /// started while an artifact was still RUNNING could land after the READY one and put the old status back.
    /// A call made while one is running schedules exactly one more pass (which sees the newest state) and waits for it.
    func refreshAll() async {
        if refreshInFlight {
            refreshAgain = true
            while refreshInFlight { try? await Task.sleep(nanoseconds: 20_000_000) }
            return
        }
        refreshInFlight = true
        defer { refreshInFlight = false }
        repeat {
            refreshAgain = false
            await refreshAllOnce()
        } while refreshAgain
    }

    private func refreshAllOnce() async {
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
                    recommendKnowledgeGraph: $0["recommend_knowledge_graph"] as? Bool ?? false,
                    errorCode: $0["error_code"] as? String ?? "",
                    errorDetail: $0["error_detail"] as? String ?? ""
                )
            }

            if let noteRes = try? await core.call(method: "note.list", params: ["notebook_id": activeNotebookId]) {
                notes = (noteRes["notes"] as? [[String: Any]] ?? []).map {
                    NoteItem(
                        id: $0["id"] as? String ?? "",
                        title: $0["title"] as? String ?? L10n.t("note.dialog_title", en: isEnglish),
                        bodyMd: $0["body_md"] as? String ?? ""
                    )
                }
            }

            // A running reply lives only in memory until it is saved; keep it on screen.
            if !isGeneratingChat {
                let chatRes = try await core.call(method: "chat.history", params: ["notebook_id": activeNotebookId])
                messages = (chatRes["messages"] as? [[String: Any]] ?? []).map(Self.parseMessage)
            }

            let artRes = try await core.call(method: "studio.list", params: ["notebook_id": activeNotebookId])
            artifacts = (artRes["artifacts"] as? [[String: Any]] ?? []).map { Self.parseArtifact($0, en: isEnglish) }
            // A preview only exists while generating; drop it once the artifact has left PENDING/RUNNING.
            if !studioPreviews.isEmpty {
                let running = Set(artifacts.filter { $0.status == "PENDING" || $0.status == "RUNNING" }.map { $0.id })
                for key in Array(studioPreviews.keys) where !running.contains(key) {
                    studioPreviews.removeValue(forKey: key)
                }
            }
            if let openId = activeArtifact?.id, let updated = artifacts.first(where: { $0.id == openId }) {
                activeArtifact = updated
            }
        } catch {
            report(error)
        }
    }

    /// `extra` carries the attachments (`images`, `files`, `links`). Returns whether the message was sent.
    @discardableResult
    func sendChat(_ text: String, extra: [String: Any] = [:]) async -> Bool {
        guard !activeNotebookId.isEmpty, !text.isEmpty || !extra.isEmpty, !isGeneratingChat else { return false }
        guard await ensurePrivacyAck() else { return false }
        isGeneratingChat = true
        messages.append(ChatMessageItem(id: "pending-user", role: "user", content: text, thoughts: []))
        var sent = false
        do {
            var params: [String: Any] = ["notebook_id": activeNotebookId, "text": text]
            params.merge(extra) { _, new in new }
            _ = try await core.call(method: "chat.send", params: params)
            sent = true
        } catch {
            report(error, titleKey: "chat.failed_title")
        }
        finishChatBuffers()
        isGeneratingChat = false
        await refreshAll()
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
            report(error)
        }
    }

    func cancelChat() async {
        guard !activeNotebookId.isEmpty else { return }
        _ = await run("chat.cancel", ["notebook_id": activeNotebookId])
    }

    func regenerateChat() async {
        guard !activeNotebookId.isEmpty, !isGeneratingChat else { return }
        isGeneratingChat = true
        // The core replaces the last answer; drop it from the screen so the new one streams in its place.
        if let last = messages.last, last.role == "assistant" { messages.removeLast() }
        do {
            _ = try await core.call(method: "chat.regenerate", params: ["notebook_id": activeNotebookId])
        } catch {
            report(error, titleKey: "chat.failed_title")
        }
        finishChatBuffers()
        isGeneratingChat = false
        await refreshAll()
    }

    // MARK: - Chat streaming

    static func parseMessage(_ m: [String: Any]) -> ChatMessageItem {
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
        var cites: [Int: CitationInfo] = [:]
        for c in m["citations"] as? [[String: Any]] ?? [] {
            let no = (c["no"] as? NSNumber)?.intValue ?? 0
            guard no > 0 else { continue }
            cites[no] = CitationInfo(
                no: no,
                title: c["source_title"] as? String ?? "",
                page: (c["page_start"] as? NSNumber)?.intValue ?? 0,
                quote: c["quote"] as? String ?? ""
            )
        }
        var item = ChatMessageItem(
            id: m["id"] as? String ?? UUID().uuidString,
            role: m["role"] as? String ?? "assistant",
            content: m["content"] as? String ?? "",
            thoughts: ths,
            attachments: m["attachments"] as? [[String: Any]] ?? []
        )
        item.citations = cites
        item.status = m["status"] as? String ?? "COMPLETE"
        item.errorCode = m["error_code"] as? String ?? ""
        item.feedback = (m["feedback"] as? NSNumber)?.intValue ?? 0
        item.followups = (m["followups"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty }
        item.artifacts = m["artifacts"] as? [[String: Any]] ?? []
        return item
    }

    static func parseArtifact(_ a: [String: Any], en: Bool = false) -> StudioArtifactItem {
        let cfg = a["config"] as? [String: Any] ?? [:]
        let rawRefs = a["source_refs"] as? [[String: Any]] ?? []
        let refs = rawRefs.map {
            ArtifactSourceRefItem(
                id: $0["id"] as? String ?? UUID().uuidString,
                title: $0["title"] as? String ?? "",
                kind: $0["kind"] as? String ?? "file"
            )
        }
        let srcCount = refs.count // only the sources the generation actually read
        let atts = a["attachments"] as? [[String: Any]] ?? []
        let isImage: ([String: Any]) -> Bool = { ($0["mime"] as? String ?? "").hasPrefix("image/") }
        var item = StudioArtifactItem(
            id: a["id"] as? String ?? "",
            notebookId: a["notebook_id"] as? String ?? "",
            artifactType: a["artifact_type"] as? String ?? "report",
            title: a["title"] as? String ?? L10n.t("appstate.artifact_fallback_title", en: en),
            status: a["status"] as? String ?? "PENDING",
            sourceCount: srcCount,
            sourceRefs: refs,
            promptTopic: (cfg["topic"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines),
            config: cfg,
            draftMarkdown: Draft.toMarkdown(a["draft"], en: en),
            payload: a["payload"] as? [String: Any] ?? [:],
            errorCode: a["error_code"] as? String ?? "",
            errorDetail: a["error_detail"] as? String ?? "",
            errorLog: a["error_log"] as? String ?? "",
            outputDir: a["output_dir"] as? String ?? "",
            progressState: a["progress_state"] as? [String: Any] ?? [:],
            stage: a["stage"] as? String ?? "",
            progress: (a["progress"] as? NSNumber)?.doubleValue ?? 0,
            createdAt: (a["created_at"] as? NSNumber)?.int64Value ?? 0,
            updatedAt: (a["updated_at"] as? NSNumber)?.int64Value ?? 0
        )
        item.imagePaths = atts.filter(isImage).compactMap { $0["path"] as? String }.filter { FileManager.default.fileExists(atPath: $0) }
        item.fileNames = atts.filter { !isImage($0) }.compactMap { $0["name"] as? String }
        return item
    }

    private func chatIndex(_ id: String) -> Int? { messages.firstIndex(where: { $0.id == id }) }
    private var streamingIndex: Int? { messages.lastIndex(where: { $0.isStreaming }) }

    private func flushStreamBuffer() {
        streamFlushScheduled = false
        guard !streamBuffer.isEmpty, let i = streamingIndex else { return }
        messages[i].content += streamBuffer
        streamBuffer = ""
    }

    private func finishChatBuffers() {
        flushStreamBuffer()
        if let i = streamingIndex { messages[i].isStreaming = false }
    }

    func handleChatEvent(_ ev: ApiEvent) {
        guard !activeNotebookId.isEmpty, ev.topic == "chat:\(activeNotebookId)" else { return }
        let d = ev.data
        switch ev.event {
        case "chat.started":
            // The pending user message is replaced by the saved one at the next refresh.
            var m = ChatMessageItem(id: d["message_id"] as? String ?? UUID().uuidString, role: "assistant", content: "", thoughts: [])
            m.isStreaming = true
            streamBuffer = ""
            messages.append(m)
        case "chat.thought_step":
            guard let i = streamingIndex else { return }
            let step = ThoughtStepItem(
                id: d["step_id"] as? String ?? UUID().uuidString,
                kind: d["kind"] as? String ?? "thought_stage",
                title: d["title"] as? String ?? "",
                detail: d["detail"] as? String ?? ""
            )
            if let k = messages[i].thoughts.firstIndex(where: { $0.id == step.id }) {
                messages[i].thoughts[k] = step
            } else {
                messages[i].thoughts.append(step)
            }
        case "chat.text_delta":
            guard let t = d["text"] as? String, !t.isEmpty else { return }
            streamBuffer += t
            if !streamFlushScheduled {
                streamFlushScheduled = true
                Task { @MainActor [weak self] in
                    try? await Task.sleep(nanoseconds: 80_000_000)
                    self?.flushStreamBuffer()
                }
            }
        case "chat.citation":
            guard let i = streamingIndex else { return }
            let no = (d["no"] as? NSNumber)?.intValue ?? 0
            guard no > 0 else { return }
            messages[i].citations[no] = CitationInfo(
                no: no,
                title: d["title"] as? String ?? "",
                page: (d["page"] as? NSNumber)?.intValue ?? 0,
                quote: d["quote"] as? String ?? ""
            )
        case "chat.artifact":
            guard let art = d["artifact"] as? [String: Any], let aid = art["id"] as? String else { return }
            let mid = d["message_id"] as? String ?? ""
            guard let i = chatIndex(mid) ?? streamingIndex else { return }
            if let k = messages[i].artifacts.firstIndex(where: { ($0["id"] as? String) == aid }) {
                messages[i].artifacts[k] = art
            } else {
                messages[i].artifacts.append(art)
            }
        case "chat.followups":
            guard let i = chatIndex(d["message_id"] as? String ?? "") else { return }
            messages[i].followups = (d["questions"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty }
        case "chat.finished":
            finishChatBuffers()
            let status = d["status"] as? String ?? "COMPLETE"
            if let i = messages.lastIndex(where: { $0.role == "assistant" }) {
                messages[i].status = status
                if let err = d["error"] as? [String: Any] { messages[i].errorCode = err["code"] as? String ?? "" }
            }
            if status == "FAILED" || status == "INCOMPLETE", let err = d["error"] as? [String: Any] {
                reportCode(err["code"] as? String ?? "", message: err["message"] as? String ?? "", titleKey: "chat.failed_title")
            }
        default:
            break
        }
    }

    // MARK: - Errors, privacy

    /// Text for an error: localized advice plus the code, or the raw message.
    func describe(_ error: Error) -> String {
        if let e = error as? CoreError {
            if let adv = L10n.advice(e.code, en: isEnglish), !adv.isEmpty { return e.code.isEmpty ? adv : "\(adv) (\(e.code))" }
            return e.code.isEmpty ? e.message : "\(e.message) (\(e.code))"
        }
        return error.localizedDescription
    }

    func report(_ error: Error, titleKey: String? = nil) {
        let text = describe(error)
        if let titleKey { statusMessage = "\(t(titleKey))\(L10n.t("common.colon", en: isEnglish))\(text)" } else { statusMessage = text }
    }

    func reportCode(_ code: String, message: String, titleKey: String? = nil) {
        report(CoreError(code: code, message: message), titleKey: titleKey)
    }

    /// Calls the core for a UI action; an error goes to the status bar. Returns nil on error.
    @discardableResult
    func run(_ method: String, _ params: [String: Any] = [:], what: String? = nil) async -> [String: Any]? {
        do {
            return try await core.call(method: method, params: params)
        } catch {
            if let what { statusMessage = "\(what)\(L10n.t("common.colon", en: isEnglish))\(describe(error))" } else { report(error) }
            return nil
        }
    }

    /// Before the first message to a model outside this computer, explains what is sent (once).
    func ensurePrivacyAck() async -> Bool {
        if let r = try? await core.call(method: "settings.get", params: ["key": "privacy.cloud_ack"]), r["value"] as? Bool == true { return true }
        guard let list = try? await core.call(method: "profile.list"),
              let active = (list["profiles"] as? [[String: Any]])?.first(where: { $0["is_active"] as? Bool == true }) else { return true }
        let base = active["base_url"] as? String ?? ""
        if let host = URL(string: base)?.host, ["localhost", "127.0.0.1", "::1"].contains(host) { return true }
        guard PrivacyAlert.confirm(en: isEnglish, title: t("nb.privacy")) else { return false }
        _ = try? await core.call(method: "settings.set", params: ["key": "privacy.cloud_ack", "value": true])
        return true
    }

    /// Ask where to save, then let the core write the artifact in `format` (`studio.export`).
    func exportArtifact(_ artifact: StudioArtifactItem, format: String, ext: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(artifact.title).\(ext)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task {
            if await run("studio.export", ["artifact_id": artifact.id, "format": format, "out_path": url.path], what: t("studio.export")) != nil {
                statusMessage = "\(t("common.exported"))\(L10n.t("common.colon", en: isEnglish))\(url.path)"
            }
        }
    }

    func exportDiagnostics() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "local-notebook-diagnostics.zip"
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                do {
                    _ = try await core.call(method: "diagnostics.export", params: ["out_path": url.path])
                    statusMessage = L10n.fmt("app.diagnostics_exported", en: isEnglish, url.lastPathComponent)
                } catch {
                    report(error)
                }
            }
        }
    }

    func exportLogs() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "local-notebook-logs.log"
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                do {
                    _ = try await core.call(method: "logs.export", params: ["out_path": url.path])
                    statusMessage = L10n.fmt("settings.export_logs_done", en: isEnglish, url.lastPathComponent)
                } catch {
                    report(error)
                }
            }
        }
    }

    /// Creates a Studio artifact from the configuration the user chose (`StudioConfigSheet`).
    func createStudio(kind: String, config: [String: Any], extra: [String: Any] = [:]) async {
        guard !activeNotebookId.isEmpty else { return }
        let actualType = kind == "knowledge_graph" ? "mindmap" : kind
        var cfg = config
        if kind == "knowledge_graph" { cfg["mode"] = "knowledge_graph" }
        var params: [String: Any] = ["notebook_id": activeNotebookId, "artifact_type": actualType, "config": cfg]
        params.merge(extra) { _, new in new }
        let name = t("studio.\(kind)")
        if await run("studio.create", params, what: name) != nil {
            statusMessage = "\(name)\(L10n.t("common.colon", en: isEnglish))\(t("studio.queued_msg"))"
        }
        await refreshAll()
    }

    /// Builds the knowledge graph of one source (shown in that source's preview, not in the Studio list).
    func createKnowledgeGraph(sourceId: String, sourceTitle: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let cfg: [String: Any] = [
            "mode": "knowledge_graph",
            "topic": L10n.fmt("kg.default_topic", en: isEnglish, sourceTitle),
            "source_id": sourceId,
            "lang": isEnglish ? "en-US" : "zh-CN",
        ]
        let res = await run("studio.create", [
            "notebook_id": activeNotebookId,
            "artifact_type": "mindmap",
            "title": L10n.fmt("kg.title_with_source", en: isEnglish, sourceTitle),
            "config": cfg,
        ], what: t("kg.title"))
        if res != nil { statusMessage = "\(t("kg.title"))\(L10n.t("common.colon", en: isEnglish))\(t("kg.building_msg"))" }
        await refreshAll()
    }

    func createConceptLink(kind: String, concept: String) async {
        guard !activeNotebookId.isEmpty else { return }
        let cfg: [String: Any] = kind == "quiz"
            ? ["topic": L10n.fmt("kg.quiz_topic", en: isEnglish, concept), "quantity": "standard", "count": 8, "difficulty": "auto"]
            : ["topic": L10n.fmt("kg.mindmap_topic", en: isEnglish, concept)]
        let title = L10n.fmt(kind == "quiz" ? "kg.quiz_title" : "kg.mindmap_title", en: isEnglish, concept)
        if await run("studio.create", [
            "notebook_id": activeNotebookId,
            "artifact_type": kind,
            "title": title,
            "config": cfg,
        ], what: t("studio.\(kind)")) != nil {
            statusMessage = "\(title)\(L10n.t("common.colon", en: isEnglish))\(t("studio.queued_msg"))"
        }
        await refreshAll()
    }

    /// Regenerates an artifact from the currently selected sources with a (possibly edited) configuration.
    func regenerateArtifact(_ art: StudioArtifactItem, config: [String: Any]? = nil, extra: [String: Any] = [:]) async {
        var params: [String: Any] = [
            "artifact_id": art.id,
            "config": config ?? art.config,
            "use_selected_sources": true,
        ]
        params.merge(extra) { _, new in new }
        _ = await run("studio.regenerate", params, what: t("studio.op_update"))
        await refreshAll()
    }

    func retryArtifact(artifactId: String) async {
        do {
            _ = try await core.call(method: "studio.retry", params: ["artifact_id": artifactId])
            await refreshAll()
        } catch {
            report(error)
        }
    }

    func reviseDraft(artifactId: String, instruction: String, currentDraft: String, extra: [String: Any] = [:]) async -> String? {
        guard !instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        var params: [String: Any] = ["artifact_id": artifactId, "instruction": instruction, "draft": currentDraft]
        params.merge(extra) { _, new in new }
        do {
            let res = try await core.call(method: "studio.revise_draft", params: params)
            if res["draft"] != nil {
                let md = Draft.toMarkdown(res["draft"], en: isEnglish)
                await refreshAll()
                if !md.isEmpty { return md }
            }
        } catch {
            report(error)
        }
        return nil
    }

    /// Splits a mind-map / knowledge-graph node into finer points; the core may decline atomic nodes.
    func refineNode(artifactId: String, nodeId: String, label: String) async {
        statusMessage = "\(t("kg.refining"))\(L10n.t("common.colon", en: isEnglish))\(label)"
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
            report(error)
        }
    }

    func approveDraft(artifactId: String, markdown: String) async {
        do {
            var params: [String: Any] = ["artifact_id": artifactId]
            if !markdown.isEmpty { params["draft"] = markdown }
            _ = try await core.call(method: "studio.approve_draft", params: params)
            statusMessage = "\(t("art.outline_approved_title"))\(L10n.t("common.colon", en: isEnglish))\(t("art.outline_approved_msg"))"
            await refreshAll()
        } catch {
            report(error)
        }
    }

    /// Fetches the current preview once (e.g. when the window opens mid-generation).
    func loadStudioPreview(artifactId: String) async {
        if studioPreviews[artifactId] != nil { return }
        guard let res = try? await core.call(method: "studio.preview.get", params: ["artifact_id": artifactId]),
              let payload = res["payload"] as? [String: Any] else { return }
        // An event may have delivered something newer while the call was in flight.
        if studioPreviews[artifactId] != nil { return }
        studioPreviews[artifactId] = StudioPreviewItem(stage: res["stage"] as? String ?? "", payload: payload)
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
