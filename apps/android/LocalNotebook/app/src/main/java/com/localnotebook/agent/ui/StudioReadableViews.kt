package com.localnotebook.agent.ui

import com.localnotebook.agent.core.L10n
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import org.json.JSONArray
import org.json.JSONObject

private fun JSONArray?.objects(): List<JSONObject> =
    if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }

private fun JSONArray?.strings(): List<String> =
    if (this == null) emptyList() else (0 until length()).map { optString(it) }.filter { it.isNotBlank() }

private fun JSONArray?.ints(): Set<Int> =
    if (this == null) emptySet() else (0 until length()).map { optInt(it, -1) }.toSet()

/** Report sections: heading, Markdown body and key takeaways. */
@Composable
fun ReportReadableView(payload: JSONObject, en: Boolean) {
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        for (sec in payload.optJSONArray("sections").objects()) {
            MarkdownInline(sec.optString("heading"), style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold))
            MarkdownText(sec.optString("body_md"))
            val takeaways = sec.optJSONArray("key_takeaways").strings()
            if (takeaways.isNotEmpty()) {
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(L10n.t("readable.key_points", en), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
                        MarkdownText(takeaways.joinToString("\n") { "- $it" }, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
        }
    }
}

/** Quiz questions with options; answers and explanations are revealed per question. */
@Composable
fun QuizReadableView(payload: JSONObject, en: Boolean) {
    val questions = payload.optJSONArray("questions").objects()
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        questions.forEachIndexed { qi, q ->
            var revealed by remember(q.optString("id"), qi) { mutableStateOf(false) }
            val correct = q.optJSONArray("answer_indices").ints()
            Surface(
                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    MarkdownText("**${qi + 1}.** " + q.optString("prompt"))
                    q.optJSONArray("options").strings().forEachIndexed { oi, opt ->
                        val isRight = revealed && oi in correct
                        Surface(
                            color = if (isRight) Color(0x3381C995) else Color.Transparent,
                            shape = RoundedCornerShape(8.dp),
                            border = BorderStroke(1.dp, if (isRight) Color(0xFF81C995) else MaterialTheme.colorScheme.outlineVariant),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Row(modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp)) {
                                Text("${'A' + oi}. ", color = MaterialTheme.colorScheme.onSurfaceVariant)
                                MarkdownInline(opt)
                            }
                        }
                    }
                    TextButton(onClick = { revealed = !revealed }) {
                        Text(if (revealed) (L10n.t("readable.hide_answer", en)) else (L10n.t("readable.show_answer", en)))
                    }
                    if (revealed) {
                        val accepted = q.optJSONArray("accepted_answers").strings()
                        if (accepted.isNotEmpty()) {
                            MarkdownText((L10n.t("readable.answer", en)) + accepted.joinToString(" / "))
                        }
                        val rubric = q.optString("rubric")
                        if (rubric.isNotBlank()) MarkdownText(rubric, style = MaterialTheme.typography.bodyMedium)
                        val explanation = q.optString("explanation")
                        if (explanation.isNotBlank()) {
                            MarkdownText(
                                explanation,
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}

/** Flashcards: tap a card to flip between front and back. */
@Composable
fun FlashcardsReadableView(payload: JSONObject, en: Boolean) {
    val cards = payload.optJSONArray("cards").objects()
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            L10n.t("readable.tap_a_card_to_flip", en),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        cards.forEachIndexed { i, card ->
            var flipped by remember(i, card.optString("front")) { mutableStateOf(false) }
            Surface(
                color = if (flipped) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth().clickable { flipped = !flipped },
            ) {
                Column(modifier = Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        "${i + 1} / ${cards.size} · " + if (flipped) (L10n.t("readable.back", en)) else (L10n.t("readable.front", en)),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    MarkdownText(card.optString(if (flipped) "back" else "front"), selectable = false)
                    val hint = card.optString("hint")
                    if (!flipped && hint.isNotBlank()) {
                        MarkdownText(
                            (L10n.t("readable.hint", en)) + hint,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            selectable = false,
                        )
                    }
                }
            }
        }
    }
}

/** Audio overview script (speaker turns). */
@Composable
fun AudioScriptReadableView(payload: JSONObject) {
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (turn in payload.optJSONArray("turns").objects()) {
            Text(turn.optString("speaker"), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
            MarkdownText(turn.optString("text"))
        }
    }
}

/** Infographic blocks as a simple list. */
@Composable
fun InfographicReadableView(payload: JSONObject) {
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        MarkdownInline(payload.optString("title"), style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold))
        val sub = payload.optString("subtitle")
        if (sub.isNotBlank()) MarkdownInline(sub, color = MaterialTheme.colorScheme.onSurfaceVariant)
        for (blk in payload.optJSONArray("blocks").objects()) {
            Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = RoundedCornerShape(12.dp), modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    MarkdownInline(blk.optString("heading"), style = MaterialTheme.typography.titleSmall)
                    val metric = blk.optString("metric")
                    if (metric.isNotBlank()) {
                        MarkdownInline(metric, style = MaterialTheme.typography.headlineSmall, color = MaterialTheme.colorScheme.primary)
                    }
                    MarkdownText(blk.optString("summary"), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
    }
}
