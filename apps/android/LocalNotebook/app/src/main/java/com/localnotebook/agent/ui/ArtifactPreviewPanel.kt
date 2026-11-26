package com.localnotebook.agent.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.localnotebook.agent.core.L10n
import com.localnotebook.agent.core.PreviewState
import com.localnotebook.agent.core.StudioArtifactItem
import com.localnotebook.agent.core.ThoughtStepItem
import org.json.JSONArray
import org.json.JSONObject

private const val MAX_REASONING_CHARS = 8000

/** Execution-process panel: stage/tool cards plus collapsed-by-default reasoning cards. */
@Composable
fun ThoughtsPanel(thoughts: List<ThoughtStepItem>, running: Boolean, en: Boolean) {
    var expanded by remember { mutableStateOf(setOf<Int>()) }
    var showFull by remember { mutableStateOf(setOf<Int>()) }
    val listState = rememberLazyListState()
    LaunchedEffect(thoughts.size) {
        // Follow new cards only while the user is already at the bottom.
        val info = listState.layoutInfo
        val last = info.visibleItemsInfo.lastOrNull()
        val atBottom = last == null || last.index >= info.totalItemsCount - 2
        if (atBottom && thoughts.isNotEmpty() && running) listState.scrollToItem(maxOf(0, info.totalItemsCount))
    }
    LazyColumn(state = listState, modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (running) {
            item {
                Text(
                    L10n.t("art.ai_workflow_in_progress", en),
                    style = MaterialTheme.typography.titleSmall,
                )
            }
        }
        if (thoughts.isEmpty()) {
            item {
                Text(
                    L10n.t("art.waiting_for_ai_workflow_steps", en),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            if (!running && thoughts.none { it.kind == "reasoning" }) {
                item {
                    Text(
                        L10n.t("studio.no_reasoning", en),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            items(thoughts) { step ->
                val isReasoning = step.kind == "reasoning"
                val dotColor = when (step.kind) {
                    "tool" -> Color(0xFF78D9EC)
                    "reasoning" -> Color(0xFFA8C7FA)
                    else -> Color(0xFF81C995)
                }
                val isOpen = !isReasoning || step.seq in expanded
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(modifier = Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                            modifier = Modifier.fillMaxWidth().then(
                                if (isReasoning) Modifier.clickable {
                                    expanded = if (step.seq in expanded) expanded - step.seq else expanded + step.seq
                                } else Modifier,
                            ),
                        ) {
                            Box(modifier = Modifier.size(7.dp).background(dotColor, RoundedCornerShape(4.dp)))
                            Text(step.title, fontWeight = FontWeight.SemiBold, fontSize = 12.sp, modifier = Modifier.weight(1f))
                            if (isReasoning) {
                                Icon(if (isOpen) Icons.Default.ExpandLess else Icons.Default.ExpandMore, contentDescription = null, modifier = Modifier.size(16.dp))
                            }
                        }
                        if (isOpen && step.body.isNotEmpty()) {
                            val full = step.seq in showFull
                            val capped = isReasoning && !full && step.body.length > MAX_REASONING_CHARS
                            val text = if (capped) L10n.t("studio.truncated_head", en) + "\n" + step.body.takeLast(MAX_REASONING_CHARS) else step.body
                            Text(text, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            if (capped) {
                                TextButton(onClick = { showFull = showFull + step.seq }) {
                                    Text(L10n.t("studio.show_full", en), fontSize = 12.sp)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** Read-only rendering of an unvalidated in-progress payload. */
@Composable
fun ArtifactPreviewPanel(artifact: StudioArtifactItem, preview: PreviewState, en: Boolean) {
    val payload = preview.payload
    val isMindmap = preview.stage == "mindmap_passes" || artifact.artifactType == "mindmap" || artifact.isKnowledgeGraph
    if (isMindmap && payload.optJSONArray("nodes") != null) {
        MindmapTreeView(
            payload = payload,
            isKnowledgeGraph = artifact.isKnowledgeGraph,
            en = en,
            onQuizConcept = {},
            onExpandMindmap = {},
            onAskConcept = {},
            readOnly = true,
        )
        return
    }
    val lines = remember(payload) { previewLines(preview.stage, artifact.artifactType, payload) }
    LazyColumn(modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (lines == null) {
            item {
                Text(
                    payload.toString(2).let { if (it.length > 4000) it.take(4000) + "…" else it },
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            items(lines) { l ->
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(modifier = Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Text(l.first, fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
                        for (d in l.second) Text(d, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

private const val MAX_ITEMS = 400
private const val MAX_BODY = 700

private fun clip(s: String, n: Int) = if (s.length <= n) s else s.take(n) + "…"

private fun JSONArray?.objs(): List<JSONObject> {
    if (this == null) return emptyList()
    return (0 until minOf(length(), MAX_ITEMS)).mapNotNull { optJSONObject(it) }
}

private fun itemText(a: JSONArray?): List<String> {
    if (a == null) return emptyList()
    val out = ArrayList<String>()
    for (i in 0 until a.length()) {
        val v = a.opt(i)
        val t = when (v) {
            is String -> v
            is JSONObject -> listOf("text", "label", "title", "heading", "value", "content").map { v.optString(it) }.firstOrNull { it.isNotEmpty() } ?: ""
            else -> ""
        }
        if (t.isNotEmpty()) out.add("• $t")
    }
    return out
}

/** Returns (title, detail lines) cards, or null to fall back to truncated JSON. */
private fun previewLines(stage: String, type: String, p: JSONObject): List<Pair<String, List<String>>>? {
    val out = ArrayList<Pair<String, List<String>>>()
    try {
        val title = p.optString("title")
        if (stage == "lecture_passes" || type == "lecture" || p.has("outline")) {
            val outline = p.optJSONObject("outline") ?: return null
            val done = p.optJSONObject("done") ?: JSONObject()
            if (outline.optString("title").isNotEmpty()) out.add(outline.optString("title") to emptyList())
            outline.optJSONArray("sections").objs().forEachIndexed { i, sec ->
                val content = done.optJSONObject("s${i + 1}")
                if (content == null) {
                    out.add("${i + 1}. ${sec.optString("title")}" to emptyList())
                } else {
                    val d = content.optJSONArray("slides").objs().mapIndexed { n, sl ->
                        val narr = sl.optJSONArray("narration").objs().map { it.optString("text") }.filter { it.isNotEmpty() }.joinToString(" ")
                        "${n + 1}. ${sl.optString("title")}" + if (narr.isNotEmpty()) "\n" + clip(narr, MAX_BODY) else ""
                    }
                    out.add("✓ ${i + 1}. ${sec.optString("title")}" to d)
                }
            }
            return out.ifEmpty { null }
        }
        val kind = if (type in setOf("quiz", "slides", "report", "flashcards", "infographic", "audio")) type else when {
            p.has("questions") -> "quiz"
            p.has("slides") -> "slides"
            p.has("cards") -> "flashcards"
            p.has("blocks") -> "infographic"
            p.has("turns") -> "audio"
            p.has("sections") -> "report"
            else -> ""
        }
        if (title.isNotEmpty()) out.add(title to listOfNotNull(p.optString("subtitle").takeIf { it.isNotEmpty() }))
        when (kind) {
            "quiz" -> p.optJSONArray("questions").objs().forEachIndexed { i, q ->
                val opts = q.optJSONArray("options")
                val d = ArrayList<String>()
                if (opts != null) for (j in 0 until opts.length()) {
                    val o = opts.opt(j)
                    val t = if (o is JSONObject) o.optString("text", o.optString("label")) else o?.toString().orEmpty()
                    d.add("${'A' + minOf(j, 25)}. $t")
                }
                out.add("${i + 1}. ${q.optString("prompt")}" to d)
            }
            "slides" -> p.optJSONArray("slides").objs().forEachIndexed { i, s ->
                val d = ArrayList<String>()
                s.optString("subtitle").takeIf { it.isNotEmpty() }?.let(d::add)
                d.addAll(itemText(s.optJSONArray("bullets")))
                s.optString("callout").takeIf { it.isNotEmpty() }?.let(d::add)
                out.add("${i + 1}. ${s.optString("title").ifEmpty { s.optString("layout") }}" to d)
            }
            "report" -> p.optJSONArray("sections").objs().forEach { s ->
                val d = ArrayList<String>()
                s.optString("body_md").takeIf { it.isNotEmpty() }?.let { d.add(clip(it, MAX_BODY)) }
                d.addAll(itemText(s.optJSONArray("key_takeaways")))
                out.add(s.optString("heading") to d)
            }
            "flashcards" -> p.optJSONArray("cards").objs().forEachIndexed { i, c ->
                out.add("${i + 1}. ${c.optString("front")}" to listOfNotNull(c.optString("back").takeIf { it.isNotEmpty() }))
            }
            "infographic" -> p.optJSONArray("blocks").objs().forEach { b ->
                val m = b.optString("metric")
                val h = b.optString("heading")
                out.add((if (m.isNotEmpty()) "$h  ·  $m" else h) to listOfNotNull(b.optString("summary").takeIf { it.isNotEmpty() }))
            }
            "audio" -> p.optJSONArray("turns").objs().forEach { t ->
                val sp = t.optString("speaker")
                out.add(((if (sp.isNotEmpty()) "$sp: " else "") + t.optString("text")) to emptyList())
            }
            else -> return null
        }
    } catch (_: Exception) {
        return null
    }
    return out.ifEmpty { null }
}
