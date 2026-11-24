//! Minimal Markdown parser shared by native clients that cannot embed a web renderer.
//!
//! Same grammar as the Android (`MdParser.kt`) and macOS (`MdParser.swift`) ports:
//! ATX headings, paragraphs (line breaks kept), nested bullet/ordered lists, block quotes,
//! fenced code, display math (`$$…$$` / `\[…\]`), rules and pipe tables; inline `**bold**`,
//! `*italic*`, `~~strike~~`, `` `code` ``, `$math$` / `\(math\)`, `[text](url)` and `[n]` citations.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: usize, text: String },
    Paragraph(String),
    ListItem { ordered: bool, marker: String, depth: usize, text: String },
    Quote(String),
    Code(String),
    Math(String),
    Rule,
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text { text: String, bold: bool, italic: bool, strike: bool },
    Code(String),
    Math(String),
    Link { text: String, url: String },
    Cite(u32),
}

fn heading(t: &str) -> Option<(usize, String)> {
    let level = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end();
    Some((level, text.to_string()))
}

fn is_rule(line: &str) -> bool {
    let lead = line.len() - line.trim_start().len();
    if lead > 3 {
        return false;
    }
    let t: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && matches!(t[0], '-' | '*' | '_') && t.iter().all(|c| *c == t[0])
}

/// `(indent, marker, text)` of a list item line.
fn list_item(line: &str) -> Option<(usize, String, String)> {
    let indent_str: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let rest = &line[indent_str.len()..];
    let indent = indent_str.replace('\t', "    ").len();
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    let marker_end = if matches!(first, '-' | '*' | '+' | '•') {
        first.len_utf8()
    } else if first.is_ascii_digit() {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 3 {
            return None;
        }
        match rest[digits..].chars().next() {
            Some('.') | Some(')') => digits + 1,
            _ => return None,
        }
    } else {
        return None;
    };
    let after = &rest[marker_end..];
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim_start();
    if text.is_empty() {
        return None;
    }
    Some((indent, rest[..marker_end].to_string(), text.to_string()))
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    let cells: Vec<&str> = t.split('|').collect();
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim().trim_start_matches(':').trim_end_matches(':');
            c.len() >= 2 && c.chars().all(|ch| ch == '-')
        })
}

fn split_row(row: &str) -> Vec<String> {
    let t = row.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

pub fn parse(src: &str) -> Vec<Block> {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out = Vec::new();
    let mut para = String::new();
    fn flush(para: &mut String, out: &mut Vec<Block>) {
        let t = para.trim_matches('\n');
        if !t.trim().is_empty() {
            out.push(Block::Paragraph(t.to_string()));
        }
        para.clear();
    }
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut para, &mut out);
            let fence = &t[..3];
            let mut body: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                body.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code(body.join("\n")));
        } else if t.starts_with("$$") || t.starts_with("\\[") {
            flush(&mut para, &mut out);
            let close = if t.starts_with("$$") { "$$" } else { "\\]" };
            let rest = &t[2..];
            if let Some(end) = rest.find(close) {
                out.push(Block::Math(rest[..end].trim().to_string()));
                let tail = rest[end + 2..].trim();
                if !tail.is_empty() {
                    para.push_str(tail);
                }
            } else {
                let mut body = rest.to_string();
                i += 1;
                while i < lines.len() {
                    let l = lines[i];
                    if let Some(k) = l.find(close) {
                        body.push(' ');
                        body.push_str(&l[..k]);
                        break;
                    }
                    body.push(' ');
                    body.push_str(l);
                    i += 1;
                }
                out.push(Block::Math(body.trim().to_string()));
            }
        } else if t.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some((level, text)) = heading(t).filter(|_| !line.starts_with("    ")) {
            flush(&mut para, &mut out);
            out.push(Block::Heading { level, text });
        } else if is_rule(line) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if t.starts_with('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            flush(&mut para, &mut out);
            let header = split_row(t);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(split_row(lines[i]));
                i += 1;
            }
            out.push(Block::Table { header, rows });
            continue;
        } else if let Some((indent, marker, first)) = list_item(line) {
            flush(&mut para, &mut out);
            let ordered = marker.starts_with(|c: char| c.is_ascii_digit());
            let mut text = first;
            while i + 1 < lines.len() {
                let next = lines[i + 1];
                let nt = next.trim();
                if nt.is_empty()
                    || list_item(next).is_some()
                    || nt.starts_with('#')
                    || nt.starts_with('>')
                    || nt.starts_with("```")
                    || nt.starts_with("$$")
                    || nt.starts_with('|')
                {
                    break;
                }
                text.push('\n');
                text.push_str(nt);
                i += 1;
            }
            let shown = if ordered {
                format!("{}.", marker.trim_end_matches([')', '.']))
            } else {
                "•".to_string()
            };
            out.push(Block::ListItem { ordered, marker: shown, depth: (indent / 2).min(4), text });
        } else if t.starts_with('>') {
            flush(&mut para, &mut out);
            let mut body: Vec<String> = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('>') {
                body.push(lines[i].trim()[1..].trim_start().to_string());
                i += 1;
            }
            out.push(Block::Quote(body.join("\n")));
            continue;
        } else {
            if !para.is_empty() {
                para.push('\n');
            }
            para.push_str(t);
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

pub fn parse_inline(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    parse_inline_into(&chars, false, false, false, &mut out);
    out
}

fn index_of(c: &[char], needle: &str, from: usize) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || from > c.len() {
        return None;
    }
    (from..=c.len().saturating_sub(n.len())).find(|&i| c[i..i + n.len()] == n[..])
}

fn sub(c: &[char], a: usize, b: usize) -> String {
    c[a..b].iter().collect()
}

fn is_word_boundary(c: &[char], idx: isize) -> bool {
    idx < 0 || idx as usize >= c.len() || !c[idx as usize].is_alphanumeric()
}

fn parse_inline_into(c: &[char], bold: bool, italic: bool, strike: bool, out: &mut Vec<Span>) {
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Span>| {
        if !buf.is_empty() {
            out.push(Span::Text { text: std::mem::take(buf), bold, italic, strike });
        }
    };
    let n = c.len();
    let mut i = 0;
    while i < n {
        let ch = c[i];
        let next = if i + 1 < n { c[i + 1] } else { '\0' };
        if ch == '\\' && (next == '(' || next == '[') {
            let close = if next == '(' { "\\)" } else { "\\]" };
            if let Some(end) = index_of(c, close, i + 2).filter(|&e| e > i + 2) {
                flush(&mut buf, out);
                out.push(Span::Math(sub(c, i + 2, end).trim().to_string()));
                i = end + 2;
                continue;
            }
        }
        if ch == '\\' && next != '\0' && "\\`*_{}[]()#+-.!|~$>".contains(next) {
            buf.push(next);
            i += 2;
            continue;
        }
        if ch == '`' {
            if let Some(end) = index_of(c, "`", i + 1) {
                flush(&mut buf, out);
                out.push(Span::Code(sub(c, i + 1, end)));
                i = end + 1;
                continue;
            }
        }
        if ch == '$' {
            if next == '$' {
                if let Some(end) = index_of(c, "$$", i + 2).filter(|&e| e > i + 2) {
                    flush(&mut buf, out);
                    out.push(Span::Math(sub(c, i + 2, end).trim().to_string()));
                    i = end + 2;
                    continue;
                }
            } else if let Some(end) = find_dollar_end(c, i + 1) {
                flush(&mut buf, out);
                out.push(Span::Math(sub(c, i + 1, end)));
                i = end + 1;
                continue;
            }
        }
        if (ch == '*' || ch == '_') && next == ch {
            let delim: String = [ch, ch].iter().collect();
            if let Some(end) = index_of(c, &delim, i + 2).filter(|&e| e > i + 2) {
                if ch == '*' || is_word_boundary(c, i as isize - 1) {
                    flush(&mut buf, out);
                    parse_inline_into(&c[i + 2..end], true, italic, strike, out);
                    i = end + 2;
                    continue;
                }
            }
        }
        if (ch == '*' || ch == '_') && next != ch && next != ' ' && next != '\0' {
            if let Some(end) = find_single_delim(c, i + 1, ch).filter(|&e| e > i + 1) {
                if ch == '*' || (is_word_boundary(c, i as isize - 1) && is_word_boundary(c, end as isize + 1)) {
                    flush(&mut buf, out);
                    parse_inline_into(&c[i + 1..end], bold, true, strike, out);
                    i = end + 1;
                    continue;
                }
            }
        }
        if ch == '~' && next == '~' {
            if let Some(end) = index_of(c, "~~", i + 2).filter(|&e| e > i + 2) {
                flush(&mut buf, out);
                parse_inline_into(&c[i + 2..end], bold, italic, true, out);
                i = end + 2;
                continue;
            }
        }
        if ch == '[' || ch == '【' {
            let close_ch = if ch == '[' { "]" } else { "】" };
            if let Some(close) = index_of(c, close_ch, i + 1).filter(|&e| e > i + 1) {
                let inner = sub(c, i + 1, close);
                if let Ok(num) = inner.trim().parse::<u32>() {
                    if num > 0 && inner.chars().count() <= 4 {
                        if buf.ends_with(' ') {
                            buf.pop();
                        }
                        flush(&mut buf, out);
                        out.push(Span::Cite(num));
                        i = close + 1;
                        continue;
                    }
                }
                if ch == '[' && close + 1 < n && c[close + 1] == '(' {
                    if let Some(url_end) = index_of(c, ")", close + 2) {
                        flush(&mut buf, out);
                        out.push(Span::Link { text: inner, url: sub(c, close + 2, url_end) });
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        buf.push(ch);
        i += 1;
    }
    flush(&mut buf, out);
}

fn find_single_delim(c: &[char], start: usize, d: char) -> Option<usize> {
    let mut j = start;
    while j < c.len() {
        let ch = c[j];
        if ch == '\n' {
            return None;
        }
        if ch == d {
            if j + 1 < c.len() && c[j + 1] == d {
                j += 2;
                continue;
            }
            return (c[j - 1] != ' ').then_some(j);
        }
        j += 1;
    }
    None
}

/// Closing `$` of inline math, rejecting `$ 5` style currency and empty spans.
fn find_dollar_end(c: &[char], start: usize) -> Option<usize> {
    if start >= c.len() || c[start].is_whitespace() {
        return None;
    }
    let mut j = start;
    while j < c.len() {
        let ch = c[j];
        if ch == '\n' {
            return None;
        }
        if ch == '\\' && j + 1 < c.len() {
            j += 2;
            continue;
        }
        if ch == '$' {
            if j + 1 < c.len() && c[j + 1] == '$' {
                return None;
            }
            if j > start && !c[j - 1].is_whitespace() {
                if j + 1 < c.len() && c[j + 1].is_ascii_digit() {
                    return None;
                }
                return Some(j);
            }
            return None;
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# 标题\n段落 **粗体** 和 *斜体*，公式 $A^{-1}$ 与 \\(x_1\\)，价格 $5 和 $6。引用[1]。见[文档](http://x)。snake_case_name\n第二行\n\n- 项一\n  - 子项 `code`\n1. 有序\n\n$$\n\\frac{1}{n}\\sum_{i=1}^n x_i\n$$\n\n| a | b |\n|---|---|\n| 1 | $x^2$ |\n\n> 引用块\n\n---\n```\ncode block\n```\n";

    #[test]
    fn parses_blocks_like_the_mobile_ports() {
        let b = parse(SAMPLE);
        assert_eq!(b[0], Block::Heading { level: 1, text: "标题".into() });
        assert!(matches!(&b[1], Block::Paragraph(p) if p.ends_with("snake_case_name\n第二行")));
        assert_eq!(b[2], Block::ListItem { ordered: false, marker: "•".into(), depth: 0, text: "项一".into() });
        assert_eq!(b[3], Block::ListItem { ordered: false, marker: "•".into(), depth: 1, text: "子项 `code`".into() });
        assert_eq!(b[4], Block::ListItem { ordered: true, marker: "1.".into(), depth: 0, text: "有序".into() });
        assert_eq!(b[5], Block::Math("\\frac{1}{n}\\sum_{i=1}^n x_i".into()));
        assert_eq!(
            b[6],
            Block::Table { header: vec!["a".into(), "b".into()], rows: vec![vec!["1".into(), "$x^2$".into()]] }
        );
        assert_eq!(b[7], Block::Quote("引用块".into()));
        assert_eq!(b[8], Block::Rule);
        assert_eq!(b[9], Block::Code("code block".into()));
        assert_eq!(b.len(), 10);
    }

    #[test]
    fn parses_inline_spans() {
        let line = SAMPLE.lines().nth(1).unwrap();
        let s = parse_inline(line);
        let t = |x: &str, b: bool, i: bool| Span::Text { text: x.into(), bold: b, italic: i, strike: false };
        assert_eq!(
            s,
            vec![
                t("段落 ", false, false),
                t("粗体", true, false),
                t(" 和 ", false, false),
                t("斜体", false, true),
                t("，公式 ", false, false),
                Span::Math("A^{-1}".into()),
                t(" 与 ", false, false),
                Span::Math("x_1".into()),
                t("，价格 $5 和 $6。引用", false, false),
                Span::Cite(1),
                t("。见", false, false),
                Span::Link { text: "文档".into(), url: "http://x".into() },
                t("。snake_case_name", false, false),
            ]
        );
    }
}
