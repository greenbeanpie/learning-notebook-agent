package com.localnotebook.agent.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.localnotebook.agent.core.AppUiState
import com.localnotebook.agent.core.L10n
import com.localnotebook.agent.core.StudioArtifactItem
import com.localnotebook.agent.core.ThoughtStepItem

/** "3 分钟前" style age of a timestamp (unix ms). */
internal fun timeAgo(createdMs: Long, nowMs: Long, en: Boolean): String {
    if (createdMs <= 0) return ""
    val secs = ((nowMs - createdMs) / 1000).coerceAtLeast(0)
    if (secs < 60) return L10n.t("time.just_now", en)
    val mins = secs / 60
    if (mins < 60) return String.format(L10n.t("time.minutes", en), mins)
    val hours = mins / 60
    if (hours < 24) return String.format(L10n.t("time.hours", en), hours)
    val days = hours / 24
    if (days < 30) return String.format(L10n.t("time.days", en), days)
    if (days < 365) return String.format(L10n.t("time.months", en), days / 30)
    return String.format(L10n.t("time.years", en), days / 365)
}

private fun artifactStatusLabel(art: StudioArtifactItem, en: Boolean): String = when (art.status) {
    "PENDING" -> L10n.t("studio.status_pending", en)
    "RUNNING" -> when (art.stage) {
        "COLLECT" -> L10n.t("studio.status_collect", en)
        "PLAN" -> L10n.t("studio.status_plan", en)
        "GENERATE" -> L10n.t("studio.status_generate", en)
        "RENDER" -> L10n.t("studio.status_render", en)
        else -> L10n.t("studio.status_processing", en)
    }
    "DRAFT_REVIEW" -> L10n.t("studio.status_draft", en)
    "READY" -> L10n.t("studio.status_ready", en)
    "FAILED" -> L10n.t("studio.status_failed", en)
    "CANCELLED" -> L10n.t("studio.status_cancelled", en)
    else -> art.status
}

@Composable
fun StudioScreen(
    state: AppUiState,
    onCreateStudio: (kind: String, topic: String, detail: String, images: org.json.JSONArray?, refArtifactIds: List<String>) -> Unit,
    onAddToSources: (String) -> Unit = {},
    onOpenArtifact: (StudioArtifactItem?) -> Unit,
    onToggleFullscreen: () -> Unit,
    onRetryArtifact: (String) -> Unit,
    onRegenerateArtifact: (StudioArtifactItem, Boolean) -> Unit,
    onApproveDraft: (String, String) -> Unit,
    onReviseDraft: (String, String, String, org.json.JSONArray?, (String) -> Unit) -> Unit = { _, _, _, _, _ -> },
    onFeedback: (String, Int) -> Unit,
    onQuizConcept: (String) -> Unit,
    onExpandMindmap: (String) -> Unit,
    onAskConcept: (String) -> Unit,
    onAddToSources: () -> Unit = {},
    modifier: Modifier = Modifier,
    onRefineNode: (artifactId: String, nodeId: String, label: String) -> Unit = { _, _, _ -> },
    core: com.localnotebook.agent.core.CoreClient? = null,
) {
    val en = state.prefs.isEnglish
    // re-evaluated every 30 s so the "x minutes ago" labels stay current
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            kotlinx.coroutines.delay(30_000)
            value = System.currentTimeMillis()
        }
    }
    val openArt = state.activeArtifact
    var pendingKind by remember { mutableStateOf<String?>(null) }

    if (openArt != null) {
        ArtifactViewerScreen(
            artifact = openArt,
            thoughts = state.studioThoughts[openArt.id].orEmpty(),
            preview = state.studioPreviews[openArt.id],
            isFullscreen = state.studioFullscreen,
            en = en,
            onClose = { onOpenArtifact(null) },
            onToggleFullscreen = onToggleFullscreen,
            onRetry = { onRetryArtifact(openArt.id) },
            onRegenerate = { onRegenerateArtifact(openArt, false) },
            onRegenerateFresh = { onRegenerateArtifact(openArt, true) },
            onApproveDraft = { md -> onApproveDraft(openArt.id, md) },
            onReviseDraft = { instr, cur, im, cb -> onReviseDraft(openArt.id, instr, cur, im, cb) },
            onFeedback = { v -> onFeedback(openArt.id, v) },
            onQuizConcept = onQuizConcept,
            onExpandMindmap = onExpandMindmap,
            onAskConcept = onAskConcept,
            onAddToSources = { onAddToSources(openArt.id) },
            modifier = modifier,
            onRefineNode = { nodeId, label -> onRefineNode(openArt.id, nodeId, label) },
            core = core,
        )
        return
    }

    val tiles = listOf(
        Triple("audio", L10n.t("studio.audio", en), Color(0xFFA8C7FA)),
        Triple("slides", L10n.t("studio.slides", en), Color(0xFFFDD663)),
        Triple("mindmap", L10n.t("studio.mindmap", en), Color(0xFFC58AF9)),
        Triple("report", L10n.t("studio.report", en), Color(0xFF81C995)),
        Triple("flashcards", L10n.t("studio.flashcards", en), Color(0xFFF6AEA9)),
        Triple("quiz", L10n.t("studio.quiz", en), Color(0xFF78D9EC)),
        Triple("infographic", L10n.t("studio.infographic", en), Color(0xFFFCAD70)),
        Triple("knowledge_graph", L10n.t("studio.knowledge_graph", en), Color(0xFF64B5F6)),
        Triple("lecture", L10n.t("studio.lecture", en), Color(0xFFF28B82)),
    )

    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // 4x2 Symmetrical 8-tile Studio Grid
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (rowIdx in 0 until (tiles.size + 1) / 2) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    for (colIdx in 0 until 2) {
                        if (rowIdx * 2 + colIdx >= tiles.size) {
                            Spacer(Modifier.weight(1f))
                            continue
                        }
                        val (kind, label, tint) = tiles[rowIdx * 2 + colIdx]
                        Surface(
                            color = MaterialTheme.colorScheme.surfaceVariant,
                            shape = RoundedCornerShape(14.dp),
                            modifier = Modifier
                                .weight(1f)
                                .height(52.dp)
                                .clickable { pendingKind = kind },
                        ) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                modifier = Modifier.padding(horizontal = 12.dp),
                            ) {
                                Icon(Icons.Default.AutoAwesome, contentDescription = null, tint = tint, modifier = Modifier.size(16.dp))
                                Text(label, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                            }
                        }
                    }
                }
            }
        }

        LazyColumn(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(state.artifacts, key = { it.id }) { art ->
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onOpenArtifact(art) },
                ) {
                    Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(art.title, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        val age = timeAgo(art.createdAt, now, en)
                        Text(
                            "${L10n.t("studio.${if (art.isKnowledgeGraph) "knowledge_graph" else art.artifactType}", en)} · ${artifactStatusLabel(art, en)}${if (age.isNotEmpty()) " · $age" else ""}",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (art.status == "PENDING" || art.status == "RUNNING") {
                            LinearProgressIndicator(progress = { art.progress.coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
                        }
                    }
                }
            }
        }
    }

    pendingKind?.let { kind ->
        var topicInput by remember(kind) { mutableStateOf("") }
        var detailLevel by remember(kind) { mutableStateOf(if (kind == "lecture") "auto" else "standard") }
        var lectureMode by remember(kind) { mutableStateOf("general") }
        var confirmScope by remember(kind) { mutableStateOf(true) }
        var selectedRefIds by remember(kind) { mutableStateOf<Set<String>>(emptySet()) }
        val cfgImages = rememberImageAttachState()
        AlertDialog(
            onDismissRequest = { pendingKind = null },
            title = { Text(L10n.t("studio.configure", en).format(L10n.t("studio.$kind", en))) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    OutlinedTextField(
                        value = topicInput,
                        onValueChange = { topicInput = it },
                        label = { Text(L10n.t("studio.cfg.topic_header", en)) },
                        placeholder = { Text(L10n.t("studio.e_g_focus_on_chapter", en)) },
                        minLines = 4,
                        maxLines = 6,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        AttachImageButton(cfgImages, en)
                        Text(L10n.t("img.cfg_hint", en), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    AttachStrip(cfgImages, en)
                    if (kind == "mindmap" || kind == "knowledge_graph") {
                        Text(L10n.t("studio.cfg.detail_header", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf(
                                "concise" to (L10n.t("studio.concise", en)),
                                "standard" to (L10n.t("studio.cfg.qty_standard", en)),
                                "detailed" to (L10n.t("studio.cfg.level_detailed", en)),
                            ).forEach { (code, lbl) ->
                                FilterChip(
                                    selected = detailLevel == code,
                                    onClick = { detailLevel = code },
                                    label = { Text(lbl) },
                                )
                            }
                        }
                    } else if (kind == "lecture") {
                        Text(L10n.t("studio.cfg.lecture_mode_header", en), style = MaterialTheme.typography.labelMedium)
                        Row(modifier = Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("general", "preview", "review", "problem").forEach { code ->
                                FilterChip(selected = lectureMode == code, onClick = { lectureMode = code }, label = { Text(L10n.t("studio.cfg.lecture_mode_$code", en)) })
                            }
                        }
                        Text(L10n.t("studio.detail", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("auto" to "studio.cfg.level_auto", "brief" to "", "standard" to "", "detailed" to "").forEach { (code, key) ->
                                val lbl = when (code) { "auto" -> L10n.t(key, en); "brief" -> L10n.t("studio.brief", en); "standard" -> L10n.t("studio.cfg.qty_standard", en); else -> L10n.t("studio.detailed", en) }
                                FilterChip(selected = detailLevel == code, onClick = { detailLevel = code }, label = { Text(lbl) })
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(checked = confirmScope, onCheckedChange = { confirmScope = it })
                            Text(L10n.t("studio.cfg.btn_lecture_confirm_scope", en))
                        }
                        Text(L10n.t("studio.cfg.lecture_hint", en), style = MaterialTheme.typography.bodySmall)
                    } else if (kind == "report") {
                        Text(L10n.t("studio.report_depth", en), style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf(
                                "brief" to (L10n.t("studio.brief_2", en)),
                                "standard" to (L10n.t("studio.cfg.qty_standard", en)),
                                "deep" to (L10n.t("studio.deep", en)),
                            ).forEach { (code, lbl) ->
                                FilterChip(
                                    selected = detailLevel == code,
                                    onClick = { detailLevel = code },
                                    label = { Text(lbl) },
                                )
                            }
                        }
                    }
                    val readyArtifacts = state.artifacts.filter { it.status == "READY" }
                    if (readyArtifacts.isNotEmpty()) {
                        Text(L10n.t("studio.cfg.ref_artifacts", en), style = MaterialTheme.typography.labelMedium)
                        Row(modifier = Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            readyArtifacts.forEach { art ->
                                val picked = art.id in selectedRefIds
                                FilterChip(
                                    selected = picked,
                                    onClick = { selectedRefIds = if (picked) selectedRefIds - art.id else selectedRefIds + art.id },
                                    label = { Text(art.title, maxLines = 1) },
                                )
                            }
                        }
                    }
                }
            },
            confirmButton = {
                Button(onClick = {
                    onCreateStudio(kind, topicInput.trim(), if (kind == "lecture") "$lectureMode|$detailLevel|${if (confirmScope) 1 else 0}" else detailLevel, cfgImages.toJson(), selectedRefIds.toList())
                    pendingKind = null
                }) {
                    Text(L10n.t("studio.generate", en))
                }
            },
            dismissButton = {
                TextButton(onClick = { pendingKind = null }) {
                    Text(L10n.t("common.cancel", en))
                }
            },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ArtifactViewerScreen(
    artifact: StudioArtifactItem,
    thoughts: List<ThoughtStepItem>,
    preview: com.localnotebook.agent.core.PreviewState?,
    isFullscreen: Boolean,
    en: Boolean,
    onClose: () -> Unit,
    onToggleFullscreen: () -> Unit,
    onRetry: () -> Unit,
    onRegenerate: () -> Unit,
    onRegenerateFresh: () -> Unit,
    onApproveDraft: (String) -> Unit,
    onReviseDraft: (String, String, org.json.JSONArray?, (String) -> Unit) -> Unit,
    onFeedback: (Int) -> Unit,
    onQuizConcept: (String) -> Unit,
    onExpandMindmap: (String) -> Unit,
    onAskConcept: (String) -> Unit,
    modifier: Modifier = Modifier,
    onRefineNode: (nodeId: String, label: String) -> Unit = { _, _ -> },
    core: com.localnotebook.agent.core.CoreClient? = null,
) {
    var showPromptDialog by remember { mutableStateOf(false) }
    var confirmRegenerate by remember { mutableStateOf(false) }
    var showDiagLog by remember { mutableStateOf(false) }
    var showThoughts by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current

    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // Header with title, "View prompt and N sources" pill, Fullscreen & Close
        Row(verticalAlignment = Alignment.Top, modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(artifact.title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val pillText = if (artifact.sourceCount > 0) {
                    L10n.t("studio.view_prompt_and_sources", en).format(artifact.sourceCount)
                } else {
                    L10n.t("studio.view_prompt", en)
                }
                OutlinedButton(
                    onClick = { showPromptDialog = true },
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 2.dp),
                    modifier = Modifier.height(30.dp),
                ) {
                    Text(pillText, fontSize = 12.sp)
                }
            }
            TextButton(onClick = { showThoughts = !showThoughts }) {
                Text(L10n.t("studio.process", en), fontSize = 12.sp)
            }
            if (artifact.status == "READY") {
                TextButton(onClick = onAddToSources) {
                    Text(L10n.t("studio.add_to_sources", en), fontSize = 12.sp)
                }
            }
            if (artifact.status == "READY" || artifact.status == "FAILED" || artifact.status == "CANCELLED") {
                IconButton(onClick = { confirmRegenerate = true }) {
                    Icon(Icons.Default.Refresh, contentDescription = L10n.t("studio.regenerate", en))
                }
            }
            IconButton(onClick = onToggleFullscreen) {
                Icon(if (isFullscreen) Icons.Default.FullscreenExit else Icons.Default.Fullscreen, contentDescription = null)
            }
            IconButton(onClick = onClose) {
                Icon(Icons.Default.Close, contentDescription = null)
            }
        }

        HorizontalDivider()

        Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
            when {
                artifact.status == "PENDING" || artifact.status == "RUNNING" || showThoughts -> {
                    val running = artifact.status == "PENDING" || artifact.status == "RUNNING"
                    val hasPreview = running && preview != null
                    var tab by remember { mutableStateOf(0) }
                    Column(modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (running) {
                            LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                        }
                        if (hasPreview) {
                            TabRow(selectedTabIndex = if (tab == 1) 1 else 0) {
                                Tab(selected = tab == 0, onClick = { tab = 0 }, text = { Text(L10n.t("studio.process", en), fontSize = 12.sp) })
                                Tab(selected = tab == 1, onClick = { tab = 1 }, text = { Text(L10n.t("studio.preview_tab", en), fontSize = 12.sp) })
                            }
                        }
                        Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
                            if (hasPreview && tab == 1 && preview != null) {
                                ArtifactPreviewPanel(artifact, preview, en)
                            } else {
                                ThoughtsPanel(thoughts, running, en)
                            }
                        }
                    }
                }

                artifact.status == "FAILED" -> {
                    val msg = L10n.advice(artifact.errorCode, en) ?: L10n.t("studio.failed", en)
                    val logText = artifact.errorLog.ifEmpty { "ArtifactId: ${artifact.id}\nErrorCode: ${artifact.errorCode}\nDetail: ${artifact.errorDetail}" }
                    Column(
                        modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(msg, color = Color(0xFFFFA08C), fontWeight = FontWeight.Medium)
                        Button(onClick = onRetry) { Text(L10n.t("studio.retry", en)) }
                        TextButton(onClick = { showDiagLog = !showDiagLog }) {
                            Text(L10n.t("studio.diag_log", en))
                        }
                        if (showDiagLog) {
                            OutlinedButton(onClick = { clipboard.setText(AnnotatedString(logText)) }) {
                                Text(L10n.t("studio.copy_log", en))
                            }
                            Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = RoundedCornerShape(8.dp)) {
                                Text(logText, fontFamily = FontFamily.Monospace, fontSize = 11.sp, modifier = Modifier.padding(10.dp))
                            }
                        }
                    }
                }

                artifact.status == "DRAFT_REVIEW" -> {
                    var draft by remember(artifact.id) { mutableStateOf(artifact.draftMarkdown) }
                    var reviseInput by remember(artifact.id) { mutableStateOf("") }
                    var revising by remember(artifact.id) { mutableStateOf(false) }
                    val reviseImages = rememberImageAttachState()
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxSize()) {
                        OutlinedTextField(value = draft, onValueChange = { draft = it }, modifier = Modifier.weight(1f).fillMaxWidth())
                        AttachStrip(reviseImages, en)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            AttachImageButton(reviseImages, en, enabled = !revising)
                            OutlinedTextField(
                                value = reviseInput,
                                onValueChange = { reviseInput = it },
                                placeholder = { Text(L10n.t("studio.revise_outline_with_prompt", en), fontSize = 12.sp) },
                                singleLine = true,
                                modifier = Modifier.weight(1f),
                            )
                            OutlinedButton(
                                enabled = !revising && (reviseInput.isNotBlank() || reviseImages.hasItems),
                                onClick = {
                                    revising = true
                                    val instr = reviseInput.ifBlank { L10n.t("img.revise_default", en) }
                                    onReviseDraft(instr, draft, reviseImages.toJson()) { updated ->
                                        draft = updated
                                        reviseInput = ""
                                        reviseImages.clear()
                                        revising = false
                                    }
                                },
                            ) {
                                Text(if (revising) (L10n.t("studio.revising", en)) else (L10n.t("studio.revise", en)), fontSize = 12.sp)
                            }
                        }
                        Button(onClick = { onApproveDraft(draft) }, modifier = Modifier.fillMaxWidth()) {
                            Text(L10n.t("studio.approve_draft", en))
                        }
                    }
                }

                artifact.artifactType == "mindmap" -> {
                    MindmapTreeView(
                        payload = artifact.payload,
                        isKnowledgeGraph = artifact.isKnowledgeGraph,
                        en = en,
                        onQuizConcept = onQuizConcept,
                        onExpandMindmap = onExpandMindmap,
                        onAskConcept = onAskConcept,
                        onRefineNode = onRefineNode,
                    )
                }

                artifact.artifactType == "slides" -> {
                    SlideDeckView(
                        payload = artifact.payload,
                        en = en,
                    )
                }

                artifact.artifactType == "lecture" -> LecturePlayerView(artifact, core, en)
                artifact.artifactType == "report" -> ReportReadableView(artifact.payload, en)
                artifact.artifactType == "quiz" -> QuizReadableView(artifact.payload, en)
                artifact.artifactType == "flashcards" -> FlashcardsReadableView(artifact.payload, en)
                artifact.artifactType == "audio" -> AudioScriptReadableView(artifact.payload)
                artifact.artifactType == "infographic" -> InfographicReadableView(artifact.payload)

                else -> {
                    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                        Text(artifact.payload.toString(2), style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
        }

        if (artifact.status == "READY") {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { onFeedback(1) }) {
                    Icon(Icons.Default.ThumbUp, contentDescription = null, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(L10n.t("studio.good", en), fontSize = 12.sp)
                }
                OutlinedButton(onClick = { onFeedback(-1) }) {
                    Icon(Icons.Default.ThumbDown, contentDescription = null, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(L10n.t("studio.bad", en), fontSize = 12.sp)
                }
            }
        }
    }

    if (confirmRegenerate) {
        AlertDialog(
            onDismissRequest = { confirmRegenerate = false },
            title = { Text(L10n.t("studio.regenerate", en)) },
            text = { Text(L10n.t("studio.regenerate_confirm", en).format(artifact.title)) },
            confirmButton = {
                TextButton(onClick = { confirmRegenerate = false; onRegenerateFresh() }) { Text(L10n.t("studio.regenerate", en)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmRegenerate = false }) { Text(L10n.t("common.cancel", en)) }
            },
        )
    }

    if (showPromptDialog) {
        AlertDialog(
            onDismissRequest = { showPromptDialog = false },
            title = {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                    Text(L10n.t("studio.prompt_header", en), modifier = Modifier.weight(1f))
                    IconButton(onClick = { showPromptDialog = false; onRegenerate() }) {
                        Icon(Icons.Default.Refresh, contentDescription = L10n.t("studio.regenerate", en))
                    }
                    IconButton(onClick = {
                        clipboard.setText(AnnotatedString(artifact.promptTopic.ifEmpty { artifact.title }))
                    }) {
                        Icon(Icons.Default.ContentCopy, contentDescription = L10n.t("studio.copy_prompt", en))
                    }
                }
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(artifact.promptTopic.ifEmpty { L10n.t("studio.prompt_empty", en) })
                    if (artifact.imagePaths.isNotEmpty() || artifact.fileNames.isNotEmpty()) {
                        Text(L10n.t("studio.attachments_header", en), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        AttachmentGallery(artifact.imagePaths)
                        for (name in artifact.fileNames) {
                            Text("📄 $name", fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    Text(L10n.t("studio.sources_header", en), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    if (artifact.sourceRefs.isEmpty()) {
                        Text(L10n.t("studio.sources_empty", en), fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (src in artifact.sourceRefs) {
                            Surface(
                                color = MaterialTheme.colorScheme.surfaceVariant,
                                shape = RoundedCornerShape(14.dp),
                                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                            ) {
                                Text(
                                    "[${src.kind.uppercase().take(4)}] ${src.title}",
                                    fontSize = 12.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                                )
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { showPromptDialog = false }) { Text(L10n.t("common.close", en)) }
            },
        )
    }
}
