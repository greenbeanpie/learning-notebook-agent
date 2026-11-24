package com.localnotebook.agent.core

/**
 * Lightweight TeX → Unicode math conversion (port of crates/lna-studio/src/texfmt.rs).
 *
 * Turns `A=S\Lambda S^{-1}` into `A=SΛS⁻¹` with no rendering engine. Script content that has
 * no Unicode super/subscript form is returned as a shifted [Run] so the UI can raise/lower it.
 */
object TexMath {
    data class Run(val text: String, val shift: Int)

    private const val SUP_OPEN = ''
    private const val SUP_CLOSE = ''
    private const val SUB_OPEN = ''
    private const val SUB_CLOSE = ''

    /** Convert a math expression (delimiters optional) into styled runs. */
    fun toRuns(s: String): List<Run> {
        val converted = collapseSpaces(Parser(repairJsonEscapes(s).toCharArray()).seq(null))
        val runs = ArrayList<Run>()
        val cur = StringBuilder()
        var shift = 0
        for (ch in converted) {
            val next = when (ch) {
                SUP_OPEN -> 1
                SUB_OPEN -> -1
                SUP_CLOSE, SUB_CLOSE -> 0
                else -> null
            }
            if (next == null) {
                cur.append(ch)
            } else {
                if (cur.isNotEmpty()) runs.add(Run(cur.toString(), shift))
                cur.setLength(0)
                shift = next
            }
        }
        if (cur.isNotEmpty()) runs.add(Run(cur.toString(), shift))
        return runs
    }

    /** Plain-text form; unmappable scripts become `^x` / `^(xy)`. */
    fun toUnicode(s: String): String {
        if (s.none { it == '\\' || it == '^' || it == '_' || it == '$' || it == '\u000C' || it == '\b' || it == '\t' }) return s
        val sb = StringBuilder()
        for (r in toRuns(s)) {
            if (r.shift == 0) {
                sb.append(r.text)
                continue
            }
            val mark = if (r.shift > 0) '^' else '_'
            if (r.text.length == 1) sb.append(mark).append(r.text) else sb.append(mark).append('(').append(r.text).append(')')
        }
        return sb.toString()
    }

    private fun repairJsonEscapes(s: String): String {
        val sb = StringBuilder(s.length + 8)
        for (i in s.indices) {
            val c = s[i]
            val nextIsLetter = i + 1 < s.length && s[i + 1].isAsciiLetter()
            when {
                c == '\u000C' -> sb.append("\\f")
                c == '\b' -> sb.append("\\b")
                c == '\t' && nextIsLetter -> sb.append("\\t")
                c == '\r' && nextIsLetter -> sb.append("\\r")
                c == '\n' && nextIsLetter -> sb.append("\\n")
                else -> sb.append(c)
            }
        }
        return sb.toString()
    }

    private fun collapseSpaces(s: String): String {
        val sb = StringBuilder(s.length)
        var prevSpace = false
        for (c in s) {
            if (c == ' ') {
                if (!prevSpace) sb.append(c)
                prevSpace = true
            } else {
                sb.append(c)
                prevSpace = false
            }
        }
        return sb.toString().trim()
    }

    private fun Char.isAsciiLetter() = this in 'a'..'z' || this in 'A'..'Z'

    private class Parser(val c: CharArray) {
        var pos = 0

        fun seq(end: Char?): String {
            val out = StringBuilder()
            while (pos < c.size) {
                val ch = c[pos]
                if (end != null && ch == end) {
                    pos++
                    return out.toString()
                }
                when (ch) {
                    '$', '}' -> pos++
                    '{' -> { pos++; out.append(seq('}')) }
                    '^', '_' -> { pos++; out.append(script(arg(), ch == '^')) }
                    '&' -> { pos++; out.append(' ') }
                    '\\' -> { pos++; out.append(command()) }
                    else -> { pos++; out.append(ch) }
                }
            }
            return out.toString()
        }

        fun arg(): String {
            while (pos < c.size && c[pos] == ' ') pos++
            if (pos >= c.size) return ""
            return when (val ch = c[pos]) {
                '{' -> { pos++; seq('}') }
                '\\' -> { pos++; command() }
                else -> { pos++; ch.toString() }
            }
        }

        fun optArg(): String? {
            if (pos < c.size && c[pos] == '[') {
                pos++
                return seq(']')
            }
            return null
        }

        fun command(): String {
            if (pos >= c.size) return ""
            val first = c[pos]
            if (!first.isAsciiLetter()) {
                pos++
                return when (first) {
                    '(', ')', '[', ']' -> ""
                    ',', ';', ':', '!', ' ' -> " "
                    '\\' -> "; "
                    '|' -> "‖"
                    else -> first.toString()
                }
            }
            val start = pos
            while (pos < c.size && c[pos].isAsciiLetter()) pos++
            val name = String(c, start, pos - start)
            val spacedBefore = start >= 2 && c[start - 2] == ' '
            if (!spacedBefore && pos < c.size && c[pos] == ' ') pos++

            return when (name) {
                "frac", "dfrac", "tfrac", "cfrac" -> {
                    val num = arg()
                    val den = arg()
                    "${wrapIfCompound(num)}/${wrapIfCompound(den)}"
                }
                "sqrt" -> {
                    val idx = optArg()
                    val body = arg()
                    val root = when (idx) {
                        null -> "√"
                        "3" -> "∛"
                        "4" -> "∜"
                        else -> script(idx, true) + "√"
                    }
                    root + wrapIfCompound(body)
                }
                "binom" -> {
                    val n = arg()
                    val k = arg()
                    "C($n,$k)"
                }
                "text", "textrm", "textbf", "textit", "mathrm", "mathbf", "mathit", "mathsf", "mathtt",
                "operatorname", "boldsymbol", "bm", "mbox", "mathcal", "mathscr", "mathfrak" -> arg()
                "displaystyle", "textstyle" -> ""
                "mathbb" -> arg().map(::doubleStruck).joinToString("")
                "hat", "widehat" -> accent(arg(), '̂')
                "bar", "overline" -> accent(arg(), '̅')
                "vec", "overrightarrow" -> accent(arg(), '⃗')
                "tilde", "widetilde" -> accent(arg(), '̃')
                "dot" -> accent(arg(), '̇')
                "ddot" -> accent(arg(), '̈')
                "underline" -> accent(arg(), '̲')
                "begin" -> when (arg()) {
                    "vmatrix" -> "|"
                    "Vmatrix" -> "‖"
                    "cases" -> "{"
                    else -> "["
                }
                "end" -> when (arg()) {
                    "vmatrix" -> "|"
                    "Vmatrix" -> "‖"
                    "cases" -> ""
                    else -> "]"
                }
                "left", "right", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr", "limits", "nolimits" -> ""
                "quad", "qquad" -> " "
                else -> SYMBOLS[name] ?: name
            }
        }
    }

    private fun wrapIfCompound(s: String): String {
        val compound = s.length > 1 && s.any { it in "+-−·×/ =" }
        return if (compound) "($s)" else s
    }

    private fun accent(s: String, mark: Char): String {
        val sb = StringBuilder()
        for (ch in s) sb.append(ch).append(mark)
        return sb.toString()
    }

    private fun script(raw: String, sup: Boolean): String {
        val s = raw.trim()
        if (s.isEmpty()) return ""
        val map = if (sup) SUP else SUB
        if (s.all { map.containsKey(it) }) return s.map { map.getValue(it) }.joinToString("")
        if (sup && s == "⊤") return "ᵀ"
        if (sup && s in setOf("*", "′", "″", "†", "∗")) return s
        val inner = s.filterNot { it == SUP_OPEN || it == SUP_CLOSE || it == SUB_OPEN || it == SUB_CLOSE }
        return if (sup) "$SUP_OPEN$inner$SUP_CLOSE" else "$SUB_OPEN$inner$SUB_CLOSE"
    }

    private fun doubleStruck(ch: Char): String = when (ch) {
        'R' -> "ℝ"; 'C' -> "ℂ"; 'N' -> "ℕ"; 'Z' -> "ℤ"; 'Q' -> "ℚ"; 'P' -> "ℙ"; 'H' -> "ℍ"
        else -> ch.toString()
    }

    private fun pairs(keys: String, values: String): Map<Char, Char> {
        val k = keys.codePoints().toArray()
        val v = values.codePoints().toArray()
        require(k.size == v.size)
        return k.indices.associate { k[it].toChar() to v[it].toChar() }
    }

    private val SUP: Map<Char, Char> = pairs(
        "0123456789+-−=()abcdefghijklmnoprstuvwxyzABDEGHIJKLMNOPRTUVW⊤αβγδθφχ′ ",
        "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁⱽᵂᵀᵅᵝᵞᵟᶿᵠᵡ′ ",
    )

    private val SUB: Map<Char, Char> = pairs(
        "0123456789+-−=()aehijklmnoprstuvxβγρφχ, ",
        "₀₁₂₃₄₅₆₇₈₉₊₋₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓᵦᵧᵨᵩᵪ, ",
    )

    private val SYMBOLS: Map<String, String> = mapOf(
        "alpha" to "α", "beta" to "β", "gamma" to "γ", "delta" to "δ", "epsilon" to "ϵ",
        "varepsilon" to "ε", "zeta" to "ζ", "eta" to "η", "theta" to "θ", "vartheta" to "ϑ",
        "iota" to "ι", "kappa" to "κ", "lambda" to "λ", "mu" to "μ", "nu" to "ν", "xi" to "ξ",
        "pi" to "π", "varpi" to "ϖ", "rho" to "ρ", "varrho" to "ϱ", "sigma" to "σ",
        "varsigma" to "ς", "tau" to "τ", "upsilon" to "υ", "phi" to "ϕ", "varphi" to "φ",
        "chi" to "χ", "psi" to "ψ", "omega" to "ω",
        "Gamma" to "Γ", "Delta" to "Δ", "Theta" to "Θ", "Lambda" to "Λ", "Xi" to "Ξ",
        "Pi" to "Π", "Sigma" to "Σ", "Upsilon" to "Υ", "Phi" to "Φ", "Psi" to "Ψ", "Omega" to "Ω",
        "times" to "×", "cdot" to "·", "div" to "÷", "pm" to "±", "mp" to "∓", "ast" to "∗",
        "star" to "⋆", "circ" to "∘", "bullet" to "•", "otimes" to "⊗", "oplus" to "⊕",
        "le" to "≤", "leq" to "≤", "leqslant" to "≤", "ge" to "≥", "geq" to "≥", "geqslant" to "≥",
        "ne" to "≠", "neq" to "≠", "approx" to "≈", "equiv" to "≡", "sim" to "∼", "simeq" to "≃",
        "cong" to "≅", "propto" to "∝", "ll" to "≪", "gg" to "≫", "lt" to "<", "gt" to ">",
        "infty" to "∞", "partial" to "∂", "nabla" to "∇", "sum" to "∑", "prod" to "∏",
        "int" to "∫", "iint" to "∬", "iiint" to "∭", "oint" to "∮",
        "to" to "→", "rightarrow" to "→", "leftarrow" to "←", "gets" to "←", "leftrightarrow" to "↔",
        "Rightarrow" to "⇒", "Leftarrow" to "⇐", "Leftrightarrow" to "⇔",
        "implies" to "⟹", "iff" to "⟺", "mapsto" to "↦", "longrightarrow" to "⟶",
        "uparrow" to "↑", "downarrow" to "↓",
        "in" to "∈", "notin" to "∉", "ni" to "∋", "subset" to "⊂", "subseteq" to "⊆",
        "supset" to "⊃", "supseteq" to "⊇", "cup" to "∪", "cap" to "∩", "setminus" to "∖",
        "emptyset" to "∅", "varnothing" to "∅", "forall" to "∀", "exists" to "∃", "neg" to "¬", "lnot" to "¬",
        "land" to "∧", "wedge" to "∧", "lor" to "∨", "vee" to "∨",
        "perp" to "⊥", "parallel" to "∥", "angle" to "∠", "triangle" to "△", "top" to "⊤",
        "prime" to "′", "dagger" to "†", "ell" to "ℓ", "hbar" to "ℏ", "Re" to "ℜ", "Im" to "ℑ",
        "ldots" to "…", "dots" to "…", "dotsc" to "…", "cdots" to "⋯", "dotsb" to "⋯", "vdots" to "⋮", "ddots" to "⋱",
        "langle" to "⟨", "rangle" to "⟩", "lceil" to "⌈", "rceil" to "⌉", "lfloor" to "⌊",
        "rfloor" to "⌋", "vert" to "|", "mid" to "|", "Vert" to "‖", "lVert" to "‖", "rVert" to "‖",
        "lvert" to "|", "rvert" to "|", "lbrace" to "{", "rbrace" to "}", "degree" to "°",
    )
}
