import Foundation

/// One spoken sentence of a lecture (display-only slides contribute an empty one).
struct LecSentence {
    let slide: Int
    let slideId: String
    let id: String
    let text: String
    let cues: [[String: Any]]
    let lastOfSlide: Bool
    let checkpoint: [String: Any]?
}

/// A question of the learner and the teacher's answer, kept in the question panel.
struct LecQA: Identifiable {
    let id = UUID()
    var question: String
    var images: [String] = []
    var answer: String = ""
    var speak: String = ""
    var pending: Bool = true
    var error: String = ""
    var suggestKind: String = ""
    var suggestReason: String = ""
}

/// Pure helpers of the lecture player (no UI): kept free of AppKit so they can be tested alone.
enum Lecture {
    /// The narration, sentence by sentence, over all slides.
    static func buildSequence(_ payload: [String: Any]) -> [LecSentence] {
        var out: [LecSentence] = []
        let slides = payload["slides"] as? [[String: Any]] ?? []
        for (si, s) in slides.enumerated() {
            let sid = s["id"] as? String ?? ""
            let cp = s["checkpoint"] as? [String: Any]
            let narr = s["narration"] as? [[String: Any]] ?? []
            let spoken = narr.filter {
                !(($0["text"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }
            if spoken.isEmpty {
                out.append(LecSentence(slide: si, slideId: sid, id: "\(sid).0", text: "", cues: [], lastOfSlide: true, checkpoint: cp))
                continue
            }
            for (ni, n) in spoken.enumerated() {
                let nid = (n["id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "\(sid).\(ni + 1)"
                out.append(LecSentence(
                    slide: si,
                    slideId: sid,
                    id: nid,
                    text: n["text"] as? String ?? "",
                    cues: n["cues"] as? [[String: Any]] ?? [],
                    lastOfSlide: ni == spoken.count - 1,
                    checkpoint: cp
                ))
            }
        }
        return out
    }

    /// The step the slide of sentence `p` has reached before that sentence is spoken.
    static func stepBefore(_ seq: [LecSentence], _ p: Int) -> Int {
        guard seq.indices.contains(p) else { return 0 }
        var step = 0
        var i = p - 1
        while i >= 0 && seq[i].slide == seq[p].slide {
            for cue in seq[i].cues where (cue["type"] as? String) == "step" {
                let n = (cue["n"] as? Int) ?? Int((cue["n"] as? Double) ?? 0)
                step = max(step, n)
            }
            i -= 1
        }
        return step
    }

    /// Same naming as the core's `lecture::slide_file_id`.
    static func slideFileId(_ slide: [String: Any], _ idx: Int) -> String {
        let raw = slide["id"] as? String ?? ""
        let id = String(raw.unicodeScalars.filter {
            ($0.isASCII && (CharacterSet.alphanumerics.contains($0))) || $0 == "-" || $0 == "_" || $0 == "."
        })
        return id.isEmpty ? "slide\(idx + 1)" : id
    }

    /// The payload with each slide's figure SVG inlined as `figure_svg`.
    static func payloadWithFigures(_ payload: [String: Any], outputDir: String) -> [String: Any] {
        guard !outputDir.isEmpty, var slides = payload["slides"] as? [[String: Any]] else { return payload }
        for i in slides.indices where slides[i]["figure"] != nil && !(slides[i]["figure"] is NSNull) {
            let file = URL(fileURLWithPath: outputDir)
                .appendingPathComponent("figures")
                .appendingPathComponent("\(slideFileId(slides[i], i)).svg")
            if let svg = try? String(contentsOf: file, encoding: .utf8) {
                slides[i]["figure_svg"] = svg
            }
        }
        var copy = payload
        copy["slides"] = slides
        return copy
    }

    /// `s` as a JSON string literal (also valid JavaScript).
    static func jsString(_ s: String) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: [s], options: []),
              let arr = String(data: data, encoding: .utf8) else { return "\"\"" }
        return String(arr.dropFirst().dropLast())
    }

    /// JSON text that is safe to embed in a `<script>` block.
    static func scriptSafeJSON(_ obj: Any) -> String {
        guard JSONSerialization.isValidJSONObject(obj),
              let data = try? JSONSerialization.data(withJSONObject: obj, options: []),
              let text = String(data: data, encoding: .utf8) else { return "{}" }
        return text
            .replacingOccurrences(of: "</", with: "<\\/")
            .replacingOccurrences(of: "\u{2028}", with: "\\u2028")
            .replacingOccurrences(of: "\u{2029}", with: "\\u2029")
    }

    /// The shared player page (apps/shared/deck_player.html) in lecture mode. `bridge` is a script
    /// that makes `window.chrome.webview.postMessage` reach the host.
    static func deckHtml(template: String, payload: [String: Any], en: Bool, bridge: String) -> String {
        var html = template
        if let r = html.range(of: "<head>") {
            html.replaceSubrange(r, with: "<head>\(bridge)")
        }
        html = html
            .replacingOccurrences(of: "__LANG__", with: en ? "en" : "zh-CN")
            .replacingOccurrences(of: "__LECTURE__", with: "true")
            .replacingOccurrences(of: "__T_PLAY_JSON__", with: jsString(en ? "Play" : "自动放映"))
            .replacingOccurrences(of: "__T_PAUSE_JSON__", with: jsString(en ? "Pause" : "暂停放映"))
            .replacingOccurrences(of: "__T_NONOTES_JSON__", with: jsString(""))
        // the remaining plain-text labels belong to the stand-alone slide viewer
        html = html.replacingOccurrences(of: "__T_[A-Z]+__", with: "", options: .regularExpression)
        // the payload last: it must not be searched for tokens
        return html.replacingOccurrences(of: "__PAYLOAD__", with: scriptSafeJSON(payload))
    }

    /// The Studio config of a lecture: `mode`, `detail` and whether the scope is confirmed first.
    static func config(mode: String, detail: String, confirmScope: Bool) -> [String: Any] {
        [
            "mode": mode.isEmpty ? "general" : mode,
            "detail": detail.isEmpty ? "auto" : detail,
            "reviewOutline": confirmScope,
        ]
    }

    /// The next speech rate when the rate button is pressed.
    static func nextRate(_ r: Double) -> Double {
        switch r {
        case 1.0: return 1.25
        case 1.25: return 1.5
        case 1.5: return 2.0
        case 2.0: return 0.75
        default: return 1.0
        }
    }
}
