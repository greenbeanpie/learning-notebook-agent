import Foundation

/// Lightweight TeX → Unicode math conversion (port of crates/lna-studio/src/texfmt.rs).
///
/// Turns `A=S\Lambda S^{-1}` into `A=SΛS⁻¹` without any rendering engine. Script content that
/// has no Unicode super/subscript form comes back as a shifted `Run` for the UI to raise/lower.
enum TexMath {
    struct Run: Equatable {
        let text: String
        let shift: Int
    }

    private static let supOpen: Character = "\u{E000}"
    private static let supClose: Character = "\u{E001}"
    private static let subOpen: Character = "\u{E002}"
    private static let subClose: Character = "\u{E003}"

    /// Convert a math expression (delimiters optional) into styled runs.
    static func toRuns(_ s: String) -> [Run] {
        var parser = Parser(chars: Array(repairJsonEscapes(s)))
        let converted = collapseSpaces(parser.seq(end: nil))
        var runs: [Run] = []
        var cur = ""
        var shift = 0
        for ch in converted {
            let next: Int?
            switch ch {
            case supOpen: next = 1
            case subOpen: next = -1
            case supClose, subClose: next = 0
            default: next = nil
            }
            if let n = next {
                if !cur.isEmpty { runs.append(Run(text: cur, shift: shift)) }
                cur = ""
                shift = n
            } else {
                cur.append(ch)
            }
        }
        if !cur.isEmpty { runs.append(Run(text: cur, shift: shift)) }
        return runs
    }

    /// Plain-text form; unmappable scripts become `^x` / `^(xy)`.
    static func toUnicode(_ s: String) -> String {
        guard s.contains(where: { "\\^_$\u{0C}\u{08}\t".contains($0) }) else { return s }
        var out = ""
        for r in toRuns(s) {
            if r.shift == 0 {
                out += r.text
                continue
            }
            let mark = r.shift > 0 ? "^" : "_"
            out += r.text.count == 1 ? "\(mark)\(r.text)" : "\(mark)(\(r.text))"
        }
        return out
    }

    private static func isAsciiLetter(_ c: Character) -> Bool {
        guard let a = c.asciiValue else { return false }
        return (a >= 65 && a <= 90) || (a >= 97 && a <= 122)
    }

    private static func repairJsonEscapes(_ s: String) -> String {
        let chars = Array(s)
        var out = ""
        for (i, c) in chars.enumerated() {
            let nextIsLetter = i + 1 < chars.count && isAsciiLetter(chars[i + 1])
            switch c {
            case "\u{0C}": out += "\\f"
            case "\u{08}": out += "\\b"
            case "\t" where nextIsLetter: out += "\\t"
            case "\r" where nextIsLetter: out += "\\r"
            case "\n" where nextIsLetter: out += "\\n"
            default: out.append(c)
            }
        }
        return out
    }

    private static func collapseSpaces(_ s: String) -> String {
        var out = ""
        var prevSpace = false
        for c in s {
            if c == " " {
                if !prevSpace { out.append(c) }
                prevSpace = true
            } else {
                out.append(c)
                prevSpace = false
            }
        }
        return out.trimmingCharacters(in: .whitespaces)
    }

    private struct Parser {
        let chars: [Character]
        var pos = 0

        mutating func seq(end: Character?) -> String {
            var out = ""
            while pos < chars.count {
                let ch = chars[pos]
                if let e = end, ch == e {
                    pos += 1
                    return out
                }
                switch ch {
                case "$", "}":
                    pos += 1
                case "{":
                    pos += 1
                    out += seq(end: "}")
                case "^", "_":
                    pos += 1
                    let a = arg()
                    out += TexMath.script(a, sup: ch == "^")
                case "&":
                    pos += 1
                    out += " "
                case "\\":
                    pos += 1
                    out += command()
                default:
                    pos += 1
                    out.append(ch)
                }
            }
            return out
        }

        mutating func arg() -> String {
            while pos < chars.count && chars[pos] == " " { pos += 1 }
            guard pos < chars.count else { return "" }
            let ch = chars[pos]
            switch ch {
            case "{":
                pos += 1
                return seq(end: "}")
            case "\\":
                pos += 1
                return command()
            default:
                pos += 1
                return String(ch)
            }
        }

        mutating func optArg() -> String? {
            if pos < chars.count && chars[pos] == "[" {
                pos += 1
                return seq(end: "]")
            }
            return nil
        }

        mutating func command() -> String {
            guard pos < chars.count else { return "" }
            let first = chars[pos]
            if !TexMath.isAsciiLetter(first) {
                pos += 1
                switch first {
                case "(", ")", "[", "]": return ""
                case ",", ";", ":", "!", " ": return " "
                case "\\": return "; "
                case "|": return "‖"
                default: return String(first)
                }
            }
            let start = pos
            while pos < chars.count && TexMath.isAsciiLetter(chars[pos]) { pos += 1 }
            let name = String(chars[start..<pos])
            let spacedBefore = start >= 2 && chars[start - 2] == " "
            if !spacedBefore && pos < chars.count && chars[pos] == " " { pos += 1 }

            switch name {
            case "frac", "dfrac", "tfrac", "cfrac":
                let num = arg()
                let den = arg()
                return "\(TexMath.wrapIfCompound(num))/\(TexMath.wrapIfCompound(den))"
            case "sqrt":
                let idx = optArg()
                let body = arg()
                let root: String
                if let i = idx {
                    root = i == "3" ? "∛" : (i == "4" ? "∜" : TexMath.script(i, sup: true) + "√")
                } else {
                    root = "√"
                }
                return root + TexMath.wrapIfCompound(body)
            case "binom":
                let n = arg()
                let k = arg()
                return "C(\(n),\(k))"
            case "text", "textrm", "textbf", "textit", "mathrm", "mathbf", "mathit", "mathsf", "mathtt",
                 "operatorname", "boldsymbol", "bm", "mbox", "mathcal", "mathscr", "mathfrak":
                return arg()
            case "displaystyle", "textstyle":
                return ""
            case "mathbb":
                return arg().map { TexMath.doubleStruck($0) }.joined()
            case "hat", "widehat": return TexMath.accent(arg(), "\u{0302}")
            case "bar", "overline": return TexMath.accent(arg(), "\u{0305}")
            case "vec", "overrightarrow": return TexMath.accent(arg(), "\u{20D7}")
            case "tilde", "widetilde": return TexMath.accent(arg(), "\u{0303}")
            case "dot": return TexMath.accent(arg(), "\u{0307}")
            case "ddot": return TexMath.accent(arg(), "\u{0308}")
            case "underline": return TexMath.accent(arg(), "\u{0332}")
            case "begin":
                switch arg() {
                case "vmatrix": return "|"
                case "Vmatrix": return "‖"
                case "cases": return "{"
                default: return "["
                }
            case "end":
                switch arg() {
                case "vmatrix": return "|"
                case "Vmatrix": return "‖"
                case "cases": return ""
                default: return "]"
                }
            case "left", "right", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr", "limits", "nolimits":
                return ""
            case "quad", "qquad":
                return " "
            default:
                return TexMath.symbols[name] ?? name
            }
        }
    }

    private static func wrapIfCompound(_ s: String) -> String {
        let compound = s.count > 1 && s.contains(where: { "+-−·×/ =".contains($0) })
        return compound ? "(\(s))" : s
    }

    /// Append a combining mark after each scalar (works for single letters and short groups).
    private static func accent(_ s: String, _ mark: String) -> String {
        var out = ""
        for ch in s {
            out.append(ch)
            out += mark
        }
        return out
    }

    private static func script(_ raw: String, sup: Bool) -> String {
        let s = raw.trimmingCharacters(in: .whitespaces)
        if s.isEmpty { return "" }
        let map = sup ? supMap : subMap
        if s.allSatisfy({ map[$0] != nil }) { return String(s.map { map[$0]! }) }
        if sup && s == "⊤" { return "ᵀ" }
        if sup && ["*", "′", "″", "†", "∗"].contains(s) { return s }
        let inner = String(s.filter { $0 != supOpen && $0 != supClose && $0 != subOpen && $0 != subClose })
        return sup ? "\(supOpen)\(inner)\(supClose)" : "\(subOpen)\(inner)\(subClose)"
    }

    private static func doubleStruck(_ ch: Character) -> String {
        switch ch {
        case "R": return "ℝ"
        case "C": return "ℂ"
        case "N": return "ℕ"
        case "Z": return "ℤ"
        case "Q": return "ℚ"
        case "P": return "ℙ"
        case "H": return "ℍ"
        default: return String(ch)
        }
    }

    private static func pairs(_ keys: String, _ values: String) -> [Character: Character] {
        let k = Array(keys), v = Array(values)
        precondition(k.count == v.count, "TexMath map size mismatch")
        var m: [Character: Character] = [:]
        for i in k.indices { m[k[i]] = v[i] }
        return m
    }

    private static let supMap = pairs(
        "0123456789+-−=()abcdefghijklmnoprstuvwxyzABDEGHIJKLMNOPRTUVW⊤αβγδθφχ′ ",
        "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁⱽᵂᵀᵅᵝᵞᵟᶿᵠᵡ′ "
    )

    private static let subMap = pairs(
        "0123456789+-−=()aehijklmnoprstuvxβγρφχ, ",
        "₀₁₂₃₄₅₆₇₈₉₊₋₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓᵦᵧᵨᵩᵪ, "
    )

    private static let symbols: [String: String] = [
        "alpha": "α", "beta": "β", "gamma": "γ", "delta": "δ", "epsilon": "ϵ",
        "varepsilon": "ε", "zeta": "ζ", "eta": "η", "theta": "θ", "vartheta": "ϑ",
        "iota": "ι", "kappa": "κ", "lambda": "λ", "mu": "μ", "nu": "ν", "xi": "ξ",
        "pi": "π", "varpi": "ϖ", "rho": "ρ", "varrho": "ϱ", "sigma": "σ",
        "varsigma": "ς", "tau": "τ", "upsilon": "υ", "phi": "ϕ", "varphi": "φ",
        "chi": "χ", "psi": "ψ", "omega": "ω",
        "Gamma": "Γ", "Delta": "Δ", "Theta": "Θ", "Lambda": "Λ", "Xi": "Ξ",
        "Pi": "Π", "Sigma": "Σ", "Upsilon": "Υ", "Phi": "Φ", "Psi": "Ψ", "Omega": "Ω",
        "times": "×", "cdot": "·", "div": "÷", "pm": "±", "mp": "∓", "ast": "∗",
        "star": "⋆", "circ": "∘", "bullet": "•", "otimes": "⊗", "oplus": "⊕",
        "le": "≤", "leq": "≤", "leqslant": "≤", "ge": "≥", "geq": "≥", "geqslant": "≥",
        "ne": "≠", "neq": "≠", "approx": "≈", "equiv": "≡", "sim": "∼", "simeq": "≃",
        "cong": "≅", "propto": "∝", "ll": "≪", "gg": "≫", "lt": "<", "gt": ">",
        "infty": "∞", "partial": "∂", "nabla": "∇", "sum": "∑", "prod": "∏",
        "int": "∫", "iint": "∬", "iiint": "∭", "oint": "∮",
        "to": "→", "rightarrow": "→", "leftarrow": "←", "gets": "←", "leftrightarrow": "↔",
        "Rightarrow": "⇒", "Leftarrow": "⇐", "Leftrightarrow": "⇔",
        "implies": "⟹", "iff": "⟺", "mapsto": "↦", "longrightarrow": "⟶",
        "uparrow": "↑", "downarrow": "↓",
        "in": "∈", "notin": "∉", "ni": "∋", "subset": "⊂", "subseteq": "⊆",
        "supset": "⊃", "supseteq": "⊇", "cup": "∪", "cap": "∩", "setminus": "∖",
        "emptyset": "∅", "varnothing": "∅", "forall": "∀", "exists": "∃", "neg": "¬", "lnot": "¬",
        "land": "∧", "wedge": "∧", "lor": "∨", "vee": "∨",
        "perp": "⊥", "parallel": "∥", "angle": "∠", "triangle": "△", "top": "⊤",
        "prime": "′", "dagger": "†", "ell": "ℓ", "hbar": "ℏ", "Re": "ℜ", "Im": "ℑ",
        "ldots": "…", "dots": "…", "dotsc": "…", "cdots": "⋯", "dotsb": "⋯", "vdots": "⋮", "ddots": "⋱",
        "langle": "⟨", "rangle": "⟩", "lceil": "⌈", "rceil": "⌉", "lfloor": "⌊",
        "rfloor": "⌋", "vert": "|", "mid": "|", "Vert": "‖", "lVert": "‖", "rVert": "‖",
        "lvert": "|", "rvert": "|", "lbrace": "{", "rbrace": "}", "degree": "°",
    ]
}
