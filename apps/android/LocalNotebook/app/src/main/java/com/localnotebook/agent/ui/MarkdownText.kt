package com.localnotebook.agent.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.BaselineShift
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.isSpecified
import com.localnotebook.agent.core.MdParser
import com.localnotebook.agent.core.TexMath

/** Colors used for inline decorations; derived from the Material theme by default. */
data class MdColors(
    val text: Color,
    val muted: Color,
    val accent: Color,
    val codeFg: Color,
    val codeBg: Color,
    val rule: Color,
)

@Composable
fun mdColors(text: Color = MaterialTheme.colorScheme.onSurface): MdColors = MdColors(
    text = text,
    muted = MaterialTheme.colorScheme.onSurfaceVariant,
    accent = MaterialTheme.colorScheme.primary,
    codeFg = Color(0xFFA0C8FF),
    codeBg = MaterialTheme.colorScheme.surfaceVariant,
    rule = MaterialTheme.colorScheme.outlineVariant,
)

/**
 * Renders Markdown with math as native Compose text: no WebView, no extra libraries.
 * TeX is converted to Unicode math (see [TexMath]); unmappable scripts are raised/lowered.
 */
@Composable
fun MarkdownText(
    markdown: String,
    modifier: Modifier = Modifier,
    style: TextStyle = MaterialTheme.typography.bodyLarge,
    color: Color = MaterialTheme.colorScheme.onSurface,
    selectable: Boolean = true,
) {
    val blocks = remember(markdown) { MdParser.parse(markdown) }
    val colors = mdColors(color)
    val content: @Composable () -> Unit = {
        Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            for (b in blocks) MdBlockView(b, style, colors)
        }
    }
    if (selectable) SelectionContainer { content() } else content()
}

/** Single-line / inline Markdown (no block structure), e.g. quiz options or card faces. */
@Composable
fun MarkdownInline(
    text: String,
    modifier: Modifier = Modifier,
    style: TextStyle = MaterialTheme.typography.bodyLarge,
    color: Color = MaterialTheme.colorScheme.onSurface,
) {
    val colors = mdColors(color)
    val annotated = remember(text, colors) { inlineAnnotated(text, colors) }
    Text(annotated, modifier = modifier, style = style, color = color)
}

@Composable
private fun MdBlockView(b: MdParser.Block, style: TextStyle, c: MdColors) {
    when (b) {
        is MdParser.Heading -> {
            val hs = when (b.level) {
                1 -> MaterialTheme.typography.titleLarge
                2 -> MaterialTheme.typography.titleMedium
                else -> MaterialTheme.typography.titleSmall
            }
            val a = remember(b, c) { inlineAnnotated(b.text, c) }
            Text(a, style = hs, fontWeight = FontWeight.SemiBold, color = c.text, modifier = Modifier.padding(top = 4.dp))
        }
        is MdParser.Paragraph -> {
            val a = remember(b, c) { inlineAnnotated(b.text, c) }
            Text(a, style = style, color = c.text)
        }
        is MdParser.ListItem -> {
            val a = remember(b, c) { inlineAnnotated(b.text, c) }
            Row(modifier = Modifier.padding(start = (b.depth * 16).dp)) {
                Text(
                    b.marker,
                    style = style,
                    color = c.muted,
                    modifier = Modifier.widthIn(min = if (b.ordered) 22.dp else 14.dp),
                )
                Text(a, style = style, color = c.text)
            }
        }
        is MdParser.Quote -> {
            val a = remember(b, c) { inlineAnnotated(b.text, c) }
            Row(modifier = Modifier.height(IntrinsicSize.Min)) {
                Box(Modifier.width(3.dp).fillMaxHeight().background(c.rule, RoundedCornerShape(2.dp)))
                Text(a, style = style, color = c.muted, modifier = Modifier.padding(start = 10.dp))
            }
        }
        is MdParser.Code -> {
            Surface(color = c.codeBg, shape = RoundedCornerShape(8.dp), modifier = Modifier.fillMaxWidth()) {
                Text(
                    b.text,
                    style = style.copy(fontFamily = FontFamily.Monospace, fontSize = style.fontSize.scaled(0.88f)),
                    color = c.text,
                    softWrap = false,
                    modifier = Modifier.horizontalScroll(rememberScrollState()).padding(10.dp),
                )
            }
        }
        is MdParser.MathBlock -> {
            val a = remember(b) { buildAnnotatedString { appendMath(b.tex) } }
            Box(modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
                Text(
                    a,
                    style = style.copy(fontSize = style.fontSize.scaled(1.1f)),
                    color = c.text,
                    softWrap = false,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            }
        }
        MdParser.Rule -> HorizontalDivider(color = c.rule, modifier = Modifier.padding(vertical = 4.dp))
        is MdParser.Table -> MdTable(b, style, c)
    }
}

@Composable
private fun MdTable(t: MdParser.Table, style: TextStyle, c: MdColors) {
    val cols = maxOf(t.header.size, t.rows.maxOfOrNull { it.size } ?: 0)
    Surface(
        color = Color.Transparent,
        shape = RoundedCornerShape(8.dp),
        border = androidx.compose.foundation.BorderStroke(1.dp, c.rule),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column {
            val all = listOf(t.header) + t.rows
            all.forEachIndexed { r, row ->
                if (r > 0) HorizontalDivider(color = c.rule)
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(if (r == 0) c.codeBg.copy(alpha = 0.5f) else Color.Transparent),
                ) {
                    for (ci in 0 until cols) {
                        val cell = row.getOrElse(ci) { "" }
                        val a = remember(cell, c) { inlineAnnotated(cell, c) }
                        Text(
                            a,
                            style = style.copy(fontSize = style.fontSize.scaled(0.9f)),
                            fontWeight = if (r == 0) FontWeight.SemiBold else null,
                            color = c.text,
                            modifier = Modifier.weight(1f).padding(horizontal = 8.dp, vertical = 6.dp),
                        )
                    }
                }
            }
        }
    }
}

private val MathFamily = FontFamily.Serif

private fun TextUnit.scaled(f: Float): TextUnit = if (isSpecified) this * f else this

private fun AnnotatedString.Builder.appendMath(tex: String) {
    withStyle(SpanStyle(fontFamily = MathFamily)) {
        for (run in TexMath.toRuns(tex)) {
            when {
                run.shift > 0 -> withStyle(SpanStyle(baselineShift = BaselineShift.Superscript, fontSize = 0.72.em)) { append(run.text) }
                run.shift < 0 -> withStyle(SpanStyle(baselineShift = BaselineShift.Subscript, fontSize = 0.72.em)) { append(run.text) }
                else -> append(run.text)
            }
        }
    }
}

internal fun inlineAnnotated(text: String, c: MdColors): AnnotatedString = buildAnnotatedString {
    for (s in MdParser.parseInline(text)) {
        when (s) {
            is MdParser.TextSpan -> {
                val st = SpanStyle(
                    fontWeight = if (s.bold) FontWeight.SemiBold else null,
                    fontStyle = if (s.italic) FontStyle.Italic else null,
                    textDecoration = if (s.strike) TextDecoration.LineThrough else null,
                )
                if (s.bold || s.italic || s.strike) withStyle(st) { append(s.text) } else append(s.text)
            }
            is MdParser.CodeSpan -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, color = c.codeFg, background = c.codeBg)) {
                append(s.text)
            }
            is MdParser.MathSpan -> appendMath(s.tex)
            is MdParser.LinkSpan -> withStyle(SpanStyle(color = c.accent, textDecoration = TextDecoration.Underline)) {
                append(s.text)
            }
            is MdParser.CiteSpan -> withStyle(
                SpanStyle(color = c.accent, fontSize = 0.72.em, baselineShift = BaselineShift.Superscript, fontWeight = FontWeight.SemiBold),
            ) {
                append("[${s.n}]")
            }
        }
    }
}
