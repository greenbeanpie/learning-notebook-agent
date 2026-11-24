import Foundation

/// Minimal Markdown parser for chat answers, guides and Studio text.
///
/// Same grammar as the Android (MdParser.kt) and Linux (md_view.rs) clients:
/// headings, paragraphs (line breaks kept), nested bullet/ordered lists, quotes, fenced code,
/// display math ($$…$$ / \[…\]), rules, pipe tables; inline **bold**, *italic*, ~~strike~~,
/// `code`, $math$ / \(math\), [text](url) and [n] citations.
enum MdParser {
    enum Block: Equatable {
        case heading(level: Int, text: String)
        case paragraph(String)
        case listItem(ordered: Bool, marker: String, depth: Int, text: String)
        case quote(String)
        case code(String)
        case math(String)
        case rule
        case table(header: [String], rows: [[String]])
    }

    enum Span: Equatable {
        case text(String, bold: Bool, italic: Bool, strike: Bool)
        case code(String)
        case math(String)
        case link(String, url: String)
        case cite(Int)
    }

    private static func re(_ p: String) -> NSRegularExpression {
        // Patterns are constants; a failure here is a programming error.
        return try! NSRegularExpression(pattern: p)
    }

    private static let headingRe = re("^(#{1,6})\\s+(.*?)\\s*#*\\s*$")
    private static let ruleRe = re("^\\s{0,3}([-*_])(\\s*\\1){2,}\\s*$")
    private static let listRe = re("^(\\s*)([-*+•]|\\d{1,3}[.)])\\s+(.*)$")
    private static let tableSepRe = re("^\\s*\\|?\\s*:?-{2,}:?\\s*(\\|\\s*:?-{2,}:?\\s*)*\\|?\\s*$")

    /// Capture groups of a whole-string match, or nil.
    private static func groups(_ r: NSRegularExpression, _ s: String) -> [String]? {
        let ns = s as NSString
        guard let m = r.firstMatch(in: s, range: NSRange(location: 0, length: ns.length)),
              m.range.location == 0, m.range.length == ns.length else { return nil }
        return (0..<m.numberOfRanges).map { i in
            let rg = m.range(at: i)
            return rg.location == NSNotFound ? "" : ns.substring(with: rg)
        }
    }

    private static func matches(_ r: NSRegularExpression, _ s: String) -> Bool { groups(r, s) != nil }

    static func parse(_ src: String) -> [Block] {
        let lines = src.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .components(separatedBy: "\n")
        var out: [Block] = []
        var para = ""
        func flushPara() {
            let t = para.trimmingCharacters(in: .newlines)
            if !t.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { out.append(.paragraph(t)) }
            para = ""
        }
        var i = 0
        while i < lines.count {
            let line = lines[i]
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") {
                flushPara()
                let fence = String(t.prefix(3))
                var body: [String] = []
                i += 1
                while i < lines.count && !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(fence) {
                    body.append(lines[i])
                    i += 1
                }
                out.append(.code(body.joined(separator: "\n")))
            } else if t.hasPrefix("$$") || t.hasPrefix("\\[") {
                flushPara()
                let close = t.hasPrefix("$$") ? "$$" : "\\]"
                let rest = String(t.dropFirst(2))
                if let r = rest.range(of: close) {
                    out.append(.math(String(rest[..<r.lowerBound]).trimmingCharacters(in: .whitespaces)))
                    let tail = String(rest[r.upperBound...]).trimmingCharacters(in: .whitespaces)
                    if !tail.isEmpty { para += tail }
                } else {
                    var body = rest
                    i += 1
                    while i < lines.count {
                        let l = lines[i]
                        if let r = l.range(of: close) {
                            body += " " + String(l[..<r.lowerBound])
                            break
                        }
                        body += " " + l
                        i += 1
                    }
                    out.append(.math(body.trimmingCharacters(in: .whitespaces)))
                }
            } else if t.isEmpty {
                flushPara()
            } else if let g = groups(headingRe, t), !line.hasPrefix("    ") {
                flushPara()
                out.append(.heading(level: g[1].count, text: g[2]))
            } else if matches(ruleRe, line) {
                flushPara()
                out.append(.rule)
            } else if t.hasPrefix("|") && i + 1 < lines.count && matches(tableSepRe, lines[i + 1]) {
                flushPara()
                let header = splitRow(t)
                var rows: [[String]] = []
                i += 2
                while i < lines.count && lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("|") {
                    rows.append(splitRow(lines[i].trimmingCharacters(in: .whitespaces)))
                    i += 1
                }
                out.append(.table(header: header, rows: rows))
                continue
            } else if let g = groups(listRe, line) {
                flushPara()
                let indent = g[1].replacingOccurrences(of: "\t", with: "    ").count
                let marker = g[2]
                let ordered = marker.first?.isNumber ?? false
                var text = g[3]
                while i + 1 < lines.count {
                    let next = lines[i + 1]
                    let nt = next.trimmingCharacters(in: .whitespaces)
                    if nt.isEmpty || matches(listRe, next) || nt.hasPrefix("#") || nt.hasPrefix(">")
                        || nt.hasPrefix("```") || nt.hasPrefix("$$") || nt.hasPrefix("|") { break }
                    text += "\n" + nt
                    i += 1
                }
                let shown = ordered ? marker.trimmingCharacters(in: CharacterSet(charactersIn: ".)")) + "." : "•"
                out.append(.listItem(ordered: ordered, marker: shown, depth: min(indent / 2, 4), text: text))
            } else if t.hasPrefix(">") {
                flushPara()
                var body: [String] = []
                while i < lines.count && lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(">") {
                    let l = lines[i].trimmingCharacters(in: .whitespaces).dropFirst()
                    body.append(String(l).trimmingCharacters(in: .whitespaces))
                    i += 1
                }
                out.append(.quote(body.joined(separator: "\n")))
                continue
            } else {
                if !para.isEmpty { para += "\n" }
                para += t
            }
            i += 1
        }
        flushPara()
        return out
    }

    private static func splitRow(_ row: String) -> [String] {
        var r = row.trimmingCharacters(in: .whitespaces)
        if r.hasPrefix("|") { r.removeFirst() }
        if r.hasSuffix("|") { r.removeLast() }
        return r.components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
    }

    // MARK: - Inline

    static func parseInline(_ text: String) -> [Span] {
        var out: [Span] = []
        parseInline(Array(text), bold: false, italic: false, strike: false, into: &out)
        return out
    }

    private static func indexOf(_ c: [Character], _ needle: String, from: Int) -> Int {
        let n = Array(needle)
        guard !n.isEmpty, from >= 0 else { return -1 }
        var i = from
        outer: while i + n.count <= c.count {
            for k in 0..<n.count where c[i + k] != n[k] {
                i += 1
                continue outer
            }
            return i
        }
        return -1
    }

    private static func sub(_ c: [Character], _ a: Int, _ b: Int) -> String { String(c[a..<b]) }

    private static func isWordBoundary(_ c: [Character], _ idx: Int) -> Bool {
        idx < 0 || idx >= c.count || !(c[idx].isLetter || c[idx].isNumber)
    }

    private static func parseInline(_ c: [Character], bold: Bool, italic: Bool, strike: Bool, into out: inout [Span]) {
        var buf = ""
        func flush() {
            if !buf.isEmpty { out.append(.text(buf, bold: bold, italic: italic, strike: strike)) }
            buf = ""
        }
        let n = c.count
        var i = 0
        while i < n {
            let ch = c[i]
            let next: Character = i + 1 < n ? c[i + 1] : "\u{0}"
            if ch == "\\" && (next == "(" || next == "[") {
                let close = next == "(" ? "\\)" : "\\]"
                let end = indexOf(c, close, from: i + 2)
                if end > i + 2 {
                    flush()
                    out.append(.math(sub(c, i + 2, end).trimmingCharacters(in: .whitespaces)))
                    i = end + 2
                    continue
                }
            }
            if ch == "\\" && next != "\u{0}" && "\\`*_{}[]()#+-.!|~$>".contains(next) {
                buf.append(next)
                i += 2
                continue
            }
            if ch == "`" {
                let end = indexOf(c, "`", from: i + 1)
                if end > i {
                    flush()
                    out.append(.code(sub(c, i + 1, end)))
                    i = end + 1
                    continue
                }
            }
            if ch == "$" {
                if next == "$" {
                    let end = indexOf(c, "$$", from: i + 2)
                    if end > i + 2 {
                        flush()
                        out.append(.math(sub(c, i + 2, end).trimmingCharacters(in: .whitespaces)))
                        i = end + 2
                        continue
                    }
                } else {
                    let end = findDollarEnd(c, i + 1)
                    if end > 0 {
                        flush()
                        out.append(.math(sub(c, i + 1, end)))
                        i = end + 1
                        continue
                    }
                }
            }
            if (ch == "*" || ch == "_") && next == ch {
                let end = indexOf(c, String([ch, ch]), from: i + 2)
                if end > i + 2 && (ch == "*" || isWordBoundary(c, i - 1)) {
                    flush()
                    parseInline(Array(c[(i + 2)..<end]), bold: true, italic: italic, strike: strike, into: &out)
                    i = end + 2
                    continue
                }
            }
            if (ch == "*" || ch == "_") && next != ch && next != " " && next != "\u{0}" {
                let end = findSingleDelim(c, i + 1, ch)
                if end > i + 1 && (ch == "*" || (isWordBoundary(c, i - 1) && isWordBoundary(c, end + 1))) {
                    flush()
                    parseInline(Array(c[(i + 1)..<end]), bold: bold, italic: true, strike: strike, into: &out)
                    i = end + 1
                    continue
                }
            }
            if ch == "~" && next == "~" {
                let end = indexOf(c, "~~", from: i + 2)
                if end > i + 2 {
                    flush()
                    parseInline(Array(c[(i + 2)..<end]), bold: bold, italic: italic, strike: true, into: &out)
                    i = end + 2
                    continue
                }
            }
            if ch == "[" || ch == "【" {
                let closeCh: String = ch == "[" ? "]" : "】"
                let close = indexOf(c, closeCh, from: i + 1)
                if close > i + 1 {
                    let inner = sub(c, i + 1, close)
                    if let num = Int(inner.trimmingCharacters(in: .whitespaces)), num > 0, inner.count <= 4 {
                        if buf.hasSuffix(" ") { buf.removeLast() }
                        flush()
                        out.append(.cite(num))
                        i = close + 1
                        continue
                    }
                    if ch == "[" && close + 1 < n && c[close + 1] == "(" {
                        let urlEnd = indexOf(c, ")", from: close + 2)
                        if urlEnd > close {
                            flush()
                            out.append(.link(inner, url: sub(c, close + 2, urlEnd)))
                            i = urlEnd + 1
                            continue
                        }
                    }
                }
            }
            buf.append(ch)
            i += 1
        }
        flush()
    }

    private static func findSingleDelim(_ c: [Character], _ start: Int, _ d: Character) -> Int {
        var j = start
        while j < c.count {
            let ch = c[j]
            if ch == "\n" { return -1 }
            if ch == d {
                if j + 1 < c.count && c[j + 1] == d { j += 2; continue }
                return c[j - 1] != " " ? j : -1
            }
            j += 1
        }
        return -1
    }

    /// Closing `$` of inline math, rejecting `$ 5` style currency and empty spans.
    private static func findDollarEnd(_ c: [Character], _ start: Int) -> Int {
        if start >= c.count || c[start].isWhitespace { return -1 }
        var j = start
        while j < c.count {
            let ch = c[j]
            if ch == "\n" { return -1 }
            if ch == "\\" && j + 1 < c.count { j += 2; continue }
            if ch == "$" {
                if j + 1 < c.count && c[j + 1] == "$" { return -1 }
                if j > start && !c[j - 1].isWhitespace {
                    if j + 1 < c.count && c[j + 1].isNumber { return -1 }
                    return j
                }
                return -1
            }
            j += 1
        }
        return -1
    }
}
