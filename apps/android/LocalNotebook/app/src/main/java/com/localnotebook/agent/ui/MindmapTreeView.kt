package com.localnotebook.agent.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Remove
import androidx.compose.material.icons.filled.UnfoldLess
import androidx.compose.material.icons.filled.UnfoldMore
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.localnotebook.agent.core.L10n
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.roundToInt

private class MindNode(
    val id: String,
    val label: String,
    val importance: String,
    val summary: String,
    val widthDp: Float,
) {
    var depth: Int = 0
    val children = mutableListOf<MindNode>()
    var xDp: Float = 0f
    var yDp: Float = 0f
}

@Composable
fun MindmapTreeView(
    payload: JSONObject,
    isKnowledgeGraph: Boolean,
    en: Boolean,
    onQuizConcept: (String) -> Unit,
    onExpandMindmap: (String) -> Unit,
    onAskConcept: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val isKg = isKnowledgeGraph || payload.optString("kind") == "knowledge_graph"
    val roots = remember(payload) { buildTree(payload.optJSONArray("nodes") ?: JSONArray()) }

    if (roots.isEmpty()) {
        Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(if (en) "No mind map nodes." else "没有导图节点。", color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }

    var collapsedIds by remember(payload) { mutableStateOf(setOf<String>()) }
    var allCollapsed by remember(payload) { mutableStateOf(false) }
    var scale by remember { mutableFloatStateOf(1f) }
    var panX by remember { mutableFloatStateOf(0f) }
    var panY by remember { mutableFloatStateOf(0f) }
    var selectedNode by remember { mutableStateOf<MindNode?>(null) }

    val nodeH = 38f
    val rowGap = 14f
    val colGap = 62f
    val toggleSize = 24f
    val padX = 64f
    val padY = 28f

    val visibleNodes = remember(roots, collapsedIds) {
        val list = mutableListOf<MindNode>()
        var cursorY = padY
        fun layout(node: MindNode, x: Float): Float {
            node.xDp = x
            list.add(node)
            val isCol = node.id in collapsedIds
            if (node.children.isEmpty() || isCol) {
                node.yDp = cursorY
                cursorY += nodeH + rowGap
            } else {
                val childX = x + node.widthDp + colGap
                val firstY = layout(node.children.first(), childX)
                var lastY = firstY
                for (i in 1 until node.children.size) {
                    lastY = layout(node.children[i], childX)
                }
                node.yDp = (firstY + lastY) / 2f
            }
            return node.yDp
        }
        for (r in roots) {
            layout(r, padX)
            cursorY += rowGap
        }
        list
    }

    val canvasWidthDp = max(420f, (visibleNodes.maxOfOrNull { it.xDp + it.widthDp + 80f } ?: 420f)) * scale
    val canvasHeightDp = max(340f, (visibleNodes.maxOfOrNull { it.yDp + nodeH + 56f } ?: 340f)) * scale
    val density = LocalDensity.current

    Box(
        modifier = modifier
            .fillMaxSize()
            .clip(RoundedCornerShape(12.dp))
            .background(Color(0xFF1D1F24)),
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        scale = (scale * zoom).coerceIn(0.45f, 2.2f)
                        panX += pan.x
                        panY += pan.y
                    }
                },
        ) {
            Box(
                modifier = Modifier
                    .offset { IntOffset(panX.roundToInt(), panY.roundToInt()) }
                    .size(canvasWidthDp.dp, canvasHeightDp.dp),
            ) {
                Canvas(modifier = Modifier.fillMaxSize()) {
                    fun drawEdges(node: MindNode) {
                        if (node.id in collapsedIds || node.children.isEmpty()) return
                        val sx = with(density) { ((node.xDp + node.widthDp + toggleSize + 6f) * scale).dp.toPx() }
                        val sy = with(density) { ((node.yDp + nodeH / 2f) * scale).dp.toPx() }
                        val color = edgeColor(node.depth, isKg)
                        for (child in node.children) {
                            val ex = with(density) { (child.xDp * scale).dp.toPx() }
                            val ey = with(density) { ((child.yDp + nodeH / 2f) * scale).dp.toPx() }
                            val mx = (sx + ex) / 2f
                            val path = Path().apply {
                                moveTo(sx, sy)
                                cubicTo(mx, sy, mx, ey, ex, ey)
                            }
                            drawPath(path, color = color, style = Stroke(width = 1.6f * density.density))
                            drawEdges(child)
                        }
                    }
                    for (r in roots) drawEdges(r)
                }

                for (node in visibleNodes) {
                    val (bg, border) = nodeColors(node.depth, isKg)
                    val leftPx = with(density) { (node.xDp * scale).dp.toPx().roundToInt() }
                    val topPx = with(density) { (node.yDp * scale).dp.toPx().roundToInt() }
                    Surface(
                        color = bg,
                        shape = RoundedCornerShape(8.dp),
                        border = BorderStroke(1.dp, border),
                        modifier = Modifier
                            .offset { IntOffset(leftPx, topPx) }
                            .size((node.widthDp * scale).dp, (nodeH * scale).dp)
                            .clickable { selectedNode = node },
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(horizontal = 10.dp),
                        ) {
                            if (node.importance.isNotEmpty()) {
                                val dotColor = when (node.importance) {
                                    "core" -> Color(0xFFFDD663)
                                    "key" -> Color(0xFF8AB4F8)
                                    else -> Color(0xFF81C995)
                                }
                                Box(
                                    modifier = Modifier
                                        .size(7.dp)
                                        .clip(CircleShape)
                                        .background(dotColor),
                                )
                                Spacer(Modifier.width(6.dp))
                            }
                            Text(
                                text = node.label,
                                color = Color(0xFFEAECEF),
                                fontSize = (12.5f * scale).coerceIn(10f, 18f).sp,
                                fontWeight = if (node.depth == 0) FontWeight.SemiBold else FontWeight.Normal,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }

                    if (node.children.isNotEmpty()) {
                        val isCol = node.id in collapsedIds
                        val tLeftPx = with(density) { ((node.xDp + node.widthDp + 6f) * scale).dp.toPx().roundToInt() }
                        val tTopPx = with(density) { ((node.yDp + (nodeH - toggleSize) / 2f) * scale).dp.toPx().roundToInt() }
                        Surface(
                            color = Color(0xFF343844),
                            shape = CircleShape,
                            border = BorderStroke(1.dp, Color(0xFF565C70)),
                            modifier = Modifier
                                .offset { IntOffset(tLeftPx, tTopPx) }
                                .size((toggleSize * scale).dp)
                                .clickable {
                                    collapsedIds = if (isCol) collapsedIds - node.id else collapsedIds + node.id
                                },
                        ) {
                            Box(contentAlignment = Alignment.Center) {
                                Text(
                                    text = if (isCol) ">" else "<",
                                    color = Color(0xFFE0E4EE),
                                    fontSize = 11.sp,
                                    fontWeight = FontWeight.SemiBold,
                                )
                            }
                        }
                    }
                }
            }
        }

        // Left floating vertical controls (Expand/Collapse All, +, -)
        Column(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(start = 12.dp, top = 14.dp),
        ) {
            FilledTonalIconButton(
                onClick = {
                    allCollapsed = !allCollapsed
                    collapsedIds = if (allCollapsed) {
                        visibleNodes.filter { it.depth > 0 && it.children.isNotEmpty() }.map { it.id }.toSet()
                    } else {
                        emptySet()
                    }
                },
                modifier = Modifier.size(36.dp),
            ) {
                Icon(if (allCollapsed) Icons.Default.UnfoldMore else Icons.Default.UnfoldLess, contentDescription = null, modifier = Modifier.size(18.dp))
            }
            Surface(shape = RoundedCornerShape(18.dp), color = Color(0xE6262930), border = BorderStroke(1.dp, Color(0xFF424652))) {
                Column {
                    IconButton(onClick = { scale = (scale + 0.15f).coerceAtMost(2.0f) }, modifier = Modifier.size(36.dp)) {
                        Icon(Icons.Default.Add, contentDescription = null, tint = Color.White, modifier = Modifier.size(18.dp))
                    }
                    IconButton(onClick = { scale = (scale - 0.15f).coerceAtLeast(0.55f) }, modifier = Modifier.size(36.dp)) {
                        Icon(Icons.Default.Remove, contentDescription = null, tint = Color.White, modifier = Modifier.size(18.dp))
                    }
                }
            }
        }

        if (isKg) {
            Surface(
                color = Color(0xE0243848),
                shape = RoundedCornerShape(12.dp),
                border = BorderStroke(1.dp, Color(0xFF5C92C4)),
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(12.dp),
            ) {
                Text(
                    text = L10n.t("studio.knowledge_graph", en),
                    color = Color(0xFFC4E0FA),
                    fontSize = 11.5.sp,
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                )
            }
        }
    }

    // Node click linkage dialog (Quiz / Expand Mind Map / Ask in Chat)
    selectedNode?.let { node ->
        val cleanConcept = node.label.substringBefore('[').trim()
        AlertDialog(
            onDismissRequest = { selectedNode = null },
            title = { Text(cleanConcept) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (node.summary.isNotEmpty()) {
                        Text(node.summary, style = MaterialTheme.typography.bodyMedium)
                    }
                    Button(
                        onClick = { selectedNode = null; onQuizConcept(cleanConcept) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(L10n.t("kg.action_quiz", en)) }
                    OutlinedButton(
                        onClick = { selectedNode = null; onExpandMindmap(cleanConcept) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(L10n.t("kg.action_mindmap", en)) }
                    OutlinedButton(
                        onClick = { selectedNode = null; onAskConcept(cleanConcept) },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(L10n.t("kg.action_chat", en)) }
                }
            },
            confirmButton = {
                TextButton(onClick = { selectedNode = null }) { Text(L10n.t("common.close", en)) }
            },
        )
    }
}

private fun buildTree(arr: JSONArray): List<MindNode> {
    val map = LinkedHashMap<String, MindNode>()
    val parents = mutableMapOf<String, String>()
    for (i in 0 until arr.length()) {
        val o = arr.optJSONObject(i) ?: continue
        val id = o.optString("id").ifEmpty { "n$i" }
        val label = o.optString("label")
        val imp = o.optString("importance")
        val sum = o.optString("summary")
        var units = 0f
        for (ch in label) units += if (ch.code > 127) 13.5f else 7.5f
        val node = MindNode(id, label, imp, sum, (units + if (imp.isNotEmpty()) 40f else 28f).coerceIn(104f, 260f))
        map[id] = node
        parents[id] = if (o.isNull("parent_id")) "" else o.optString("parent_id", "")
    }
    val roots = mutableListOf<MindNode>()
    for ((id, node) in map) {
        val pid = parents[id].orEmpty()
        val p = if (pid.isNotEmpty() && pid != id) map[pid] else null
        if (p != null) p.children.add(node) else roots.add(node)
    }
    if (roots.isEmpty() && map.isNotEmpty()) roots.add(map.values.first())
    fun setDepth(n: MindNode, d: Int) {
        n.depth = d
        n.children.forEach { setDepth(it, d + 1) }
    }
    roots.forEach { setDepth(it, 0) }
    return roots
}

private fun nodeColors(depth: Int, isKg: Boolean): Pair<Color, Color> = if (isKg) {
    when (depth) {
        0 -> Color(0xFF2A3E58) to Color(0xFF5C8AC4)
        1 -> Color(0xFF243644) to Color(0xFF446C8A)
        2 -> Color(0xFF233A38) to Color(0xFF3E6A64)
        else -> Color(0xFF2A382E) to Color(0xFF486652)
    }
} else {
    when (depth) {
        0 -> Color(0xFF434663) to Color(0xFF666C98)
        1 -> Color(0xFF2B3245) to Color(0xFF44506C)
        2 -> Color(0xFF25373A) to Color(0xFF38545A)
        else -> Color(0xFF27382E) to Color(0xFF3C5848)
    }
}

private fun edgeColor(depth: Int, isKg: Boolean): Color = if (isKg) {
    when (depth) {
        0 -> Color(0xDC72A8E0)
        1 -> Color(0xDC68BAB6)
        else -> Color(0xDC7ABE94)
    }
} else {
    when (depth) {
        0 -> Color(0xDC848CD2)
        1 -> Color(0xDC7AA6C6)
        else -> Color(0xDC73B29C)
    }
}
