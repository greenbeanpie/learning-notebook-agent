package com.localnotebook.agent.core

import com.localnotebook.agent.ui.putAttachments

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Unvalidated in-progress artifact payload pushed by `studio.preview`. */
data class PreviewState(val stage: String, val validated: Boolean, val payload: JSONObject)

data class AppUiState(
    val coreReady: Boolean = false,
    val prefs: UiPreferences = UiPreferences(),
    val notebooks: List<NotebookItem> = emptyList(),
    val activeNotebookId: String = "",
    val summaryText: String = "",
    val suggestedQuestions: List<String> = emptyList(),
    val sources: List<SourceItem> = emptyList(),
    val notes: List<NoteItem> = emptyList(),
    val messages: List<ChatMessageItem> = emptyList(),
    val artifacts: List<StudioArtifactItem> = emptyList(),
    val studioThoughts: Map<String, List<ThoughtStepItem>> = emptyMap(),
    val studioPreviews: Map<String, PreviewState> = emptyMap(),
    val activeArtifact: StudioArtifactItem? = null,
    val activeSourcePreview: SourceItem? = null,
    val pdfPreview: PdfPreviewTarget? = null,
    val activeCitation: CitationDetail? = null,
    val studioFullscreen: Boolean = false,
    val showSettings: Boolean = false,
    val providerPresets: List<ProviderPresetItem> = emptyList(),
    val modelProfiles: List<ModelProfileItem> = emptyList(),
    val mediaRouting: MediaRoutingConfig = MediaRoutingConfig(),
    val statusBanner: String = "",
    val isSendingChat: Boolean = false,
    val showCreateNotebook: Boolean = false,
    val indexingSourceIds: Set<String> = emptySet(),
)

class AppViewModel(application: Application) : AndroidViewModel(application) {
    val core = CoreClient(application.applicationContext)

    private val _ui = MutableStateFlow(AppUiState())
    val ui: StateFlow<AppUiState> = _ui.asStateFlow()

    init {
        viewModelScope.launch {
            val ok = core.open()
            _ui.update { it.copy(coreReady = ok) }
            if (!ok) {
                _ui.update {
                    it.copy(
                        statusBanner = L10n.t("app.native_core_missing", it.prefs.isEnglish),
                    )
                }
                return@launch
            }
            loadPreferences()
            loadProviderPresets()
            loadModelProfiles()
            loadMediaRouting()
            loadInitialNotebook()
            launch {
                core.events.collect { ev ->
                    when (ev.event) {
                        "source.status", "studio.status", "chat.finished", "notebook.summary_updated" -> refreshAll()
                        "studio.thought" -> {
                            val aid = ev.data.optString("artifact_id")
                            val kind = ev.data.optString("kind", "stage")
                            if (aid.isNotEmpty() && kind != "retry") {
                                val seq = ev.data.optInt("seq", 0)
                                val item = ThoughtStepItem(
                                    seq = seq,
                                    kind = kind,
                                    title = ev.data.optString("title"),
                                    body = ev.data.optString("body"),
                                )
                                _ui.update { st ->
                                    val cur = st.studioThoughts[aid].orEmpty().toMutableList()
                                    val idx = cur.indexOfFirst { it.seq == seq && seq > 0 }
                                    if (idx >= 0) cur[idx] = item else cur.add(item)
                                    st.copy(studioThoughts = st.studioThoughts + (aid to cur))
                                }
                            }
                        }
                        "studio.thought_delta" -> {
                            val aid = ev.data.optString("artifact_id")
                            val seq = ev.data.optInt("seq", 0)
                            val delta = ev.data.optString("delta")
                            if (aid.isNotEmpty() && seq > 0 && delta.isNotEmpty()) {
                                _ui.update { st ->
                                    val cur = st.studioThoughts[aid].orEmpty().toMutableList()
                                    val idx = cur.indexOfFirst { it.seq == seq }
                                    if (idx >= 0) {
                                        cur[idx] = cur[idx].copy(body = cur[idx].body + delta)
                                        st.copy(studioThoughts = st.studioThoughts + (aid to cur))
                                    } else st
                                }
                            }
                        }
                        "studio.preview" -> {
                            val aid = ev.data.optString("artifact_id")
                            val pl = ev.data.optJSONObject("payload")
                            if (aid.isNotEmpty() && pl != null) {
                                val pv = PreviewState(ev.data.optString("stage"), ev.data.optBoolean("validated", false), pl)
                                _ui.update { st -> st.copy(studioPreviews = st.studioPreviews + (aid to pv)) }
                            }
                        }
                        "ai.activity" -> {
                            val active = ev.data.optBoolean("active", false)
                            val title = ev.data.optString("title")
                            val body = ev.data.optString("body")
                            if (ev.data.optString("scope") == "source_index") {
                                // Shown as an in-place spinner on the source, not a banner.
                                val sid = ev.data.optString("source_id")
                                _ui.update { st ->
                                    st.copy(indexingSourceIds = if (active) st.indexingSourceIds + sid else st.indexingSourceIds - sid)
                                }
                                if (!active) refreshAll()
                                return@collect
                            }
                            _ui.update { st ->
                                st.copy(
                                    statusBanner = if (active && title.isNotEmpty()) {
                                        if (body.isNotEmpty()) "$title · $body" else title
                                    } else if (!active) "" else st.statusBanner
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    override fun onCleared() {
        super.onCleared()
        core.close()
    }

    fun dismissBanner() = _ui.update { it.copy(statusBanner = "") }
    fun setShowSettings(show: Boolean) = _ui.update { it.copy(showSettings = show) }
    fun openArtifact(art: StudioArtifactItem?) {
        _ui.update { it.copy(activeArtifact = art, studioFullscreen = if (art == null) false else it.studioFullscreen) }
        if (art != null) {
            viewModelScope.launch {
                if (art.status == "PENDING" || art.status == "RUNNING") {
                    runCatching {
                        val pv = core.call("studio.preview.get", JSONObject().put("artifact_id", art.id))
                        val pl = pv.optJSONObject("payload")
                        if (pl != null) {
                            _ui.update { st ->
                                if (st.studioPreviews.containsKey(art.id)) st
                                else st.copy(studioPreviews = st.studioPreviews + (art.id to PreviewState(pv.optString("stage"), pv.optBoolean("validated", false), pl)))
                            }
                        }
                    }
                }
                runCatching {
                    val res = core.call("studio.get", JSONObject().put("artifact_id", art.id))
                    val raw = res.optJSONArray("thoughts") ?: return@runCatching
                    val list = mutableListOf<ThoughtStepItem>()
                    for (i in 0 until raw.length()) {
                        val o = raw.optJSONObject(i) ?: continue
                        val k = o.optString("kind", "stage")
                        if (k == "retry") continue
                        list.add(ThoughtStepItem(seq = o.optInt("seq", i + 1), kind = k, title = o.optString("title"), body = o.optString("body")))
                    }
                    _ui.update { it.copy(studioThoughts = it.studioThoughts + (art.id to list)) }
                }
            }
        }
    }
    fun toggleStudioFullscreen() = _ui.update { it.copy(studioFullscreen = !it.studioFullscreen) }
    fun openSourcePreview(src: SourceItem?) {
        _ui.update { it.copy(activeSourcePreview = src) }
        // Guide is generated automatically; kick it off if it never ran for this ready source.
        if (src != null && src.status == "READY" && src.guideSummaryMd.isEmpty() && src.id !in _ui.value.indexingSourceIds) {
            viewModelScope.launch {
                _ui.update { it.copy(indexingSourceIds = it.indexingSourceIds + src.id) }
                runCatching { core.call("source.generate_guide", JSONObject().put("source_id", src.id)) }
                _ui.update { it.copy(indexingSourceIds = it.indexingSourceIds - src.id) }
                refreshAll()
            }
        }
    }
    fun closePdfPreview() = _ui.update { it.copy(pdfPreview = null) }
    fun dismissCitation() = _ui.update { it.copy(activeCitation = null) }

    fun openPdfAtPage(source: SourceItem, page: Int) {
        if (source.blobPath.isNotEmpty() && File(source.blobPath).exists()) {
            _ui.update {
                it.copy(
                    pdfPreview = PdfPreviewTarget(
                        sourceId = source.id,
                        title = source.title,
                        filePath = source.blobPath,
                        initialPage = page.coerceAtLeast(1),
                    ),
                )
            }
        }
    }

    private suspend fun loadPreferences() {
        runCatching {
            val res = core.call("settings.get", JSONObject().put("key", "ui.preferences"))
            val prefs = UiPreferences.fromJson(res.optJSONObject("value"))
            core.english = prefs.isEnglish
            _ui.update { it.copy(prefs = prefs) }
        }
    }

    fun updatePreferences(newPrefs: UiPreferences) {
        val prevEn = _ui.value.prefs.isEnglish
        core.english = newPrefs.isEnglish
        _ui.update { it.copy(prefs = newPrefs) }
        viewModelScope.launch {
            runCatching {
                core.call("settings.set", JSONObject().put("key", "ui.preferences").put("value", newPrefs.toJson()))
                val nid = _ui.value.activeNotebookId
                if (prevEn != newPrefs.isEnglish && nid.isNotEmpty()) {
                    core.call(
                        "i18n.translate_notebook_content",
                        JSONObject().put("notebook_id", nid).put("target_lang", newPrefs.targetLangCode),
                    )
                    refreshAll()
                }
                _ui.update { it.copy(statusBanner = L10n.t("settings.saved", newPrefs.isEnglish)) }
            }
        }
    }

    private suspend fun loadInitialNotebook() {
        runCatching {
            val list = core.call("notebook.list")
            val arr = list.optJSONArray("notebooks") ?: JSONArray()
            if (arr.length() > 0) {
                _ui.update { it.copy(activeNotebookId = arr.getJSONObject(0).optString("id")) }
                refreshAll()
            } else {
                _ui.update { it.copy(activeNotebookId = "", notebooks = emptyList(), showCreateNotebook = true) }
            }
        }
    }

    fun selectNotebook(id: String) {
        _ui.update { it.copy(activeNotebookId = id, activeArtifact = null, activeSourcePreview = null, studioFullscreen = false) }
        viewModelScope.launch { refreshAll() }
    }

    fun createNotebook() = _ui.update { it.copy(showCreateNotebook = true) }

    fun dismissCreateNotebook() = _ui.update { it.copy(showCreateNotebook = false) }

    /** Creates a notebook from the dialog; [files] are optional sources imported right after creation. */
    fun confirmCreateNotebook(title: String, files: List<Uri>) {
        val name = title.trim()
        if (name.isEmpty()) return
        _ui.update { it.copy(showCreateNotebook = false) }
        viewModelScope.launch {
            runCatching {
                val created = core.call("notebook.create", JSONObject().put("title", name))
                val id = created.optString("id")
                if (id.isNotEmpty()) {
                    _ui.update { it.copy(activeNotebookId = id, activeArtifact = null, activeSourcePreview = null) }
                    refreshAll()
                    if (files.isNotEmpty()) {
                        val paths = JSONArray()
                        for (u in files) paths.put(copyUriToCache(u).absolutePath)
                        core.call("source.add_files", JSONObject().put("notebook_id", id).put("paths", paths))
                        refreshAll()
                    }
                }
            }.onFailure { reportError(it) }
        }
    }

    fun renameNotebook(title: String) {
        val nid = _ui.value.activeNotebookId
        val name = title.trim()
        if (nid.isEmpty() || name.isEmpty()) return
        if (_ui.value.notebooks.firstOrNull { it.id == nid }?.title == name) return
        viewModelScope.launch {
            runCatching {
                core.call("notebook.rename", JSONObject().put("notebook_id", nid).put("title", name))
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    private suspend fun copyUriToCache(uri: Uri): File {
        val ctx = getApplication<Application>()
        return withContext(Dispatchers.IO) {
            var name = "imported_file"
            ctx.contentResolver.query(uri, null, null, null, null)?.use { c ->
                val idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0 && c.moveToFirst()) name = c.getString(idx) ?: name
            }
            val dir = File(ctx.cacheDir, "imports").apply { mkdirs() }
            val out = File(dir, "${System.currentTimeMillis()}_$name")
            ctx.contentResolver.openInputStream(uri)?.use { input ->
                out.outputStream().use { output -> input.copyTo(output) }
            }
            out
        }
    }

    suspend fun refreshAll() {
        val nid = _ui.value.activeNotebookId
        if (nid.isEmpty() || !_ui.value.coreReady) return
        val en = _ui.value.prefs.isEnglish
        runCatching {
            val nbRes = core.call("notebook.list")
            val nbArr = nbRes.optJSONArray("notebooks") ?: JSONArray()
            val notebooks = (0 until nbArr.length()).map { i ->
                val o = nbArr.getJSONObject(i)
                val sqArr = o.optJSONArray("suggested_questions") ?: JSONArray()
                val sq = (0 until sqArr.length()).mapNotNull { j -> sqArr.optString(j).takeIf { it.isNotEmpty() } }
                val summaryMd = if (o.isNull("summary_md")) "" else o.optString("summary_md", "")
                NotebookItem(o.optString("id"), o.optString("title"), summaryMd, sq)
            }
            val curNb = notebooks.firstOrNull { it.id == nid }

            val srcRes = core.call("source.list", JSONObject().put("notebook_id", nid))
            val srcArr = srcRes.optJSONArray("sources") ?: JSONArray()
            val sources = (0 until srcArr.length()).map { i ->
                val o = srcArr.getJSONObject(i)
                val tArr = o.optJSONArray("guide_topics") ?: JSONArray()
                val topics = (0 until tArr.length()).mapNotNull { j -> tArr.optString(j).takeIf { it.isNotEmpty() } }
                SourceItem(
                    id = o.optString("id"),
                    title = o.optString("title"),
                    kind = o.optString("kind", "txt"),
                    status = o.optString("status", "READY"),
                    isSelected = o.optBoolean("is_selected", true),
                    blobPath = o.optString("blob_path", ""),
                    pageCount = o.optInt("page_count", 0),
                    guideSummaryMd = o.optString("guide_summary_md", ""),
                    guideTopics = topics,
                    recommendKnowledgeGraph = o.optBoolean("recommend_knowledge_graph", false) || topics.size >= 3,
                )
            }

            val noteRes = core.call("note.list", JSONObject().put("notebook_id", nid))
            val noteArr = noteRes.optJSONArray("notes") ?: JSONArray()
            val notes = (0 until noteArr.length()).map { i ->
                val o = noteArr.getJSONObject(i)
                NoteItem(o.optString("id"), o.optString("title"), o.optString("body_md"))
            }

            val chatRes = core.call("chat.history", JSONObject().put("notebook_id", nid))
            val msgArr = chatRes.optJSONArray("messages") ?: JSONArray()
            val messages = (0 until msgArr.length()).map { i ->
                val m = msgArr.getJSONObject(i)
                val thArr = m.optJSONArray("thoughts") ?: JSONArray()
                val thoughts = (0 until thArr.length()).mapNotNull { j ->
                    val th = thArr.getJSONObject(j)
                    val k = th.optString("kind", "thought_stage")
                    if (k == "retry") null else ThoughtStepItem(
                        id = th.optString("step_id", "$i-$j"),
                        kind = k,
                        title = th.optString("title"),
                        detail = th.optString("detail"),
                    )
                }
                val citArr = m.optJSONArray("citations") ?: JSONArray()
                val citations = (0 until citArr.length()).map { j ->
                    val c = citArr.getJSONObject(j)
                    CitationRef(
                        no = c.optInt("citation_no", j + 1),
                        sourceTitle = c.optString("source_title"),
                        pageStart = c.optInt("page_start", 0),
                        quote = c.optString("quote"),
                    )
                }
                val attArr = m.optJSONArray("attachments") ?: JSONArray()
                val attachments = (0 until attArr.length()).map { j ->
                    val a = attArr.getJSONObject(j)
                    ChatAttachment(
                        a.optString("name"),
                        a.optString("path"),
                        kind = a.optString("kind", "image"),
                        url = a.optString("url").takeIf { it.isNotEmpty() },
                        bytes = a.optLong("bytes", 0),
                        id = a.optString("id"),
                    )
                }.filter { it.path.isNotEmpty() }
                ChatMessageItem(m.optString("id"), m.optString("role", "assistant"), m.optString("content"), thoughts, citations, attachments)
            }

            val artRes = core.call("studio.list", JSONObject().put("notebook_id", nid))
            val artArr = artRes.optJSONArray("artifacts") ?: JSONArray()
            // the core lists newest first; keep that order
            val artifacts = (0 until artArr.length()).map { i ->
                val a = artArr.getJSONObject(i)
                val cfg = a.optJSONObject("config") ?: JSONObject()
                val payload = a.optJSONObject("payload") ?: JSONObject()
                val refArr = a.optJSONArray("source_refs") ?: JSONArray()
                val refs = (0 until refArr.length()).map { j ->
                    val r = refArr.getJSONObject(j)
                    ArtifactSourceRef(r.optString("id"), r.optString("title"), r.optString("kind", "file"))
                }
                val srcCount = refs.size // only the sources the generation actually read
                val attArr = a.optJSONArray("attachments") ?: JSONArray()
                val atts = (0 until attArr.length()).map { attArr.getJSONObject(it) }
                val (imgAtts, fileAtts) = atts.partition { it.optString("mime").startsWith("image/") }
                StudioArtifactItem(
                    id = a.optString("id"),
                    artifactType = a.optString("artifact_type", "report"),
                    title = L10n.localizeDefaultArtifactTitle(a.optString("title"), en),
                    status = a.optString("status", "PENDING"),
                    stage = a.optString("stage", ""),
                    progress = a.optDouble("progress", 0.0).toFloat(),
                    promptTopic = cfg.optString("topic", "").trim(),
                    isKnowledgeGraph = cfg.optString("mode") == "knowledge_graph" || payload.optString("kind") == "knowledge_graph",
                    sourceRefs = refs,
                    sourceCount = srcCount,
                    imagePaths = imgAtts.map { it.optString("path") }.filter { java.io.File(it).exists() },
                    fileNames = fileAtts.map { it.optString("name") },
                    draftMarkdown = a.optString("draft", ""),
                    payload = payload,
                    config = cfg,
                    errorCode = a.optString("error_code", ""),
                    errorDetail = a.optString("error_detail", ""),
                    errorLog = a.optString("error_log", ""),
                    outputDir = a.optString("output_dir", ""),
                    progressState = a.optJSONObject("progress_state") ?: JSONObject(),
                    createdAt = a.optLong("created_at", 0L),
                )
            }

            _ui.update { st ->
                val refreshedOpenArt = st.activeArtifact?.id?.let { oid -> artifacts.firstOrNull { it.id == oid } }
                val refreshedOpenSrc = st.activeSourcePreview?.id?.let { sid -> sources.firstOrNull { it.id == sid } }
                st.copy(
                    notebooks = notebooks,
                    summaryText = curNb?.summaryMd?.ifEmpty { L10n.t("chat.empty_summary", en) } ?: L10n.t("chat.empty_summary", en),
                    suggestedQuestions = curNb?.suggestedQuestions ?: emptyList(),
                    sources = sources,
                    notes = notes,
                    messages = messages,
                    artifacts = artifacts,
                    activeArtifact = refreshedOpenArt,
                    activeSourcePreview = refreshedOpenSrc,
                    // previews are only meaningful while the run is in flight
                    studioPreviews = st.studioPreviews.filterKeys { pid ->
                        val a = artifacts.firstOrNull { it.id == pid }
                        a == null || a.status == "PENDING" || a.status == "RUNNING"
                    },
                )
            }
        }
    }

    fun importFileUri(uri: Uri) {
        viewModelScope.launch {
            runCatching {
                val ctx = getApplication<Application>()
                val displayName = withContext(Dispatchers.IO) {
                    var name = "imported_file"
                    ctx.contentResolver.query(uri, null, null, null, null)?.use { c ->
                        val idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (idx >= 0 && c.moveToFirst()) name = c.getString(idx) ?: name
                    }
                    name
                }
                val dst = withContext(Dispatchers.IO) {
                    val dir = File(ctx.cacheDir, "imports").apply { mkdirs() }
                    val out = File(dir, "${System.currentTimeMillis()}_$displayName")
                    ctx.contentResolver.openInputStream(uri)?.use { input ->
                        out.outputStream().use { output -> input.copyTo(output) }
                    }
                    out
                }
                val nid = _ui.value.activeNotebookId
                core.call("source.add_files", JSONObject().put("notebook_id", nid).put("paths", JSONArray().put(dst.absolutePath)))
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun addUrlSource(url: String) {
        if (url.isBlank()) return
        viewModelScope.launch {
            runCatching {
                core.call("source.add_url", JSONObject().put("notebook_id", _ui.value.activeNotebookId).put("url", url.trim()))
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun addTextSource(title: String, text: String) {
        if (text.isBlank()) return
        viewModelScope.launch {
            runCatching {
                core.call(
                    "source.add_text",
                    JSONObject()
                        .put("notebook_id", _ui.value.activeNotebookId)
                        .put("title", title.ifBlank { "pasted.txt" })
                        .put("text", text),
                )
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun createNote(title: String, body: String) {
        viewModelScope.launch {
            runCatching {
                core.call(
                    "note.save",
                    JSONObject()
                        .put("notebook_id", _ui.value.activeNotebookId)
                        .put("title", title.ifBlank { L10n.t("sources.create_note", _ui.value.prefs.isEnglish) })
                        .put("body_md", body),
                )
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun toggleSourceSelected(sourceId: String, selected: Boolean) {
        viewModelScope.launch {
            runCatching {
                core.call("source.set_selected", JSONObject().put("source_id", sourceId).put("selected", selected))
                refreshAll()
            }
        }
    }

    /** Deleting is the only thing allowed for a source that is not fully parsed yet. */
    fun removeSource(sourceId: String) {
        viewModelScope.launch {
            runCatching {
                core.call("source.remove", JSONObject().put("source_id", sourceId))
                refreshAll()
            }
        }
    }

    fun sendChat(text: String, images: JSONArray? = null) {
        val clean = text.trim()
        if (clean.isEmpty() && images == null) return
        viewModelScope.launch {
            _ui.update { it.copy(isSendingChat = true) }
            runCatching {
                val params = JSONObject().put("notebook_id", _ui.value.activeNotebookId)
                if (clean.isNotEmpty()) params.put("text", clean)
                params.putAttachments(images)
                core.call("chat.send", params)
                refreshAll()
            }.onFailure { reportError(it) }
            _ui.update { it.copy(isSendingChat = false) }
        }
    }

    fun openCitation(messageId: String, citationNo: Int) {
        viewModelScope.launch {
            runCatching {
                val d = core.call(
                    "chat.citation_detail",
                    JSONObject().put("message_id", messageId).put("citation_no", citationNo),
                )
                val title = d.optString("source_title")
                val page = d.optInt("page_start", 0)
                val raw = d.optString("content")
                val removed = d.optBoolean("source_removed", false)
                val matchedSource = _ui.value.sources.firstOrNull { it.title == title }
                val unclear = isQuoteUnclear(raw)
                val sanitized = sanitizeQuote(raw)

                // If unclear on a PDF, directly open the native PDF viewer at the cited page
                if (unclear && !removed && matchedSource != null && title.endsWith(".pdf", ignoreCase = true) && page > 0) {
                    openPdfAtPage(matchedSource, page)
                }

                _ui.update {
                    it.copy(
                        activeCitation = CitationDetail(
                            no = citationNo,
                            sourceId = matchedSource?.id ?: "",
                            sourceTitle = title,
                            sectionPath = d.optString("section_path"),
                            pageStart = page,
                            content = sanitized,
                            isUnclear = unclear,
                            removed = removed,
                        ),
                    )
                }
            }.onFailure { reportError(it) }
        }
    }

    fun createStudio(kind: String, topic: String = "", sourceId: String? = null, detail: String = "standard", images: JSONArray? = null) {
        viewModelScope.launch {
            runCatching {
                val en = _ui.value.prefs.isEnglish
                val actualType = if (kind == "knowledge_graph") "mindmap" else kind
                val cfg = JSONObject().put("lang", _ui.value.prefs.defaultStudioLang.ifEmpty { _ui.value.prefs.targetLangCode })
                if (topic.isNotBlank()) cfg.put("topic", topic)
                if (kind == "mindmap" || kind == "knowledge_graph") cfg.put("detail", detail)
                if (kind == "report") cfg.put("depth", detail)
                if (kind == "lecture") {
                    // detail carries "mode|detail|confirm" from the lecture dialog
                    val parts = detail.split("|")
                    cfg.put("mode", parts.getOrNull(0)?.ifEmpty { null } ?: "general")
                    cfg.put("detail", parts.getOrNull(1)?.ifEmpty { null } ?: "auto")
                    cfg.put("reviewOutline", parts.getOrNull(2) == "1")
                }
                if (kind == "slides") {
                    cfg.put("pages", 8)
                    cfg.put("reviewOutline", true)
                }
                if (kind == "knowledge_graph") {
                    cfg.put("mode", "knowledge_graph")
                    if (!sourceId.isNullOrEmpty()) cfg.put("source_id", sourceId)
                }
                val params = JSONObject()
                    .put("notebook_id", _ui.value.activeNotebookId)
                    .put("artifact_type", actualType)
                    .put("config", cfg)
                if (kind == "knowledge_graph" && topic.isNotBlank()) {
                    params.put("title", L10n.t("app.knowledge_graph", en).format(topic))
                }
                params.putAttachments(images)
                core.call("studio.create", params)
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun createConceptLink(kind: String, concept: String) {
        val en = _ui.value.prefs.isEnglish
        val prompt = if (kind == "quiz") {
            L10n.t("app.focused_quiz_on_concept", en).format(concept)
        } else {
            L10n.t("app.detailed_mind_map_expanding_concept", en).format(concept)
        }
        createStudio(kind, topic = prompt)
    }

    fun retryArtifact(artifactId: String) {
        viewModelScope.launch {
            runCatching {
                core.call("studio.retry", JSONObject().put("artifact_id", artifactId))
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    /** [fresh]: same requirements and the artifact's own sources, from scratch; otherwise the
     *  notebook's currently selected sources. */
    fun regenerateArtifact(art: StudioArtifactItem, fresh: Boolean = false) {
        viewModelScope.launch {
            runCatching {
                val params = JSONObject().put("artifact_id", art.id).put("use_selected_sources", !fresh)
                if (!fresh) params.put("config", art.config)
                core.call("studio.regenerate", params)
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    /** Adds a one-off attachment of a message to the notebook's sources. */
    fun promoteAttachment(attachmentId: String) {
        viewModelScope.launch {
            runCatching {
                core.call(
                    "attachment.promote",
                    JSONObject().put("notebook_id", _ui.value.activeNotebookId).put("attachment_id", attachmentId),
                )
                refreshAll()
                _ui.update { it.copy(statusBanner = L10n.t("ctx.promoted", it.prefs.isEnglish)) }
            }.onFailure { reportError(it) }
        }
    }

    fun reviseDraft(artifactId: String, instruction: String, currentDraft: String, images: JSONArray?, onRevised: (String) -> Unit) {
        val instr = instruction.trim()
        if (instr.isEmpty()) return
        viewModelScope.launch {
            runCatching {
                val params = JSONObject()
                    .put("artifact_id", artifactId)
                    .put("instruction", instr)
                    .put("draft", currentDraft)
                params.putAttachments(images)
                val res = core.call("studio.revise_draft", params)
                val newMd = res.optString("draft")
                if (newMd.isNotEmpty()) onRevised(newMd)
                refreshAll()
            }
        }
    }

    /** Splits a mind-map / knowledge-graph node into finer points; the core may decline atomic nodes. */
    fun refineNode(artifactId: String, nodeId: String, label: String) {
        val en = _ui.value.prefs.isEnglish
        _ui.update { it.copy(statusBanner = "${L10n.t("kg.refining", en)}${L10n.t("studio.colon", en)}$label") }
        viewModelScope.launch {
            runCatching {
                val res = core.call(
                    "studio.refine_node",
                    JSONObject().put("artifact_id", artifactId).put("node_id", nodeId),
                )
                val msg = if (res.optBoolean("refined")) {
                    L10n.t("kg.refined_fmt", en).replace("{0}", label).replace("{1}", res.optInt("added").toString())
                } else {
                    L10n.t("kg.refine_declined", en).replace("{0}", label) +
                        res.optString("reason").takeIf { it.isNotBlank() }?.let { " — $it" }.orEmpty()
                }
                _ui.update { it.copy(statusBanner = msg) }
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun approveDraft(artifactId: String, markdown: String) {
        viewModelScope.launch {
            runCatching {
                core.call("studio.approve_draft", JSONObject().put("artifact_id", artifactId).put("draft", markdown))
                refreshAll()
            }.onFailure { reportError(it) }
        }
    }

    fun submitFeedback(artifactId: String, feedback: Int) {
        viewModelScope.launch {
            runCatching {
                core.call("studio.set_feedback", JSONObject().put("artifact_id", artifactId).put("feedback", feedback))
            }
        }
    }

    private fun loadProviderPresets() {
        viewModelScope.launch {
            runCatching {
                val res = core.call("profile.presets")
                val arr = res.optJSONArray("presets") ?: return@runCatching
                val list = buildList {
                    for (i in 0 until arr.length()) {
                        val o = arr.optJSONObject(i) ?: continue
                        val hdrObj = o.optJSONObject("extra_headers")
                        val hdrMap = mutableMapOf<String, String>()
                        if (hdrObj != null) {
                            val keys = hdrObj.keys()
                            while (keys.hasNext()) {
                                val k = keys.next()
                                hdrMap[k] = hdrObj.optString(k, "")
                            }
                        }
                        add(
                            ProviderPresetItem(
                                id = o.optString("id"),
                                name = o.optString("name"),
                                nameEn = o.optString("name_en"),
                                protocol = o.optString("protocol", "chat_completion"),
                                baseUrl = o.optString("base_url"),
                                defaultModel = o.optString("default_model"),
                                contextWindow = o.optInt("context_window", 32000),
                                maxOutputTokens = o.optInt("max_output_tokens", 8000),
                                supportsVision = o.optBoolean("supports_vision", false),
                                supportsAudio = o.optBoolean("supports_audio", false),
                                supportsVideo = o.optBoolean("supports_video", false),
                                extraHeaders = hdrMap,
                            )
                        )
                    }
                }
                _ui.update { it.copy(providerPresets = list) }
            }
        }
    }

    fun loadModelProfiles() {
        viewModelScope.launch {
            runCatching {
                val res = core.call("profile.list")
                val arr = res.optJSONArray("profiles") ?: return@runCatching
                val list = buildList {
                    for (i in 0 until arr.length()) {
                        val o = arr.optJSONObject(i) ?: continue
                        val caps = o.optJSONObject("capabilities")
                        val hdrObj = o.optJSONObject("extra_headers")
                        val hdrMap = mutableMapOf<String, String>()
                        if (hdrObj != null) {
                            val keys = hdrObj.keys()
                            while (keys.hasNext()) {
                                val k = keys.next()
                                hdrMap[k] = hdrObj.optString(k, "")
                            }
                        }
                        add(
                            ModelProfileItem(
                                id = o.optString("id"),
                                displayName = o.optString("display_name"),
                                protocol = o.optString("protocol", "chat_completion"),
                                baseUrl = o.optString("base_url"),
                                modelId = o.optString("model_id"),
                                keychainRef = o.optString("keychain_ref"),
                                hasSecret = o.optBoolean("has_secret", false),
                                isActive = o.optBoolean("is_active", false),
                                allowInsecureHttp = o.optBoolean("allow_insecure_http", false),
                                contextWindow = o.optInt("context_window", 32000),
                                maxOutputTokens = o.optInt("max_output_tokens", 8000),
                                maxParallel = o.optInt("max_parallel", 4),
                                supportsVision = caps?.optBoolean("supports_vision", false) ?: false,
                                supportsAudio = caps?.optBoolean("supports_audio", false) ?: false,
                                supportsVideo = caps?.optBoolean("supports_video", false) ?: false,
                                extraHeaders = hdrMap,
                                eligible = o.optBoolean("eligible", true),
                                unsupportedReason = o.optString("unsupported_reason").takeIf { it.isNotEmpty() && it != "null" },
                                probePending = o.optBoolean("probe_pending", false),
                                capsSummary = capsSummary(caps, _ui.value.prefs.isEnglish),
                            )
                        )
                    }
                }
                _ui.update { it.copy(modelProfiles = list) }
            }
        }
    }

    private fun loadMediaRouting() {
        viewModelScope.launch {
            runCatching {
                val res = core.call("settings.get", JSONObject().put("key", "media_processing"))
                val mv = res.optJSONObject("value") ?: return@runCatching
                _ui.update {
                    it.copy(
                        mediaRouting = MediaRoutingConfig(
                            audioVideoEnabled = mv.optBoolean("audio_video_enabled", true),
                            visionProfileId = mv.optString("vision_profile_id", ""),
                            audioProfileId = mv.optString("audio_profile_id", ""),
                            videoProfileId = mv.optString("video_profile_id", ""),
                        )
                    )
                }
            }
        }
    }

    fun saveMediaRouting(cfg: MediaRoutingConfig) {
        _ui.update { it.copy(mediaRouting = cfg) }
        viewModelScope.launch {
            runCatching {
                val v = JSONObject().put("audio_video_enabled", cfg.audioVideoEnabled)
                if (cfg.visionProfileId.isNotEmpty()) v.put("vision_profile_id", cfg.visionProfileId)
                if (cfg.audioProfileId.isNotEmpty()) v.put("audio_profile_id", cfg.audioProfileId)
                if (cfg.videoProfileId.isNotEmpty()) v.put("video_profile_id", cfg.videoProfileId)
                core.call("settings.set", JSONObject().put("key", "media_processing").put("value", v))
            }.onFailure { reportError(it) }
        }
    }

    fun saveModelProfile(
        profileId: String? = null,
        protocol: String,
        baseUrl: String,
        modelId: String,
        apiKey: String,
        extraHeaders: Map<String, String> = emptyMap(),
        contextWindow: Int = 32000,
        maxOutputTokens: Int = 8000,
        activateChat: Boolean = true,
        allowInsecureHttp: Boolean = false,
        supportsVision: Boolean = false,
        supportsAudio: Boolean = false,
        supportsVideo: Boolean = false,
        maxParallel: Int = 4,
    ) {
        viewModelScope.launch {
            runCatching {
                val body = JSONObject()
                    .put("display_name", "$protocol / $modelId")
                    .put("max_parallel", maxParallel.coerceIn(1, 64))
                    .put("protocol", protocol)
                    .put("base_url", baseUrl.trim())
                    .put("model_id", modelId.trim())
                    .put("context_window", contextWindow)
                    .put("max_output_tokens", maxOutputTokens)
                    .put("allow_insecure_http", allowInsecureHttp)
                    .put("supports_vision", supportsVision)
                    .put("supports_audio", supportsAudio)
                    .put("supports_video", supportsVideo)
                    .put("extra_headers", JSONObject(extraHeaders))
                    .put("activate", activateChat)
                if (!profileId.isNullOrEmpty()) body.put("id", profileId)
                // the core probes tool calling and structured output with this key before saving
                if (apiKey.isNotEmpty()) body.put("api_key", apiKey)
                val en = _ui.value.prefs.isEnglish
                _ui.update { it.copy(statusBanner = L10n.t("model.probing", en)) }
                val prof = core.call("profile.upsert", body)
                val newId = prof.optString("id")
                val kref = prof.optString("keychain_ref")
                if (kref.isNotEmpty() && apiKey.isNotEmpty()) {
                    core.persistSecret(kref, apiKey)
                }
                if (activateChat && newId.isNotEmpty()) {
                    core.call("profile.activate", JSONObject().put("profile_id", newId))
                }
                loadModelProfiles()
                _ui.update { it.copy(statusBanner = L10n.t("model.saved_checked", it.prefs.isEnglish)) }
            }.onFailure { reportError(it) }
        }
    }

    fun deleteModelProfile(profileId: String) {
        viewModelScope.launch {
            runCatching {
                core.call("profile.delete", JSONObject().put("profile_id", profileId))
                loadModelProfiles()
            }.onFailure { reportError(it) }
        }
    }

    fun testModelProfile(profileId: String) {
        viewModelScope.launch {
            runCatching {
                val res = core.call("profile.test", JSONObject().put("profile_id", profileId))
                val reply = res.optString("reply", "OK")
                _ui.update {
                    it.copy(
                        statusBanner = L10n.t("app.connected", it.prefs.isEnglish).format(reply)
                    )
                }
            }.onFailure { reportError(it) }
            loadModelProfiles()
        }
    }

    fun clearModelKey(keychainRef: String) {
        viewModelScope.launch {
            runCatching {
                core.call("secret.clear", JSONObject().put("keychain_ref", keychainRef))
                loadModelProfiles()
                _ui.update {
                    it.copy(
                        statusBanner = L10n.t("model.api_key_cleared", it.prefs.isEnglish)
                    )
                }
            }.onFailure { reportError(it) }
        }
    }

    fun downloadEmbeddingModel() {
        viewModelScope.launch {
            runCatching { core.call("embedding.download") }.onFailure { reportError(it) }
        }
    }

    private fun reportError(err: Throwable) {
        val en = _ui.value.prefs.isEnglish
        val msg = if (err is CoreCallException && err.code == "CAPABILITY_MISSING" && !err.message.isNullOrEmpty()) {
            // a refused model says exactly what it lacks
            String.format(L10n.t("model.refused", en), err.message)
        } else if (err is CoreCallException) {
            L10n.advice(err.code, en) ?: err.message
        } else {
            err.message ?: L10n.t("studio.failed", en)
        }
        _ui.update { it.copy(statusBanner = msg) }
    }

    companion object {
        /** "工具调用 ✓ · 结构化输出 ✓ · 识图" for the model list. */
        fun capsSummary(caps: JSONObject?, en: Boolean): String {
            if (caps == null) return ""
            if (caps.optBoolean("embedding_only", false)) return L10n.t("model.embedding_only", en)
            if (caps.isNull("probed_at") || !caps.has("probed_at")) return L10n.t("model.probe_pending", en)
            val parts = mutableListOf(
                L10n.t("model.cap_tools", en) + if (caps.optBoolean("tools")) " ✓" else " ✗",
                L10n.t("model.cap_structured", en) +
                    if (caps.optBoolean("json_schema") || caps.optBoolean("json_mode")) " ✓" else " ✗",
            )
            if (caps.optBoolean("supports_vision")) parts += L10n.t("app.vision", en)
            if (caps.optBoolean("supports_audio")) parts += L10n.t("model.audio", en)
            if (caps.optBoolean("supports_video")) parts += L10n.t("model.video", en)
            return parts.joinToString(" · ")
        }

        private val dotLeaderRegex = Regex("[·.…•\u2024\u2026]{3,}")
        private val spacedDotRegex = Regex("(\\s*[.·]\\s*){4,}")
        private val multiSpaceRegex = Regex("\\s{2,}")

        fun sanitizeQuote(raw: String): String =
            raw.replace(dotLeaderRegex, " ")
                .replace(spacedDotRegex, " ")
                .replace(multiSpaceRegex, " ")
                .trim()

        fun isQuoteUnclear(raw: String): Boolean {
            if (raw.isBlank()) return true
            var meaningful = 0
            var noise = 0
            for (ch in raw) {
                if (ch.isLetterOrDigit() || ch.code in 0x4E00..0x9FFF) meaningful++
                else if (ch in setOf('.', '·', '…', '•', '_', '-')) noise++
            }
            if (meaningful < 22) return true
            val total = meaningful + noise
            return total > 0 && noise.toDouble() / total > 0.32
        }
    }
}
