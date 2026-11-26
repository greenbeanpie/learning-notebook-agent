//! Pure helpers of the lecture player (no GTK): the narration sequence, figure inlining and the
//! shared player page. Kept apart from the UI so they can be unit-tested alone.
use crate::i18n::tr;
use serde_json::{json, Value};
use std::path::Path;

/// One spoken sentence of a lecture (display-only slides contribute an empty one).
#[derive(Debug, Clone)]
pub struct LecSentence {
    pub slide: usize,
    pub slide_id: String,
    pub id: String,
    pub text: String,
    pub cues: Vec<Value>,
    pub last_of_slide: bool,
    pub checkpoint: Option<Value>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The narration, sentence by sentence, over all slides.
pub fn build_sequence(payload: &Value) -> Vec<LecSentence> {
    let mut out = Vec::new();
    let slides = payload.get("slides").and_then(Value::as_array).cloned().unwrap_or_default();
    for (si, slide) in slides.iter().enumerate() {
        let sid = s(slide, "id");
        let cp = slide.get("checkpoint").filter(|c| c.is_object()).cloned();
        let spoken: Vec<&Value> = slide
            .get("narration")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter(|n| !s(n, "text").trim().is_empty()).collect())
            .unwrap_or_default();
        if spoken.is_empty() {
            out.push(LecSentence {
                slide: si,
                slide_id: sid.clone(),
                id: format!("{sid}.0"),
                text: String::new(),
                cues: vec![],
                last_of_slide: true,
                checkpoint: cp,
            });
            continue;
        }
        for (ni, n) in spoken.iter().enumerate() {
            let nid = s(n, "id");
            out.push(LecSentence {
                slide: si,
                slide_id: sid.clone(),
                id: if nid.is_empty() { format!("{sid}.{}", ni + 1) } else { nid },
                text: s(n, "text"),
                cues: n.get("cues").and_then(Value::as_array).cloned().unwrap_or_default(),
                last_of_slide: ni == spoken.len() - 1,
                checkpoint: cp.clone(),
            });
        }
    }
    out
}

/// The step the slide of sentence `p` has reached before that sentence is spoken.
pub fn step_before(seq: &[LecSentence], p: usize) -> i64 {
    if p >= seq.len() {
        return 0;
    }
    let mut step = 0;
    let mut i = p;
    while i > 0 && seq[i - 1].slide == seq[p].slide {
        i -= 1;
        for cue in &seq[i].cues {
            if cue.get("type").and_then(Value::as_str) == Some("step") {
                step = step.max(cue.get("n").and_then(Value::as_i64).unwrap_or(0));
            }
        }
    }
    step
}

/// Same naming as the core's `lecture::slide_file_id`.
pub fn slide_file_id(slide: &Value, idx: usize) -> String {
    let id: String = s(slide, "id")
        .chars()
        .filter(|c| (c.is_ascii_alphanumeric()) || matches!(c, '-' | '_' | '.'))
        .collect();
    if id.is_empty() {
        format!("slide{}", idx + 1)
    } else {
        id
    }
}

/// The payload with each slide's figure SVG inlined as `figure_svg`.
pub fn payload_with_figures(payload: &Value, output_dir: &str) -> Value {
    let mut copy = payload.clone();
    if output_dir.is_empty() {
        return copy;
    }
    if let Some(slides) = copy.get_mut("slides").and_then(Value::as_array_mut) {
        for (i, slide) in slides.iter_mut().enumerate() {
            if !slide.get("figure").is_some_and(|f| !f.is_null()) {
                continue;
            }
            let file = Path::new(output_dir).join("figures").join(format!("{}.svg", slide_file_id(slide, i)));
            if let Ok(svg) = std::fs::read_to_string(file) {
                slide["figure_svg"] = Value::String(svg);
            }
        }
    }
    copy
}

/// JSON text that is safe to embed in a `<script>` block.
pub fn script_safe_json(v: &Value) -> String {
    v.to_string()
        .replace("</", "<\\/")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// `text` as a JavaScript string literal.
pub fn js_string(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

/// The shared player page (apps/shared/deck_player.html) in lecture mode. `bridge` is a script
/// that makes `window.chrome.webview.postMessage` reach the host.
pub fn deck_html(template: &str, payload: &Value, en: bool, bridge: &str) -> String {
    let mut html = template.replacen("<head>", &format!("<head>{bridge}"), 1);
    html = html
        .replace("__LANG__", tr(en, "slides.en"))
        .replace("__LECTURE__", "true")
        .replace("__T_PLAY_JSON__", &js_string(tr(en, "slides.play")))
        .replace("__T_PAUSE_JSON__", &js_string(tr(en, "slides.pause")))
        .replace("__T_NONOTES_JSON__", &js_string(""));
    // the remaining plain-text labels belong to the stand-alone slide viewer
    let mut cleaned = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(start) = rest.find("__T_") {
        let after = &rest[start + 4..];
        let name_len = after.chars().take_while(|c| c.is_ascii_uppercase()).count();
        if name_len > 0 && after[name_len..].starts_with("__") {
            cleaned.push_str(&rest[..start]);
            rest = &after[name_len + 2..];
        } else {
            cleaned.push_str(&rest[..start + 4]);
            rest = after;
        }
    }
    cleaned.push_str(rest);
    // the payload last: it must not be searched for tokens
    cleaned.replace("__PAYLOAD__", &script_safe_json(payload))
}

/// The Studio config of a lecture: `mode`, `detail` and whether the scope is confirmed first.
pub fn lecture_config(mode: &str, detail: &str, confirm_scope: bool) -> Value {
    json!({
        "mode": if mode.is_empty() { "general" } else { mode },
        "detail": if detail.is_empty() { "auto" } else { detail },
        "reviewOutline": confirm_scope,
    })
}

/// The next speech rate when the rate button is pressed.
pub fn next_rate(r: f64) -> f64 {
    match r {
        x if (x - 1.0).abs() < 1e-9 => 1.25,
        x if (x - 1.25).abs() < 1e-9 => 1.5,
        x if (x - 1.5).abs() < 1e-9 => 2.0,
        x if (x - 2.0).abs() < 1e-9 => 0.75,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        json!({ "title": "T", "slides": [
            { "id": "s1", "figure": { "kind": "svg" }, "narration": [
                { "id": "s1.1", "text": "Hello", "cues": [{ "type": "step", "n": 1 }] },
                { "id": "s1.2", "text": "World", "cues": [{ "type": "laser", "x": 1 }] } ] },
            { "id": "s2", "narration": [], "checkpoint": { "kind": "attempt", "question": "Q?" } },
            { "id": "s3", "narration": [{ "text": "  " }, { "text": "Third" }] },
        ]})
    }

    #[test]
    fn the_sequence_flags_the_last_sentence_of_each_slide() {
        let seq = build_sequence(&payload());
        assert_eq!(seq.len(), 4);
        assert!(!seq[0].last_of_slide && seq[1].last_of_slide);
        assert!(seq[2].text.is_empty() && seq[2].checkpoint.is_some() && seq[2].id == "s2.0");
        assert_eq!((seq[3].id.as_str(), seq[3].slide), ("s3.1", 2));
    }

    #[test]
    fn steps_before_a_sentence_come_from_the_cues_of_its_slide() {
        let seq = build_sequence(&payload());
        assert_eq!(step_before(&seq, 1), 1);
        assert_eq!(step_before(&seq, 0), 0);
        assert_eq!(step_before(&seq, 99), 0);
    }

    #[test]
    fn figure_names_match_the_core() {
        assert_eq!(slide_file_id(&json!({ "id": "a/b c" }), 0), "abc");
        assert_eq!(slide_file_id(&json!({ "id": "" }), 2), "slide3");
    }

    #[test]
    fn figures_are_inlined_only_for_slides_that_have_one() {
        let dir = std::env::temp_dir().join(format!("lec_fig_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("figures")).unwrap();
        std::fs::write(dir.join("figures/s1.svg"), "<svg>ok</svg>").unwrap();
        let out = payload_with_figures(&payload(), dir.to_str().unwrap());
        assert_eq!(out["slides"][0]["figure_svg"], json!("<svg>ok</svg>"));
        assert!(out["slides"][1].get("figure_svg").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_page_is_filled_in_for_lecture_mode() {
        let tpl = include_str!("../assets/deck_player.html");
        let html = deck_html(tpl, &json!({ "title": "x</script>", "slides": [] }), true, "<script>/*bridge*/</script>");
        assert!(html.contains("const LECTURE = true;"));
        assert!(html.contains("<head><script>/*bridge*/</script>"));
        assert!(!html.contains("__PAYLOAD__") && !html.contains("__LANG__") && !html.contains("__T_"));
        assert!(html.contains("x<\\/script>"), "the payload cannot close the script block");
        assert!(html.contains("const T_PLAY = \"Play\";"));
    }

    #[test]
    fn rate_and_config() {
        assert_eq!((next_rate(1.0), next_rate(2.0), next_rate(0.75)), (1.25, 0.75, 1.0));
        let c = lecture_config("", "brief", true);
        assert_eq!((c["mode"].as_str(), c["detail"].as_str(), c["reviewOutline"].as_bool()), (Some("general"), Some("brief"), Some(true)));
    }
}
