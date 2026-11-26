//! Lightweight TeX → Unicode conversion for short labels (mind map / knowledge graph nodes).
//!
//! Node labels are drawn natively on every client (WinUI, Compose, SwiftUI, Cairo), none of
//! which can typeset TeX. Converting `A=S\Lambda S^{-1}` into `A=SΛS⁻¹` in the core gives all
//! platforms readable math without per-platform renderers.

use serde_json::Value;

// Private-use markers wrapping script content that has no Unicode super/subscript form.
const SUP_OPEN: char = '\u{E000}';
const SUP_CLOSE: char = '\u{E001}';
const SUB_OPEN: char = '\u{E002}';
const SUB_CLOSE: char = '\u{E003}';

/// Convert TeX fragments in `s` (delimited by `$…$`, `\(…\)` or bare) to Unicode text.
/// Strings without any TeX marker characters are returned unchanged.
pub fn tex_to_unicode(s: &str) -> String {
    if !s.contains(['\\', '^', '_', '$', '\u{0c}', '\u{08}', '\t']) {
        return s.to_string();
    }
    let mut out = String::new();
    for run in convert_runs(s) {
        let mark = match run.shift {
            1 => "^",
            -1 => "_",
            _ => {
                out.push_str(&run.text);
                continue;
            }
        };
        if run.text.chars().count() == 1 {
            out.push_str(&format!("{mark}{}", run.text));
        } else {
            out.push_str(&format!("{mark}({})", run.text));
        }
    }
    out
}

/// A piece of converted math text. `shift` is 1 for superscript, -1 for subscript, 0 for
/// baseline; shifted runs carry content that has no Unicode script form, for UIs that can
/// raise/lower text themselves (Pango `<sup>`, Compose `BaselineShift`, SwiftUI `baselineOffset`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathRun {
    pub text: String,
    pub shift: i8,
}

/// Convert a TeX math expression (delimiters optional) into styled runs.
pub fn tex_to_runs(s: &str) -> Vec<MathRun> {
    convert_runs(s)
}

fn convert_runs(s: &str) -> Vec<MathRun> {
    let repaired = repair_json_escapes(s);
    let chars: Vec<char> = repaired.chars().collect();
    let mut pos = 0;
    let converted = collapse_spaces(&convert_seq(&chars, &mut pos, None));
    let mut runs: Vec<MathRun> = Vec::new();
    let mut cur = String::new();
    let mut shift: i8 = 0;
    for ch in converted.chars() {
        let next_shift = match ch {
            SUP_OPEN => Some(1),
            SUB_OPEN => Some(-1),
            SUP_CLOSE | SUB_CLOSE => Some(0),
            _ => None,
        };
        match next_shift {
            Some(ns) => {
                if !cur.is_empty() {
                    runs.push(MathRun {
                        text: std::mem::take(&mut cur),
                        shift,
                    });
                }
                shift = ns;
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        runs.push(MathRun { text: cur, shift });
    }
    runs
}

/// Apply [`tex_to_unicode`] to the text fields of every mind map node in `payload`.
pub fn convert_mindmap_payload(payload: &mut Value) {
    let Some(nodes) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for node in nodes {
        let Some(obj) = node.as_object_mut() else {
            continue;
        };
        for key in ["label", "summary"] {
            if let Some(Value::String(text)) = obj.get_mut(key) {
                *text = tex_to_unicode(text);
            }
        }
        for list_key in ["prerequisites", "facets"] {
            let Some(Value::Array(prereqs)) = obj.get_mut(list_key) else {
                continue;
            };
            for p in prereqs {
                if let Value::String(text) = p {
                    *text = tex_to_unicode(text);
                }
            }
        }
    }
}

/// Flashcards are drawn as plain text on every client, so TeX in them becomes Unicode.
pub fn convert_flashcards_payload(payload: &mut Value) {
    if let Some(Value::String(t)) = payload.get_mut("title") {
        *t = tex_to_unicode(t).replace('$', "");
    }
    let Some(cards) = payload.get_mut("cards").and_then(Value::as_array_mut) else {
        return;
    };
    for c in cards {
        for k in ["front", "back", "hint"] {
            if let Some(Value::String(t)) = c.get_mut(k) {
                *t = tex_to_unicode(t).replace('$', "");
            }
        }
    }
}

/// Report headings and the title are plain text (also the table of contents), so TeX in them
/// becomes Unicode; section bodies keep `$...$` for the clients' formula renderers.
pub fn convert_report_headings(payload: &mut Value) {
    if let Some(Value::String(t)) = payload.get_mut("title") {
        *t = tex_to_unicode(t).replace('$', "");
    }
    if let Some(Value::Array(sections)) = payload.get_mut("sections") {
        for sec in sections {
            if let Some(Value::String(h)) = sec.get_mut("heading") {
                *h = tex_to_unicode(h).replace('$', "");
            }
        }
    }
}

/// Slides are drawn natively (and exported to .pptx), so their visible text gets the same
/// TeX → Unicode treatment as mind map labels. Figure specs and ids are left alone.
pub fn convert_slides_payload(payload: &mut Value) {
    fn text(v: &mut Value, keys: &[&str]) {
        let Some(obj) = v.as_object_mut() else { return };
        for k in keys {
            if let Some(Value::String(t)) = obj.get_mut(*k) {
                *t = tex_to_unicode(t);
            }
        }
    }
    fn list(v: &mut Value, key: &str) {
        if let Some(Value::Array(items)) = v.get_mut(key) {
            for it in items {
                if let Value::String(t) = it {
                    *t = tex_to_unicode(t);
                }
            }
        }
    }
    text(payload, &["title", "subtitle"]);
    let Some(slides) = payload.get_mut("slides").and_then(Value::as_array_mut) else {
        return;
    };
    for sl in slides {
        text(sl, &["title", "subtitle", "callout", "speaker_notes"]);
        list(sl, "bullets");
        if let Some(Value::Array(cards)) = sl.get_mut("cards") {
            for c in cards {
                text(c, &["title", "body", "tag"]);
            }
        }
        if let Some(Value::Array(cols)) = sl.get_mut("columns") {
            for c in cols {
                text(c, &["heading"]);
                list(c, "items");
            }
        }
        if let Some(table) = sl.get_mut("table") {
            list(table, "headers");
            if let Some(Value::Array(rows)) = table.get_mut("rows") {
                for r in rows {
                    if let Value::Array(cells) = r {
                        for c in cells {
                            if let Value::String(t) = c {
                                *t = tex_to_unicode(t);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Single-backslash JSON escapes corrupt `\frac`, `\beta`, `\times`, `\rightarrow`, `\nabla`
/// into control characters; restore them.
fn repair_json_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        let next_is_letter = it.peek().is_some_and(|n| n.is_ascii_alphabetic());
        match c {
            '\u{0c}' => out.push_str("\\f"),
            '\u{08}' => out.push_str("\\b"),
            '\t' if next_is_letter => out.push_str("\\t"),
            '\r' if next_is_letter => out.push_str("\\r"),
            '\n' if next_is_letter => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(c);
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Convert until end of input or the closing `end` char (consumed).
fn convert_seq(c: &[char], pos: &mut usize, end: Option<char>) -> String {
    let mut out = String::new();
    while *pos < c.len() {
        let ch = c[*pos];
        if Some(ch) == end {
            *pos += 1;
            return out;
        }
        match ch {
            '$' => *pos += 1,
            '{' => {
                *pos += 1;
                out.push_str(&convert_seq(c, pos, Some('}')));
            }
            '}' => *pos += 1,
            '^' | '_' => {
                *pos += 1;
                let arg = read_arg(c, pos);
                out.push_str(&script(&arg, ch == '^'));
            }
            '&' => {
                *pos += 1;
                out.push(' ');
            }
            '\\' => {
                *pos += 1;
                out.push_str(&command(c, pos));
            }
            _ => {
                *pos += 1;
                out.push(ch);
            }
        }
    }
    out
}

/// Read one argument: a `{group}`, a `\command`, or a single char. Returns converted text.
fn read_arg(c: &[char], pos: &mut usize) -> String {
    while *pos < c.len() && c[*pos] == ' ' {
        *pos += 1;
    }
    if *pos >= c.len() {
        return String::new();
    }
    match c[*pos] {
        '{' => {
            *pos += 1;
            convert_seq(c, pos, Some('}'))
        }
        '\\' => {
            *pos += 1;
            command(c, pos)
        }
        ch => {
            *pos += 1;
            ch.to_string()
        }
    }
}

/// Read an optional `[...]` argument (raw, converted).
fn read_opt_arg(c: &[char], pos: &mut usize) -> Option<String> {
    if *pos < c.len() && c[*pos] == '[' {
        *pos += 1;
        Some(convert_seq(c, pos, Some(']')))
    } else {
        None
    }
}

fn command(c: &[char], pos: &mut usize) -> String {
    if *pos >= c.len() {
        return String::new();
    }
    let first = c[*pos];
    if !first.is_ascii_alphabetic() {
        *pos += 1;
        return match first {
            '(' | ')' | '[' | ']' => String::new(), // math delimiters \( \) \[ \]
            ',' | ';' | ':' | '!' | ' ' => " ".into(),
            '\\' => "; ".into(),
            '{' => "{".into(),
            '}' => "}".into(),
            '|' => "‖".into(),
            other => other.to_string(),
        };
    }
    let start = *pos;
    while *pos < c.len() && c[*pos].is_ascii_alphabetic() {
        *pos += 1;
    }
    let name: String = c[start..*pos].iter().collect();
    // A space after a letter command only terminates it (`S\Lambda S` → `SΛS`), unless the
    // command was itself preceded by a space (`(A) \neq 0` keeps symmetric spacing).
    let spaced_before = start >= 2 && c[start - 2] == ' ';
    if !spaced_before && *pos < c.len() && c[*pos] == ' ' {
        *pos += 1;
    }

    match name.as_str() {
        "frac" | "dfrac" | "tfrac" | "cfrac" => {
            let num = read_arg(c, pos);
            let den = read_arg(c, pos);
            format!("{}/{}", wrap_if_compound(&num), wrap_if_compound(&den))
        }
        "sqrt" => {
            let idx = read_opt_arg(c, pos);
            let body = read_arg(c, pos);
            let root = match idx.as_deref() {
                Some("3") => "∛".to_string(),
                Some("4") => "∜".to_string(),
                Some(n) => format!("{}√", script(n, true)),
                None => "√".to_string(),
            };
            format!("{root}{}", wrap_if_compound(&body))
        }
        "binom" => {
            let n = read_arg(c, pos);
            let k = read_arg(c, pos);
            format!("C({n},{k})")
        }
        "text" | "textrm" | "textbf" | "textit" | "mathrm" | "mathbf" | "mathit" | "mathsf"
        | "mathtt" | "operatorname" | "boldsymbol" | "bm" | "mbox" | "mathcal" | "mathscr"
        | "mathfrak" | "displaystyle" | "textstyle" => {
            if matches!(name.as_str(), "displaystyle" | "textstyle") {
                String::new()
            } else {
                read_arg(c, pos)
            }
        }
        "mathbb" => read_arg(c, pos).chars().map(double_struck).collect(),
        "hat" | "widehat" => accent(&read_arg(c, pos), '\u{0302}'),
        "bar" | "overline" => accent(&read_arg(c, pos), '\u{0305}'),
        "vec" | "overrightarrow" => accent(&read_arg(c, pos), '\u{20D7}'),
        "tilde" | "widetilde" => accent(&read_arg(c, pos), '\u{0303}'),
        "dot" => accent(&read_arg(c, pos), '\u{0307}'),
        "ddot" => accent(&read_arg(c, pos), '\u{0308}'),
        "underline" => accent(&read_arg(c, pos), '\u{0332}'),
        "begin" => {
            let env = read_arg(c, pos);
            match env.as_str() {
                "vmatrix" => "|".into(),
                "Vmatrix" => "‖".into(),
                "cases" => "{".into(),
                _ => "[".into(),
            }
        }
        "end" => {
            let env = read_arg(c, pos);
            match env.as_str() {
                "vmatrix" => "|".into(),
                "Vmatrix" => "‖".into(),
                "cases" => String::new(),
                _ => "]".into(),
            }
        }
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr"
        | "limits" | "nolimits" => String::new(),
        "quad" | "qquad" => " ".into(),
        _ => symbol(&name).map(str::to_string).unwrap_or(name),
    }
}

/// Parenthesise multi-term fraction / root operands: `\frac{a+b}{2}` → `(a+b)/2`.
fn wrap_if_compound(s: &str) -> String {
    let compound = s.chars().count() > 1
        && s.chars()
            .any(|ch| matches!(ch, '+' | '-' | '−' | '·' | '×' | '/' | ' ' | '='));
    if compound {
        format!("({s})")
    } else {
        s.to_string()
    }
}

fn accent(s: &str, mark: char) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        out.push(mark);
    }
    out
}

/// Map to Unicode super/subscript; fall back to `^(…)` / `_(…)` when a char has no form.
fn script(s: &str, sup: bool) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let mapped: Option<String> = s
        .chars()
        .map(|ch| if sup { sup_char(ch) } else { sub_char(ch) })
        .collect();
    match mapped {
        Some(m) => m,
        None if sup && matches!(s, "*" | "′" | "″" | "†" | "∗" | "⊤") => {
            if s == "⊤" {
                "ᵀ".into()
            } else {
                s.to_string()
            }
        }
        None => {
            // Nested fallbacks flatten into the outer shifted run.
            let inner: String = s
                .chars()
                .filter(|c| !matches!(*c, SUP_OPEN | SUP_CLOSE | SUB_OPEN | SUB_CLOSE))
                .collect();
            let (open, close) = if sup {
                (SUP_OPEN, SUP_CLOSE)
            } else {
                (SUB_OPEN, SUB_CLOSE)
            };
            format!("{open}{inner}{close}")
        }
    }
}

fn sup_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' | '−' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'f' => 'ᶠ',
        'g' => 'ᵍ',
        'h' => 'ʰ',
        'i' => 'ⁱ',
        'j' => 'ʲ',
        'k' => 'ᵏ',
        'l' => 'ˡ',
        'm' => 'ᵐ',
        'n' => 'ⁿ',
        'o' => 'ᵒ',
        'p' => 'ᵖ',
        'r' => 'ʳ',
        's' => 'ˢ',
        't' => 'ᵗ',
        'u' => 'ᵘ',
        'v' => 'ᵛ',
        'w' => 'ʷ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'z' => 'ᶻ',
        'A' => 'ᴬ',
        'B' => 'ᴮ',
        'D' => 'ᴰ',
        'E' => 'ᴱ',
        'G' => 'ᴳ',
        'H' => 'ᴴ',
        'I' => 'ᴵ',
        'J' => 'ᴶ',
        'K' => 'ᴷ',
        'L' => 'ᴸ',
        'M' => 'ᴹ',
        'N' => 'ᴺ',
        'O' => 'ᴼ',
        'P' => 'ᴾ',
        'R' => 'ᴿ',
        'T' | '⊤' => 'ᵀ',
        'U' => 'ᵁ',
        'V' => 'ⱽ',
        'W' => 'ᵂ',
        'α' => 'ᵅ',
        'β' => 'ᵝ',
        'γ' => 'ᵞ',
        'δ' => 'ᵟ',
        'θ' => 'ᶿ',
        'φ' => 'ᵠ',
        'χ' => 'ᵡ',
        '′' => '′',
        ' ' => ' ',
        _ => return None,
    })
}

fn sub_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' | '−' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        'β' => 'ᵦ',
        'γ' => 'ᵧ',
        'ρ' => 'ᵨ',
        'φ' => 'ᵩ',
        'χ' => 'ᵪ',
        ',' => ',',
        ' ' => ' ',
        _ => return None,
    })
}

fn double_struck(ch: char) -> char {
    match ch {
        'R' => 'ℝ',
        'C' => 'ℂ',
        'N' => 'ℕ',
        'Z' => 'ℤ',
        'Q' => 'ℚ',
        'P' => 'ℙ',
        'H' => 'ℍ',
        'E' => '𝔼',
        'F' => '𝔽',
        '1' => '𝟙',
        other => other,
    }
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "ϕ",
        "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "times" => "×",
        "cdot" => "·",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "ast" => "∗",
        "star" => "⋆",
        "circ" => "∘",
        "bullet" => "•",
        "otimes" => "⊗",
        "oplus" => "⊕",
        "le" | "leq" | "leqslant" => "≤",
        "ge" | "geq" | "geqslant" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "simeq" => "≃",
        "cong" => "≅",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",
        "lt" => "<",
        "gt" => ">",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "sum" => "∑",
        "prod" => "∏",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" => "⇔",
        "implies" => "⟹",
        "iff" => "⟺",
        "mapsto" => "↦",
        "longrightarrow" => "⟶",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "perp" => "⊥",
        "parallel" => "∥",
        "angle" => "∠",
        "triangle" => "△",
        "top" => "⊤",
        "prime" => "′",
        "dagger" => "†",
        "ell" => "ℓ",
        "hbar" => "ℏ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "ldots" | "dots" | "dotsc" => "…",
        "cdots" | "dotsb" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "vert" | "mid" => "|",
        "Vert" | "lVert" | "rVert" => "‖",
        "lvert" | "rvert" => "|",
        "lbrace" => "{",
        "rbrace" => "}",
        "degree" => "°",
        "det" => "det",
        "dim" => "dim",
        "ker" => "ker",
        "rank" => "rank",
        "tr" => "tr",
        "lim" => "lim",
        "log" => "log",
        "ln" => "ln",
        "exp" => "exp",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "max" => "max",
        "min" => "min",
        "sup" => "sup",
        "inf" => "inf",
        "arg" => "arg",
        "span" => "span",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::tex_to_unicode as t;

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(t("Gram-Schmidt 与 QR 分解"), "Gram-Schmidt 与 QR 分解");
        assert_eq!(t("第1~3章"), "第1~3章");
    }

    #[test]
    fn bare_and_delimited_math() {
        assert_eq!(t(r"对角化 A=S\Lambda S^{-1}"), "对角化 A=SΛS⁻¹");
        assert_eq!(t(r"对角化 $A=S\Lambda S^{-1}$"), "对角化 A=SΛS⁻¹");
        assert_eq!(t(r"正规方程 \(A^TA\hat{x}=A^Tb\)"), "正规方程 AᵀAx̂=Aᵀb");
        assert_eq!(t("A^T"), "Aᵀ");
        assert_eq!(t("x_{ij}"), "xᵢⱼ");
        assert_eq!(t("x_1 + x_2"), "x₁ + x₂");
    }

    #[test]
    fn fractions_roots_and_fallbacks() {
        assert_eq!(t(r"\frac{a+b}{2}"), "(a+b)/2");
        assert_eq!(t(r"\frac{1}{n}"), "1/n");
        assert_eq!(t(r"\sqrt{2}"), "√2");
        assert_eq!(t(r"\mathbb{R}^n"), "ℝⁿ");
        assert_eq!(t("x_{q}"), "x_q");
        assert_eq!(t(r"\det(A) \neq 0"), "det(A) ≠ 0");
    }

    #[test]
    fn runs_mark_unmappable_scripts() {
        use super::{tex_to_runs, MathRun};
        let r = |t: &str, s: i8| MathRun {
            text: t.into(),
            shift: s,
        };
        assert_eq!(tex_to_runs("e^{i\\pi}"), vec![r("e", 0), r("iπ", 1)]);
        assert_eq!(
            tex_to_runs("x_{q}+1"),
            vec![r("x", 0), r("q", -1), r("+1", 0)]
        );
        assert_eq!(tex_to_runs("A^{-1}"), vec![r("A⁻¹", 0)]);
    }

    #[test]
    fn repairs_json_escape_damage() {
        assert_eq!(t("\u{0c}rac{1}{2}"), "1/2");
        assert_eq!(t("A\times B"), "A×B");
    }

    #[test]
    fn slides_payload_text_becomes_unicode() {
        let mut p = serde_json::json!({
            "title": "R^n 与 $A^T$",
            "slides": [{
                "title": "N(A)⊆R^n",
                "bullets": ["dim $N(A)=n-r$"],
                "cards": [{"title": "t", "body": "$x_1$"}],
                "columns": [{"heading": "h", "items": ["C(A^T)"]}],
                "table": {"headers": ["a"], "rows": [["R^m"]]},
                "figure": {"expr": "x^2+y^2"}
            }]
        });
        super::convert_slides_payload(&mut p);
        assert_eq!(p["title"], "Rⁿ 与 Aᵀ");
        assert_eq!(p["slides"][0]["title"], "N(A)⊆Rⁿ");
        assert_eq!(p["slides"][0]["cards"][0]["body"], "x₁");
        assert_eq!(p["slides"][0]["columns"][0]["items"][0], "C(Aᵀ)");
        assert_eq!(p["slides"][0]["table"]["rows"][0][0], "Rᵐ");
        // figure specs are not text and must stay untouched
        assert_eq!(p["slides"][0]["figure"]["expr"], "x^2+y^2");
    }

    #[test]
    fn report_headings_lose_dollars() {
        let mut p = serde_json::json!({
            "title": "$\\mathbb{R}^n$ 与 $N(A)$",
            "sections": [{"heading": "零空间 $N(A)$ 与 $Ax=0$", "body_md": "$x_1$"}]
        });
        super::convert_report_headings(&mut p);
        assert_eq!(p["title"], "ℝⁿ 与 N(A)");
        assert_eq!(p["sections"][0]["heading"], "零空间 N(A) 与 Ax=0");
        assert_eq!(p["sections"][0]["body_md"], "$x_1$");
    }

    #[test]
    fn flashcards_become_plain_unicode() {
        let mut p = serde_json::json!({"cards": [{"front": "什么是 $V$ 的子空间", "back": "$\\mathbb{R}^3$ 中的 $N(A)$", "hint": "x^2"}]});
        super::convert_flashcards_payload(&mut p);
        assert_eq!(p["cards"][0]["front"], "什么是 V 的子空间");
        assert_eq!(p["cards"][0]["back"], "ℝ³ 中的 N(A)");
        assert_eq!(p["cards"][0]["hint"], "x²");
    }
}
