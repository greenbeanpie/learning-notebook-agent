package com.localnotebook.agent.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
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

@Composable
fun StudioScreen(
    state: AppUiState,
    onCreateStudio: (kind: String, topic: String, detail: String) -> Unit,
    onOpenArtifact: (StudioArtifactItem?) -> Unit,
    onToggleFullscreen: () -> Unit,
    onRetryArtifact: (String) -> Unit,
    onRegenerateArtifact: (StudioArtifactItem) -> Unit,
    onApproveDraft: (String, String) -> Unit,
    onReviseDraft: (String, String, String, (String) -> Unit) -> Unit = { _, _, _, _ -> },
    onFeedback: (String, Int) -> Unit,
    onQuizConcept: (String) -> Unit,
    onExpandMindmap: (String) -> Unit,
    onAskConcept: (String) -> Unit,
    modifier: Modifier = Modifier,
    onRefineNode: (artifactId: String, nodeId: String, label: String) -> Unit = { _, _, _ -> },
    core: com.localnotebook.agent.core.CoreClient? = null,
) {
    val en = state.prefs.isEnglish
    val openArt = state.activeArtifact
    var pendingKind by remember { mutableStateOf<String?>(null) }

    if (openArt != null) {
        ArtifactViewerScreen(
            artifact = openArt,
            thoughts = state.studioThoughts[openArt.id].orEmpty(),
            isFullscreen = state.studioFullscreen,
            en = en,
            onClose = { onOpenArtifact(null) },
            onToggleFullscreen = onToggleFullscreen,
            onRetry = { onRetryArtifact(openArt.id) },
            onRegenerate = { onRegenerateArtifact(openArt) },
            onApproveDraft = { md -> onApproveDraft(openArt.id, md) },
            onReviseDraft = { instr, cur, cb -> onReviseDraft(openArt.id, instr, cur, cb) },
            onFeedback = { v -> onFeedback(openArt.id, v) },
            onQuizConcept = onQuizConcept,
            onExpandMindmap = onExpandMindmap,
            onAskConcept = onAskConcept,
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
                        Text(
                            "${L10n.t("studio.${if (art.isKnowledgeGraph) "knowledge_graph" else art.artifactType}", en)} · ${art.status}",
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
        AlertDialog(
            onDismissRequest = { pendingKind = null },
            title = { Text(if (en) "Configure ${L10n.t("studio.$kind", en)}" else "生成配置 · ${L10n.t("studio.$kind", en)}") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    OutlinedTextField(
                        value = topicInput,
                        onValueChange = { topicInput = it },
                        label = { Text(if (en) "Prompt / Focus & Scope (Optional)" else "生成提示与范围要求（可选）") },
                        placeholder = { Text(if (en) "e.g. Focus on Chapter 3 key equations..." else "例如：仅关注第三节氧化还原反应与典型方程式…") },
                        minLines = 4,
                        maxLines = 6,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    if (kind == "mindmap" || kind == "knowledge_graph") {
                        Text(if (en) "Detail Level" else "详尽程度", style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf(
                                "concise" to (if (en) "Concise" else "简洁"),
                                "standard" to (if (en) "Standard" else "标准"),
                                "detailed" to (if (en) "Detailed" else "详尽"),
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
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("general", "preview", "review", "practice").forEach { code ->
                                FilterChip(selected = lectureMode == code, onClick = { lectureMode = code }, label = { Text(L10n.t("studio.cfg.lecture_mode_$code", en)) })
                            }
                        }
                        Text(if (en) "Detail" else "详细程度", style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf("auto" to "studio.cfg.level_auto", "brief" to "", "standard" to "", "detailed" to "").forEach { (code, key) ->
                                val lbl = when (code) { "auto" -> L10n.t(key, en); "brief" -> if (en) "Brief" else "简要"; "standard" -> if (en) "Standard" else "标准"; else -> if (en) "Detailed" else "详细" }
                                FilterChip(selected = detailLevel == code, onClick = { detailLevel = code }, label = { Text(lbl) })
                            }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(checked = confirmScope, onCheckedChange = { confirmScope = it })
                            Text(L10n.t("studio.cfg.btn_lecture_confirm_scope", en))
                        }
                        Text(L10n.t("studio.cfg.lecture_hint", en), style = MaterialTheme.typography.bodySmall)
                    } else if (kind == "report") {
                        Text(if (en) "Report Depth" else "报告深度", style = MaterialTheme.typography.labelMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            listOf(
                                "brief" to (if (en) "Brief" else "简明"),
                                "standard" to (if (en) "Standard" else "标准"),
                                "deep" to (if (en) "Deep" else "深度"),
                            ).forEach { (code, lbl) ->
                                FilterChip(
                                    selected = detailLevel == code,
                                    onClick = { detailLevel = code },
                                    label = { Text(lbl) },
                                )
                            }
                        }
                    }
                }
            },
            confirmButton = {
                Button(onClick = {
                    onCreateStudio(kind, topicInput.trim(), if (kind == "lecture") "$lectureMode|$detailLevel|${if (confirmScope) 1 else 0}" else detailLevel)
                    pendingKind = null
                }) {
                    Text(if (en) "Generate" else "开始生成")
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
    isFullscreen: Boolean,
    en: Boolean,
    onClose: () -> Unit,
    onToggleFullscreen: () -> Unit,
    onRetry: () -> Unit,
    onRegenerate: () -> Unit,
    onApproveDraft: (String) -> Unit,
    onReviseDraft: (String, String, (String) -> Unit) -> Unit,
    onFeedback: (Int) -> Unit,
    onQuizConcept: (String) -> Unit,
    onExpandMindmap: (String) -> Unit,
    onAskConcept: (String) -> Unit,
    modifier: Modifier = Modifier,
    onRefineNode: (nodeId: String, label: String) -> Unit = { _, _ -> },
    core: com.localnotebook.agent.core.CoreClient? = null,
) {
    var showPromptDialog by remember { mutableStateOf(false) }
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
                    if (en) "View prompt and ${artifact.sourceCount} sources" else "查看提示和 ${artifact.sourceCount} 个来源"
                } else {
                    if (en) "View prompt" else "查看提示"
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
                Text(if (en) "Thoughts" else "思考过程", fontSize = 12.sp)
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
                    Column(
                        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        if (artifact.status == "PENDING" || artifact.status == "RUNNING") {
                            LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                            Text(
                                if (en) "AI workflow in progress..." else "AI 正在执行工具调用与思考…",
                                style = MaterialTheme.typography.titleSmall,
                            )
                        }
                        if (thoughts.isEmpty()) {
                            Text(
                                if (en) "Waiting for AI workflow steps..." else "正在等待 AI 工作步骤与思考记录…",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        } else {
                            thoughts.forEach { step ->
                                val dotColor = when (step.kind) {
                                    "tool" -> Color(0xFF78D9EC)
                                    "reasoning" -> Color(0xFFA8C7FA)
                                    else -> Color(0xFF81C995)
                                }
                                Surface(
                                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
                                    shape = RoundedCornerShape(10.dp),
                                    modifier = Modifier.fillMaxWidth(),
                                ) {
                                    Column(modifier = Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                            Box(modifier = Modifier.size(7.dp).background(dotColor, RoundedCornerShape(4.dp)))
                                            Text(step.title, fontWeight = FontWeight.SemiBold, fontSize = 12.sp)
                                        }
                                        if (step.body.isNotEmpty()) {
                                            Text(step.body, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        }
                                    }
                                }
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
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxSize()) {
                        OutlinedTextField(value = draft, onValueChange = { draft = it }, modifier = Modifier.weight(1f).fillMaxWidth())
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                            OutlinedTextField(
                                value = reviseInput,
                                onValueChange = { reviseInput = it },
                                placeholder = { Text(if (en) "Revise outline with prompt..." else "输入提示词调整提纲…", fontSize = 12.sp) },
                                singleLine = true,
                                modifier = Modifier.weight(1f),
                            )
                            OutlinedButton(
                                enabled = !revising && reviseInput.isNotBlank(),
                                onClick = {
                                    revising = true
                                    onReviseDraft(reviseInput, draft) { updated ->
                                        draft = updated
                                        reviseInput = ""
                                        revising = false
                                    }
                                },
                            ) {
                                Text(if (revising) (if (en) "..." else "调整中") else (if (en) "Revise" else "AI 调整"), fontSize = 12.sp)
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
                    Text(L10n.t("studio.sources_header", en), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
