//! Studio "lecture" (讲解): a narrated, step-by-step slide lesson.
//!
//! Generation is multi-pass like knowledge graphs: an outline (scope + objectives + sections),
//! then each section is authored separately (slides + spoken narration + laser/step cues),
//! figures are drawn in their own clean-context calls (`figure.rs` DSL, never raw SVG), and
//! every section is validated — problems go back to the model for repair instead of being
//! silently "fixed" locally. During playback the learner can interrupt with a question
//! (`lecture_ask`); the model judges whether the rest of the lecture should be adapted
//! (`lecture_adapt`: insert supplementary slides or regenerate the remaining slides), and any
//! adaptation can be undone (`lecture_undo`).

use super::*;
use crate::figure::{render_figure, svg_to_png, FIGURE_DSL_GUIDE};
use crate::material::{config_for_prompt, image_parts, with_images};
use std::collections::HashSet;

pub const LECTURE_REPAIR_ROUNDS: usize = 2;
/// The most repair rounds the setting can ask for.
pub const MAX_LECTURE_REPAIR_ROUNDS: usize = 20;
/// Independent animation reviews per section (each costs minutes, so one by default).
pub const LECTURE_REVIEW_ROUNDS: usize = 1;
/// A section stops asking for more reviews after this long.
pub const LECTURE_SECTION_BUDGET_SECS: u64 = 420;

/// Repair/review rounds per section: `LECTURE_REPAIR_ROUNDS`, or the number in the environment
/// variable `LNA_LECTURE_REPAIR_ROUNDS` (a test hook for watching how repairs converge).
pub(crate) fn lecture_review_rounds() -> usize {
    std::env::var("LNA_LECTURE_REVIEW_ROUNDS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(LECTURE_REVIEW_ROUNDS)
}

/// Seconds a section may spend before it stops asking for further reviews (`LNA_LECTURE_
/// SECTION_BUDGET_SECS` overrides). Content repairs that are needed are still made.
pub(crate) fn lecture_section_budget() -> std::time::Duration {
    std::time::Duration::from_secs(
        std::env::var("LNA_LECTURE_SECTION_BUDGET_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(LECTURE_SECTION_BUDGET_SECS),
    )
}

/// App setting that holds the lecture limits the user can change in Settings.
pub(crate) const LECTURE_LIMITS_KEY: &str = "lecture.limits";

pub(crate) fn lecture_repair_rounds() -> usize {
    std::env::var("LNA_LECTURE_REPAIR_ROUNDS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(LECTURE_REPAIR_ROUNDS)
}
const FIGURE_REVIEW_ROUNDS: usize = 2;
/// Hard per-request output token cap for lecture slide generation & repair (prevents runaway output).
pub const LECTURE_MAX_OUTPUT_TOKENS: u32 = 32_768;
/// Target upper bound of slides written per batch (3–4 slides per batch for coherence and stability).
pub const LECTURE_SLIDE_BATCH_MAX: usize = 4;

pub(crate) const ANIMATION_NOTE: &str = "ANIMATION: Omit cues/builds (a later pass adds them). Narrate in reveal order, introduce each symbol/figure part before using it using its exact label, state results after their explanation, one idea per sentence.";

pub(crate) const FIGURE_REQUEST_GUIDE: &str = "FIGURES: Geometric/physical content (vectors, forces, torques, axes, angles, trajectories, graphs, surfaces) MUST include `figure_request`: {\"kind\":\"plot|plot3d|diagram\",\"purpose\":...,\"details\": exact functions/ranges/points/vectors/labels with explicit part ids (e.g. `curve id f`, `vector id G`)}. Always pair every figure on the SAME slide with concise explaining text (2–3 short bullets <=16 zh / 8 en words, a formula, or a compact <=4x3 table) using matching symbols; never use `display_only` on a figure slide.";

/// Judgement rules shared by the planner and the authors: how much to say, to whom, and what
/// never to talk about. They are principles, not page counts, so the model sizes the lecture itself.
pub(crate) const TEACHING_JUDGEMENT: &str = "TEACHING RULES:\n- ECONOMY: Every slide must add new substance. Skip agenda/divider/recap slides unless spanning multiple topics; a single exercise/derivation is ONE section. Move briskly through routine algebra and pause only on non-obvious steps.\n- VISUALS & DENSITY: Geometric/physical reasoning (including vector directions/components/cross products in derivations) MUST include a figure paired with concise text/formula/table on the same 16:9 slide; avoid sparse slides.\n- LEARNER & PROBLEMS: Assume standard course prerequisites—never add prerequisite/concept-review slides before solving a problem. Name known theorems in one brief clause where used. After solving, optionally close with 1 concise slide of method takeaway, check, or common pitfalls tied to the problem.\n- VOICE: Speak purely as the teacher about the subject; never mention OCR, scans, image extraction, outlines, plans, or generation steps.";

/// Errors that end an on-demand evidence read instead of falling back to plain retrieval: a
/// cancelled job, and a model that refused the task (its reason is shown to the user).
pub(crate) fn ends_evidence_read(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::Cancelled | ErrorCode::ModelRefused)
}

/// Mode/detail rules for the lecture author.
pub(crate) fn lecture_rules(mode: &str, detail: &str) -> String {
    let mode_rule = match mode {
        "preview" => "MODE preview: state learning goals, then build intuition gently (concept → intuition → example → pitfalls).",
        "review" => "MODE review: consolidate learned material—recap key points, connect/contrast ideas, flag common mistakes, prioritize listed weak concepts and skim mastered ones.",
        "problem" => "MODE problem (讲题): solve and explain the given problem(s) directly with zero prerequisite-review slides or forced attempt checkpoints. Per problem: (1) problem statement + conditions + figure on one slide; (2) core breakthrough idea; (3) step-by-step derivation citing known theorems briefly in place; (4) answer check + at most 1 concise takeaway/pitfall slide.",
        _ => "MODE general: explain the material directly (what it says and how it works); add context or a summary only where genuinely helpful.",
    };
    let detail_rule = match detail {
        "brief" => "DETAIL brief: essentials only, few slides, short narration.",
        "detailed" => "DETAIL detailed: thorough derivations, examples, and edge cases.",
        "standard" => "DETAIL standard: balanced coverage.",
        _ => "DETAIL auto: match depth to scope (1 exercise/page = a handful of slides; a chapter = longer); never pad or skip substance.",
    };
    format!(
        "{mode_rule}\n{detail_rule}\n{TEACHING_JUDGEMENT}\nCHECKPOINTS: do NOT ask a question after every point. Questions happen when the learner interrupts, or — sparingly, about one per section at most — when you judge a pause genuinely helps (a key difficulty or a common confusion): add \"checkpoint\": {{\"kind\":\"ask\",\"question\":...,\"expect\":...,\"reason\":<why a pause helps here>}} to that slide."
    )
}

const NARRATION_RULES: &str = "NARRATION: Each slide has `narration`: ordered TTS sentences `{\"text\":...}` (<=60 zh chars / 35 en words each). Use plain spoken words only—NO LaTeX, markdown, `$`, or unpronounceable symbols (speak formulas in words). Keep on-screen text concise (LaTeX `$...$` allowed on screen). Only omit narration if `display_only: {\"reason\":...}` is set (never two in a row).";

// ---------------------------------------------------------------------------------------------
// schemas

fn slide_item_schema() -> Value {
    let base: Value = serde_json::from_str(SCHEMA_SLIDES).unwrap_or_else(|_| json!({}));
    base["properties"]["slides"]["items"].clone()
}

/// How a lecture is planned (scope, sections, visuals): shared by the separate outline planner
/// and the source reader that writes the outline itself once it has read enough.
pub(crate) fn lecture_plan_rules(art: &StudioArtifact) -> String {
    let mode = art
        .config
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("general");
    let detail = art
        .config
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    format!(
        "Resolve the requested scope (topic, problem or unit given in config.topic; the user's attached material if any; the whole selection only if neither is given) against the sources, then plan a narrated lesson: title, learning objectives, and ordered sections. Each section has a title and a \"focus\" naming exactly which concepts/problems/source parts (chapters, sections, pages) it covers, plus an optional slides_hint (keep it small: slides are only as many as the section needs). Also give every section \"visual_need\": \"required\" when its content is obviously geometric or physical (vectors, forces, torques, angles, shapes, trajectories, graphs, surfaces) — including a derivation whose steps reason about vector directions, components or cross products, \"suggested\" when it is mainly a worked derivation, proof or calculation, otherwise \"none\"; and \"visual\": what to draw and how its parts are revealed step by step together with the explanation (empty when none). {}\nStrictly ground the plan in the sources. Match the language requested in config or the primary language of the sources.{}",
        lecture_rules(mode, detail),
        if has_user_material(&art.config) {
            format!("\n{USER_MATERIAL_SCOPE} A single exercise needs only a few sections (the problem and its solution steps), never the whole chapter and never a concept-review or prerequisites section before the solution; a short closing part after the solution (method summary, extension, common mistakes) is fine when it adds value.")
        } else {
            String::new()
        }
    )
}

/// A lecture with at most this many sections is a small scope: its sections reuse the evidence
/// collected for the whole lecture instead of reading the sources again one by one.
pub(crate) const SMALL_LECTURE_SECTIONS: usize = 3;

/// Tokens of source text a section's authoring prompt can hold.
pub(crate) fn section_prompt_room(resolved: &ResolvedProfile) -> usize {
    let window = resolved.context_window as usize;
    let reserved_output = (resolved.output_budget() as usize).min(window / 4);
    let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
    available_input.saturating_mul(70) / 100
}

/// Token budget of one section's own source reading.
pub(crate) fn section_read_budget(resolved: &ResolvedProfile) -> usize {
    section_prompt_room(resolved).min(16_000)
}

/// What the collecting reader is told when its last reply should be the lecture outline.
pub(crate) fn lecture_reader_finish(art: &StudioArtifact, kg_hint: &str) -> ReaderFinish {
    let schema = outline_schema();
    ReaderFinish {
        instruction: format!(
            "When the excerpts you have read cover the requested scope, do NOT call more tools and do NOT reply DONE: reply with the lecture plan itself — one JSON object (no prose, no code fence) matching this JSON Schema:\n{schema}\n\nPlanning rules: {}{}",
            lecture_plan_rules(art),
            if kg_hint.is_empty() {
                String::new()
            } else {
                format!("\n{kg_hint}")
            }
        ),
        schema,
        schema_name: "lecture_outline",
        thought_title: "讲解大纲规划思考".into(),
    }
}

/// A lecture slide the author writes: a regular slide plus narration, checkpoint and figure
/// request. Builds and cues are added afterwards by the animation step.
pub(crate) fn lecture_slide_schema() -> Value {
    let mut item = slide_item_schema();
    if let Some(props) = item.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(
            "narration".into(),
            json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["text"],
                    "properties": {
                        "text": { "type": "string" }
                    }
                }
            }),
        );
        props.insert(
            "display_only".into(),
            json!({ "type": "object", "additionalProperties": false, "required": ["reason"], "properties": { "reason": { "type": "string" } } }),
        );
        props.insert(
            "checkpoint".into(),
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "question"],
                "properties": {
                    "kind": { "type": "string", "enum": ["ask", "attempt"] },
                    "question": { "type": "string" },
                    "expect": { "type": "string" },
                    "reason": { "type": "string" }
                }
            }),
        );
        props.insert("figure_request".into(), figure_request_schema());
        // builds and cues are the animation step's business (`animation_author_schema`)
        props.remove("builds");
    }
    item["required"] = json!(["layout", "title", "narration"]);
    item
}

pub(crate) fn figure_request_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "purpose", "details"],
        "properties": {
            "kind": { "type": "string", "enum": ["plot", "plot3d", "diagram"] },
            "purpose": { "type": "string" },
            "details": { "type": "string" }
        }
    })
}

fn section_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["slides"],
        "properties": { "slides": { "type": "array", "items": lecture_slide_schema() } }
    })
}

fn section_page_skeleton_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["pages"],
        "properties": {
            "pages": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "points", "needs_figure"],
                    "properties": {
                        "title": { "type": "string" },
                        "points": { "type": "array", "items": { "type": "string" } },
                        "needs_figure": { "type": "boolean" },
                        "figure_hint": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Splits `total` planned slides into balanced 0-based `[start, end)` batches of at most
/// `LECTURE_SLIDE_BATCH_MAX` (3–4 slides per batch whenever `total >= 5`).
fn split_slide_batches(total: usize) -> Vec<(usize, usize)> {
    if total == 0 {
        return Vec::new();
    }
    if total <= LECTURE_SLIDE_BATCH_MAX {
        return vec![(0, total)];
    }
    let batches = total.div_ceil(LECTURE_SLIDE_BATCH_MAX);
    let base = total / batches;
    let rem = total % batches;
    let mut out = Vec::with_capacity(batches);
    let mut start = 0;
    for i in 0..batches {
        let len = base + usize::from(i < rem);
        out.push((start, start + len));
        start += len;
    }
    out
}

fn format_page_skeleton(pages: &[Value]) -> String {
    pages
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let title = sv(p, "title");
            let pts: Vec<&str> = p
                .get("points")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let needs_fig = p
                .get("needs_figure")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let fig_hint = sv(p, "figure_hint");
            let fig_str = if needs_fig {
                if fig_hint.is_empty() {
                    " [figure: required]".to_string()
                } else {
                    format!(" [figure: {fig_hint}]")
                }
            } else {
                String::new()
            };
            format!("{}. {}{} — {}", idx + 1, title, fig_str, pts.join("; "))
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn outline_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "objectives", "sections"],
        "properties": {
            "title": { "type": "string" },
            "subtitle": { "type": "string" },
            "theme": { "type": "string", "enum": ["executive_navy", "editorial_slate", "academic_sage", "warm_terracotta", "midnight_aurora"] },
            "objectives": { "type": "array", "items": { "type": "string" } },
            "scope": { "type": "string" },
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "focus"],
                    "properties": {
                        "title": { "type": "string" },
                        "focus": { "type": "string" },
                        "slides_hint": { "type": "integer" },
                        "visual_need": { "type": "string", "enum": ["required", "suggested", "none"] },
                        "visual": { "type": "string" }
                    }
                }
            }
        }
    })
}

pub(crate) fn figure_spec_schema() -> Value {
    let range = json!({ "type": "array", "items": { "type": ["number", "string"] } });
    json!({
        "type": "object",
        "required": ["kind", "elements"],
        "properties": {
            "kind": { "type": "string", "enum": ["plot", "plot3d", "diagram"] },
            "title": { "type": "string" },
            "aspect": { "type": "string", "enum": ["4:3", "16:9", "1:1", "3:4"] },
            "x_range": range, "y_range": range, "z_range": range,
            "azimuth": { "type": "number" },
            "elevation": { "type": "number" },
            "grid": { "type": "boolean" },
            "equal_scale": { "type": "boolean" },
            "axis_labels": { "type": "array", "items": { "type": "string" } },
            "elements": { "type": "array", "items": { "type": "object" } }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// validation

fn sv(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn arr_len(v: &Value, k: &str) -> usize {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Every element id a cue/build may target on this slide.
pub fn slide_element_ids(slide: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    if !sv(slide, "title").is_empty() {
        ids.push("title".into());
    }
    if !sv(slide, "subtitle").is_empty() {
        ids.push("subtitle".into());
    }
    for i in 1..=arr_len(slide, "bullets") {
        ids.push(format!("b{i}"));
    }
    for i in 1..=arr_len(slide, "cards") {
        ids.push(format!("card{i}"));
    }
    for i in 1..=arr_len(slide, "metrics") {
        ids.push(format!("metric{i}"));
    }
    for i in 1..=arr_len(slide, "steps") {
        ids.push(format!("step{i}"));
    }
    if let Some(cols) = slide.get("columns").and_then(Value::as_array) {
        for (ci, c) in cols.iter().enumerate() {
            ids.push(format!("col{}", ci + 1));
            for j in 1..=arr_len(c, "items") {
                ids.push(format!("col{}.i{}", ci + 1, j));
            }
        }
    }
    if let Some(t) = slide.get("table") {
        if t.is_object() {
            ids.push("table".into());
            for r in 1..=arr_len(t, "rows") {
                ids.push(format!("table.r{r}"));
            }
        }
    }
    if !sv(slide, "formula").is_empty() {
        ids.push("formula".into());
    }
    if !sv(slide, "callout").is_empty() {
        ids.push("callout".into());
    }
    if let Some(fig) = slide.get("figure").filter(|f| f.is_object()) {
        ids.push("figure".into());
        if let Some(parts) = fig.pointer("/meta/parts").and_then(Value::as_array) {
            for p in parts {
                let id = sv(p, "id");
                if !id.is_empty() && id != "title" && id != "grid" {
                    ids.push(format!("figure:{id}"));
                }
            }
        }
    }
    ids
}

fn has_tex(s: &str) -> bool {
    if s.contains('$') || s.contains("^{") || s.contains("_{") {
        return true;
    }
    let b = s.as_bytes();
    b.windows(2)
        .any(|w| w[0] == b'\\' && w[1].is_ascii_alphabetic())
}

/// One content problem the model is asked to fix (animation consistency is never reported
/// here: [`crate::lecture_anim`] establishes it deterministically).
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub slide: usize,
    pub msg: String,
}

/// Phrases that show the text is about how the lecture was produced (reading the scan, the
/// plan, "this page" in a chain) instead of about the subject. Mentions of the book as a
/// teaching resource are deliberately not in the list.
/// Slide titles of a prerequisite/review detour (taught before getting to the actual problem).
/// Checked outside review mode only, where recapping is the point.
const PREREQ_TITLES: &[&str] = &[
    "前置知识",
    "预备知识",
    "知识回顾",
    "知识复习",
    "基础回顾",
    "基础知识",
    "概念回顾",
    "先复习",
    "先回顾",
    "prerequisite",
    "background knowledge",
    "review of basics",
];

fn prereq_title(title: &str) -> Option<&'static str> {
    let lower = title.to_lowercase();
    PREREQ_TITLES.iter().find(|p| lower.contains(**p)).copied()
}

const PROCESS_PHRASES: &[&str] = &[
    "ocr",
    "识别噪声",
    "噪声很大",
    "扫描版",
    "扫描件",
    "识别结果",
    "识别错误",
    "识别质量",
    "图片识别",
    "图像识别",
    "推导链",
    "这一页在",
    "这一页的位置",
    "本页在",
    "页面解析",
    "用户上传",
    "用户附带",
    "你上传的",
];

fn process_talk(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    PROCESS_PHRASES
        .iter()
        .find(|p| lower.contains(**p))
        .copied()
}

fn has_figure(slide: &Value) -> bool {
    slide.get("figure_request").is_some_and(Value::is_object)
        || slide.get("figure").is_some_and(Value::is_object)
}

/// A content slide that shows only a little text (a short table, a few bullets or one formula)
/// and no figure: on a 16:9 slide that leaves most of the page empty.
pub(crate) fn sparse_text_only(slide: &Value) -> bool {
    if has_figure(slide) || slide.get("display_only").is_some_and(Value::is_object) {
        return false;
    }
    let layout = sv(slide, "layout");
    if ["title", "summary", "section", "agenda", "quote"].contains(&layout.as_str()) {
        return false;
    }
    let table_rows = arr_len(slide.get("table").unwrap_or(&Value::Null), "rows");
    if layout == "table" && table_rows > 0 {
        return table_rows <= 4;
    }
    let col_items: usize = slide
        .get("columns")
        .and_then(Value::as_array)
        .map(|c| c.iter().map(|c| arr_len(c, "items")).sum())
        .unwrap_or(0);
    let items = arr_len(slide, "bullets")
        + arr_len(slide, "cards")
        + arr_len(slide, "metrics")
        + arr_len(slide, "steps")
        + col_items
        + table_rows
        + usize::from(!sv(slide, "formula").is_empty());
    items <= 3
}

/// Section-level visual checks: a geometric section needs figures on its slides, not one
/// figure somewhere; a derivation section should at least reveal short slides step by step.
pub(crate) fn check_section_visuals(slides: &[Value], visual_need: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if visual_need == "required" && !slides.iter().any(has_figure) {
        issues.push(Issue {
            slide: 0,
            msg: "this section is obviously geometric or physical but no slide has a \"figure_request\"; give the slide that introduces it a figure (layout \"figure\", with the explaining bullets/formula/table beside it) and reveal its parts step by step with builds on figure:<id> in step with the narration".into(),
        });
    }
    for (si, s) in slides.iter().enumerate() {
        if !sparse_text_only(s) {
            continue;
        }
        if visual_need == "required" {
            issues.push(Issue {
                slide: si,
                msg: "shows only a short table/list with no figure, so most of the slide is empty, in a section that is geometric or physical; add a \"figure_request\" that draws what this text describes (for a table of directions, components or products of vectors: draw those vectors and the axis), keep the table/bullets/formula on the same slide beside the figure, and reveal each row/bullet together with its figure part (builds on table.rN or bN plus figure:<id>, laser on the part being spoken about)".into(),
            });
        }
    }
    issues
}

/// System prompt of the animation author: a fresh context that sees only the finished slides.
pub(crate) const ANIMATION_AUTHOR_SYSTEM: &str = "You are the Local Notebook Studio lecture animator. You direct the animation of finished lecture slides: for every narration sentence you decide which elements appear when it starts and where the laser pointer points while it is spoken. You see only the finished slides, never how they were written, and you change no text. Output {\"slides\": [...]} and nothing else.\n\nPLAYBACK\n- Whatever you do not reveal is on screen from the start. So list every element that has to wait: bullets (b1..), table rows (table.rN), formula, callout, and EVERY figure part that is not background, labels, helper lines, angle marks, projections and components included. Only background (axes, outlines, the object the page is about) stays visible from the start.\n- \"reveal\" of sentence k lists the ids that appear when that sentence starts. An element appears on the first sentence that speaks about it, neither earlier nor later. A label, angle mark, projection or helper line appears together with (or after) the point, vector or line it belongs to (figure_parts tell what each part hangs on, text elements say what they label with \"for\"); a result or formula appears on the sentence that states it, never before the sentences that explain it. A sentence that introduces something new reveals it; a sentence that only explains what is already on screen reveals nothing. Never leave a part for a later sentence than the one that names it, and never show a part before the sentence that first mentions it.\n- \"points\" of sentence k lists the laser targets in the order they are spoken: one per thing named, always the precise element (one figure part, one bullet, one table row, never a whole figure), with \"at\" optionally giving the exact words at which pointing starts. Point at something in every sentence that speaks about something on screen. A target must already be visible (revealed in this or an earlier sentence).\n- \"effect\": draw for curves, arrows and lines of a figure; fade for text; appear otherwise.\n- Give every slide that has narration, every sentence of it, in order (sentence numbers are 1-based). When a slide comes with current_animation, that is its present plan: return the corrected plan in the same format.\nELEMENT IDS: \"title\", \"b1\"..\"bN\" (bullets), \"table.r1\".. (table rows), \"formula\", \"callout\", \"figure:<part id>\" (a figure part), \"card1\"/\"metric1\"/\"step1\".. where the slide has them. Only ids of elements that exist on the slide are valid (see valid_targets).";

pub(crate) fn animation_author_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["slides"],
        "properties": {
            "slides": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["slide", "sentences"],
                    "properties": {
                        "slide": { "type": "integer", "minimum": 1 },
                        "sentences": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["sentence", "reveal", "points"],
                                "properties": {
                                    "sentence": { "type": "integer", "minimum": 1 },
                                    "reveal": { "type": "array", "items": { "type": "string" } },
                                    "effect": { "type": "string", "enum": ["appear", "fade", "fly_up", "draw"] },
                                    "points": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "additionalProperties": false,
                                            "required": ["target"],
                                            "properties": {
                                                "target": { "type": "string" },
                                                "at": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// What the animation author is shown: per slide the on-screen text, the figure's parts (what
/// they hang on) and spec elements, the valid targets and the numbered narration. No builds, no
/// cues (those are what it writes), no pictures.
pub(crate) fn animation_author_input(slides: &[Value]) -> Value {
    let full = animation_review_input(slides);
    let list = full
        .get("slides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let out: Vec<Value> = list
        .into_iter()
        .zip(slides)
        .filter(|(_, s)| {
            s.get("narration")
                .and_then(Value::as_array)
                .is_some_and(|n| n.iter().any(|x| !sv(x, "text").is_empty()))
        })
        .map(|(mut v, s)| {
            if let Some(o) = v.as_object_mut() {
                o.remove("builds");
                if let Some(ns) = o.get_mut("narration").and_then(Value::as_array_mut) {
                    for n in ns {
                        if let Some(no) = n.as_object_mut() {
                            no.remove("cues");
                        }
                    }
                }
                o.insert("valid_targets".into(), json!(slide_element_ids(s)));
            }
            v
        })
        .collect();
    json!({ "slides": out })
}

/// The slides (1-based numbers) the plan covers completely: every sentence with text has an
/// entry. A plan that skips slides or sentences is asked for again.
pub(crate) fn plan_complete_slides(slides: &[Value], plan: &Value) -> Vec<usize> {
    plan.get("slides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|sp| {
            let n = sp.get("slide").and_then(Value::as_u64)? as usize;
            let slide = slides.get(n.checked_sub(1)?)?;
            let given: HashSet<u64> = sp
                .get("sentences")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|x| x.get("sentence").and_then(Value::as_u64))
                .collect();
            // an empty plan for a slide that has things to show is no plan
            let does_something = sp
                .get("sentences")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|x| {
                    ["reveal", "points"].iter().any(|k| {
                        x.get(*k)
                            .and_then(Value::as_array)
                            .is_some_and(|a| !a.is_empty())
                    })
                });
            let has_content = slide_element_ids(slide)
                .iter()
                .any(|i| i != "title" && i != "subtitle");
            if has_content && !does_something {
                return None;
            }
            let all = slide
                .get("narration")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, x)| !sv(x, "text").trim().is_empty())
                .all(|(j, _)| given.contains(&(j as u64 + 1)));
            all.then_some(n)
        })
        .collect()
}

/// The element id a plan's target means: models often drop the "figure:" prefix of a figure
/// part or change its case. Anything that matches nothing is returned as written (the
/// normalizer drops it).
fn resolve_target(t: &str, ids: &[String]) -> String {
    let t = t.trim();
    if ids.iter().any(|i| i == t) {
        return t.to_string();
    }
    let prefixed = format!("figure:{t}");
    if ids.contains(&prefixed) {
        return prefixed;
    }
    let low = t.to_lowercase();
    ids.iter()
        .find(|i| i.to_lowercase() == low || i.to_lowercase() == format!("figure:{low}"))
        .cloned()
        .unwrap_or_else(|| t.to_string())
}

/// Writes the animation author's plan into the slides: every build and cue is replaced by what
/// the plan says (a reveal becomes one build step plus its step cue on that sentence, a point
/// becomes a laser cue). Slides the plan does not mention keep nothing, so they are static.
/// Consistency of the result (dependencies, ordering, parameters) is `normalize_animation`'s job.
/// Returns what was applied, for the log.
pub(crate) fn apply_animation(slides: &mut [Value], plan: &Value) -> Vec<String> {
    let mut log = Vec::new();
    for sp in plan
        .get("slides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(idx) = sp
            .get("slide")
            .and_then(Value::as_u64)
            .and_then(|n| (n as usize).checked_sub(1))
            .filter(|i| *i < slides.len())
        else {
            continue;
        };
        let slide = &mut slides[idx];
        let ids = slide_element_ids(slide);
        let Some(narr) = slide.get_mut("narration").and_then(Value::as_array_mut) else {
            continue;
        };
        for n in narr.iter_mut() {
            if let Some(o) = n.as_object_mut() {
                o.insert("cues".into(), json!([]));
            }
        }
        let mut builds: Vec<Value> = Vec::new();
        let mut revealed = HashSet::new();
        let mut pointed = 0usize;
        for sent in sp
            .get("sentences")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(j) = sent
                .get("sentence")
                .and_then(Value::as_u64)
                .and_then(|n| (n as usize).checked_sub(1))
                .filter(|j| *j < narr.len())
            else {
                continue;
            };
            let mut cues: Vec<Value> = Vec::new();
            let targets: Vec<String> = sent
                .get("reveal")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|t| resolve_target(t, &ids))
                .filter(|t| revealed.insert(t.clone()))
                .collect();
            if !targets.is_empty() {
                let step = builds.len() + 1;
                let effect = sent
                    .get("effect")
                    .and_then(Value::as_str)
                    .filter(|e| ["appear", "fade", "fly_up", "draw"].contains(e))
                    .unwrap_or("fade");
                builds.push(json!({"step": step, "targets": targets, "effect": effect}));
                cues.push(json!({"type": "step", "n": step}));
            }
            for p in sent
                .get("points")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let target = resolve_target(&sv(p, "target"), &ids);
                if target.is_empty() {
                    continue;
                }
                let mut cue = json!({"type": "laser", "target": target, "shape": "ellipse"});
                let at = sv(p, "at");
                if !at.is_empty() {
                    cue["at"] = json!(at);
                }
                cues.push(cue);
                pointed += 1;
            }
            narr[j]["cues"] = json!(cues);
        }
        let steps = builds.len();
        if let Some(o) = slide.as_object_mut() {
            if builds.is_empty() {
                o.remove("builds");
            } else {
                o.insert("builds".into(), json!(builds));
            }
        }
        log.push(format!(
            "第 {} 页：{} 个分步揭示，{} 处激光指向",
            idx + 1,
            steps,
            pointed
        ));
    }
    log
}

/// A slide's current animation in the animation author's own format (per sentence: what it
/// reveals and where it points), so a repair can start from it and change only what is wrong.
pub(crate) fn animation_plan_of(slide: &Value) -> Value {
    let builds = slide
        .get("builds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let sentences: Vec<Value> = slide
        .get("narration")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(j, n)| {
            let cues = n
                .get("cues")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut reveal: Vec<Value> = Vec::new();
            let mut effect = Value::Null;
            let mut points: Vec<Value> = Vec::new();
            for c in &cues {
                match sv(c, "type").as_str() {
                    "step" => {
                        let k = c.get("n").and_then(Value::as_i64).unwrap_or(0);
                        for b in builds
                            .iter()
                            .filter(|b| b.get("step").and_then(Value::as_i64) == Some(k))
                        {
                            reveal.extend(
                                b.get("targets")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default(),
                            );
                            if effect.is_null() {
                                effect = b.get("effect").cloned().unwrap_or(Value::Null);
                            }
                        }
                    }
                    "laser" => {
                        let mut p = json!({"target": sv(c, "target")});
                        if !sv(c, "at").is_empty() {
                            p["at"] = json!(sv(c, "at"));
                        }
                        points.push(p);
                    }
                    _ => {}
                }
            }
            let mut o = json!({"sentence": j + 1, "reveal": reveal, "points": points});
            if !effect.is_null() {
                o["effect"] = effect;
            }
            o
        })
        .collect();
    json!({ "sentences": sentences })
}

/// A repaired plan may contain only the sentences the author changed. Lay it over the slides'
/// current animation: provided sentences replace their old versions, every other sentence keeps
/// what it had, except that a target the repair revealed somewhere else is no longer revealed
/// at its old place (so moving a reveal really moves it).
pub(crate) fn merge_animation_plan(slides: &[Value], plan: &Value) -> Value {
    let merged: Vec<Value> = plan
        .get("slides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|sp| {
            let idx = (sp.get("slide").and_then(Value::as_u64)? as usize).checked_sub(1)?;
            let base = animation_plan_of(slides.get(idx)?);
            let provided: Vec<&Value> = sp
                .get("sentences")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .collect();
            let number = |v: &Value| v.get("sentence").and_then(Value::as_u64).unwrap_or(0);
            let moved: HashSet<String> = provided
                .iter()
                .flat_map(|p| {
                    p.get("reveal")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|t| t.as_str().map(String::from))
                .collect();
            let sentences: Vec<Value> = base["sentences"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut b| {
                    if let Some(p) = provided.iter().find(|p| number(p) == number(&b)) {
                        return (*p).clone();
                    }
                    if let Some(r) = b.get_mut("reveal").and_then(Value::as_array_mut) {
                        r.retain(|t| t.as_str().is_none_or(|t| !moved.contains(t)));
                    }
                    b
                })
                .collect();
            Some(json!({"slide": idx + 1, "sentences": sentences}))
        })
        .collect();
    json!({ "slides": merged })
}

/// Removes everything animation from authored slides (a model may emit builds or cues even
/// though it was told not to; the animation step is the only author of them).
pub(crate) fn strip_animation(slides: &mut [Value]) {
    for s in slides.iter_mut() {
        if let Some(o) = s.as_object_mut() {
            o.remove("builds");
        }
        if let Some(ns) = s.get_mut("narration").and_then(Value::as_array_mut) {
            for n in ns {
                if let Some(o) = n.as_object_mut() {
                    o.remove("cues");
                }
            }
        }
    }
}

/// System prompt of the fresh-context animation reviewer (it never sees how the section was
/// written, only the finished data and the playback rules).
pub(crate) const ANIMATION_REVIEW_SYSTEM: &str = "You are the Local Notebook Studio lecture reviewer. You review the animation order of finished lecture slides. You see only the slides' data, not how they were written. Output {\"problems\": [...]} and nothing else; an empty list means everything is consistent.\n\nPLAYBACK RULES\n- Elements in no entrance build (effect appear/fade/fly_up/draw; default fade) are visible from the start. Targets: title, b1.. (bullets), table.rN, formula, callout, figure:<partId> for figure parts.\n- A build {\"step\":k,\"targets\":[..]} becomes visible when a narration cue {\"type\":\"step\",\"n\":k} is reached; a sentence's step cues apply when it starts, before its laser cues. A laser cue points at its target while the sentence is spoken.\n- Figure parts come with figure_elements (type, text, label, for) so you can tell a label from the line it labels.\n\nFor every slide, go through the narration sentence by sentence, work out what is visible, and report only REAL problems of these kinds: (1) the sentence speaks about, or points at, a point/vector/line/bullet/formula that is not visible yet; (2) a result, formula or conclusion is revealed before the sentence that explains it; (3) a label, angle mark, projection or helper line is visible before the thing it belongs to; (4) a figure part that the narration discusses is listed in no build although the things it labels appear later. Do not report style, wording, or things that are merely early by a fraction of a sentence. A formula, bullet or table row is one element: it is correctly revealed as a whole on the first sentence that starts to state it, so do not report it because later sentences explain its parts one by one; report it only when it appears before the sentence that first speaks about it, or after a sentence that already relies on it. Each problem: slide (1-based), sentence (1-based, 0 if not tied to one), element (id), fix (\"animation\" when moving a reveal or a pointer solves it; \"narration\" when the narration itself speaks about something that is never shown at that point and only rewording or reordering the sentences can solve it), problem (one concrete sentence quoting the build/cue data that proves it). At most 12 problems, most serious first.";

pub(crate) fn animation_review_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["problems"],
        "properties": {
            "problems": {
                "type": "array",
                "maxItems": 12,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["slide", "sentence", "element", "fix", "problem"],
                    "properties": {
                        "slide": { "type": "integer", "minimum": 1 },
                        "sentence": { "type": "integer", "minimum": 0 },
                        "element": { "type": "string" },
                        "fix": { "type": "string", "enum": ["animation", "narration"] },
                        "problem": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// The slides as the reviewer needs them: text elements, builds, narration with cues, and the
/// figure's parts (ids + what they hang on) and spec elements. No pictures, no SVG.
pub(crate) fn animation_review_input(slides: &[Value]) -> Value {
    let out: Vec<Value> = slides
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let fig = s.get("figure").filter(|f| f.is_object());
            // what each part waits for as the playback rules resolve it (the engine's raw
            // geometric anchors are noisy and would disagree with what really plays)
            let deps = crate::lecture_anim::part_dependencies(s);
            let parts: Vec<Value> = fig
                .and_then(|f| f.pointer("/meta/parts"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| {
                    let id = sv(p, "id");
                    json!({"hangs_on": deps.get(&id), "id": id})
                })
                .collect();
            let elements: Vec<Value> = fig
                .and_then(|f| f.pointer("/spec/elements"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|e| {
                    json!({
                        "id": sv(e, "id"), "type": sv(e, "type"),
                        "text": sv(e, "text"), "label": sv(e, "label"), "for": sv(e, "for"),
                    })
                })
                .collect();
            json!({
                "slide": i + 1,
                "title": sv(s, "title"),
                "bullets": s.get("bullets"),
                "formula": sv(s, "formula"),
                "callout": sv(s, "callout"),
                "table_rows": s.pointer("/table/rows"),
                "builds": s.get("builds"),
                "narration": s.get("narration").and_then(Value::as_array).map(|ns| {
                    ns.iter().enumerate().map(|(j, n)| json!({
                        "sentence": j + 1, "text": sv(n, "text"), "cues": n.get("cues"),
                    })).collect::<Vec<_>>()
                }),
                "figure_parts": parts,
                "figure_elements": elements,
            })
        })
        .collect();
    json!({ "slides": out })
}

/// The reviewer's problems, split by who can fix them: `(animation, narration)`. Animation
/// problems go back to a fresh animation author, narration problems to the section author
/// (unknown slides are ignored).
pub(crate) fn animation_review_issues(out: &Value, slides: &[Value]) -> (Vec<Issue>, Vec<Issue>) {
    let (mut animation, mut narration) = (Vec::new(), Vec::new());
    for p in out
        .get("problems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(12)
    {
        let Some(slide) = p.get("slide").and_then(Value::as_u64).map(|n| n as usize) else {
            continue;
        };
        if slide == 0 || slide > slides.len() {
            continue;
        }
        let (sentence, element, problem) = (
            p.get("sentence").and_then(Value::as_u64).unwrap_or(0),
            sv(p, "element"),
            sv(p, "problem"),
        );
        if problem.trim().is_empty() {
            continue;
        }
        let place = format!(
            "{}{}",
            if sentence > 0 {
                format!(", sentence {sentence}")
            } else {
                String::new()
            },
            if element.is_empty() {
                String::new()
            } else {
                format!(", element {element}")
            },
        );
        if sv(p, "fix") == "narration" {
            narration.push(Issue {
                slide: slide - 1,
                msg: format!("narration order{place}: {problem} (fix by rewording or reordering the narration sentences so nothing is spoken about before it can be on screen; do not change the figure)"),
            });
        } else {
            animation.push(Issue {
                slide: slide - 1,
                msg: format!("animation order{place}: {problem}"),
            });
        }
    }
    (animation, narration)
}

/// Checks lecture slides (one section or a whole lecture). `label(i)` names slide i in messages.
pub fn check_lecture_content(slides: &[Value], mode: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut prev_display_only = false;
    for (si, slide) in slides.iter().enumerate() {
        let narration = slide
            .get("narration")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let spoken: Vec<&Value> = narration
            .iter()
            .filter(|n| !sv(n, "text").is_empty())
            .collect();
        let display_only = slide
            .get("display_only")
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            .is_some_and(|r| !r.trim().is_empty());
        if spoken.is_empty() && !display_only {
            issues.push(Issue { slide: si, msg: "has no narration; add narration sentences or mark it \"display_only\": {\"reason\": ...}".into() });
        }
        if spoken.is_empty() && display_only && prev_display_only {
            issues.push(Issue {
                slide: si,
                msg: "is the second display-only slide in a row; give one of them narration".into(),
            });
        }
        prev_display_only = spoken.is_empty() && display_only;

        // talk about how the material was read, or where "this page" sits in the plan, is not
        // teaching content (the teacher never saw an OCR run or an outline)
        let mut visible = vec![
            sv(slide, "title"),
            sv(slide, "subtitle"),
            sv(slide, "callout"),
        ];
        visible.extend(
            slide
                .get("bullets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|b| b.as_str().map(str::to_string)),
        );
        visible.extend(spoken.iter().map(|n| sv(n, "text")));
        if let Some(term) = visible.iter().find_map(|t| process_talk(t)) {
            issues.push(Issue {
                slide: si,
                msg: format!("talks about the reading/generation process (\"{term}\"); rewrite that text so it teaches the content itself and never mentions OCR, scan or recognition quality, the outline/plan, or where \"this page\" sits in the derivation chain"),
            });
        }

        // a detour through prerequisites before the actual material (review mode recaps on purpose)
        if mode != "review" {
            if let Some(term) = prereq_title(&sv(slide, "title")) {
                issues.push(Issue {
                    slide: si,
                    msg: format!("is a prerequisite/review slide (\"{term}\"); the learner already knows the prerequisites, so do not teach known concepts here: go straight to the material and, where a step needs a known result, name it in one clause at that step (unless the user's request explicitly asked for background)"),
                });
            }
        }

        // a figure never stands alone: the text that explains it sits on the same slide
        if (slide.get("figure_request").is_some_and(Value::is_object)
            || slide.get("figure").is_some_and(Value::is_object))
            && sv(slide, "formula").is_empty()
            && arr_len(slide.get("table").unwrap_or(&Value::Null), "rows") == 0
            && !slide
                .get("bullets")
                .and_then(Value::as_array)
                .is_some_and(|b| {
                    b.iter()
                        .any(|x| x.as_str().is_some_and(|t| !t.trim().is_empty()))
                })
        {
            issues.push(Issue {
                slide: si,
                msg: "has a figure but no on-screen text beside it; add 2-3 very short \"bullets\" (and/or a \"formula\") that state the problem, the key result and what each labelled part of the figure means, so text and figure are read together".into(),
            });
        }
    }
    issues
}

/// Things a model never needs to be asked about, fixed in place: LaTeX in narration becomes
/// plain words, a checkpoint that is missing its question/answer/reason is dropped. Animation
/// is not touched here (see [`crate::lecture_anim`]). Returns what was changed.
pub fn apply_mechanical_fixes(slides: &mut [Value], _mode: &str) -> Vec<String> {
    let mut log = Vec::new();
    for (si, slide) in slides.iter_mut().enumerate() {
        if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
            for n in ns.iter_mut() {
                let text = sv(n, "text");
                if has_tex(&text) {
                    let t = crate::texfmt::tex_to_unicode(&text).replace('$', "");
                    n["text"] = Value::String(t);
                    log.push(format!("slide {}: narration LaTeX converted", si + 1));
                }
            }
        }
        let drop_cp = slide
            .get("checkpoint")
            .filter(|c| c.is_object())
            .is_some_and(|cp| {
                let kind = sv(cp, "kind");
                sv(cp, "question").is_empty()
                    || (kind == "attempt" && sv(cp, "expect").is_empty())
                    || (kind == "ask" && sv(cp, "reason").is_empty())
            });
        if drop_cp {
            if let Some(o) = slide.as_object_mut() {
                o.remove("checkpoint");
            }
            log.push(format!("slide {}: incomplete checkpoint dropped", si + 1));
        }
    }
    log
}

/// Deck (non-lecture) builds: the same consistency rules as a lecture without narration (no
/// step cues, no pointing). Decks are clicked through manually.
pub fn sanitize_deck_builds(slides: &mut [Value]) -> Vec<String> {
    let mut log = Vec::new();
    for (si, slide) in slides.iter_mut().enumerate() {
        for l in crate::lecture_anim::normalize_deck_animation(slide) {
            log.push(format!("slide {}: {l}", si + 1));
        }
    }
    log
}

/// Last resort for a slide that still has neither narration nor a reason: it is display-only.
pub fn mark_unnarrated_display_only(slides: &mut [Value]) -> Vec<String> {
    let mut log = Vec::new();
    for (si, slide) in slides.iter_mut().enumerate() {
        let spoken = slide
            .get("narration")
            .and_then(Value::as_array)
            .is_some_and(|n| n.iter().any(|x| !sv(x, "text").is_empty()));
        let has_reason = slide
            .get("display_only")
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            .is_some_and(|r| !r.trim().is_empty());
        if !spoken && !has_reason {
            slide["display_only"] = json!({ "reason": "(no narration provided)" });
            log.push(format!("slide {}: marked display-only", si + 1));
        }
    }
    log
}

fn issues_text(issues: &[Issue], slides: &[Value]) -> String {
    issues
        .iter()
        .map(|i| {
            format!(
                "- slide {} (\"{}\"): {}",
                i.slide + 1,
                slides
                    .get(i.slide)
                    .map(|s| sv(s, "title"))
                    .unwrap_or_default(),
                i.msg
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Fills defaults a lecture slide needs to also be a valid deck slide, assigns ids.
fn finalize_slides(slides: &mut [Value], section_id: &str, prefix: &str) {
    for (i, s) in slides.iter_mut().enumerate() {
        texify_slide_text(s);
        let Some(o) = s.as_object_mut() else { continue };
        o.insert("id".into(), json!(format!("{prefix}{}", i + 1)));
        o.insert("section".into(), json!(section_id));
        o.entry("bullets").or_insert_with(|| json!([]));
        o.entry("citations").or_insert_with(|| json!([]));
        let notes: String = o
            .get("narration")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|n| sv(n, "text")).collect::<Vec<_>>().join(""))
            .unwrap_or_default();
        o.insert("speaker_notes".into(), json!(notes));
        let sid = format!("{prefix}{}", i + 1);
        if let Some(ns) = o.get_mut("narration").and_then(Value::as_array_mut) {
            for (j, n) in ns.iter_mut().enumerate() {
                if let Some(no) = n.as_object_mut() {
                    no.insert("id".into(), json!(format!("{sid}.{}", j + 1)));
                    no.entry("cues").or_insert_with(|| json!([]));
                }
            }
        }
    }
}

/// Estimated speaking time: ~250 Chinese characters or ~150 English words per minute.
pub fn estimate_minutes(slides: &[Value]) -> f64 {
    let mut cjk = 0usize;
    let mut words = 0usize;
    for s in slides {
        for n in s
            .get("narration")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let t = sv(n, "text");
            cjk += t.chars().filter(|c| (*c as u32) >= 0x2E80).count();
            words += t
                .split_whitespace()
                .filter(|w| w.chars().any(|c| c.is_ascii_alphanumeric()))
                .count();
        }
    }
    let secs = cjk as f64 / 250.0 * 60.0 + words as f64 / 150.0 * 60.0 + slides.len() as f64 * 1.5;
    (secs / 60.0 * 10.0).round() / 10.0
}

/// Step at which each figure part becomes visible (0 = from the start).
pub fn figure_part_steps(slide: &Value) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for b in slide
        .get("builds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let step = b.get("step").and_then(Value::as_i64).unwrap_or(0);
        let effect = sv(b, "effect");
        if effect == "emphasis" || effect == "dim_others" {
            continue;
        }
        for t in b
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(p) = t.as_str().and_then(|t| t.strip_prefix("figure:")) {
                out.entry(p.to_string())
                    .and_modify(|s: &mut i64| *s = (*s).min(step))
                    .or_insert(step);
            }
        }
    }
    out
}

/// Writes figure assets next to the artifact: `figures/<slide id>.svg` (all parts, with
/// `data-part` groups for client-side builds) and `figures/<slide id>_<k>.png` for every
/// figure build step k (cumulative; k = 0 is the initial state).
pub fn render_figure_assets(payload: &Value, out_dir: &std::path::Path) -> LnaResult<usize> {
    let dir = out_dir.join("figures");
    let mut written = 0;
    for (idx, slide) in payload
        .get("slides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(spec) = slide.pointer("/figure/spec") else {
            continue;
        };
        std::fs::create_dir_all(&dir).map_err(|e| LnaError::internal(e.to_string()))?;
        let sid = slide_file_id(slide, idx);
        let full = render_figure(spec, &HashSet::new());
        std::fs::write(dir.join(format!("{sid}.svg")), &full.svg)
            .map_err(|e| LnaError::internal(e.to_string()))?;
        let steps = figure_part_steps(slide);
        let max = steps.values().copied().max().unwrap_or(0);
        for k in 0..=max {
            let hidden: HashSet<String> = steps
                .iter()
                .filter(|(_, s)| **s > k)
                .map(|(p, _)| p.clone())
                .collect();
            let out = if hidden.is_empty() {
                full.clone()
            } else {
                render_figure(spec, &hidden)
            };
            match svg_to_png(&out.svg, 2.0) {
                Ok(png) => {
                    std::fs::write(dir.join(format!("{sid}_{k}.png")), png)
                        .map_err(|e| LnaError::internal(e.to_string()))?;
                    written += 1;
                }
                Err(e) => tracing::warn!("figure png for slide {sid} failed: {e}"),
            }
        }
    }
    Ok(written)
}

/// File-safe id for a slide's figure assets (lecture slides have ids; deck slides use the index).
pub fn slide_file_id(slide: &Value, idx: usize) -> String {
    let id = sv(slide, "id");
    let id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if id.is_empty() {
        format!("slide{}", idx + 1)
    } else {
        id
    }
}

// ---------------------------------------------------------------------------------------------

struct SectionCtx<'a> {
    lecture_title: &'a str,
    objectives: &'a [String],
    outline_txt: &'a str,
    section_title: &'a str,
    section_focus: &'a str,
    position: String,
    extra: String,
    /// "required" when the planner judged the section obviously geometric or physical: it must
    /// carry at least one figure. "suggested" and "none" are guidance in the prompt only.
    visual_need: &'a str,
    /// Optional checkpoint key (e.g. `lecture_sec_s1`) to persist page skeleton & completed batches.
    checkpoint_key: Option<String>,
    /// When true, plan per-page skeleton first and author 3–4 slides per batch.
    batch_by_page: bool,
}

impl StudioService {
    fn vision_profile(&self, resolved: &ResolvedProfile) -> Option<ResolvedProfile> {
        if resolved.capabilities.supports_vision {
            return Some(resolved.clone());
        }
        let prof = lna_storage::resolve_modality_profile(&self.db, "vision")
            .ok()
            .flatten()?;
        let api_key = prof
            .keychain_ref
            .as_deref()
            .and_then(|k| self.secrets.get(k).ok().flatten());
        Some(ResolvedProfile::from_model(&prof, api_key))
    }

    /// Draws one figure in a clean context: request → DSL → render → (errors/warnings or a
    /// visual check) → corrected DSL, at most `FIGURE_REVIEW_ROUNDS` review rounds.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn generate_figure(
        &self,
        art: &StudioArtifact,
        _profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        slide_title: &str,
        request: &Value,
        excerpt: &str,
        cancel: CancellationToken,
    ) -> LnaResult<(Option<Value>, Usage)> {
        let mut usage = Usage::default();
        let schema = figure_spec_schema();
        let system = format!(
            "You are the Local Notebook Studio figure designer (precise mathematics/physics figures) producing structured JSON for figure. You do not write SVG; you describe the figure in the DSL below and a renderer draws it exactly. Use exactly the part ids named in the request. Labels are short plain text (Unicode like x², θ, F₁ is fine; no LaTeX).\n\n{FIGURE_DSL_GUIDE}"
        );
        let user = format!(
            "Slide: {slide_title}\nFigure request: {}\n\nRelevant source excerpt (for exact values; may be empty):\n{}",
            request,
            truncate_str(excerpt, 3000)
        );
        let req = LlmRequest::simple(system.clone(), user.clone(), resolved.bounded_output(4000));
        let (mut spec, u) = {
            self.run_structured_with_thought(
                artifact_id,
                &art.notebook_id,
                &format!("绘制配图：{slide_title}"),
                resolved,
                &req,
                "figure_spec",
                &schema,
                2,
                cancel.clone(),
            )
            .await?
        };
        add_usage(&mut usage, &u);
        let vision = self.vision_profile(resolved);
        let mut visual_checked = false;
        let mut out = render_figure(&spec, &HashSet::new());
        for round in 0..FIGURE_REVIEW_ROUNDS {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let problems = !out.errors.is_empty() || !out.warnings.is_empty();
            let mut parts = vec![Part::Text(format!(
                "{user}\n\nYour figure JSON:\n{}\n\nRender report:\n{}\n{}",
                spec,
                out.report_text(),
                if problems {
                    "Fix every ERROR and WARNING (adjust ranges, positions, label placement or ids) and return the corrected figure as {\"ok\": false, \"problems\": <what you changed>, \"figure\": {...}}."
                } else {
                    "Look at the rendered figure. If it correctly and clearly shows what the request asks for (correct shapes and values, readable labels, key features visible), return {\"ok\": true}. Otherwise return {\"ok\": false, \"problems\": ..., \"figure\": <corrected figure>}."
                }
            ))];
            if !problems {
                // nothing mechanical to fix: one visual look when a vision-capable model exists
                if visual_checked || vision.is_none() {
                    break;
                }
                match svg_to_png(&out.svg, 1.0) {
                    Ok(png) => {
                        use base64::Engine as _;
                        parts.push(Part::Media {
                            mime_type: "image/png".into(),
                            data_base64: Some(
                                base64::engine::general_purpose::STANDARD.encode(png),
                            ),
                            file_uri: None,
                            audio_format: None,
                        });
                    }
                    Err(_) => break,
                }
                visual_checked = true;
            }
            let review_schema = json!({
                "type": "object",
                "required": ["ok"],
                "properties": { "ok": { "type": "boolean" }, "problems": { "type": "string" }, "figure": figure_spec_schema() }
            });
            let review_req =
                LlmRequest::from_parts(system.clone(), parts, resolved.bounded_output(4000));
            let target = if !problems {
                vision.clone().unwrap_or_else(|| resolved.clone())
            } else {
                resolved.clone()
            };
            let res = {
                self.run_structured_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &format!("检查配图（第 {} 轮）：{slide_title}", round + 1),
                    &target,
                    &review_req,
                    "figure_review",
                    &review_schema,
                    1,
                    cancel.clone(),
                )
                .await
            };
            match res {
                Ok((v, u)) => {
                    add_usage(&mut usage, &u);
                    if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        break;
                    }
                    if let Some(f) = v.get("figure").filter(|f| f.get("elements").is_some()) {
                        spec = f.clone();
                        out = render_figure(&spec, &HashSet::new());
                    } else {
                        break;
                    }
                }
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    tracing::warn!("figure review failed: {}", e.message);
                    break;
                }
            }
        }
        let content_parts = out
            .parts
            .iter()
            .filter(|p| !matches!(p.id.as_str(), "title" | "grid" | "axes"))
            .count();
        if content_parts == 0 {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &format!("配图未能生成：{slide_title}"),
                &out.report_text(),
            );
            return Ok((None, usage));
        }
        let incomplete = !out.errors.is_empty();
        if incomplete || !out.warnings.is_empty() {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &format!("配图可能不完整：{slide_title}"),
                &format!(
                    "经 {FIGURE_REVIEW_ROUNDS} 轮检查后仍有问题，已保留可绘制的部分：\n{}",
                    out.report_text()
                ),
            );
        }
        Ok((
            Some(json!({
                "kind": sv(&spec, "kind"),
                "spec": spec,
                "meta": out.meta_json(),
                "incomplete": incomplete,
            })),
            usage,
        ))
    }

    /// Draws every `figure_request` in `slides` (replacing it with a rendered `figure`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fulfil_figure_requests(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        slides: &mut [Value],
        excerpt: &str,
        cancel: CancellationToken,
    ) -> LnaResult<Usage> {
        let mut usage = Usage::default();
        // Each figure is an independent executor with its own clean context.
        let jobs: Vec<(usize, String, Value)> = slides
            .iter()
            .enumerate()
            .filter_map(|(i, slide)| {
                let req = slide
                    .get("figure_request")
                    .cloned()
                    .filter(|r| r.is_object())?;
                if slide.get("figure").is_some_and(|f| f.is_object()) {
                    return None;
                }
                Some((i, sv(slide, "title"), req))
            })
            .collect();
        let limit = self.parallelism(profile).min(jobs.len().max(1));
        lna_llm::run_parallel_unordered(
            jobs,
            limit,
            &cancel,
            |_, (i, title, req)| {
                let cancel_c = cancel.clone();
                async move {
                    // A figure that could not be drawn (model error, invalid spec, no content parts)
                    // is tried once more before the slide is left without it.
                    let mut res = self
                        .generate_figure(
                            art,
                            profile,
                            resolved,
                            artifact_id,
                            &title,
                            &req,
                            excerpt,
                            cancel_c.clone(),
                        )
                        .await;
                    let failed = match &res {
                        Ok((fig, _)) => fig.is_none(),
                        Err(e) => e.code != ErrorCode::Cancelled,
                    };
                    if failed && !cancel_c.is_cancelled() {
                        res = self
                            .generate_figure(
                                art,
                                profile,
                                resolved,
                                artifact_id,
                                &title,
                                &req,
                                excerpt,
                                cancel_c,
                            )
                            .await;
                    }
                    (i, title, res)
                }
            },
            |_, (i, title, res)| {
                match res {
                    Ok((fig, u)) => {
                        add_usage(&mut usage, &u);
                        match fig {
                            Some(fig) => slides[i]["figure"] = fig,
                            // no figure after the retry: a "figure" layout without one would play as
                            // an empty-looking slide, so it becomes an ordinary slide
                            None => {
                                self.append_thought(
                                    artifact_id,
                                    &art.notebook_id,
                                    "stage",
                                    &format!("配图未能画出：{title}"),
                                    "两次尝试都没有得到可用的图，该页不带图，指向图形部件的动画提示已被移除。",
                                );
                                if sv(&slides[i], "layout") == "figure" {
                                    slides[i]["layout"] = json!("bullets");
                                }
                            }
                        }
                    }
                    Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                    Err(e) => {
                        self.append_thought(
                            artifact_id,
                            &art.notebook_id,
                            "stage",
                            &format!("配图失败：{title}"),
                            &truncate_str(&e.message, 300),
                        );
                    }
                }
                Ok(())
            },
        )
        .await?;
        Ok(usage)
    }

    /// Evidence for one section: full text for small sources, else multi-round on-demand
    /// reading focused on the section, else plain retrieval.
    #[allow(clippy::too_many_arguments)]
    async fn section_evidence(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        focus: &str,
        full_text: Option<&str>,
        cancel: CancellationToken,
    ) -> LnaResult<String> {
        if let Some(t) = full_text {
            return Ok(t.to_string());
        }
        // The user's own material (briefed once at collection time) goes first for every
        // section; the source passages below only support it.
        let mut materials = String::new();
        for (key, tag) in [("file_brief", "user_files"), ("image_brief", "user_images")] {
            if let Ok(Some(saved)) = self.get_checkpoint(artifact_id, key) {
                if let Some(t) = saved
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                {
                    materials.push_str(&format!("<{tag}>\n{t}\n</{tag}>\n\n"));
                }
            }
        }
        if let Ok(Some(ref_arts)) = self.artifact_brief(art) {
            materials.push_str(&format!("<referenced_artifacts>\n{ref_arts}\n</referenced_artifacts>\n\n"));
        }
        let with_materials = |body: String| format!("{materials}{body}");
        let target_ids = Self::generation_source_ids(art);
        let budget = section_read_budget(resolved);
        match self
            .gather_evidence_on_demand(
                art,
                profile,
                resolved,
                artifact_id,
                Some(focus),
                budget,
                Some(&target_ids),
                true,
                None,
                cancel,
            )
            .await
        {
            Ok(Some((t, _))) if !t.trim().is_empty() => Ok(with_materials(t)),
            Err(e) if ends_evidence_read(e.code) => Err(e),
            _ => {
                let queries = vec![focus.to_string()];
                Ok(with_materials(
                    match self
                        .retrieval
                        .search(&art.notebook_id, &queries, &target_ids, 8_000, 14)
                        .await
                    {
                        Ok(res) => res
                            .evidences
                            .iter()
                            .enumerate()
                            .map(|(i, e)| format!("[{}] ({}) {}", i + 1, e.source_title, e.content))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                        Err(_) => String::new(),
                    },
                ))
            }
        }
    }

    /// Authors one section (or an insertion/continuation): slides with narration and cues,
    /// figures drawn separately, then validation with up to `LECTURE_REPAIR_ROUNDS` repairs.
    #[allow(clippy::too_many_arguments)]
    async fn author_section(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        sc: &SectionCtx<'_>,
        evidence: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Vec<Value>, Usage)> {
        let started = std::time::Instant::now();
        let mode = art
            .config
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("general");
        let detail = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let rules = lecture_rules(mode, detail);
        let schema = section_schema();
        // the user's pictures go to the section author itself when the model can see them; the
        // repair requests below are cloned from the same request, so they keep them too
        let images = image_parts(&self.db.paths().root, art, resolved);
        let system = format!(
            "You are the Local Notebook Studio lecture author producing structured JSON for lecture slides of ONE section of a narrated lesson (a teacher speaking over slides). Output {{\"slides\": [...]}} for this section only. Slide layouts as in a deck (title/section/agenda/bullets/two_column/cards/metrics/timeline/table/quote/formula/summary/figure); keep on-screen text concise and let the narration carry the explanation. Preserve source citation indices [n] in each slide's citations array.\n{NARRATION_RULES}\n{ANIMATION_NOTE}\n{FIGURE_REQUEST_GUIDE}\n{rules}\nStrictly ground every fact in the provided sources. Match the language requested in config or the primary language of the sources. Sections are written separately and played in order: unless the section position below asks for a title or summary slide, do not add an opening or closing slide of your own, only slides that teach this section's focus.{}",
            if has_user_material(&art.config) {
                format!("\n{USER_MATERIAL_SCOPE} Write only this section's focus; do not add material beyond it.")
            } else {
                String::new()
            }
        );
        let user = format!(
            // Everything shared by the sections comes first and what is specific to this section
            // comes last, so parallel sections share a long prompt prefix (provider prompt cache).
            "Parameters: {}\nLecture: {}\nObjectives: {}\nOutline:\n{}\n\nSources:\n{}\n\n=== THIS SECTION ({}): {} ===\nFocus: {}\n{}",
            config_for_prompt(&art.config, !images.is_empty()),
            sc.lecture_title,
            sc.objectives.join("; "),
            sc.outline_txt,
            evidence,
            sc.position,
            sc.section_title,
            sc.section_focus,
            sc.extra
        );
        let max_out = resolved.hard_output_cap(LECTURE_MAX_OUTPUT_TOKENS);
        let mk_req = |sys: String, text: String, cap: u32| {
            with_images(
                LlmRequest::simple(sys, text, cap).with_reasoning(reasoning.clone()),
                &images,
            )
        };
        let mut usage = Usage::default();
        let mut slides: Vec<Value> = if sc.batch_by_page {
            // Load section-level checkpoint (skeleton + already-written batches) so a failure on
            // batch K only retries batch K without losing earlier batches.
            let saved_sec = sc
                .checkpoint_key
                .as_deref()
                .and_then(|ck| self.get_checkpoint(artifact_id, ck).ok().flatten())
                .filter(|v| {
                    v.get("sources_fingerprint").and_then(Value::as_str)
                        == Some(art.sources_fingerprint.as_str())
                });
            let (skeleton_pages, mut done_batches): (Vec<Value>, BTreeMap<String, Vec<Value>>) =
                match saved_sec {
                    Some(v) => {
                        let pages = v
                            .get("pages")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let batches = v
                            .get("batches")
                            .and_then(Value::as_object)
                            .map(|o| {
                                o.iter()
                                    .filter_map(|(k, val)| {
                                        val.as_array().map(|a| (k.clone(), a.clone()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        (pages, batches)
                    }
                    None => (Vec::new(), BTreeMap::new()),
                };

            let skeleton_pages = if !skeleton_pages.is_empty() {
                if !done_batches.is_empty() {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &format!("复用已撰写页批次：{}", sc.section_title),
                        &format!(
                            "已从检查点恢复本节 {} 页骨架与 {} 个已完成批次，仅续写剩余批次。",
                            skeleton_pages.len(),
                            done_batches.len()
                        ),
                    );
                }
                skeleton_pages
            } else {
                // Reuse the exact same `system` prompt as slide authoring (putting the skeleton-only
                // instruction at the END of the user message) so Phase 1 warms the provider's prompt
                // cache for Phase 2's slide batches.
                let skel_user = format!(
                    "{user}\n\nPLANNING TASK (do NOT write full slides or narration yet): Output ONLY the per-page skeleton JSON ({{\"pages\": [{{\"title\": ..., \"points\": [...], \"needs_figure\": true|false, \"figure_hint\": ...}}]}}) for this section. Plan only as many pages as needed (2–4 concise points per page, needs_figure + short figure_hint when geometric/physical)."
                );
                let skel_req = mk_req(system.clone(), skel_user, resolved.hard_output_cap(8_192));
                let (skel_out, u) = self
                    .run_structured_with_thought_ex(
                        artifact_id,
                        &art.notebook_id,
                        &format!("规划逐页骨架：{}", sc.section_title),
                        resolved,
                        &skel_req,
                        "lecture_section_skeleton",
                        &section_page_skeleton_schema(),
                        max_repairs,
                        Some("pages"),
                        cancel.clone(),
                    )
                    .await?;
                add_usage(&mut usage, &u);
                let pages = skel_out
                    .get("pages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if let Some(ck) = sc.checkpoint_key.as_deref() {
                    let _ = self.save_checkpoint(
                        artifact_id,
                        ck,
                        json!({
                            "sources_fingerprint": art.sources_fingerprint,
                            "pages": pages,
                            "batches": done_batches,
                        }),
                    );
                }
                pages
            };

            if skeleton_pages.is_empty() {
                // Fallback: single capped call with array continuation if skeleton came back empty
                let (out, u) = self
                    .run_structured_with_thought_ex(
                        artifact_id,
                        &art.notebook_id,
                        &format!("撰写讲解：{}", sc.section_title),
                        resolved,
                        &mk_req(system.clone(), user.clone(), max_out),
                        "lecture_section",
                        &schema,
                        max_repairs,
                        Some("slides"),
                        cancel.clone(),
                    )
                    .await?;
                add_usage(&mut usage, &u);
                out.get("slides")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                let batches = split_slide_batches(skeleton_pages.len());
                let total_batches = batches.len();
                let skeleton_txt = format_page_skeleton(&skeleton_pages);
                let mut written_titles: Vec<String> = Vec::new();
                let mut collected: Vec<Value> = Vec::new();

                for (bi, (start, end)) in batches.into_iter().enumerate() {
                    if cancel.is_cancelled() {
                        return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                    }
                    let bkey = format!("b{bi}");
                    if let Some(cached) = done_batches.get(&bkey) {
                        for s in cached {
                            let t = sv(s, "title");
                            if !t.is_empty() {
                                written_titles.push(t);
                            }
                        }
                        collected.extend(cached.iter().cloned());
                        continue;
                    }
                    let slice_txt = skeleton_pages[start..end]
                        .iter()
                        .enumerate()
                        .map(|(offset, p)| {
                            let pno = start + offset + 1;
                            let title = sv(p, "title");
                            let pts: Vec<&str> = p
                                .get("points")
                                .and_then(Value::as_array)
                                .map(|a| a.iter().filter_map(Value::as_str).collect())
                                .unwrap_or_default();
                            let needs_fig = p
                                .get("needs_figure")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let fig_hint = sv(p, "figure_hint");
                            format!(
                                "- Page {pno}: {title}
  Points: {}
  Figure: {}",
                                pts.join("; "),
                                if needs_fig {
                                    if fig_hint.is_empty() {
                                        "required"
                                    } else {
                                        fig_hint.as_str()
                                    }
                                } else {
                                    "none"
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        );
                    let prev_txt = if written_titles.is_empty() {
                        "(none yet — this is the first batch of this section)".to_string()
                    } else {
                        written_titles
                            .iter()
                            .enumerate()
                            .map(|(idx, t)| format!("{}. {t}", idx + 1))
                            .collect::<Vec<_>>()
                            .join(
                                "
",
                            )
                    };
                    let batch_user = format!(
                        "{user}

=== SECTION PAGE SKELETON ({} pages total) ===
{skeleton_txt}

=== ALREADY WRITTEN PAGES IN THIS SECTION (do NOT repeat; continue seamlessly) ===
{prev_txt}

=== THIS BATCH ({}/{}, write ONLY pages {}-{}) ===
{slice_txt}

Output {{\"slides\": [...]}} containing ONLY these {} slide(s) in order, following the skeleton and connecting smoothly from the already written pages.",
                        skeleton_pages.len(),
                        bi + 1,
                        total_batches,
                        start + 1,
                        end,
                        end - start,
                    );
                    let thought_label = if total_batches == 1 {
                        format!("撰写讲解：{}", sc.section_title)
                    } else {
                        format!(
                            "撰写讲解：{}（第 {}/{} 批，第 {}-{} 页）",
                            sc.section_title,
                            bi + 1,
                            total_batches,
                            start + 1,
                            end
                        )
                    };
                    let (out, u) = self
                        .run_structured_with_thought_ex(
                            artifact_id,
                            &art.notebook_id,
                            &thought_label,
                            resolved,
                            &mk_req(system.clone(), batch_user, max_out),
                            "lecture_section",
                            &schema,
                            max_repairs,
                            Some("slides"),
                            cancel.clone(),
                        )
                        .await?;
                    add_usage(&mut usage, &u);
                    let batch_slides = out
                        .get("slides")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if batch_slides.is_empty() {
                        return Err(LnaError::new(
                            ErrorCode::OutputInvalid,
                            format!(
                                "Lecture section batch {}/{} produced no slides",
                                bi + 1,
                                total_batches
                            ),
                        ));
                    }
                    for s in &batch_slides {
                        let t = sv(s, "title");
                        if !t.is_empty() {
                            written_titles.push(t);
                        }
                    }
                    done_batches.insert(bkey, batch_slides.clone());
                    if let Some(ck) = sc.checkpoint_key.as_deref() {
                        let _ = self.save_checkpoint(
                            artifact_id,
                            ck,
                            json!({
                                "sources_fingerprint": art.sources_fingerprint,
                                "pages": skeleton_pages,
                                "batches": done_batches,
                            }),
                        );
                    }
                    collected.extend(batch_slides);
                }
                collected
            }
        } else {
            let (out, u) = self
                .run_structured_with_thought_ex(
                    artifact_id,
                    &art.notebook_id,
                    &format!("撰写讲解：{}", sc.section_title),
                    resolved,
                    &mk_req(system.clone(), user.clone(), max_out),
                    "lecture_section",
                    &schema,
                    max_repairs,
                    Some("slides"),
                    cancel.clone(),
                )
                .await?;
            add_usage(&mut usage, &u);
            out.get("slides")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        if slides.is_empty() {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Lecture section produced no slides",
            ));
        }
        strip_animation(&mut slides);
        let u = self
            .fulfil_figure_requests(
                art,
                profile,
                resolved,
                artifact_id,
                &mut slides,
                evidence,
                cancel.clone(),
            )
            .await?;
        add_usage(&mut usage, &u);

        // animation is its own step, after the content exists: a fresh-context call that sees
        // only the finished slides decides what appears when and where the laser points
        let u = self
            .animate_section(
                artifact_id,
                &art.notebook_id,
                resolved,
                sc.section_title,
                &mut slides,
                &[],
                max_out,
                &reasoning,
                cancel.clone(),
            )
            .await?;
        add_usage(&mut usage, &u);
        self.tidy_section(
            artifact_id,
            &art.notebook_id,
            sc.section_title,
            &mut slides,
            mode,
        );
        // after the content checks pass, a second fresh-context call reviews the result as a
        // viewer sees it. Animation problems go back to a new animation author, narration
        // problems to the section author; the loop ends when a review finds nothing
        let rounds = self.lecture_repair_rounds_setting();
        let (max_reviews, budget) = (lecture_review_rounds(), lecture_section_budget());
        let mut reviews = 0usize;
        for round in 0..=rounds {
            let mut content = check_lecture_content(&slides, mode);
            content.extend(check_section_visuals(&slides, sc.visual_need));
            let (mut anim_issues, mut narr_issues) = (Vec::new(), Vec::new());
            if content.is_empty()
                && round < rounds
                && reviews < max_reviews
                && started.elapsed() < budget
            {
                reviews += 1;
                let (found, u) = self
                    .review_animation(
                        artifact_id,
                        &art.notebook_id,
                        resolved,
                        sc.section_title,
                        &slides,
                        LlmRequest::simple(
                            ANIMATION_REVIEW_SYSTEM.to_string(),
                            animation_review_input(&slides).to_string(),
                            max_out,
                        )
                        .with_reasoning(reasoning.clone()),
                        cancel.clone(),
                    )
                    .await?;
                add_usage(&mut usage, &u);
                (anim_issues, narr_issues) = found;
            }
            let mut writer_issues = content.clone();
            writer_issues.append(&mut narr_issues);
            if writer_issues.is_empty() && anim_issues.is_empty() {
                break;
            }
            if round == rounds {
                let log = mark_unnarrated_display_only(&mut slides);
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &format!("讲解校验：{} 仍有问题", sc.section_title),
                    &format!(
                        "经 {rounds} 轮返修后仍有内容问题，保留现状：\n{}\n\n{}",
                        issues_text(&writer_issues, &slides),
                        log.join("\n")
                    ),
                );
                break;
            }
            if writer_issues.is_empty() {
                // only the animation is wrong: a new animation author, given the problems
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &format!("动画返修：{}（第 {} 轮）", sc.section_title, round + 1),
                    &issues_text(&anim_issues, &slides),
                );
                let u = self
                    .animate_section(
                        artifact_id,
                        &art.notebook_id,
                        resolved,
                        sc.section_title,
                        &mut slides,
                        &anim_issues,
                        max_out,
                        &reasoning,
                        cancel.clone(),
                    )
                    .await?;
                add_usage(&mut usage, &u);
                self.tidy_section(
                    artifact_id,
                    &art.notebook_id,
                    sc.section_title,
                    &mut slides,
                    mode,
                );
                continue;
            }
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &if content.is_empty() {
                    format!("动画审查返修：{}（第 {} 轮）", sc.section_title, round + 1)
                } else {
                    format!("讲解校验：{}（返修第 {} 轮）", sc.section_title, round + 1)
                },
                &issues_text(&writer_issues, &slides),
            );
            // figures already drawn are kept; show the model their real part ids. The author
            // never wrote animation, so it is not shown any
            let figures: Vec<(usize, Value)> = slides
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    s.get("figure")
                        .filter(|f| f.is_object())
                        .map(|f| (i, f.clone()))
                })
                .collect();
            let mut shown = slides.clone();
            strip_animation(&mut shown);
            for s in shown.iter_mut() {
                if let Some(o) = s.as_object_mut() {
                    if let Some(f) = o.remove("figure") {
                        let ids: Vec<String> = f
                            .pointer("/meta/parts")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|p| format!("figure:{}", sv(p, "id")))
                            .collect();
                        o.insert("figure_parts_available".into(), json!(ids));
                    }
                }
            }
            let repair = format!(
                "{user}\n\nYour previous output for this section:\n{}\n\nValidation problems:\n{}\n\nReturn the corrected {{\"slides\": [...]}} for this section. Keep the same slides in the same order and keep every \"figure_request\" unchanged (those figures are already drawn; their real part ids are listed in figure_parts_available). Fix only what is listed.",
                json!({ "slides": shown }),
                issues_text(&writer_issues, &slides)
            );
            let res = {
                self.run_structured_with_thought_ex(
                    artifact_id,
                    &art.notebook_id,
                    &format!("返修讲解：{}", sc.section_title),
                    resolved,
                    &mk_req(system.clone(), repair, max_out),
                    "lecture_section",
                    &schema,
                    max_repairs.min(1),
                    Some("slides"),
                    cancel.clone(),
                )
                .await
            };
            match res {
                Ok((fixed, u)) => {
                    add_usage(&mut usage, &u);
                    let mut new_slides = fixed
                        .get("slides")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if new_slides.is_empty() {
                        continue;
                    }
                    for (i, f) in &figures {
                        if let Some(s) = new_slides.get_mut(*i) {
                            if s.get("figure_request").is_some() || s.get("figure").is_none() {
                                s["figure"] = f.clone();
                            }
                        }
                    }
                    for s in new_slides.iter_mut() {
                        if let Some(o) = s.as_object_mut() {
                            o.remove("figure_parts_available");
                        }
                    }
                    strip_animation(&mut new_slides);
                    // a repair may add a brand-new figure request
                    let u = self
                        .fulfil_figure_requests(
                            art,
                            profile,
                            resolved,
                            artifact_id,
                            &mut new_slides,
                            evidence,
                            cancel.clone(),
                        )
                        .await?;
                    add_usage(&mut usage, &u);
                    // the text changed, so the animation is written again from scratch
                    let u = self
                        .animate_section(
                            artifact_id,
                            &art.notebook_id,
                            resolved,
                            sc.section_title,
                            &mut new_slides,
                            &[],
                            max_out,
                            &reasoning,
                            cancel.clone(),
                        )
                        .await?;
                    add_usage(&mut usage, &u);
                    self.tidy_section(
                        artifact_id,
                        &art.notebook_id,
                        sc.section_title,
                        &mut new_slides,
                        mode,
                    );
                    slides = new_slides;
                }
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    tracing::warn!("lecture repair failed: {}", e.message);
                }
            }
        }
        for s in slides.iter_mut() {
            if let Some(o) = s.as_object_mut() {
                o.remove("figure_request");
            }
        }
        // the final word on animation order, whatever the repair rounds did
        self.tidy_section(
            artifact_id,
            &art.notebook_id,
            sc.section_title,
            &mut slides,
            mode,
        );
        for (i, s) in slides.iter().enumerate() {
            let left = crate::lecture_anim::animation_violations(s);
            if !left.is_empty() {
                tracing::warn!(
                    "lecture slide {} animation still inconsistent: {:?}",
                    i + 1,
                    left
                );
            }
        }
        Ok((slides, usage))
    }

    /// One fresh-context model call that reads a finished section the way a viewer sees it
    /// (what is on screen while each sentence is spoken) and lists order problems. It never
    /// blocks generation: a failed review (other than cancellation) means "no problems found".
    #[allow(clippy::too_many_arguments)]
    async fn review_animation(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        resolved: &ResolvedProfile,
        section_title: &str,
        slides: &[Value],
        req: LlmRequest,
        cancel: CancellationToken,
    ) -> LnaResult<((Vec<Issue>, Vec<Issue>), Usage)> {
        let schema = animation_review_schema();
        let res = self
            .run_structured_with_thought_ex(
                artifact_id,
                notebook_id,
                &format!("动画审查：{section_title}"),
                resolved,
                &req,
                "animation_review",
                &schema,
                1,
                None,
                cancel,
            )
            .await;
        match res {
            Ok((out, u)) => {
                let (anim, narr) = animation_review_issues(&out, slides);
                let found = anim.len() + narr.len();
                self.append_thought(
                    artifact_id,
                    notebook_id,
                    "stage",
                    &format!("动画审查：{section_title}"),
                    &if found == 0 {
                        "独立审查未发现先后关系问题。".to_string()
                    } else {
                        format!(
                            "独立审查报告了 {found} 个问题（{} 个交回动画重编，{} 个交回讲解作者改旁白）。",
                            anim.len(),
                            narr.len()
                        )
                    },
                );
                Ok(((anim, narr), u))
            }
            Err(e) if e.code == ErrorCode::Cancelled => Err(e),
            Err(e) => {
                tracing::warn!("animation review failed: {}", e.message);
                Ok(((Vec::new(), Vec::new()), Usage::default()))
            }
        }
    }

    /// One fresh-context model call that animates a section: it sees only the finished slides
    /// (never how they were written) and says per sentence what appears and where the laser
    /// points; the plan replaces whatever animation the slides had. `hints` are problems found
    /// in an earlier animation of the same slides. A failed call (other than cancellation)
    /// leaves the slides static instead of blocking the lecture.
    #[allow(clippy::too_many_arguments)]
    async fn animate_section(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        resolved: &ResolvedProfile,
        section_title: &str,
        slides: &mut [Value],
        hints: &[Issue],
        max_out: u32,
        reasoning: &ReasoningOptions,
        cancel: CancellationToken,
    ) -> LnaResult<Usage> {
        // a repair is anchored: only the slides with problems are redone, each starting from its
        // current animation, so a fixed problem cannot come back and a good slide is never lost
        let repair = !hints.is_empty();
        let mut remaining: Vec<usize> = if repair {
            let mut v: Vec<usize> = hints.iter().map(|h| h.slide + 1).collect();
            v.sort_unstable();
            v.dedup();
            v
        } else {
            animation_author_input(slides)
                .get("slides")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.get("slide").and_then(Value::as_u64).map(|n| n as usize))
                .collect()
        };
        let mut usage = Usage::default();
        let mut log: Vec<String> = Vec::new();
        let mut failures = 0usize;
        // a first plan must cover every narrated slide and every sentence: what the model left
        // out is asked for again (only that), as long as each call gets something done
        loop {
            let mut full = animation_author_input(slides);
            if let Some(list) = full.get_mut("slides").and_then(Value::as_array_mut) {
                list.retain(|v| {
                    v.get("slide")
                        .and_then(Value::as_u64)
                        .is_some_and(|n| remaining.contains(&(n as usize)))
                });
                if repair {
                    for v in list.iter_mut() {
                        let idx = v.get("slide").and_then(Value::as_u64).unwrap_or(1) as usize - 1;
                        if let Some(sl) = slides.get(idx) {
                            v["current_animation"] = animation_plan_of(sl);
                        }
                    }
                }
                if list.is_empty() {
                    break;
                }
            }
            let mut input = full.to_string();
            if repair {
                input.push_str(&format!(
                    "\n\nThe current animation of the slides above (current_animation) has these problems:\n{}\n\nReturn the corrected plan for exactly these slides: every sentence of each, all of current_animation with only the fixes applied (a sentence you leave out keeps what it has now). Change only what is needed to fix the listed problems.",
                    issues_text(hints, slides)
                ));
            } else if !log.is_empty() {
                input.push_str(
                    "\n\nThese slides were missing from your previous plan or it skipped some of their sentences. Give the complete plan for every sentence of exactly these slides.",
                );
            }
            let req = LlmRequest::simple(ANIMATION_AUTHOR_SYSTEM.to_string(), input, max_out)
                .with_reasoning(reasoning.clone());
            let schema = animation_author_schema();
            let res = self
                .run_structured_with_thought_ex(
                    artifact_id,
                    notebook_id,
                    &format!("编排动画：{section_title}"),
                    resolved,
                    &req,
                    "animation_plan",
                    &schema,
                    1,
                    None,
                    cancel.clone(),
                )
                .await;
            let (mut plan, u) = match res {
                Ok(x) => x,
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    // a failed call (unparseable output, a provider error) is tried once more:
                    // giving up leaves the whole section without animation
                    tracing::warn!("animation authoring failed: {}", e.message);
                    failures += 1;
                    if failures < 2 {
                        continue;
                    }
                    break;
                }
            };
            add_usage(&mut usage, &u);
            if let Some(list) = plan.get_mut("slides").and_then(Value::as_array_mut) {
                list.retain(|v| {
                    v.get("slide")
                        .and_then(Value::as_u64)
                        .is_some_and(|n| remaining.contains(&(n as usize)))
                });
            }
            let complete: Vec<usize> = plan_complete_slides(slides, &plan);
            if repair {
                plan = merge_animation_plan(slides, &plan);
            }
            log.extend(apply_animation(slides, &plan));
            if repair {
                break;
            }
            let before = remaining.len();
            remaining.retain(|n| !complete.contains(n));
            if remaining.is_empty() || remaining.len() == before {
                break;
            }
        }
        self.append_thought(
            artifact_id,
            notebook_id,
            "stage",
            &format!("编排动画：{section_title}"),
            &log.join("\n"),
        );
        Ok(usage)
    }

    /// Repair rounds per section: the "lecture.limits" setting (`repair_rounds`, saved from
    /// Settings) when it holds a whole number, otherwise the built-in default.
    fn lecture_repair_rounds_setting(&self) -> usize {
        lna_storage::get_setting_json(&self.db, LECTURE_LIMITS_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.get("repair_rounds").and_then(Value::as_u64))
            .map(|n| n.min(MAX_LECTURE_REPAIR_ROUNDS as u64) as usize)
            .unwrap_or_else(lecture_repair_rounds)
    }

    /// Mechanical clean-up of a freshly written/repaired section: animation consistency and
    /// the fixes no model round trip is needed for. Logged as an automatic stage (it never
    /// counts as a model failure).
    fn tidy_section(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        section_title: &str,
        slides: &mut [Value],
        mode: &str,
    ) {
        let mut lines: Vec<String> = Vec::new();
        for (i, s) in slides.iter_mut().enumerate() {
            for l in crate::lecture_anim::normalize_animation(s) {
                lines.push(format!("第 {} 页：{l}", i + 1));
            }
        }
        lines.extend(apply_mechanical_fixes(slides, mode));
        if !lines.is_empty() {
            self.append_thought(
                artifact_id,
                notebook_id,
                "stage",
                &format!("整理动画提示：{section_title}"),
                &lines.join("\n"),
            );
        }
    }

    pub(crate) fn lecture_kg_hint(&self, art: &StudioArtifact) -> String {
        let mode = art
            .config
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("general");
        if mode != "review" {
            return String::new();
        }
        let Ok(sum) = self.compute_kg_progress_summary(&art.notebook_id, None) else {
            return String::new();
        };
        if sum.get("has_knowledge_graph").and_then(Value::as_bool) != Some(true) {
            return String::new();
        }
        let list = |k: &str| -> Vec<String> {
            sum.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .take(30)
                        .collect()
                })
                .unwrap_or_default()
        };
        let (weak, unstarted, mastered) = (
            list("weak_concepts"),
            list("unstarted_concepts"),
            list("mastered_concepts"),
        );
        format!(
            "Learner knowledge-graph status — weak (prioritise): {}; not yet studied: {}; mastered (pass quickly): {}.",
            if weak.is_empty() { "(none)".into() } else { weak.join(", ") },
            if unstarted.is_empty() { "(none)".into() } else { unstarted.join(", ") },
            if mastered.is_empty() { "(none)".into() } else { mastered.join(", ") },
        )
    }

    /// Lecture generation: outline pass, then one authoring pass per section (checkpointed).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn generate_lecture_multipass(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        evidence_txt: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage)> {
        let mode = art
            .config
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("general")
            .to_string();
        let detail = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string();
        let mut usage_sum = Usage::default();
        let target_ids = Self::generation_source_ids(art);
        let mut full_text = if self.sources_fit_directly(&target_ids, resolved)? {
            Some(evidence_txt)
        } else {
            None
        };
        let kg_hint = self.lecture_kg_hint(art);

        let saved = self
            .get_checkpoint(artifact_id, "lecture_passes")?
            .filter(|v| {
                v.get("sources_fingerprint").and_then(Value::as_str)
                    == Some(art.sources_fingerprint.as_str())
            });
        let (outline, mut done) = match saved {
            Some(v) => {
                let done: BTreeMap<String, Value> = v
                    .get("done")
                    .and_then(Value::as_object)
                    .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                if done.is_empty() {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        "讲解大纲已确定",
                        "大纲已在查阅来源时一并确定（或从检查点恢复），直接开始撰写小节。",
                    );
                } else {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        "复用已完成的讲解小节",
                        &format!("已从检查点恢复大纲与 {} 个已完成的小节。", done.len()),
                    );
                }
                (v.get("outline").cloned().unwrap_or_else(|| json!({})), done)
            }
            None => {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "规划讲解大纲",
                    "第 1 步：确定讲解范围、学习目标与小节；之后逐个小节查阅原文，撰写幻灯片、讲稿与激光/动画提示。",
                );
                let images = image_parts(&self.db.paths().root, art, resolved);
                let req = with_images(
                    LlmRequest::simple(
                        format!(
                            "You are the Local Notebook Studio lecture planner producing structured JSON for lecture outline. {}\nIf an approved outline is given, follow it.",
                            lecture_plan_rules(art)
                        ),
                        format!(
                            "Parameters: {}\nApproved outline: {}\n{}\n\nSources:\n{}",
                            config_for_prompt(&art.config, !images.is_empty()),
                            art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                            kg_hint,
                            evidence_txt
                        ),
                        resolved.hard_output_cap(8_192),
                    )
                    .with_reasoning(reasoning.clone()),
                    &images,
                );
                let (outline, u) = {
                    self.run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        "讲解大纲规划思考",
                        resolved,
                        &req,
                        "lecture_outline",
                        &outline_schema(),
                        max_repairs,
                        cancel.clone(),
                    )
                    .await?
                };
                add_usage(&mut usage_sum, &u);
                if arr_len(&outline, "sections") == 0 {
                    return Err(LnaError::new(
                        ErrorCode::OutputInvalid,
                        "Lecture outline has no sections",
                    ));
                }
                self.save_checkpoint(artifact_id, "lecture_passes", json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": {} }))?;
                self.emit_preview(
                    artifact_id,
                    &art.notebook_id,
                    "lecture_passes",
                    &json!({ "outline": outline, "done": {} }),
                );
                (outline, BTreeMap::new())
            }
        };

        let title = {
            let t = sv(&outline, "title");
            if t.is_empty() {
                art.title.clone()
            } else {
                t
            }
        };
        let objectives: Vec<String> = outline
            .get("objectives")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let sections: Vec<Value> = outline
            .get("sections")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let outline_txt = sections
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {} — {}", i + 1, sv(s, "title"), sv(s, "focus")))
            .collect::<Vec<_>>()
            .join("\n");
        let total = sections.len();
        // A small scope (the user's own material, or a lecture of a few sections) was read in
        // full by the collecting reader: every section reuses that evidence instead of reading
        // the same passages again. Whole books and multi-chapter lectures still read per section.
        if full_text.is_none()
            && !evidence_txt.trim().is_empty()
            && (has_user_material(&art.config) || total <= SMALL_LECTURE_SECTIONS)
            && evidence_txt.len() <= section_prompt_room(resolved) * 3
        {
            full_text = Some(evidence_txt);
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "沿用已收集的原文",
                "范围较小，各小节直接使用查阅来源时已收集的原文，不再重复查阅。",
            );
        }
        // Orchestrator/executor: the outline above is the plan; every pending section is an
        // independent executor (own evidence reading + authoring) and up to the profile's
        // parallel-request limit of them run at once. Only this task merges and checkpoints.
        let pending: Vec<(usize, Value)> = sections
            .iter()
            .cloned()
            .enumerate()
            .filter(|(i, _)| !done.contains_key(&format!("s{}", i + 1)))
            .collect();
        let limit = self.parallelism(profile).min(pending.len().max(1));
        if pending.len() > 1 {
            let detail = if profile.max_parallel == 0 {
                "模型配置的并行请求数不限".to_string()
            } else {
                format!(
                    "取小节数与模型配置中并行请求数上限 {} 中较小的一个",
                    self.parallelism(profile)
                )
            };
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "并行撰写小节",
                &format!(
                    "共 {} 个小节待撰写，本次同时并行 {limit} 个（{detail}）。",
                    pending.len(),
                ),
            );
        }
        let (title_ref, objectives_ref, outline_ref, kg_ref) =
            (&title, &objectives, &outline_txt, &kg_hint);
        lna_llm::run_parallel_unordered(
            pending,
            limit,
            &cancel,
            |_, (i, sec)| {
                let reasoning_c = reasoning.clone();
                let cancel_c = cancel.clone();
                async move {
                    let sid = format!("s{}", i + 1);
                    let stitle = sv(&sec, "title");
                    let focus = sv(&sec, "focus");
                    let visual_need = match sv(&sec, "visual_need").as_str() {
                        v @ ("required" | "suggested") => v.to_string(),
                        _ => "none".to_string(),
                    };
                    let visual = sv(&sec, "visual");
                    let visual_note = match visual_need.as_str() {
                        "required" => format!("\nVISUAL (required for this section — it is obviously geometric or physical, so at least one slide must carry a figure_request with step-by-step builds): {visual}"),
                        "suggested" => format!("\nVISUAL (suggested — mainly a worked derivation: reveal the steps one at a time and highlight the current one; add a figure only if it truly clarifies): {visual}"),
                        _ => String::new(),
                    };
                    let res: LnaResult<(Vec<Value>, Usage)> = async {
                        if cancel_c.is_cancelled() {
                            return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                        }
                        self.append_thought(
                            artifact_id,
                            &art.notebook_id,
                            "stage",
                            &format!("撰写小节 {}/{}：{}", i + 1, total, stitle),
                            "正在查阅该小节对应的原文，先规划逐页骨架再按批撰写幻灯片、讲稿与激光/分步动画提示…",
                        );
                        let evidence = self
                            .section_evidence(
                                art,
                                profile,
                                resolved,
                                artifact_id,
                                &format!("{stitle} — {focus}"),
                                full_text,
                                cancel_c.clone(),
                            )
                            .await?;
                        let sc = SectionCtx {
                            lecture_title: title_ref,
                            objectives: objectives_ref,
                            outline_txt: outline_ref,
                            section_title: &stitle,
                            section_focus: &focus,
                            visual_need: &visual_need,
                            position: format!(
                                "section {} of {}{}",
                                i + 1,
                                total,
                                if i == 0 {
                                    "; open the lecture with a title slide"
                                } else if i + 1 == total {
                                    "; close the lecture with a summary slide"
                                } else {
                                    ""
                                }
                            ),
                            extra: format!(
                                "Sections are written in parallel by separate authors: cover only this section's focus and leave the topics of the other outline entries to them.
{kg_ref}{visual_note}"
                            ),
                            checkpoint_key: Some(format!("lecture_sec_{sid}")),
                            batch_by_page: true,
                        };
                        self.author_section(
                            art,
                            profile,
                            resolved,
                            artifact_id,
                            &sc,
                            &evidence,
                            reasoning_c,
                            max_repairs,
                            cancel_c.clone(),
                        )
                        .await
                    }
                    .await;
                    (sid, stitle, res)
                }
            },
            |_, (sid, stitle, res)| {
                match res {
                    Ok((mut slides, u)) => {
                        add_usage(&mut usage_sum, &u);
                        finalize_slides(&mut slides, &sid, &format!("{sid}-"));
                        done.insert(sid.clone(), Value::Array(slides));
                    }
                    Err(e) if e.code == ErrorCode::Cancelled || cancel.is_cancelled() => {
                        return Err(e)
                    }
                    Err(e) => {
                        // a failed section is reported; finished ones stay in the checkpoint
                        self.append_thought(
                            artifact_id,
                            &art.notebook_id,
                            "stage",
                            &format!("小节生成失败：{stitle}"),
                            &truncate_str(&e.message, 300),
                        );
                        self.save_checkpoint(
                            artifact_id,
                            "lecture_passes",
                            json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": done }),
                        )?;
                        return Err(e);
                    }
                }
                self.save_checkpoint(
                    artifact_id,
                    "lecture_passes",
                    json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": done }),
                )?;
                self.emit_preview(
                    artifact_id,
                    &art.notebook_id,
                    "lecture_passes",
                    &json!({ "outline": outline, "done": done }),
                );
                Ok(())
            },
        )
        .await?;

        let mut slides: Vec<Value> = Vec::new();
        for i in 0..total {
            if let Some(v) = done.get(&format!("s{}", i + 1)).and_then(Value::as_array) {
                slides.extend(v.iter().cloned());
            }
        }
        let theme = match sv(&outline, "theme") {
            t if t.is_empty() => "academic_sage".to_string(),
            t => t,
        };
        let payload = json!({
            "title": title,
            "subtitle": sv(&outline, "subtitle"),
            "theme": theme,
            "mode": mode,
            "detail": detail,
            "scope": { "query": art.config.get("topic").and_then(Value::as_str).unwrap_or(""), "summary": sv(&outline, "scope") },
            "objectives": objectives,
            "sections": sections.iter().enumerate().map(|(i, s)| json!({ "id": format!("s{}", i + 1), "title": sv(s, "title") })).collect::<Vec<_>>(),
            "slides": slides,
            "adaptations": [],
            "estimated_minutes": estimate_minutes(&slides),
        });
        Ok((payload, usage_sum))
    }

    // -----------------------------------------------------------------------------------------
    // interactive: ask / adapt / undo

    fn lecture_payload(&self, artifact_id: &str) -> LnaResult<(StudioArtifact, Value)> {
        let art = self.get(artifact_id)?;
        if art.artifact_type != ArtifactType::Lecture {
            return Err(LnaError::invalid("Not a lecture"));
        }
        let payload = art
            .payload
            .clone()
            .ok_or_else(|| LnaError::new(ErrorCode::Conflict, "Lecture has no content yet"))?;
        Ok((art, payload))
    }

    fn slide_text(slide: &Value) -> String {
        let mut s = format!("# {}\n", sv(slide, "title"));
        let sub = sv(slide, "subtitle");
        if !sub.is_empty() {
            s.push_str(&format!("{sub}\n"));
        }
        for b in slide
            .get("bullets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            s.push_str(&format!("- {}\n", b.as_str().unwrap_or("")));
        }
        for k in ["cards", "steps", "metrics", "columns"] {
            if let Some(v) = slide.get(k).filter(|v| v.is_array()) {
                s.push_str(&format!("{k}: {}\n", v));
            }
        }
        for k in ["formula", "callout"] {
            let v = sv(slide, k);
            if !v.is_empty() {
                s.push_str(&format!("{k}: {v}\n"));
            }
        }
        if let Some(t) = slide.get("table").filter(|t| t.is_object()) {
            s.push_str(&format!("table: {}\n", t));
        }
        if let Some(f) = slide.pointer("/figure/spec") {
            s.push_str(&format!("figure: {}\n", truncate_str(&f.to_string(), 800)));
        }
        s
    }

    /// Answers an interruption question in the context of the current slide and judges whether
    /// the remaining lecture should be adapted.
    pub async fn lecture_ask(
        &self,
        artifact_id: &str,
        slide_id: &str,
        sentence_id: Option<&str>,
        question: &str,
        history: &[Value],
        images: &[Attachment],
    ) -> LnaResult<Value> {
        let (art, payload) = self.lecture_payload(artifact_id)?;
        let slides = payload
            .get("slides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let idx = slides
            .iter()
            .position(|s| sv(s, "id") == slide_id)
            .unwrap_or(0);
        let slide = slides.get(idx).cloned().unwrap_or_else(|| json!({}));
        let mut spoken = String::new();
        for n in slide
            .get("narration")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            spoken.push_str(&sv(n, "text"));
            if Some(sv(n, "id").as_str()) == sentence_id {
                break;
            }
        }
        let covered: Vec<String> = slides[..idx].iter().map(|s| sv(s, "title")).collect();
        let upcoming: Vec<String> = slides
            .iter()
            .skip(idx + 1)
            .take(8)
            .map(|s| sv(s, "title"))
            .collect();
        let (_profile, resolved) = self.resolve_active_profile()?;
        let target_ids = Self::generation_source_ids(&art);
        let queries = vec![
            question.to_string(),
            format!("{} {}", sv(&slide, "title"), question),
        ];
        let evidence = match self
            .retrieval
            .search(&art.notebook_id, &queries, &target_ids, 6_000, 10)
            .await
        {
            Ok(res) => res
                .evidences
                .iter()
                .enumerate()
                .map(|(i, e)| format!("[{}] ({}) {}", i + 1, e.source_title, e.content))
                .collect::<Vec<_>>()
                .join("\n\n"),
            Err(_) => String::new(),
        };
        let hist: String = history
            .iter()
            .rev()
            .take(6)
            .rev()
            .map(|h| format!("{}: {}", sv(h, "role"), sv(h, "text")))
            .collect::<Vec<_>>()
            .join("\n");
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["answer", "speak", "adapt"],
            "properties": {
                "answer": { "type": "string" },
                "speak": { "type": "string" },
                "adapt": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind", "reason"],
                    "properties": {
                        "kind": { "type": "string", "enum": ["none", "extend", "slower", "simpler", "skip_known", "more_examples"] },
                        "reason": { "type": "string" }
                    }
                }
            }
        });
        let files = lna_ingest::context_files::context_items(images);
        let evidence = if files.is_empty() {
            evidence
        } else {
            let ctx = lna_ingest::context_files::ToolCtx {
                db: &self.db,
                secrets: &self.secrets,
                llm: &self.llm,
                notebook_id: &art.notebook_id,
                profile: &resolved,
                atts: &files,
            };
            let b = lna_ingest::context_files::brief(
                &ctx,
                &format!("回答学生在讲解中提出的问题：{question}"),
                4,
                CancellationToken::new(),
            )
            .await?;
            format!("{evidence}\n\nFrom the learner's attached files:\n{b}")
        };
        let parts = attachments::user_parts(
            &self.db,
            &self.secrets,
            &self.llm,
            &art.notebook_id,
            &resolved,
            format!(
                "Lecture: {} (mode {})\nCovered slides: {}\nCurrent slide:\n{}\nSpoken so far on this slide: {}\nUpcoming slides: {}\nEarlier Q&A:\n{}\n\nQuestion: {}\n\nSources:\n{}",
                sv(&payload, "title"),
                sv(&payload, "mode"),
                covered.join(" | "),
                Self::slide_text(&slide),
                spoken,
                upcoming.join(" | "),
                if hist.is_empty() { "(none)".into() } else { hist },
                question,
                evidence
            ),
            images,
        )
        .await;
        let req = LlmRequest::from_parts(
            "You are the Local Notebook Studio lecture teacher of a narrated lecture and the learner just paused you with a question. Answer it directly and accurately from the sources and the lecture context, at the learner's level, in the learner's language. \"answer\": Markdown (LaTeX $...$ allowed). \"speak\": the same answer as 1-4 plain spoken sentences for text-to-speech (no LaTeX/markdown/symbols). Then judge from the question whether the REST of the lecture should change: \"extend\" if something already explained was unclear or a prerequisite is missing (insert 1-2 supplementary slides now); \"slower\" or \"simpler\" if the pace/level is too high; \"more_examples\" if the learner needs worked examples; \"skip_known\" if the learner is clearly ahead; \"none\" for an isolated detail question (the usual case). Give a short reason in the learner's language. Respond with structured JSON for lecture answer.",
            parts,
            resolved.bounded_output(3000),
        );
        let (out, usage) = self
            .llm
            .complete_structured(
                &resolved,
                &req,
                "lecture_answer",
                &schema,
                CancellationToken::new(),
            )
            .await?;
        self.record_usage(&art.notebook_id, &resolved.id, "studio_lecture_ask", Some(artifact_id), &usage);
        let kind = out
            .pointer("/adapt/kind")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string();
        Ok(json!({
            // shown without a TeX engine on every client
            "answer": crate::texfmt::tex_to_unicode(&sv(&out, "answer")),
            "speak": sv(&out, "speak"),
            "adapt": { "needed": kind != "none", "kind": kind, "reason": out.pointer("/adapt/reason").and_then(Value::as_str).unwrap_or("") },
        }))
    }

    /// Adapts the lecture after an interruption. `extend` inserts 1-2 supplementary slides after
    /// the current slide; other kinds regenerate every slide after it. Recorded for undo.
    pub async fn lecture_adapt(
        &self,
        artifact_id: &str,
        slide_id: &str,
        kind: &str,
        reason: &str,
        question: &str,
    ) -> LnaResult<Value> {
        let (art, payload) = self.lecture_payload(artifact_id)?;
        let slides = payload
            .get("slides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let idx = slides
            .iter()
            .position(|s| sv(s, "id") == slide_id)
            .ok_or_else(|| LnaError::new(ErrorCode::NotFound, "Slide not found"))?;
        let (profile, resolved) = self.resolve_active_profile()?;
        let reasoning = ReasoningOptions {
            show_summary: resolved.show_thinking || resolved.capabilities.reasoning_summary,
            effort: resolved.effort.clone(),
        };
        let cancel = CancellationToken::new();
        let title = sv(&payload, "title");
        let objectives: Vec<String> = payload
            .get("objectives")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let sections: Vec<Value> = payload
            .get("sections")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let outline_txt = sections
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, sv(s, "title")))
            .collect::<Vec<_>>()
            .join("\n");
        let target_ids = Self::generation_source_ids(&art);
        let full = if self.sources_fit_directly(&target_ids, &resolved)? {
            Some(self.load_full_source_text(&target_ids)?.1)
        } else {
            None
        };
        let n_adapt = payload
            .get("adaptations")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0)
            + 1;
        let aid = format!("a{}", n_adapt);
        let cur = &slides[idx];
        let cur_section = sv(cur, "section");
        let covered: Vec<String> = slides[..=idx].iter().map(|s| sv(s, "title")).collect();
        let direction = match kind {
            "slower" => "Slow down: smaller steps, more explanation per idea, more slides, recap before moving on.",
            "simpler" => "Simplify: lower the level, use intuition and plain words first, avoid unnecessary formalism.",
            "more_examples" => "Add worked examples for each remaining idea before moving on.",
            "skip_known" => "The learner is ahead: compress what they already know, keep only what is new or hard.",
            _ => "Insert supplementary explanation.",
        };
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            "根据提问调整后续讲解",
            &format!("{kind}：{reason}"),
        );

        let mut usage = Usage::default();
        let (new_slides, removed, from) = if kind == "extend" {
            let focus = format!("{} — {}", sv(cur, "title"), question);
            let evidence = self
                .section_evidence(
                    &art,
                    &profile,
                    &resolved,
                    artifact_id,
                    &focus,
                    full.as_deref(),
                    cancel.clone(),
                )
                .await?;
            let sc = SectionCtx {
                lecture_title: &title,
                objectives: &objectives,
                outline_txt: &outline_txt,
                section_title: &format!("补充讲解：{}", sv(cur, "title")),
                section_focus: question,
                visual_need: "none",
                position: "a supplementary insertion of 1-2 slides right after the current slide".into(),
                extra: format!(
                    "The learner interrupted at slide \"{}\" asking: {question}\nReason for this insertion: {reason}\nCurrent slide:\n{}\nProduce ONLY 1-2 supplementary slides that resolve this difficulty; do not repeat the current slide and do not continue the rest of the lecture.",
                    sv(cur, "title"),
                    Self::slide_text(cur)
                ),
                checkpoint_key: None,
                batch_by_page: false,
            };
            let (mut add, u) = self
                .author_section(
                    &art,
                    &profile,
                    &resolved,
                    artifact_id,
                    &sc,
                    &evidence,
                    reasoning.clone(),
                    2,
                    cancel.clone(),
                )
                .await?;
            add_usage(&mut usage, &u);
            add.truncate(3);
            finalize_slides(&mut add, &cur_section, &format!("{aid}-"));
            (add, Vec::new(), idx + 1)
        } else {
            // regenerate every remaining slide, section by section, with the adaptation applied
            let rest: Vec<Value> = slides[idx + 1..].to_vec();
            let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
            for s in &rest {
                let sec = sv(s, "section");
                match groups.last_mut() {
                    Some((g, v)) if *g == sec => v.push(s.clone()),
                    _ => groups.push((sec, vec![s.clone()])),
                }
            }
            let total_groups = groups.len();
            let limit = self.parallelism(&profile).min(total_groups.max(1));
            let (art_ref, prof_ref, res_ref, full_ref) =
                (&art, &profile, &resolved, full.as_deref());
            let (title_ref, obj_ref, outline_ref, sec_ref, cov_ref, aid_ref) =
                (&title, &objectives, &outline_txt, &sections, &covered, &aid);
            let parts = lna_llm::run_parallel_ordered(groups, limit, &cancel, |gi, (sec, old)| {
                let reasoning_c = reasoning.clone();
                let cancel_c = cancel.clone();
                async move {
                    let stitle = sec_ref
                        .iter()
                        .find(|s| sv(s, "id") == sec)
                        .map(|s| sv(s, "title"))
                        .unwrap_or_else(|| sec.clone());
                    let planned: Vec<String> = old.iter().map(|s| sv(s, "title")).collect();
                    let focus = format!("{stitle}: {}", planned.join("; "));
                    let evidence = self
                        .section_evidence(
                            art_ref,
                            prof_ref,
                            res_ref,
                            artifact_id,
                            &focus,
                            full_ref,
                            cancel_c.clone(),
                        )
                        .await?;
                    let sc = SectionCtx {
                        lecture_title: title_ref,
                        objectives: obj_ref,
                        outline_txt: outline_ref,
                        section_title: &stitle,
                        section_focus: &planned.join("; "),
                        visual_need: "none",
                        position: format!(
                            "re-authoring remaining part {} of {} after an interruption",
                            gi + 1,
                            total_groups
                        ),
                        extra: format!(
                            "ADAPTATION: {direction}\nThe learner asked: {question}\nWhy: {reason}\nAlready covered (do not repeat): {}\nPreviously planned slides for this part (cover the same content, adapted): {}",
                            cov_ref.join(" | "),
                            planned.join(" | ")
                        ),
                        checkpoint_key: None,
                        batch_by_page: true,
                    };
                    let (mut part, u) = self
                        .author_section(
                            art_ref,
                            prof_ref,
                            res_ref,
                            artifact_id,
                            &sc,
                            &evidence,
                            reasoning_c,
                            2,
                            cancel_c,
                        )
                        .await?;
                    finalize_slides(&mut part, &sec, &format!("{aid_ref}{}-", gi + 1));
                    Ok((part, u))
                }
            })
            .await?;
            let mut out = Vec::new();
            for (part, u) in parts {
                add_usage(&mut usage, &u);
                out.extend(part);
            }
            (out, rest, idx + 1)
        };
        self.record_usage(&art.notebook_id, &profile.id, "studio_lecture_adapt", Some(artifact_id), &usage);

        // apply to the latest stored payload
        let (_, mut latest) = self.lecture_payload(artifact_id)?;
        let arr = latest
            .get_mut("slides")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LnaError::internal("payload without slides"))?;
        let at = arr
            .iter()
            .position(|s| sv(s, "id") == slide_id)
            .map(|i| i + 1)
            .unwrap_or(from);
        if !removed.is_empty() {
            let rm_ids: HashSet<String> = removed.iter().map(|s| sv(s, "id")).collect();
            arr.retain(|s| !rm_ids.contains(&sv(s, "id")));
        }
        let inserted_ids: Vec<String> = new_slides.iter().map(|s| sv(s, "id")).collect();
        for (k, s) in new_slides.iter().enumerate() {
            let pos = (at + k).min(arr.len());
            arr.insert(pos, s.clone());
        }
        let record = json!({
            "id": aid,
            "kind": kind,
            "reason": reason,
            "question": question,
            "at_slide": slide_id,
            "inserted": inserted_ids,
            "removed": removed,
            "created_at": now_ms(),
        });
        if let Some(a) = latest.get_mut("adaptations").and_then(Value::as_array_mut) {
            a.push(record.clone());
        } else {
            latest["adaptations"] = json!([record.clone()]);
        }
        let all: Vec<Value> = latest
            .get("slides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        latest["estimated_minutes"] = json!(estimate_minutes(&all));
        self.store_lecture_payload(artifact_id, &latest)?;
        let mut summary = record.clone();
        if let Some(o) = summary.as_object_mut() {
            o.insert(
                "removed_titles".into(),
                json!(removed.iter().map(|s| sv(s, "title")).collect::<Vec<_>>()),
            );
            o.insert(
                "inserted_titles".into(),
                json!(new_slides
                    .iter()
                    .map(|s| sv(s, "title"))
                    .collect::<Vec<_>>()),
            );
            o.remove("removed");
        }
        self.emit_status_event(artifact_id, "READY", "READY", 1.0, None, None);
        Ok(json!({ "adaptation": summary, "artifact": self.get(artifact_id)? }))
    }

    /// Undoes an adaptation (only the most recent one still applied, so the slide order stays
    /// consistent); `all` restores the original lecture.
    pub fn lecture_undo(
        &self,
        artifact_id: &str,
        adaptation_id: Option<&str>,
        all: bool,
    ) -> LnaResult<Value> {
        let (_, mut payload) = self.lecture_payload(artifact_id)?;
        let mut undone = Vec::new();
        while let Some(last) = payload
            .get("adaptations")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .cloned()
        {
            let id = sv(&last, "id");
            if !all && adaptation_id.is_some_and(|want| want != id) {
                return Err(LnaError::invalid(
                    "Only the most recent adaptation can be undone",
                ));
            }
            let inserted: HashSet<String> = last
                .get("inserted")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            let removed: Vec<Value> = last
                .get("removed")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let at = sv(&last, "at_slide");
            if let Some(arr) = payload.get_mut("slides").and_then(Value::as_array_mut) {
                arr.retain(|s| !inserted.contains(&sv(s, "id")));
                let pos = arr
                    .iter()
                    .position(|s| sv(s, "id") == at)
                    .map(|i| i + 1)
                    .unwrap_or(arr.len());
                for (k, s) in removed.into_iter().enumerate() {
                    arr.insert((pos + k).min(arr.len()), s);
                }
            }
            if let Some(a) = payload.get_mut("adaptations").and_then(Value::as_array_mut) {
                a.pop();
            }
            undone.push(id);
            if !all {
                break;
            }
        }
        let all_slides: Vec<Value> = payload
            .get("slides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        payload["estimated_minutes"] = json!(estimate_minutes(&all_slides));
        self.store_lecture_payload(artifact_id, &payload)?;
        self.emit_status_event(artifact_id, "READY", "READY", 1.0, None, None);
        Ok(json!({ "undone": undone, "artifact": self.get(artifact_id)? }))
    }

    fn store_lecture_payload(&self, artifact_id: &str, payload: &Value) -> LnaResult<()> {
        let out_dir = self.db.paths().artifacts_dir.join(artifact_id);
        if let Err(e) = render_figure_assets(payload, &out_dir) {
            tracing::warn!("lecture figure assets: {}", e.message);
        }
        let s = payload.to_string();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET payload_json=?1, updated_at=?2 WHERE id=?3",
                params![s, now_ms(), artifact_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slide() -> Value {
        json!({
            "layout": "bullets", "title": "逆矩阵", "bullets": ["定义", "求法"],
            "builds": [{"step": 1, "targets": ["b1"], "effect": "fade"}, {"step": 2, "targets": ["b2"], "effect": "fade"}],
            "narration": [
                {"text": "先看定义。", "cues": [{"type": "step", "n": 1}, {"type": "laser", "target": "b1", "shape": "ellipse", "scale": 1.2, "speed": 0.8, "duration_ms": 2000}]},
                {"text": "再看求法。", "cues": [{"type": "step", "n": 2}]}
            ]
        })
    }

    #[test]
    fn valid_slide_has_no_issues() {
        assert!(check_lecture_content(&[slide()], "general").is_empty());
    }

    #[test]
    fn talk_about_the_reading_process_is_reported_but_the_book_may_be_named() {
        let mut s = slide();
        s["narration"][0]["text"] = json!("扫描版的识别噪声很大，下标要按标准的矢量分解去理解。");
        let issues = check_lecture_content(&[s], "general");
        assert!(issues
            .iter()
            .any(|i| i.msg.contains("reading/generation process")));
        let mut t = slide();
        t["title"] = json!("先定位：这一页在推导链中的位置");
        assert!(check_lecture_content(&[t], "general")
            .iter()
            .any(|i| i.msg.contains("reading/generation process")));
        let mut ok = slide();
        ok["narration"][0]["text"] = json!("课本上的例题用的正是这个结论。");
        assert!(check_lecture_content(&[ok], "general").is_empty());
    }

    #[test]
    fn reports_missing_narration() {
        let empty = json!({"layout": "title", "title": "封面", "narration": []});
        let issues = check_lecture_content(&[slide(), empty], "general");
        let msgs: Vec<&str> = issues.iter().map(|i| i.msg.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("no narration")), "{msgs:?}");
    }

    /// The model writes JSON fields in the order the schema lists them (constrained decoding),
    /// so the schema must keep its authored order: `layout` and `title` first, not alphabetical
    /// (which put `narration` before `title` and let long narration spill into the title).
    #[test]
    fn schemas_keep_their_authored_field_order() {
        let slide = lecture_slide_schema();
        let keys: Vec<&str> = slide["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(&keys[..2], ["layout", "title"]);
        let pos = |k: &str| keys.iter().position(|x| *x == k).unwrap();
        assert!(pos("bullets") < pos("narration"));
        // animation is not the author's business: no builds, no cues
        assert!(!keys.contains(&"builds"));
        assert!(slide["properties"]["narration"]["items"]["properties"]
            .get("cues")
            .is_none());
        let skel = section_page_skeleton_schema();
        let skel_keys: Vec<&str> = skel["properties"]["pages"]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(skel_keys.first(), Some(&"title"));
    }

    #[test]
    fn animation_review_input_and_issues() {
        let mut s = slide();
        s["figure"] = json!({
            "spec": {"elements": [{"id": "v", "type": "vector"}, {"id": "lbl_v", "type": "text", "text": "v", "for": "v"}]},
            "meta": {"parts": [{"id": "v"}, {"id": "lbl_v", "anchors": [["v"]]}]}
        });
        let input = animation_review_input(std::slice::from_ref(&s));
        let sl = &input["slides"][0];
        assert_eq!(sl["slide"], 1);
        assert_eq!(sl["figure_parts"][1]["hangs_on"], json!([["v"]]));
        assert_eq!(sl["figure_elements"][1]["for"], "v");
        assert_eq!(sl["narration"][1]["sentence"], 2);
        let schema = animation_review_schema();
        assert_eq!(schema["required"], json!(["problems"]));
        let ok = json!({"problems": [
            {"slide": 1, "sentence": 2, "element": "b2", "fix": "animation", "problem": "revealed early"},
            {"slide": 1, "sentence": 3, "element": "", "fix": "narration", "problem": "speaks about v before it exists"},
            {"slide": 9, "sentence": 1, "element": "x", "fix": "animation", "problem": "unknown slide"},
            {"slide": 1, "sentence": 0, "element": "", "fix": "animation", "problem": "  "}
        ]});
        let (anim, narr) = animation_review_issues(&ok, std::slice::from_ref(&s));
        assert_eq!(anim.len(), 1);
        assert_eq!(anim[0].slide, 0);
        assert!(anim[0].msg.contains("sentence 2") && anim[0].msg.contains("element b2"));
        assert_eq!(narr.len(), 1);
        assert!(narr[0].msg.contains("sentence 3") && narr[0].msg.contains("narration"));
        let (a2, n2) = animation_review_issues(&json!({"problems": []}), &[s]);
        assert!(a2.is_empty() && n2.is_empty());
    }

    #[test]
    fn animation_author_input_hides_animation_and_lists_targets() {
        let s = json!({
            "layout": "figure", "title": "T", "bullets": ["a", "b"],
            "builds": [{"step": 1, "targets": ["b1"]}],
            "narration": [{"text": "一", "cues": [{"type": "step", "n": 1}]}, {"text": "二"}],
            "figure": {"meta": {"parts": [{"id": "v"}]}, "spec": {"elements": [{"id": "v", "type": "vector"}]}}
        });
        let silent = json!({"layout": "figure", "title": "S", "narration": [], "display_only": {"reason": "r"}});
        let input = animation_author_input(&[silent, s]);
        let slides = input["slides"].as_array().unwrap();
        assert_eq!(slides.len(), 1, "slides without narration are not animated");
        let sl = &slides[0];
        assert_eq!(sl["slide"], 2);
        assert!(sl.get("builds").is_none());
        assert!(sl["narration"][0].get("cues").is_none());
        let ids = sl["valid_targets"].as_array().unwrap();
        assert!(ids.iter().any(|v| v == "b2") && ids.iter().any(|v| v == "figure:v"));
        assert_eq!(animation_author_schema()["required"], json!(["slides"]));
    }

    #[test]
    fn animation_plan_becomes_builds_and_cues() {
        let mut slides = vec![json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b"],
            "builds": [{"step": 7, "targets": ["b2"]}],
            "narration": [
                {"text": "一", "cues": [{"type": "laser", "target": "b2"}]},
                {"text": "二"}, {"text": "三"}
            ]
        })];
        let plan = json!({"slides": [
            {"slide": 1, "sentences": [
                {"sentence": 1, "reveal": ["b1"], "effect": "fade", "points": [{"target": "b1", "at": "一"}]},
                {"sentence": 2, "reveal": [], "points": []},
                {"sentence": 3, "reveal": ["b2", "b1"], "effect": "bogus", "points": [{"target": "b2"}, {"target": ""}]},
                {"sentence": 9, "reveal": ["b1"], "points": []}
            ]},
            {"slide": 4, "sentences": []}
        ]});
        let log = apply_animation(&mut slides, &plan);
        assert_eq!(log.len(), 1);
        let s = &slides[0];
        // each reveal is its own step with its step cue; a target is revealed only once
        assert_eq!(
            s["builds"],
            json!([
                {"step": 1, "targets": ["b1"], "effect": "fade"},
                {"step": 2, "targets": ["b2"], "effect": "fade"}
            ])
        );
        assert_eq!(
            s["narration"][0]["cues"][0],
            json!({"type": "step", "n": 1})
        );
        assert_eq!(s["narration"][0]["cues"][1]["target"], "b1");
        assert_eq!(s["narration"][0]["cues"][1]["at"], "一");
        assert_eq!(s["narration"][1]["cues"], json!([]));
        assert_eq!(
            s["narration"][2]["cues"][0],
            json!({"type": "step", "n": 2})
        );
        assert_eq!(s["narration"][2]["cues"].as_array().unwrap().len(), 2);
        // a plan read back from the slide is the plan that was written (repairs start from it)
        let back = animation_plan_of(&slides[0]);
        assert_eq!(back["sentences"][0]["reveal"], json!(["b1"]));
        assert_eq!(back["sentences"][0]["points"][0]["at"], "一");
        assert_eq!(back["sentences"][2]["reveal"], json!(["b2"]));
        assert_eq!(back["sentences"][1]["reveal"], json!([]));
        // the result survives the normalizer unchanged in meaning
        crate::lecture_anim::normalize_animation(&mut slides[0]);
        assert!(crate::lecture_anim::animation_violations(&slides[0]).is_empty());
        strip_animation(&mut slides);
        assert!(slides[0].get("builds").is_none());
        assert!(slides[0]["narration"][0].get("cues").is_none());
    }

    #[test]
    fn a_partial_repair_plan_keeps_the_rest_of_the_animation() {
        let mut slides = vec![json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b", "c"],
            "narration": [{"text": "一"}, {"text": "二"}, {"text": "三"}, {"text": "四"}]
        })];
        apply_animation(
            &mut slides,
            &json!({"slides": [{"slide": 1, "sentences": [
                {"sentence": 1, "reveal": ["b1"], "points": [{"target": "b1"}]},
                {"sentence": 2, "reveal": ["b2"], "points": [{"target": "b2"}]},
                {"sentence": 3, "reveal": ["b3"], "points": [{"target": "b3"}]},
                {"sentence": 4, "reveal": [], "points": []}
            ]}]}),
        );
        // the repair only moves b2 to sentence 4: it returns that sentence and the one it left
        let partial = json!({"slides": [{"slide": 1, "sentences": [
            {"sentence": 2, "reveal": [], "points": [{"target": "b2"}]},
            {"sentence": 4, "reveal": ["b2"], "points": []}
        ]}]});
        let merged = merge_animation_plan(&slides, &partial);
        apply_animation(&mut slides, &merged);
        let back = animation_plan_of(&slides[0]);
        assert_eq!(back["sentences"][0]["reveal"], json!(["b1"]));
        assert_eq!(back["sentences"][1]["reveal"], json!([]));
        assert_eq!(back["sentences"][2]["reveal"], json!(["b3"]));
        assert_eq!(back["sentences"][3]["reveal"], json!(["b2"]));
        assert_eq!(back["sentences"][2]["points"][0]["target"], "b3");
    }

    #[test]
    fn plan_targets_are_resolved_and_incomplete_plans_are_found() {
        let mut slides = vec![
            json!({"layout": "figure", "title": "T", "bullets": ["a"],
                "figure": {"meta": {"parts": [{"id": "vec_F"}]}, "spec": {"elements": [{"id": "vec_F", "type": "vector"}]}},
                "narration": [{"text": "一"}, {"text": "二"}]}),
            json!({"layout": "bullets", "title": "S", "bullets": ["x"], "narration": [{"text": "三"}]}),
        ];
        let plan = json!({"slides": [{"slide": 1, "sentences": [
            {"sentence": 1, "reveal": ["vec_f"], "points": [{"target": "vec_F"}, {"target": "B1"}]}
        ]}]});
        // slide 1 is missing its second sentence, slide 2 is missing altogether
        assert!(plan_complete_slides(&slides, &plan).is_empty());
        apply_animation(&mut slides, &plan);
        assert_eq!(slides[0]["builds"][0]["targets"], json!(["figure:vec_F"]));
        assert_eq!(
            slides[0]["narration"][0]["cues"][1]["target"],
            "figure:vec_F"
        );
        assert_eq!(slides[0]["narration"][0]["cues"][2]["target"], "b1");
        // an empty plan for slides that have content is not complete either
        let empty = json!({"slides": [
            {"slide": 1, "sentences": [{"sentence": 1, "reveal": [], "points": []}, {"sentence": 2, "reveal": [], "points": []}]},
            {"slide": 2, "sentences": [{"sentence": 1, "reveal": [], "points": []}]}
        ]});
        assert!(plan_complete_slides(&slides, &empty).is_empty());
        let full = json!({"slides": [
            {"slide": 1, "sentences": [{"sentence": 1, "reveal": ["b1"], "points": []}, {"sentence": 2, "reveal": [], "points": []}]},
            {"slide": 2, "sentences": [{"sentence": 1, "reveal": [], "points": [{"target": "b1"}]}]}
        ]});
        assert_eq!(plan_complete_slides(&slides, &full), vec![1, 2]);
    }

    #[test]
    fn display_only_rules() {
        let d = json!({"layout": "figure", "title": "图", "narration": [], "display_only": {"reason": "大图展示"}});
        assert!(check_lecture_content(std::slice::from_ref(&d), "general").is_empty());
        let issues = check_lecture_content(&[d.clone(), d], "general");
        assert!(issues.iter().any(|i| i.msg.contains("second display-only")));
    }

    /// Animation problems are never a reason to ask the model again: the content check does not
    /// report them and the deterministic normalisation settles them.
    #[test]
    fn animation_problems_are_not_content_problems() {
        let mut s = slide();
        s["narration"][1]["cues"] = json!([]);
        s["narration"][0]["cues"][1]["target"] = json!("b9");
        s["narration"][0]["cues"][1]["speed"] = json!(5);
        assert!(check_lecture_content(&[s.clone()], "general").is_empty());
        crate::lecture_anim::normalize_animation(&mut s);
        assert!(crate::lecture_anim::animation_violations(&s).is_empty());
    }

    #[test]
    fn mechanical_fixes_convert_tex_and_drop_incomplete_checkpoints() {
        let mut s = slide();
        s["narration"][1]["text"] = json!("所以 $A^{-1}$ 存在。");
        s["checkpoint"] = json!({"kind": "ask", "question": "为什么？"});
        let mut slides = vec![s];
        let log = apply_mechanical_fixes(&mut slides, "general");
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(!slides[0]["narration"][1]["text"]
            .as_str()
            .unwrap()
            .contains('$'));
        assert!(slides[0].get("checkpoint").is_none());
        let mut ok = slide();
        ok["checkpoint"] = json!({"kind": "ask", "question": "为什么？", "reason": "易混淆"});
        let mut slides = vec![ok];
        assert!(apply_mechanical_fixes(&mut slides, "general").is_empty());
        let mut bare = vec![json!({"layout": "title", "title": "封面", "narration": []})];
        assert_eq!(mark_unnarrated_display_only(&mut bare).len(), 1);
        assert!(check_lecture_content(&bare, "general").is_empty());
    }

    #[test]
    fn figure_parts_are_targets_and_assets_render_per_step() {
        let spec = json!({"kind": "plot", "x_range": [-2, 2], "elements": [{"id": "f", "type": "curve", "expr": "x^2"}, {"id": "V", "type": "point", "at": [0, 0]}]});
        let out = render_figure(&spec, &HashSet::new());
        let mut s = json!({
            "id": "s1-2", "layout": "figure", "title": "抛物线",
            "bullets": ["曲线 f：y = x²", "顶点 V 在原点"],
            "figure": {"spec": spec, "meta": out.meta_json()},
            "builds": [{"step": 1, "targets": ["figure:f"], "effect": "draw"}],
            "narration": [{"text": "画出曲线。", "cues": [{"type": "step", "n": 1}, {"type": "laser", "target": "figure:V", "shape": "circle", "speed": 1.0}]}]
        });
        assert!(slide_element_ids(&s).contains(&"figure:V".to_string()));
        assert!(check_lecture_content(&[s.clone()], "general").is_empty());
        let dir = std::env::temp_dir().join(format!("lna_fig_{}", std::process::id()));
        let n = render_figure_assets(&json!({"slides": [s.clone()]}), &dir).unwrap();
        assert_eq!(n, 2);
        assert!(
            dir.join("figures/s1-2.svg").is_file()
                && dir.join("figures/s1-2_0.png").is_file()
                && dir.join("figures/s1-2_1.png").is_file()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let mut alone = s.clone();
        alone["bullets"] = json!([]);
        let issues = check_lecture_content(&[alone], "general");
        assert!(
            issues
                .iter()
                .any(|i| i.msg.contains("no on-screen text beside")),
            "{issues:?}"
        );
        // a pointer at a part the figure does not have is cleaned, not sent back to the model
        s["narration"][0]["cues"][1]["target"] = json!("figure:Q");
        assert!(check_lecture_content(&[s.clone()], "general").is_empty());
        crate::lecture_anim::normalize_animation(&mut s);
        // the sentence still reveals the figure, so the pointer goes to what it reveals
        let cues = s["narration"][0]["cues"].as_array().unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1]["target"], "figure:f");
        assert!(crate::lecture_anim::animation_violations(&s).is_empty());
    }

    #[test]
    fn finalize_assigns_ids_notes_and_estimate() {
        let mut v = vec![
            slide(),
            json!({"layout": "summary", "title": "小结", "narration": [{"text": "今天我们学习了逆矩阵的定义和求法。"}]}),
        ];
        finalize_slides(&mut v, "s1", "s1-");
        assert_eq!(v[0]["id"], "s1-1");
        assert_eq!(v[1]["narration"][0]["id"], "s1-2.1");
        assert_eq!(v[1]["citations"], json!([]));
        assert!(estimate_minutes(&v) > 0.0);
    }
}

/// On-screen slide text is shown by HTML/native renderers without a TeX engine: convert
/// `$...$` math in every visible text field to Unicode (narration and figures are separate).
/// The player typesets real math, so the TeX original of the converted fields is kept once in
/// `slide["_src"]` (same structure, only those fields); it is never overwritten.
pub fn texify_slide_text(slide: &mut Value) {
    fn has_tex(v: &Value) -> bool {
        match v {
            Value::String(s) => s.contains(['$', '\\', '^']),
            Value::Array(a) => a.iter().any(has_tex),
            Value::Object(o) => o.values().any(has_tex),
            _ => false,
        }
    }
    /// Copy of the visible text fields `texify_slide_text` converts, or `None` if no TeX in them.
    fn snapshot_src(o: &serde_json::Map<String, Value>) -> Option<Value> {
        let mut src = serde_json::Map::new();
        for k in ["title", "subtitle", "formula", "callout", "bullets"] {
            if let Some(v) = o.get(k) {
                src.insert(k.into(), v.clone());
            }
        }
        for (k, fields) in [
            ("cards", &["tag", "title", "body"][..]),
            ("metrics", &["value", "label", "detail"][..]),
            ("steps", &["title", "detail"][..]),
            ("columns", &["heading", "items"][..]),
        ] {
            let Some(arr) = o.get(k).and_then(Value::as_array) else {
                continue;
            };
            let items = arr
                .iter()
                .map(|item| {
                    let mut m = serde_json::Map::new();
                    for f in fields {
                        if let Some(v) = item.get(*f) {
                            m.insert((*f).into(), v.clone());
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            src.insert(k.into(), Value::Array(items));
        }
        if let Some(cp) = o.get("checkpoint").and_then(Value::as_object) {
            let mut m = serde_json::Map::new();
            for k in ["question", "expect"] {
                if let Some(v) = cp.get(k) {
                    m.insert(k.into(), v.clone());
                }
            }
            src.insert("checkpoint".into(), Value::Object(m));
        }
        if let Some(t) = o.get("table").and_then(Value::as_object) {
            let mut m = serde_json::Map::new();
            for k in ["headers", "rows"] {
                if let Some(v) = t.get(k) {
                    m.insert(k.into(), v.clone());
                }
            }
            src.insert("table".into(), Value::Object(m));
        }
        let src = Value::Object(src);
        has_tex(&src).then_some(src)
    }
    fn conv(v: &mut Value) {
        match v {
            Value::String(s) if s.contains(['$', '\\', '^']) => {
                *s = crate::texfmt::tex_to_unicode(s)
            }
            Value::Array(a) => a.iter_mut().for_each(conv),
            _ => {}
        }
    }
    let Some(o) = slide.as_object_mut() else {
        return;
    };
    if !o.contains_key("_src") {
        if let Some(src) = snapshot_src(o) {
            o.insert("_src".into(), src);
        }
    }
    for k in ["title", "subtitle", "formula", "callout", "bullets"] {
        if let Some(v) = o.get_mut(k) {
            conv(v);
        }
    }
    for (k, fields) in [
        ("cards", &["tag", "title", "body"][..]),
        ("metrics", &["value", "label", "detail"][..]),
        ("steps", &["title", "detail"][..]),
        ("columns", &["heading", "items"][..]),
    ] {
        for item in o
            .get_mut(k)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            for f in fields {
                if let Some(v) = item.get_mut(*f) {
                    conv(v);
                }
            }
        }
    }
    if let Some(cp) = o.get_mut("checkpoint").and_then(Value::as_object_mut) {
        for k in ["question", "expect"] {
            if let Some(v) = cp.get_mut(k) {
                conv(v);
            }
        }
    }
    if let Some(t) = o.get_mut("table") {
        for k in ["headers", "rows"] {
            if let Some(Value::Array(rows)) = t.get_mut(k) {
                for r in rows.iter_mut() {
                    conv(r);
                }
            }
        }
    }
}

#[cfg(test)]
mod texify_tests {
    use super::*;

    #[test]
    fn converts_visible_text_only() {
        let mut s = json!({
            "title": "$y=1/x$：$x=0$ 时没有函数值",
            "bullets": ["$x\\neq0$ 时，$1/x$ 存在"],
            "formula": "$A^{-1}A=I$",
            "table": {"headers": ["$n$"], "rows": [["$x\\to0$"]]},
            "narration": [{"text": "保持 $原样$"}]
        });
        texify_slide_text(&mut s);
        assert_eq!(s["title"], "y=1/x：x=0 时没有函数值");
        assert_eq!(s["bullets"][0], "x≠0 时，1/x 存在");
        assert_eq!(s["formula"], "A⁻¹A=I");
        assert_eq!(s["table"]["rows"][0][0], "x→0");
        assert_eq!(s["narration"][0]["text"], "保持 $原样$");
    }

    #[test]
    fn keeps_tex_original_in_src_once() {
        let mut s = json!({
            "title": "向量 $\\vec r_{oi}$",
            "formula": "$A^{-1}A=I$",
            "bullets": ["$x_i$ 与 plain"],
            "cards": [{"tag": "t", "title": "$a^2$", "body": "b", "extra": "x"}],
            "table": {"headers": ["$n$"], "rows": [["$x\\to0$"]]},
            "narration": [{"text": "$n$"}]
        });
        texify_slide_text(&mut s);
        assert_eq!(s["_src"]["title"], "向量 $\\vec r_{oi}$");
        assert_eq!(s["_src"]["formula"], "$A^{-1}A=I$");
        assert_eq!(s["_src"]["cards"][0]["title"], "$a^2$");
        assert!(s["_src"]["cards"][0].get("extra").is_none());
        assert_eq!(s["_src"]["table"]["rows"][0][0], "$x\\to0$");
        assert!(s["_src"].get("narration").is_none());
        assert_eq!(s["formula"], "A⁻¹A=I");
        // second call: already converted, `_src` is kept as is
        let before = s["_src"].clone();
        texify_slide_text(&mut s);
        assert_eq!(s["_src"], before);
        assert_eq!(s["formula"], "A⁻¹A=I");
    }

    #[test]
    fn no_src_without_tex() {
        let mut s = json!({"title": "纯文本", "bullets": ["a", "b"], "table": {"headers": ["h"], "rows": [["c"]]}});
        texify_slide_text(&mut s);
        assert!(s.get("_src").is_none());
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;

    fn table_slide(rows: usize) -> Value {
        json!({"layout": "table", "title": "t", "narration": [{"text": "x"}],
               "table": {"headers": ["项", "方向"], "rows": vec![json!(["a", "b"]); rows]}})
    }

    #[test]
    fn short_table_without_figure_is_sparse() {
        assert!(sparse_text_only(&table_slide(4)));
        assert!(!sparse_text_only(&table_slide(6)));
        let mut with_fig = table_slide(4);
        with_fig["figure_request"] = json!({"kind": "diagram", "purpose": "p", "details": "d"});
        assert!(!sparse_text_only(&with_fig));
        assert!(!sparse_text_only(&json!({"layout": "title", "title": "t"})));
        assert!(sparse_text_only(
            &json!({"layout": "bullets", "title": "t", "bullets": ["a", "b"]})
        ));
    }

    #[test]
    fn geometric_section_flags_each_sparse_slide() {
        let mut fig = table_slide(2);
        fig["figure"] = json!({"meta": {"parts": []}});
        let slides = vec![fig, table_slide(4)];
        let issues = check_section_visuals(&slides, "required");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].slide, 1);

        // a derivation section asks nothing of the author: the animation step reveals its rows
        assert!(check_section_visuals(&[table_slide(3)], "suggested").is_empty());
        assert!(check_section_visuals(&[table_slide(3)], "none").is_empty());
    }

    #[test]
    fn table_counts_as_text_beside_a_figure() {
        let mut s = table_slide(2);
        s["figure"] = json!({"meta": {"parts": []}});
        let issues = check_lecture_content(&[s], "general");
        assert!(!issues.iter().any(|i| i.msg.contains("no on-screen text")));
    }
}

#[cfg(test)]
mod prereq_tests {
    use super::*;

    fn slide(title: &str) -> Value {
        json!({"layout": "bullets", "title": title, "bullets": ["a", "b", "c", "d"], "narration": [{"text": "讲解"}]})
    }

    #[test]
    fn prerequisite_detours_are_flagged_outside_review_mode() {
        let flagged = |t: &str, mode: &str| {
            check_lecture_content(&[slide(t)], mode)
                .iter()
                .any(|i| i.msg.contains("prerequisite/review slide"))
        };
        assert!(flagged("前置知识：叉乘", "general"));
        assert!(flagged("先回顾一下转动惯量", "problem"));
        assert!(flagged("Prerequisites", "preview"));
        assert!(lecture_rules("problem", "auto").contains("MODE problem"));
        assert!(!flagged("前置知识：叉乘", "review"));
        assert!(!flagged("第一步：写出力矩", "general"));
    }

    #[test]
    fn refusal_and_cancel_end_the_evidence_read_other_errors_fall_back() {
        assert!(ends_evidence_read(ErrorCode::ModelRefused));
        assert!(ends_evidence_read(ErrorCode::Cancelled));
        assert!(!ends_evidence_read(ErrorCode::FetchFailed));
        assert!(!ends_evidence_read(ErrorCode::ModelRateLimited));
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn split_slide_batches_keeps_three_to_four_slides_per_batch() {
        assert!(split_slide_batches(0).is_empty());
        assert_eq!(split_slide_batches(1), vec![(0, 1)]);
        assert_eq!(split_slide_batches(4), vec![(0, 4)]);
        assert_eq!(split_slide_batches(5), vec![(0, 3), (3, 5)]);
        assert_eq!(split_slide_batches(6), vec![(0, 3), (3, 6)]);
        assert_eq!(split_slide_batches(7), vec![(0, 4), (4, 7)]);
        assert_eq!(split_slide_batches(8), vec![(0, 4), (4, 8)]);
        assert_eq!(split_slide_batches(9), vec![(0, 3), (3, 6), (6, 9)]);
        assert_eq!(split_slide_batches(10), vec![(0, 4), (4, 7), (7, 10)]);
    }

    #[test]
    fn page_skeleton_schema_requires_title_points_and_figure_flag() {
        let s = section_page_skeleton_schema();
        assert_eq!(s["required"], json!(["pages"]));
        let item_req = &s["properties"]["pages"]["items"]["required"];
        assert_eq!(*item_req, json!(["title", "points", "needs_figure"]));
        assert_eq!(LECTURE_MAX_OUTPUT_TOKENS, 32_768);
    }
}
