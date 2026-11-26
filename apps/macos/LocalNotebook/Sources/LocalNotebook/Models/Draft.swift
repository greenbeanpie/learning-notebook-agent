import Foundation
import SwiftUI

extension AppState {
    func palette(_ scheme: ColorScheme) -> AppThemePalette {
        AppThemeId.resolve(prefs.theme).palette(systemScheme: scheme)
    }
}

/// A slide outline draft arrives as JSON (or a JSON string); the review screen edits it as Markdown.
enum Draft {
    static func toMarkdown(_ draft: Any?, en: Bool) -> String {
        if let s = draft as? String {
            let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.hasPrefix("{") || trimmed.hasPrefix("["),
               let data = trimmed.data(using: .utf8),
               let obj = try? JSONSerialization.jsonObject(with: data) {
                return outline(obj, en: en)
            }
            return s
        }
        if let obj = draft, !(obj is NSNull) { return outline(obj, en: en) }
        return ""
    }

    private static func outline(_ any: Any, en: Bool) -> String {
        guard let el = any as? [String: Any] else { return "" }
        var out = ""
        if let t = el["title"] as? String, !t.isEmpty { out += "# \(t)\n\n" }
        let list = (el["slides"] as? [[String: Any]]) ?? (el["sections"] as? [[String: Any]]) ?? (el["outline"] as? [[String: Any]])
        if let list {
            for (i, item) in list.enumerated() {
                let n = i + 1
                var t = item["title"] as? String ?? ""
                if t.isEmpty { t = item["heading"] as? String ?? "" }
                if t.isEmpty { t = L10n.fmt("art.slide_fallback_title", en: en, n) }
                out += L10n.fmt("art.slide_heading", en: en, n, t) + "\n"
                for b in item["bullets"] as? [String] ?? [] { out += "- \(b)\n" }
                for b in item["points"] as? [String] ?? [] { out += "- \(b)\n" }
                if let sn = item["speaker_notes"] as? String, !sn.isEmpty {
                    out += L10n.fmt("art.slide_notes_quote", en: en, sn) + "\n"
                }
                out += "\n"
            }
            return out.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        for key in el.keys.sorted() {
            out += "## \(key)\n"
            if let s = el[key] as? String { out += s } else { out += "\(el[key] ?? "")" }
            out += "\n\n"
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
