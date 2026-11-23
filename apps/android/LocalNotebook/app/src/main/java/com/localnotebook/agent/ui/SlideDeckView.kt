package com.localnotebook.agent.ui

import androidx.compose.animation.*
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject

@Composable
fun SlideDeckView(
    payload: JSONObject,
    en: Boolean,
    modifier: Modifier = Modifier,
) {
    val slidesArr = payload.optJSONArray("slides")
    val slides = remember(payload) {
        buildList {
            if (slidesArr != null) {
                for (i in 0 until slidesArr.length()) {
                    add(slidesArr.getJSONObject(i))
                }
            }
        }
    }

    if (slides.isEmpty()) {
        Text(
            if (en) "No slides available." else "暂无幻灯片内容。",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = modifier.padding(16.dp),
        )
        return
    }

    val deckTitle = payload.optString("title", if (en) "Presentation" else "演示文稿")
    var index by remember { mutableIntStateOf(0) }
    var isPlaying by remember { mutableStateOf(false) }
    var showNotes by remember { mutableStateOf(false) }

    LaunchedEffect(isPlaying, index, slides.size) {
        if (isPlaying) {
            delay(5000L)
            if (index + 1 < slides.size) {
                index += 1
            } else {
                isPlaying = false
            }
        }
    }

    Column(
        modifier = modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        // 16:9 Animated Presentation Stage with swipe-to-flip
        Surface(
            shape = RoundedCornerShape(12.dp),
            border = BorderStroke(1.dp, Color.White.copy(alpha = 0.14f)),
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(16f / 9f)
                .pointerInput(slides.size) {
                    var totalDrag = 0f
                    detectHorizontalDragGestures(
                        onDragStart = { totalDrag = 0f },
                        onHorizontalDrag = { _, dragAmount -> totalDrag += dragAmount },
                        onDragEnd = {
                            if (totalDrag < -60f && index + 1 < slides.size) {
                                isPlaying = false
                                index += 1
                            } else if (totalDrag > 60f && index > 0) {
                                isPlaying = false
                                index -= 1
                            }
                        },
                    )
                },
        ) {
            AnimatedContent(
                targetState = index,
                transitionSpec = {
                    val dir = if (targetState > initialState) 1 else -1
                    (slideInHorizontally(animationSpec = tween(320)) { dir * it / 2 } + fadeIn(tween(300)))
                        .togetherWith(slideOutHorizontally(animationSpec = tween(260)) { -dir * it / 2 } + fadeOut(tween(240)))
                },
                label = "slideTransition",
            ) { currentIdx ->
                val s = slides[currentIdx.coerceIn(0, slides.lastIndex)]
                SlideStageCanvas(slide = s, idx = currentIdx, total = slides.size, deckTitle = deckTitle)
            }
        }

        // Playback & Navigation Controls
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            OutlinedButton(
                onClick = { isPlaying = false; if (index > 0) index-- },
                enabled = index > 0,
                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
            ) {
                Icon(Icons.Default.ChevronLeft, contentDescription = null, modifier = Modifier.size(18.dp))
            }
            Text("${index + 1} / ${slides.size}", style = MaterialTheme.typography.labelLarge)
            OutlinedButton(
                onClick = { isPlaying = false; if (index + 1 < slides.size) index++ },
                enabled = index + 1 < slides.size,
                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
            ) {
                Icon(Icons.Default.ChevronRight, contentDescription = null, modifier = Modifier.size(18.dp))
            }
            Spacer(Modifier.weight(1f))
            Button(
                onClick = { isPlaying = !isPlaying },
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp),
            ) {
                Icon(
                    if (isPlaying) Icons.Default.Pause else Icons.Default.PlayArrow,
                    contentDescription = null,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(Modifier.width(4.dp))
                Text(if (isPlaying) (if (en) "Pause" else "暂停") else (if (en) "Play" else "放映"), fontSize = 12.sp)
            }
            TextButton(onClick = { showNotes = !showNotes }) {
                Text(if (en) "Notes" else "备注", fontSize = 12.sp)
            }
        }

        // Clickable Slide Filmstrip
        Row(
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
        ) {
            slides.forEachIndexed { i, s ->
                val active = i == index
                Surface(
                    color = if (active) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
                    shape = RoundedCornerShape(8.dp),
                    border = if (active) BorderStroke(1.dp, MaterialTheme.colorScheme.primary) else null,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable { isPlaying = false; index = i },
                ) {
                    Text(
                        "${i + 1}. ${s.optString("title")}",
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.labelMedium,
                        modifier = Modifier.widthIn(max = 140.dp).padding(horizontal = 8.dp, vertical = 6.dp),
                    )
                }
            }
        }

        if (showNotes) {
            val notes = slides[index].optString("speaker_notes").ifBlank {
                if (en) "No speaker notes for this slide." else "本页暂无演讲备注。"
            }
            Surface(
                color = MaterialTheme.colorScheme.surfaceVariant,
                shape = RoundedCornerShape(8.dp),
                modifier = Modifier.fillMaxWidth().heightIn(max = 100.dp),
            ) {
                Text(
                    notes,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(10.dp).verticalScroll(rememberScrollState()),
                )
            }
        }
    }
}

@Composable
private fun SlideStageCanvas(slide: JSONObject, idx: Int, total: Int, deckTitle: String) {
    val layout = slide.optString("layout", "bullets").lowercase()
    val isDark = layout == "title" || layout == "section" || layout == "summary"
    val bg = if (isDark) Color(0xFF0F172A) else Color(0xFFF8FAFC)
    val primary = Color(0xFF1D4ED8)
    val accent = Color(0xFF0EA5E9)
    val fgTitle = if (isDark) Color(0xFFF8FAFC) else Color(0xFF0F172A)
    val fgBody = if (isDark) Color(0xFFCBD5E1) else Color(0xFF334155)

    Column(modifier = Modifier.fillMaxSize().background(bg)) {
        if (!isDark) {
            Box(modifier = Modifier.fillMaxWidth().height(4.dp).background(primary))
        }
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(layout.uppercase(), fontSize = 9.sp, fontWeight = FontWeight.Bold, color = if (isDark) accent else primary)
            Text(
                slide.optString("title"),
                fontSize = if (isDark) 20.sp else 17.sp,
                fontWeight = FontWeight.Bold,
                color = fgTitle,
                lineHeight = 22.sp,
            )
            val sub = slide.optString("subtitle")
            if (sub.isNotBlank()) {
                Text(sub, fontSize = 11.sp, color = fgBody.copy(alpha = 0.85f))
            }

            val formula = slide.optString("formula")
            if (formula.isNotBlank()) {
                Surface(color = Color(0xFF0F172A), shape = RoundedCornerShape(6.dp), modifier = Modifier.fillMaxWidth()) {
                    Text(
                        formula,
                        fontFamily = FontFamily.Monospace,
                        fontWeight = FontWeight.Bold,
                        color = Color.White,
                        fontSize = 13.sp,
                        modifier = Modifier.padding(10.dp),
                    )
                }
            }

            val cards = slide.optJSONArray("cards")
            if (cards != null && cards.length() > 0) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
                    for (i in 0 until minOf(cards.length(), 3)) {
                        val c = cards.getJSONObject(i)
                        Surface(color = Color.White, shape = RoundedCornerShape(6.dp), border = BorderStroke(1.dp, Color(0xFFE2E8F0)), modifier = Modifier.weight(1f)) {
                            Column(modifier = Modifier.padding(8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(c.optString("title"), fontSize = 11.sp, fontWeight = FontWeight.Bold, color = Color(0xFF0F172A))
                                Text(c.optString("body"), fontSize = 10.sp, color = Color(0xFF334155))
                            }
                        }
                    }
                }
            }

            val bullets = slide.optJSONArray("bullets")
            if (bullets != null) {
                for (i in 0 until minOf(bullets.length(), 6)) {
                    val b = bullets.optString(i)
                    if (b.isNotBlank()) {
                        Text("▸ $b", fontSize = 11.5.sp, color = fgBody, lineHeight = 16.sp)
                    }
                }
            }
        }
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(deckTitle, fontSize = 9.sp, color = fgBody.copy(alpha = 0.65f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("${idx + 1} / $total", fontSize = 9.sp, fontWeight = FontWeight.Bold, color = fgBody.copy(alpha = 0.8f))
        }
    }
}
