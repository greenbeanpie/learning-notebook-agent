package com.localnotebook.agent.core

/**
 * Minimal Markdown parser for chat answers, guides and Studio text.
 *
 * Block grammar: ATX headings, paragraphs (line breaks kept), bullet/ordered lists with nesting,
 * block quotes, fenced code, display math ($$…$$ / \[…\]), rules and pipe tables.
 * Inline grammar: **bold**, *italic*, ~~strike~~, `code`, $math$ / \(math\), [text](url), [n] citations.
 * The same rules are implemented for macOS (MarkdownView.swift) and Linux (md_view.rs).
 */
object MdParser {
    sealed interface Block
    data class Heading(val level: Int, val text: String) : Block
    data class Paragraph(val text: String) : Block
    data class ListItem(val ordered: Boolean, val marker: String, val depth: Int, val text: String) : Block
    data class Quote(val text: String) : Block
    data class Code(val text: String) : Block
    data class MathBlock(val tex: String) : Block
    data object Rule : Block
    data class Table(val header: List<String>, val rows: List<List<String>>) : Block

    sealed interface Span
    data class TextSpan(val text: String, val bold: Boolean, val italic: Boolean, val strike: Boolean) : Span
    data class CodeSpan(val text: String) : Span
    data class MathSpan(val tex: String) : Span
    data class LinkSpan(val text: String, val url: String) : Span
    data class CiteSpan(val n: Int) : Span

    private val headingRe = Regex("^(#{1,6})\\s+(.*?)\\s*#*\\s*$")
    private val ruleRe = Regex("^\\s{0,3}([-*_])(\\s*\\1){2,}\\s*$")
    private val listRe = Regex("^(\\s*)([-*+•]|\\d{1,3}[.)])\\s+(.*)$")
    private val tableSepRe = Regex("^\\s*\\|?\\s*:?-{2,}:?\\s*(\\|\\s*:?-{2,}:?\\s*)*\\|?\\s*$")

    fun parse(src: String): List<Block> {
        val lines = src.replace("\r\n", "\n").replace('\r', '\n').split('\n')
        val out = ArrayList<Block>()
        val para = StringBuilder()
        fun flushPara() {
            if (para.isNotBlank()) out.add(Paragraph(para.toString().trim('\n')))
            para.setLength(0)
        }
        var i = 0
        while (i < lines.size) {
            val line = lines[i]
            val t = line.trim()
            when {
                t.startsWith("```") || t.startsWith("~~~") -> {
                    flushPara()
                    val fence = t.substring(0, 3)
                    val body = StringBuilder()
                    i++
                    while (i < lines.size && !lines[i].trim().startsWith(fence)) {
                        if (body.isNotEmpty()) body.append('\n')
                        body.append(lines[i])
                        i++
                    }
                    out.add(Code(body.toString()))
                }
                t.startsWith("$$") || t.startsWith("\\[") -> {
                    flushPara()
                    val close = if (t.startsWith("$$")) "$$" else "\\]"
                    val rest = t.substring(2)
                    val endIdx = rest.indexOf(close)
                    if (endIdx >= 0) {
                        out.add(MathBlock(rest.substring(0, endIdx).trim()))
                        val tail = rest.substring(endIdx + 2).trim()
                        if (tail.isNotEmpty()) para.append(tail)
                    } else {
                        val body = StringBuilder(rest)
                        i++
                        while (i < lines.size) {
                            val l = lines[i]
                            val k = l.indexOf(close)
                            if (k >= 0) {
                                body.append(' ').append(l.substring(0, k))
                                break
                            }
                            body.append(' ').append(l)
                            i++
                        }
                        out.add(MathBlock(body.toString().trim()))
                    }
                }
                t.isEmpty() -> flushPara()
                headingRe.matches(t) && !line.startsWith("    ") -> {
                    flushPara()
                    val m = headingRe.find(t)!!
                    out.add(Heading(m.groupValues[1].length, m.groupValues[2]))
                }
                ruleRe.matches(line) -> {
                    flushPara()
                    out.add(Rule)
                }
                t.startsWith("|") && i + 1 < lines.size && tableSepRe.matches(lines[i + 1]) -> {
                    flushPara()
                    val header = splitRow(t)
                    val rows = ArrayList<List<String>>()
                    i += 2
                    while (i < lines.size && lines[i].trim().startsWith("|")) {
                        rows.add(splitRow(lines[i].trim()))
                        i++
                    }
                    out.add(Table(header, rows))
                    continue
                }
                listRe.matches(line) -> {
                    flushPara()
                    val m = listRe.find(line)!!
                    val indent = m.groupValues[1].replace("\t", "    ").length
                    val marker = m.groupValues[2]
                    val ordered = marker.first().isDigit()
                    val text = StringBuilder(m.groupValues[3])
                    // Lazy continuation lines belong to the item.
                    while (i + 1 < lines.size) {
                        val next = lines[i + 1]
                        val nt = next.trim()
                        if (nt.isEmpty() || listRe.matches(next) || nt.startsWith("#") || nt.startsWith(">") ||
                            nt.startsWith("```") || nt.startsWith("$$") || nt.startsWith("|")
                        ) break
                        text.append('\n').append(nt)
                        i++
                    }
                    out.add(
                        ListItem(
                            ordered = ordered,
                            marker = if (ordered) marker.trimEnd(')').trimEnd('.') + "." else "•",
                            depth = (indent / 2).coerceAtMost(4),
                            text = text.toString(),
                        ),
                    )
                }
                t.startsWith(">") -> {
                    flushPara()
                    val body = StringBuilder()
                    while (i < lines.size && lines[i].trim().startsWith(">")) {
                        if (body.isNotEmpty()) body.append('\n')
                        body.append(lines[i].trim().removePrefix(">").trimStart())
                        i++
                    }
                    out.add(Quote(body.toString()))
                    continue
                }
                else -> {
                    if (para.isNotEmpty()) para.append('\n')
                    para.append(t)
                }
            }
            i++
        }
        flushPara()
        return out
    }

    private fun splitRow(row: String): List<String> =
        row.trim().removePrefix("|").removeSuffix("|").split('|').map { it.trim() }

    fun parseInline(text: String): List<Span> {
        val out = ArrayList<Span>()
        parseInlineInto(text, bold = false, italic = false, strike = false, out = out)
        return out
    }

    private fun parseInlineInto(text: String, bold: Boolean, italic: Boolean, strike: Boolean, out: MutableList<Span>) {
        val buf = StringBuilder()
        fun flush() {
            if (buf.isNotEmpty()) out.add(TextSpan(buf.toString(), bold, italic, strike))
            buf.setLength(0)
        }
        var i = 0
        val n = text.length
        while (i < n) {
            val c = text[i]
            val next = if (i + 1 < n) text[i + 1] else '\u0000'
            // Math \( … \) and \[ … \]
            if (c == '\\' && (next == '(' || next == '[')) {
                val close = if (next == '(') "\\)" else "\\]"
                val end = text.indexOf(close, i + 2)
                if (end > i + 2) {
                    flush()
                    out.add(MathSpan(text.substring(i + 2, end).trim()))
                    i = end + 2
                    continue
                }
            }
            // Escaped punctuation
            if (c == '\\' && next != '\u0000' && next in "\\`*_{}[]()#+-.!|~$>") {
                buf.append(next)
                i += 2
                continue
            }
            if (c == '`') {
                val end = text.indexOf('`', i + 1)
                if (end > i) {
                    flush()
                    out.add(CodeSpan(text.substring(i + 1, end)))
                    i = end + 1
                    continue
                }
            }
            if (c == '$') {
                if (next == '$') {
                    val end = text.indexOf("$$", i + 2)
                    if (end > i + 2) {
                        flush()
                        out.add(MathSpan(text.substring(i + 2, end).trim()))
                        i = end + 2
                        continue
                    }
                } else {
                    val end = findDollarEnd(text, i + 1)
                    if (end > 0) {
                        flush()
                        out.add(MathSpan(text.substring(i + 1, end)))
                        i = end + 1
                        continue
                    }
                }
            }
            if ((c == '*' || c == '_') && next == c) {
                val delim = "$c$c"
                val end = text.indexOf(delim, i + 2)
                if (end > i + 2 && (c == '*' || isWordBoundary(text, i - 1))) {
                    flush()
                    parseInlineInto(text.substring(i + 2, end), true, italic, strike, out)
                    i = end + 2
                    continue
                }
            }
            if ((c == '*' || c == '_') && next != c && next != ' ' && next != '\u0000') {
                val end = findSingleDelim(text, i + 1, c)
                if (end > i + 1 && (c == '*' || (isWordBoundary(text, i - 1) && isWordBoundary(text, end + 1)))) {
                    flush()
                    parseInlineInto(text.substring(i + 1, end), bold, true, strike, out)
                    i = end + 1
                    continue
                }
            }
            if (c == '~' && next == '~') {
                val end = text.indexOf("~~", i + 2)
                if (end > i + 2) {
                    flush()
                    parseInlineInto(text.substring(i + 2, end), bold, italic, true, out)
                    i = end + 2
                    continue
                }
            }
            if (c == '[' || c == '【') {
                val closeCh = if (c == '[') ']' else '】'
                val close = text.indexOf(closeCh, i + 1)
                if (close > i + 1) {
                    val inner = text.substring(i + 1, close)
                    val num = inner.trim().toIntOrNull()
                    if (num != null && num > 0 && inner.length <= 4) {
                        if (buf.endsWith(" ")) buf.setLength(buf.length - 1)
                        flush()
                        out.add(CiteSpan(num))
                        i = close + 1
                        continue
                    }
                    if (c == '[' && close + 1 < n && text[close + 1] == '(') {
                        val urlEnd = text.indexOf(')', close + 2)
                        if (urlEnd > close) {
                            flush()
                            out.add(LinkSpan(inner, text.substring(close + 2, urlEnd)))
                            i = urlEnd + 1
                            continue
                        }
                    }
                }
            }
            buf.append(c)
            i++
        }
        flush()
    }

    private fun isWordBoundary(text: String, idx: Int): Boolean =
        idx < 0 || idx >= text.length || !text[idx].isLetterOrDigit()

    private fun findSingleDelim(text: String, start: Int, d: Char): Int {
        var j = start
        while (j < text.length) {
            val ch = text[j]
            if (ch == '\n') return -1
            if (ch == d) {
                if (j + 1 < text.length && text[j + 1] == d) { j += 2; continue }
                return if (text[j - 1] != ' ') j else -1
            }
            j++
        }
        return -1
    }

    /** Closing `$` of inline math, rejecting `$ 5` style currency and empty spans. */
    private fun findDollarEnd(text: String, start: Int): Int {
        if (start >= text.length || text[start].isWhitespace()) return -1
        var j = start
        while (j < text.length) {
            val ch = text[j]
            if (ch == '\n') return -1
            if (ch == '\\' && j + 1 < text.length) { j += 2; continue }
            if (ch == '$') {
                if (j + 1 < text.length && text[j + 1] == '$') return -1
                if (j > start && !text[j - 1].isWhitespace()) {
                    // "$5 and $6" — a digit right after the closing $ means currency.
                    if (j + 1 < text.length && text[j + 1].isDigit()) return -1
                    return j
                }
                return -1
            }
            j++
        }
        return -1
    }
}
