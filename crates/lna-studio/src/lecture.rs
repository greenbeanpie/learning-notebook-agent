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
use futures_util::StreamExt as _;
use std::collections::HashSet;

pub const LECTURE_REPAIR_ROUNDS: usize = 2;
const FIGURE_REVIEW_ROUNDS: usize = 2;

pub(crate) const ELEMENT_IDS_GUIDE: &str = "ELEMENT IDS (laser targets and build targets) on a slide: \"title\", \"subtitle\", \"b1\"..\"bN\" (bullets, 1-based), \"card1\".., \"metric1\".., \"step1\".. (timeline steps), \"col1\"/\"col2\" (whole column) and \"col1.i2\" (item 2 of column 1), \"table\" and \"table.r1\" (row 1), \"formula\", \"callout\", \"figure\" (the whole figure) and \"figure:<part id>\" (one part of the figure, e.g. \"figure:V\", \"figure:F1\"). Only ids of elements that exist on that slide are valid.";

pub(crate) const CUES_GUIDE: &str = "CUES (attached to the narration sentence during which they happen):\n- Laser pointer: {\"type\":\"laser\",\"target\":<element id>,\"shape\":\"circle\"|\"ellipse\",\"scale\":1.0-1.6 (ring size relative to the element),\"speed\":0.3-1.5 (revolutions per second),\"duration_ms\":800-6000, \"angle_deg\": -90..90 (optional ellipse tilt to align with a slanted element such as an incline, a slanted vector or a rotated block; omit for horizontal), \"at\": <optional: the exact words of this sentence at which pointing starts>, \"style\": <optional: \"glow\" (the figure part flashes in colour; default for figure parts), \"marker\" (a highlighter sweep; default for wide text such as a table row), \"ring\" (a circling dot; default for small text)>}. Use it for the things the sentence is about, never decoratively. ONE POINTER PER THING NAMED: when a sentence names two or more things on the slide (e.g. \"the part perpendicular to the axis is r-perp, the part along the axis is z times k\"), give each its own laser cue, in the order they are spoken, each with \"at\" set to the words where it is named; the pointer then moves from one to the next as they are spoken. Point at the precise thing (one figure part, one bullet, one table row), not at a whole figure or table.\n- Step reveal: {\"type\":\"step\",\"n\":k} shows build step k of this slide.\nBUILDS (step-by-step animation): \"builds\": [{\"step\":1,\"targets\":[ids],\"effect\":\"appear\"|\"fade\"|\"fly_up\"|\"draw\"|\"emphasis\"|\"dim_others\"}]. Steps are numbered 1..K consecutively; elements not listed are visible from the start. Every step must be revealed by exactly one {\"type\":\"step\"} cue, in increasing order, on the sentence where that content is first explained. Use builds for derivations, procedures, comparisons and lists explained one item at a time; keep simple slides static. \"draw\" suits figure curves/arrows; \"emphasis\"/\"dim_others\" highlight something already visible. A figure part never appears before what it hangs on: projection or guide lines, labels, angle marks and components appear in the same step as (or after) the vector, line or point they belong to — put them in that step's targets (parts left out of every build are visible from the start).";

pub(crate) const FIGURE_REQUEST_GUIDE: &str = "FIGURES: content that is obviously geometric or physical (vectors, forces, torques, coordinate systems, angles, shapes, trajectories, function graphs, surfaces) MUST have a figure; for anything else add one when a function graph, implicit curve, 3-D surface, geometric figure or physics diagram genuinely helps. Add \"figure_request\": {\"kind\":\"plot\"|\"plot3d\"|\"diagram\",\"purpose\":\"what it must show\",\"details\":\"exact functions, ranges, points, forces, angles, labels AND the part ids you will reference\"} to the slide (normally with layout \"figure\"). A separate drawing step turns it into a precise figure. In details, name each part you reference with an explicit id (e.g. \"curve id f: y=x^2-2x on [-1,3]; point id V at (1,-1) labelled 顶点; vector id G downward labelled G\") and use exactly those ids as figure:<id> in cues and builds. FIGURE + TEXT TOGETHER (important): a figure must never sit alone on a slide. The slide that shows a figure also carries the text that explains it — 2-3 very short (under ~16 Chinese characters or ~8 English words each) \"bullets\", a \"formula\" and/or a compact \"table\" (at most 4 rows, 2-3 short columns) stating the problem or claim and the key result, shown left of the figure — and the figure's labels use the same symbols as that text. Put the example's statement and its figure on the SAME slide; do not state an example on one slide and show its figure on the next, and do not use \"display_only\" for a figure slide. Narrate the text and the figure together: in each sentence point the laser at the figure part (figure:<id>) or bullet (b1..) being spoken about, and use builds so each bullet and its matching figure part appear in the same step.";

/// Judgement rules shared by the planner and the authors: how much to say, to whom, and what
/// never to talk about. They are principles, not page counts, so the model sizes the lecture itself.
pub(crate) const TEACHING_JUDGEMENT: &str = "TEACHING JUDGEMENT (applies to the plan and to every slide):\n- ECONOMY: every slide must earn its place. Before adding one ask: would a student who already has the prerequisites miss something if it were removed? If not, leave it out. No orientation, agenda, section-divider, recap or \"where we are\" slides unless the lecture truly spans several topics (a recap repeats what was just said; a method summary or extension after a problem adds something and is allowed, see PROBLEMS). A derivation is taught as its sequence of steps in order, pausing only where a step is not obvious (a non-trivial identity, a choice, a physical argument) and moving quickly through routine algebra. Plan the fewest sections that teach the scope: a single derivation or exercise is ONE section.\n- VISUALS (show before you tell): content that is obviously geometric or physical — vectors, forces and torques, coordinate systems and angles, shapes, motion paths, function graphs, surfaces — MUST be shown with a figure (figure_request: diagram, plot or plot3d) while it is explained, and the figure's parts appear step by step (builds on figure:<id>) in step with the equations or bullets, with the laser on the part being spoken about. A page that is only a worked derivation (algebra steps, a proof, a calculation) is SUGGESTED to use visuals or animation: reveal the steps one at a time, highlight the current step, and add a small figure when one truly clarifies; when no figure fits, the step-by-step reveal is enough. Never invent a figure just to have one. A slide that carries a needed figure or a step-by-step build earns its place under ECONOMY. Reasoning about vectors inside a derivation (directions of terms, components along an axis, which cross products vanish, signs of torques) is geometric content, not mere algebra: draw the vectors and the axis.\n- FILL THE SLIDE: a 16:9 slide holding only a short table, two or three bullets or one formula is mostly empty space. Put the figure that shows what that text states beside it (the text — bullets, formula or a compact table — sits left of the figure on the same slide), or merge the text into the neighbouring slide it belongs to. A table that lists directions, components or cases of vectors belongs next to a drawing of those vectors, with each row and its figure part revealed together.\n- LEARNER: infer the learner's level from the material itself (a university student working at this point of the course) and assume the usual prerequisites are known: do not define or re-teach prerequisite concepts, standard operations or notation (for example the cross product, derivatives or vector decomposition in a mechanics derivation). Explain only what is new or non-obvious in the material. If the user's request explicitly asks for background, give it.\n- PROBLEMS (an exercise, an example, a question or a derivation to work through): go straight to the problem and solve it. No slides that review, define or teach concepts, theorems, formulas or methods before the solution (no \"prerequisites\", \"knowledge review\", \"first let's recall ...\" or \"basic concepts\" slides or sections). When a step uses a known result, name it in one short clause at that step as the reason for the step (e.g. \"by the parallel-axis theorem, ...\") and move on; do not derive, prove or illustrate the known result itself. Only the non-obvious idea that unlocks this particular problem deserves explanation, and it is explained inside the step where it is used. AFTER THE SOLUTION (allowed, not required): once the problem is solved you may close with what the learner should take away from it — a method summary (the general approach and when it applies), an extension or generalisation (what changes if a condition changes, a variant, a related problem type), common mistakes, or a quick check of the result. Keep it to what genuinely adds value (usually one slide, at most two) and keep it tied to this problem; this is where connections to the wider topic belong, never before the solution.\n- NEVER TALK ABOUT HOW THIS LECTURE WAS PRODUCED: no OCR, text recognition or scan quality/noise, no image extraction, no outline, plan, section or generation steps, and no navigation such as \"this page sits in the derivation chain\". Speak as the teacher who wrote the lesson, about the content. Mentioning the textbook as a teaching resource (e.g. \"see the book's worked example\") is fine when it helps.";

/// Mode/detail rules for the lecture author.
pub(crate) fn lecture_rules(mode: &str, detail: &str) -> String {
    let mode_rule = match mode {
        "preview" => "MODE preview (first-time learning of a unit): state the learning goals first, then teach concepts in a gentle order — concept → intuition → example → common pitfalls; highlight each new term with the laser the first time it appears.",
        "review" => "MODE review (consolidation of material already learned): recap key points, connect and contrast related ideas, interleave topics, call out common mistakes, end with a compact summary. Give extra time to the weak concepts listed below and pass quickly over mastered ones. Do not re-teach from scratch.",
        "practice" => "MODE practice (problem solving): build the lecture around problems. Start with the first problem — no concept-review slides before it. For each problem: one slide presenting the problem, with checkpoint {\"kind\":\"attempt\",\"question\":<the task>,\"expect\":<final answer>} so the learner solves it first; then worked-solution slides revealed step by step with builds and laser cues on each step; then a variant problem with less guidance (fading). Draw figures for problems that need them.",
        _ => "MODE general: explain the requested material clearly for any kind of content (textbook, paper, article, a question, a derivation): lead with what the material says and how it works; add why it matters or a summary only where they genuinely help. Do not assume a textbook structure, and do not force a what → why → how template onto a short or purely technical piece of material.",
    };
    let detail_rule = match detail {
        "brief" => "DETAIL brief: only the essentials, few slides per section, short narration.",
        "detailed" => "DETAIL detailed: thorough coverage with derivations, examples and edge cases; more slides per section.",
        "standard" => "DETAIL standard: balanced coverage.",
        _ => "DETAIL auto: decide the depth yourself from the amount and difficulty of the material in scope. The lecture is as long as the teaching needs and no longer: one page of material, one exercise or one derivation is a short lecture (a handful of slides), a section is a medium one, a whole chapter a long one. Do not pad and do not skip substance.",
    };
    format!(
        "{mode_rule}\n{detail_rule}\n{TEACHING_JUDGEMENT}\nCHECKPOINTS: do NOT ask a question after every point. Questions happen when the learner interrupts, or — sparingly, about one per section at most — when you judge a pause genuinely helps (a key difficulty or a common confusion): add \"checkpoint\": {{\"kind\":\"ask\",\"question\":...,\"expect\":...,\"reason\":<why a pause helps here>}} to that slide.{}",
        if mode == "practice" { " In practice mode the per-problem \"attempt\" checkpoint is part of the format and does not count." } else { "" }
    )
}

const NARRATION_RULES: &str = "NARRATION: every slide has \"narration\": an ordered list of sentences {\"text\":...,\"cues\":[...]} that the teacher SAYS while the slide is shown (it is read by text-to-speech). Narration is plain spoken language: NO LaTeX, NO markdown, NO $, no symbols that cannot be read aloud — read formulas in words (e.g. \"A 的逆等于 A 的伴随矩阵除以 A 的行列式\"). One sentence per item, at most ~60 Chinese characters or ~35 English words. On-screen text stays concise (keywords, formulas in LaTeX $...$ are fine on screen). A slide may have no narration only if it sets \"display_only\": {\"reason\": ...} (e.g. a full-screen figure explained on the next slide); never two display-only slides in a row.";

// ---------------------------------------------------------------------------------------------
// schemas

fn slide_item_schema() -> Value {
    let base: Value = serde_json::from_str(SCHEMA_SLIDES).unwrap_or_else(|_| json!({}));
    base["properties"]["slides"]["items"].clone()
}

pub(crate) fn cue_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["type"],
        "properties": {
            "type": { "type": "string", "enum": ["laser", "step"] },
            "target": { "type": "string" },
            "shape": { "type": "string", "enum": ["circle", "ellipse"] },
            "scale": { "type": "number" },
            "angle_deg": { "type": "number" },
            "speed": { "type": "number" },
            "duration_ms": { "type": "integer" },
            "n": { "type": "integer" },
            "at": { "type": "string" },
            "style": { "type": "string", "enum": ["auto", "ring", "glow", "marker"] }
        }
    })
}

/// A lecture slide: a regular slide plus narration, cues, builds, checkpoint and figure request.
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
                        "text": { "type": "string" },
                        "cues": { "type": "array", "items": cue_schema() }
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

/// One validation problem; `fix` is the local fallback applied only after repair rounds fail.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub slide: usize,
    pub msg: String,
    pub fix: Fix,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    RemoveCue { sentence: usize, cue: usize },
    MarkDisplayOnly,
    RemoveBuildStep(i64),
    RemoveBuilds,
    PlainNarration { sentence: usize },
    DropCheckpoint,
    DropLaserTarget { target: String },
    None,
}

fn num_in(v: &Value, k: &str, lo: f64, hi: f64) -> Result<(), String> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(()),
        Some(x) => match x.as_f64() {
            Some(n) if n >= lo && n <= hi => Ok(()),
            Some(n) => Err(format!("\"{k}\" = {n} is outside {lo}..{hi}")),
            None => Err(format!("\"{k}\" must be a number")),
        },
    }
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
            fix: Fix::None,
        });
    }
    for (si, s) in slides.iter().enumerate() {
        if !sparse_text_only(s) {
            continue;
        }
        match visual_need {
            "required" => issues.push(Issue {
                slide: si,
                msg: "shows only a short table/list with no figure, so most of the slide is empty, in a section that is geometric or physical; add a \"figure_request\" that draws what this text describes (for a table of directions, components or products of vectors: draw those vectors and the axis), keep the table/bullets/formula on the same slide beside the figure, and reveal each row/bullet together with its figure part (builds on table.rN or bN plus figure:<id>, laser on the part being spoken about)".into(),
                fix: Fix::None,
            }),
            "suggested" if slide_builds_empty(s) => issues.push(Issue {
                slide: si,
                msg: "shows only a short table/list with no figure and no step-by-step reveal, so the slide is static and mostly empty; reveal its rows/bullets one at a time with builds and step cues in step with the narration, and add a \"figure_request\" (with the text kept beside the figure) if a diagram genuinely shows what it states".into(),
                fix: Fix::None,
            }),
            _ => {}
        }
    }
    issues
}

fn slide_builds_empty(slide: &Value) -> bool {
    slide
        .get("builds")
        .and_then(Value::as_array)
        .is_none_or(|b| b.is_empty())
}

/// Checks lecture slides (one section or a whole lecture). `label(i)` names slide i in messages.
pub fn check_lecture_slides(slides: &[Value], mode: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut prev_display_only = false;
    for (si, slide) in slides.iter().enumerate() {
        let ids: HashSet<String> = slide_element_ids(slide).into_iter().collect();
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
            issues.push(Issue { slide: si, msg: "has no narration; add narration sentences or mark it \"display_only\": {\"reason\": ...}".into(), fix: Fix::MarkDisplayOnly });
        }
        if spoken.is_empty() && display_only && prev_display_only {
            issues.push(Issue {
                slide: si,
                msg: "is the second display-only slide in a row; give one of them narration".into(),
                fix: Fix::None,
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
                fix: Fix::None,
            });
        }

        // a detour through prerequisites before the actual material (review mode recaps on purpose)
        if mode != "review" {
            if let Some(term) = prereq_title(&sv(slide, "title")) {
                issues.push(Issue {
                    slide: si,
                    msg: format!("is a prerequisite/review slide (\"{term}\"); the learner already knows the prerequisites, so do not teach known concepts here: go straight to the material and, where a step needs a known result, name it in one clause at that step (unless the user's request explicitly asked for background)"),
                    fix: Fix::None,
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
                fix: Fix::None,
            });
        }

        // builds
        let builds = slide
            .get("builds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut steps: Vec<i64> = Vec::new();
        for b in &builds {
            let step = b.get("step").and_then(Value::as_i64).unwrap_or(0);
            if step < 1 {
                issues.push(Issue {
                    slide: si,
                    msg: "has a build without a positive \"step\"".into(),
                    fix: Fix::RemoveBuilds,
                });
                continue;
            }
            steps.push(step);
            for t in b
                .get("targets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let t = t.as_str().unwrap_or("");
                if !ids.contains(t) {
                    issues.push(Issue {
                        slide: si,
                        msg: format!("build step {step} targets \"{t}\", which does not exist on this slide (valid: {})", join_ids(&ids)),
                        fix: Fix::RemoveBuildStep(step),
                    });
                }
            }
            let eff = sv(b, "effect");
            if !eff.is_empty()
                && !["appear", "fade", "fly_up", "draw", "emphasis", "dim_others"]
                    .contains(&eff.as_str())
            {
                issues.push(Issue {
                    slide: si,
                    msg: format!("build step {step} has unknown effect \"{eff}\""),
                    fix: Fix::RemoveBuildStep(step),
                });
            }
        }
        let mut uniq = steps.clone();
        uniq.sort();
        uniq.dedup();
        let max_step = uniq.last().copied().unwrap_or(0);
        if !uniq.is_empty() && uniq != (1..=max_step).collect::<Vec<_>>() {
            issues.push(Issue {
                slide: si,
                msg: format!(
                    "build steps must be numbered 1..K consecutively, got {:?}",
                    uniq
                ),
                fix: Fix::RemoveBuilds,
            });
        }

        // cues
        let mut revealed: Vec<i64> = Vec::new();
        for (ni, n) in narration.iter().enumerate() {
            let text = sv(n, "text");
            if has_tex(&text) {
                issues.push(Issue {
                    slide: si,
                    msg: format!("narration sentence {} contains LaTeX/markup (\"{}\"); narration must be plain spoken words", ni + 1, truncate_str(&text, 60)),
                    fix: Fix::PlainNarration { sentence: ni },
                });
            }
            for (ci, c) in n
                .get("cues")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                match sv(c, "type").as_str() {
                    "laser" => {
                        let target = sv(c, "target");
                        if !ids.contains(&target) {
                            issues.push(Issue {
                                slide: si,
                                msg: format!("laser cue in sentence {} targets \"{}\", which does not exist on this slide (valid: {})", ni + 1, target, join_ids(&ids)),
                                fix: Fix::RemoveCue { sentence: ni, cue: ci },
                            });
                        }
                        let shape = sv(c, "shape");
                        if !shape.is_empty() && shape != "circle" && shape != "ellipse" {
                            issues.push(Issue { slide: si, msg: format!("laser cue in sentence {} has shape \"{}\" (use circle or ellipse)", ni + 1, shape), fix: Fix::RemoveCue { sentence: ni, cue: ci } });
                        }
                        for (k, lo, hi) in [
                            ("scale", 1.0, 1.6),
                            ("speed", 0.3, 1.5),
                            ("angle_deg", -90.0, 90.0),
                            ("duration_ms", 800.0, 6000.0),
                        ] {
                            if let Err(e) = num_in(c, k, lo, hi) {
                                issues.push(Issue {
                                    slide: si,
                                    msg: format!("laser cue in sentence {}: {}", ni + 1, e),
                                    fix: Fix::RemoveCue {
                                        sentence: ni,
                                        cue: ci,
                                    },
                                });
                            }
                        }
                    }
                    "step" => {
                        let k = c.get("n").and_then(Value::as_i64).unwrap_or(0);
                        if k < 1 || k > max_step {
                            issues.push(Issue {
                                slide: si,
                                msg: format!("step cue n={} in sentence {} does not match a build step (this slide has {} steps)", k, ni + 1, max_step),
                                fix: Fix::RemoveCue { sentence: ni, cue: ci },
                            });
                        } else if revealed.last().is_some_and(|last| k <= *last) {
                            issues.push(Issue {
                                slide: si,
                                msg: format!(
                                    "step cue n={} in sentence {} is out of order or repeated",
                                    k,
                                    ni + 1
                                ),
                                fix: Fix::RemoveCue {
                                    sentence: ni,
                                    cue: ci,
                                },
                            });
                        } else {
                            revealed.push(k);
                        }
                    }
                    other => issues.push(Issue {
                        slide: si,
                        msg: format!("unknown cue type \"{}\" in sentence {}", other, ni + 1),
                        fix: Fix::RemoveCue {
                            sentence: ni,
                            cue: ci,
                        },
                    }),
                }
            }
        }
        for k in 1..=max_step {
            if !revealed.contains(&k) && !spoken.is_empty() {
                issues.push(Issue { slide: si, msg: format!("build step {k} is never revealed by a step cue, so its content would stay hidden"), fix: Fix::RemoveBuildStep(k) });
            }
        }

        // checkpoint
        if let Some(cp) = slide.get("checkpoint").filter(|c| c.is_object()) {
            let kind = sv(cp, "kind");
            if sv(cp, "question").is_empty() {
                issues.push(Issue {
                    slide: si,
                    msg: "checkpoint has no question".into(),
                    fix: Fix::DropCheckpoint,
                });
            }
            if kind == "attempt" && mode != "practice" && sv(cp, "expect").is_empty() {
                issues.push(Issue {
                    slide: si,
                    msg: "attempt checkpoint needs \"expect\" (the answer)".into(),
                    fix: Fix::DropCheckpoint,
                });
            }
            if kind == "ask" && sv(cp, "reason").is_empty() {
                issues.push(Issue { slide: si, msg: "checkpoint must state \"reason\" (why pausing here helps); drop it if a pause is not really needed".into(), fix: Fix::DropCheckpoint });
            }
        }
    }
    issues
}

fn join_ids(ids: &HashSet<String>) -> String {
    let mut v: Vec<&String> = ids.iter().collect();
    v.sort();
    v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

/// Last-resort local fallback after the repair rounds: removes only what is broken.
pub fn apply_fallbacks(slides: &mut [Value], issues: &[Issue]) -> Vec<String> {
    let mut log = Vec::new();
    // cue removals must go from the highest index down so indices stay valid
    let mut cue_removals: Vec<(usize, usize, usize)> = Vec::new();
    for is in issues {
        let Some(slide) = slides.get_mut(is.slide) else {
            continue;
        };
        match &is.fix {
            Fix::RemoveCue { sentence, cue } => cue_removals.push((is.slide, *sentence, *cue)),
            Fix::MarkDisplayOnly => {
                slide["display_only"] = json!({ "reason": "(no narration provided)" });
                log.push(format!("slide {}: marked display-only", is.slide + 1));
            }
            Fix::RemoveBuildStep(step) => {
                if let Some(b) = slide.get_mut("builds").and_then(Value::as_array_mut) {
                    b.retain(|x| x.get("step").and_then(Value::as_i64) != Some(*step));
                    log.push(format!(
                        "slide {}: removed build step {}",
                        is.slide + 1,
                        step
                    ));
                }
            }
            Fix::RemoveBuilds => {
                if let Some(o) = slide.as_object_mut() {
                    o.remove("builds");
                }
                log.push(format!("slide {}: removed builds", is.slide + 1));
            }
            Fix::PlainNarration { sentence } => {
                if let Some(n) = slide
                    .get_mut("narration")
                    .and_then(|n| n.get_mut(*sentence))
                {
                    let t = crate::texfmt::tex_to_unicode(&sv(n, "text")).replace('$', "");
                    n["text"] = Value::String(t);
                }
            }
            Fix::DropCheckpoint => {
                if let Some(o) = slide.as_object_mut() {
                    o.remove("checkpoint");
                }
                log.push(format!("slide {}: dropped checkpoint", is.slide + 1));
            }
            Fix::DropLaserTarget { .. } | Fix::None => {}
        }
    }
    cue_removals.sort();
    cue_removals.dedup();
    for (s, n, c) in cue_removals.into_iter().rev() {
        if let Some(cues) = slides[s]
            .pointer_mut(&format!("/narration/{n}/cues"))
            .and_then(Value::as_array_mut)
        {
            if c < cues.len() {
                cues.remove(c);
                log.push(format!(
                    "slide {}: removed an invalid cue in sentence {}",
                    s + 1,
                    n + 1
                ));
            }
        }
    }
    // builds whose steps were removed must be renumbered, with step cues remapped
    for slide in slides.iter_mut() {
        renumber_builds(slide);
    }
    log
}

/// Deck (non-lecture) builds: drop builds that target missing elements or use unknown
/// effects, then renumber steps 1..K. Decks are clicked through manually, so there are no
/// step cues to check.
pub fn sanitize_deck_builds(slides: &mut [Value]) -> Vec<String> {
    let mut log = Vec::new();
    for (si, slide) in slides.iter_mut().enumerate() {
        let ids: HashSet<String> = slide_element_ids(slide).into_iter().collect();
        let Some(builds) = slide.get_mut("builds").and_then(Value::as_array_mut) else {
            continue;
        };
        let before = builds.len();
        builds.retain(|b| {
            let step_ok = b
                .get("step")
                .and_then(Value::as_i64)
                .is_some_and(|s| s >= 1);
            let eff = b.get("effect").and_then(Value::as_str).unwrap_or("fade");
            let eff_ok =
                ["appear", "fade", "fly_up", "draw", "emphasis", "dim_others"].contains(&eff);
            let targets_ok = b.get("targets").and_then(Value::as_array).is_some_and(|t| {
                !t.is_empty()
                    && t.iter()
                        .all(|x| x.as_str().is_some_and(|x| ids.contains(x)))
            });
            step_ok && eff_ok && targets_ok
        });
        if builds.len() != before {
            log.push(format!(
                "slide {}: removed {} invalid build(s)",
                si + 1,
                before - builds.len()
            ));
        }
        renumber_builds(slide);
    }
    log
}

const ENTRANCE_EFFECTS: [&str; 4] = ["appear", "fade", "fly_up", "draw"];

/// Keeps a figure part from appearing before what it hangs on: a projection line drawn from
/// an arrow tip, the label of a vector or an angle mark between two lines waits for them.
/// Uses the attachment groups the figure engine reports (`figure.meta.parts[].anchors`): a
/// part may appear only once every attachment point of it is drawn by something already
/// visible. Parts the narration points at earlier stay where they are. Returns the moved
/// part ids.
pub(crate) fn order_figure_builds(slide: &mut Value) -> Vec<String> {
    let parts = slide
        .pointer("/figure/meta/parts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let builds = slide
        .get("builds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if parts.is_empty() || builds.is_empty() {
        return Vec::new();
    }
    let is_entrance = |b: &Value| {
        ENTRANCE_EFFECTS.contains(&b.get("effect").and_then(Value::as_str).unwrap_or("fade"))
    };
    let mut step: HashMap<String, i64> = parts.iter().map(|p| (sv(p, "id"), 0)).collect();
    for b in builds.iter().filter(|b| is_entrance(b)) {
        let s = b.get("step").and_then(Value::as_i64).unwrap_or(0);
        for t in b
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let t = t.as_str().unwrap_or("");
            if t == "figure" {
                return Vec::new(); // the whole figure is one build: nothing to order inside it
            }
            if let Some(e) = t.strip_prefix("figure:").and_then(|id| step.get_mut(id)) {
                *e = if *e == 0 { s } else { (*e).min(s) };
            }
        }
    }
    // the step during which the narration first points at each part
    let mut pointed: HashMap<String, i64> = HashMap::new();
    let mut cur = 0;
    for n in slide
        .get("narration")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for c in n
            .get("cues")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if sv(c, "type") == "step" {
                cur = cur.max(c.get("n").and_then(Value::as_i64).unwrap_or(0));
            }
        }
        for c in n
            .get("cues")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if sv(c, "type") == "laser" {
                if let Some(id) = sv(c, "target").strip_prefix("figure:") {
                    pointed.entry(id.to_string()).or_insert(cur);
                }
            }
        }
    }
    let anchors: Vec<(String, Vec<Vec<String>>)> = parts
        .iter()
        .filter_map(|p| {
            let groups: Vec<Vec<String>> =
                serde_json::from_value(p.get("anchors")?.clone()).ok()?;
            Some((sv(p, "id"), groups))
        })
        .collect();
    let mut moved: Vec<String> = Vec::new();
    for _ in 0..=parts.len() {
        let mut changed = false;
        for (id, groups) in &anchors {
            let need = groups
                .iter()
                .map(|g| {
                    g.iter()
                        .map(|m| step.get(m).copied().unwrap_or(0))
                        .min()
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            let have = step.get(id).copied().unwrap_or(0);
            if have < need && pointed.get(id).is_none_or(|p| *p >= need) {
                step.insert(id.clone(), need);
                if !moved.contains(id) {
                    moved.push(id.clone());
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if moved.is_empty() {
        return moved;
    }
    let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    for id in &moved {
        let target = format!("figure:{id}");
        for b in bs.iter_mut().filter(|b| is_entrance(b)) {
            if let Some(t) = b.get_mut("targets").and_then(Value::as_array_mut) {
                t.retain(|x| x.as_str() != Some(target.as_str()));
            }
        }
        let k = step[id];
        match bs
            .iter_mut()
            .find(|b| is_entrance(b) && b.get("step").and_then(Value::as_i64) == Some(k))
        {
            Some(b) => {
                if let Some(t) = b.get_mut("targets").and_then(Value::as_array_mut) {
                    t.push(json!(target));
                }
            }
            None => bs.push(json!({"step": k, "targets": [target], "effect": "fade"})),
        }
    }
    bs.retain(|b| {
        b.get("targets")
            .and_then(Value::as_array)
            .is_some_and(|t| !t.is_empty())
    });
    renumber_builds(slide);
    moved
}

fn renumber_builds(slide: &mut Value) {
    let Some(builds) = slide.get("builds").and_then(Value::as_array).cloned() else {
        return;
    };
    let mut steps: Vec<i64> = builds
        .iter()
        .filter_map(|b| b.get("step").and_then(Value::as_i64))
        .collect();
    steps.sort();
    steps.dedup();
    let map: HashMap<i64, i64> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (*s, i as i64 + 1))
        .collect();
    if map.iter().all(|(k, v)| k == v) {
        return;
    }
    if let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) {
        for b in bs.iter_mut() {
            if let Some(s) = b.get("step").and_then(Value::as_i64) {
                b["step"] = json!(map.get(&s).copied().unwrap_or(s));
            }
        }
    }
    if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
        for n in ns.iter_mut() {
            if let Some(cues) = n.get_mut("cues").and_then(Value::as_array_mut) {
                cues.retain(|c| {
                    sv(c, "type") != "step"
                        || c.get("n")
                            .and_then(Value::as_i64)
                            .is_some_and(|k| map.contains_key(&k))
                });
                for c in cues.iter_mut() {
                    if sv(c, "type") == "step" {
                        if let Some(k) = c.get("n").and_then(Value::as_i64) {
                            c["n"] = json!(map[&k]);
                        }
                    }
                }
            }
        }
    }
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
        Some(ResolvedProfile {
            id: prof.id,
            display_name: prof.display_name,
            protocol: prof.protocol,
            base_url: prof.base_url,
            model_id: prof.model_id,
            api_key,
            show_thinking: prof.show_thinking,
            effort: prof.effort,
            context_window: prof.context_window,
            max_output_tokens: prof.max_output_tokens,
            capabilities: prof.capabilities,
            extra_headers: prof.extra_headers,
            anthropic_version: "2023-06-01".into(),
        })
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
        let req = LlmRequest {
            system: system.clone(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(user.clone())],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens.min(4000),
            reasoning: ReasoningOptions::default(),
        };
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
            let review_req = LlmRequest {
                system: system.clone(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts,
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: resolved.max_output_tokens.min(4000),
                reasoning: ReasoningOptions::default(),
            };
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
        let mut running = futures_util::stream::iter(jobs.into_iter().map(|(i, title, req)| {
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
        }))
        .buffer_unordered(limit);
        while let Some((i, title, res)) = next_or_cancel(&mut running, &cancel).await? {
            match res {
                Ok((fig, u)) => {
                    add_usage(&mut usage, &u);
                    match fig {
                        Some(fig) => slides[i]["figure"] = fig,
                        // no figure after the retry: a "figure" layout without one would play as
                        // an empty-looking slide, so it becomes an ordinary slide
                        None => {
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
        }
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
        let with_materials = |body: String| format!("{materials}{body}");
        let target_ids = Self::generation_source_ids(art);
        let window = resolved.context_window as usize;
        let reserved_output = (resolved.max_output_tokens as usize).min(window / 4);
        let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
        let budget = (available_input.saturating_mul(70) / 100).min(16_000);
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
                cancel,
            )
            .await
        {
            Ok(Some(t)) if !t.trim().is_empty() => Ok(with_materials(t)),
            Err(e) if e.code == ErrorCode::Cancelled => Err(e),
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
        let system = format!(
            "You are the Local Notebook Studio lecture author producing structured JSON for lecture slides of ONE section of a narrated lesson (a teacher speaking over slides). Output {{\"slides\": [...]}} for this section only. Slide layouts as in a deck (title/section/agenda/bullets/two_column/cards/metrics/timeline/table/quote/formula/summary/figure); keep on-screen text concise and let the narration carry the explanation. Preserve source citation indices [n] in each slide's citations array.\n{NARRATION_RULES}\n{CUES_GUIDE}\n{ELEMENT_IDS_GUIDE}\n{FIGURE_REQUEST_GUIDE}\n{rules}\nStrictly ground every fact in the provided sources. Match the language requested in config or the primary language of the sources. Sections are written separately and played in order: unless the section position below asks for a title or summary slide, do not add an opening or closing slide of your own, only slides that teach this section's focus.{}",
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
            art.config,
            sc.lecture_title,
            sc.objectives.join("; "),
            sc.outline_txt,
            evidence,
            sc.position,
            sc.section_title,
            sc.section_focus,
            sc.extra
        );
        let mk_req = |text: String| LlmRequest {
            system: system.clone(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(text)],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning: reasoning.clone(),
        };
        let mut usage = Usage::default();
        let (out, u) = {
            self.run_structured_with_thought(
                artifact_id,
                &art.notebook_id,
                &format!("撰写讲解：{}", sc.section_title),
                resolved,
                &mk_req(user.clone()),
                "lecture_section",
                &schema,
                max_repairs,
                cancel.clone(),
            )
            .await?
        };
        add_usage(&mut usage, &u);
        let mut slides = out
            .get("slides")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if slides.is_empty() {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Lecture section produced no slides",
            ));
        }
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

        for round in 0..=LECTURE_REPAIR_ROUNDS {
            let mut issues = check_lecture_slides(&slides, mode);
            issues.extend(check_section_visuals(&slides, sc.visual_need));
            if issues.is_empty() {
                break;
            }
            if round == LECTURE_REPAIR_ROUNDS {
                let log = apply_fallbacks(&mut slides, &issues);
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &format!("讲解校验：{} 仍有问题", sc.section_title),
                    &format!("经 {LECTURE_REPAIR_ROUNDS} 轮返修后仍有问题，已仅移除出错的部分：\n{}\n\n{}", issues_text(&issues, &slides), log.join("\n")),
                );
                break;
            }
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &format!("讲解校验：{}（返修第 {} 轮）", sc.section_title, round + 1),
                &issues_text(&issues, &slides),
            );
            // figures already drawn are kept; show the model their real part ids
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
                issues_text(&issues, &slides)
            );
            let res = {
                self.run_structured_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &format!("返修讲解：{}", sc.section_title),
                    resolved,
                    &mk_req(repair),
                    "lecture_section",
                    &schema,
                    max_repairs.min(1),
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
                    slides = new_slides;
                }
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    tracing::warn!("lecture repair failed: {}", e.message);
                }
            }
        }
        let mut moved_log = Vec::new();
        for (i, s) in slides.iter_mut().enumerate() {
            if let Some(o) = s.as_object_mut() {
                o.remove("figure_request");
            }
            let moved = order_figure_builds(s);
            if !moved.is_empty() {
                moved_log.push(format!("第 {} 页：{}", i + 1, moved.join("、")));
            }
        }
        if !moved_log.is_empty() {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &format!("图形出现顺序：{}", sc.section_title),
                &format!(
                    "以下图形部件依附于后出现的部件，已改为与其一同出现：\n{}",
                    moved_log.join("\n")
                ),
            );
        }
        Ok((slides, usage))
    }

    fn lecture_kg_hint(&self, art: &StudioArtifact) -> String {
        let mode = art
            .config
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("general");
        if mode != "review" && mode != "practice" {
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
        let full_text = if self.sources_fit_directly(&target_ids, resolved)? {
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
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "复用已完成的讲解小节",
                    &format!("已从检查点恢复大纲与 {} 个已完成的小节。", done.len()),
                );
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
                let req = LlmRequest {
                    system: format!(
                        "You are the Local Notebook Studio lecture planner producing structured JSON for lecture outline. Resolve the requested scope (topic, problem or unit given in config.topic; the user's attached material if any; the whole selection only if neither is given) against the sources, then plan a narrated lesson: title, learning objectives, and ordered sections. Each section has a title and a \"focus\" naming exactly which concepts/problems/source parts (chapters, sections, pages) it covers, plus an optional slides_hint (keep it small: slides are only as many as the section needs). Also give every section \"visual_need\": \"required\" when its content is obviously geometric or physical (vectors, forces, torques, angles, shapes, trajectories, graphs, surfaces) — including a derivation whose steps reason about vector directions, components or cross products, \"suggested\" when it is mainly a worked derivation, proof or calculation, otherwise \"none\"; and \"visual\": what to draw and how its parts are revealed step by step together with the explanation (empty when none). {}\nIf an approved outline is given, follow it. Strictly ground the plan in the sources. Match the language requested in config or the primary language of the sources.{}",
                        lecture_rules(&mode, &detail),
                        if has_user_material(&art.config) {
                            format!("\n{USER_MATERIAL_SCOPE} A single exercise needs only a few sections (the problem and its solution steps), never the whole chapter and never a concept-review or prerequisites section before the solution; a short closing part after the solution (method summary, extension, common mistakes) is fine when it adds value.")
                        } else {
                            String::new()
                        }
                    ),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Parameters: {}\nApproved outline: {}\n{}\n\nSources:\n{}",
                            art.config,
                            art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                            kg_hint,
                            evidence_txt
                        ))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.max_output_tokens.min(6000),
                    reasoning: reasoning.clone(),
                };
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
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "并行撰写小节",
                &format!(
                    "共 {} 个小节待撰写，最多同时并行 {limit} 个（模型配置中的并行请求数上限）。",
                    pending.len()
                ),
            );
        }
        let (title_ref, objectives_ref, outline_ref, kg_ref) =
            (&title, &objectives, &outline_txt, &kg_hint);
        let mut running = futures_util::stream::iter(pending.into_iter().map(|(i, sec)| {
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
                        "正在查阅该小节对应的原文，并撰写幻灯片、讲稿与激光/分步动画提示…",
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
        }))
        .buffer_unordered(limit);
        while let Some((sid, stitle, res)) = next_or_cancel(&mut running, &cancel).await? {
            match res {
                Ok((mut slides, u)) => {
                    add_usage(&mut usage_sum, &u);
                    finalize_slides(&mut slides, &sid, &format!("{sid}-"));
                    done.insert(sid.clone(), Value::Array(slides));
                }
                Err(e) if e.code == ErrorCode::Cancelled || cancel.is_cancelled() => return Err(e),
                Err(e) => {
                    // a failed section is reported; finished ones stay in the checkpoint
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &format!("小节生成失败：{stitle}"),
                        &truncate_str(&e.message, 300),
                    );
                    self.save_checkpoint(artifact_id, "lecture_passes", json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": done }))?;
                    return Err(e);
                }
            }
            self.save_checkpoint(artifact_id, "lecture_passes", json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": done }))?;
            self.emit_preview(
                artifact_id,
                &art.notebook_id,
                "lecture_passes",
                &json!({ "outline": outline, "done": done }),
            );
        }
        drop(running);

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
        let req = LlmRequest {
            system: "You are the Local Notebook Studio lecture teacher of a narrated lecture and the learner just paused you with a question. Answer it directly and accurately from the sources and the lecture context, at the learner's level, in the learner's language. \"answer\": Markdown (LaTeX $...$ allowed). \"speak\": the same answer as 1-4 plain spoken sentences for text-to-speech (no LaTeX/markdown/symbols). Then judge from the question whether the REST of the lecture should change: \"extend\" if something already explained was unclear or a prerequisite is missing (insert 1-2 supplementary slides now); \"slower\" or \"simpler\" if the pace/level is too high; \"more_examples\" if the learner needs worked examples; \"skip_known\" if the learner is clearly ahead; \"none\" for an isolated detail question (the usual case). Give a short reason in the learner's language. Respond with structured JSON for lecture answer.".into(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: attachments::user_parts(
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
                .await,
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens.min(3000),
            reasoning: ReasoningOptions::default(),
        };
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
        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&resolved.id),
            "studio_lecture_ask",
            Some(artifact_id),
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
        );
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
            let mut out = Vec::new();
            for (gi, (sec, old)) in groups.iter().enumerate() {
                let stitle = sections
                    .iter()
                    .find(|s| sv(s, "id") == *sec)
                    .map(|s| sv(s, "title"))
                    .unwrap_or_else(|| sec.clone());
                let planned: Vec<String> = old.iter().map(|s| sv(s, "title")).collect();
                let focus = format!("{stitle}: {}", planned.join("; "));
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
                    section_title: &stitle,
                    section_focus: &planned.join("; "),
                    visual_need: "none",
                    position: format!("re-authoring remaining part {} of {} after an interruption", gi + 1, groups.len()),
                    extra: format!(
                        "ADAPTATION: {direction}\nThe learner asked: {question}\nWhy: {reason}\nAlready covered (do not repeat): {}\nPreviously planned slides for this part (cover the same content, adapted): {}",
                        covered.join(" | "),
                        planned.join(" | ")
                    ),
                };
                let (mut part, u) = self
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
                finalize_slides(&mut part, sec, &format!("{aid}{}-", gi + 1));
                out.extend(part);
            }
            (out, rest, idx + 1)
        };
        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&profile.id),
            "studio_lecture_adapt",
            Some(artifact_id),
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
        );

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
        assert!(check_lecture_slides(&[slide()], "general").is_empty());
    }

    #[test]
    fn talk_about_the_reading_process_is_reported_but_the_book_may_be_named() {
        let mut s = slide();
        s["narration"][0]["text"] = json!("扫描版的识别噪声很大，下标要按标准的矢量分解去理解。");
        let issues = check_lecture_slides(&[s], "general");
        assert!(issues
            .iter()
            .any(|i| i.msg.contains("reading/generation process")));
        let mut t = slide();
        t["title"] = json!("先定位：这一页在推导链中的位置");
        assert!(check_lecture_slides(&[t], "general")
            .iter()
            .any(|i| i.msg.contains("reading/generation process")));
        let mut ok = slide();
        ok["narration"][0]["text"] = json!("课本上的例题用的正是这个结论。");
        assert!(check_lecture_slides(&[ok], "general").is_empty());
    }

    #[test]
    fn reports_bad_targets_params_tex_and_missing_narration() {
        let mut s = slide();
        s["narration"][0]["cues"][1]["target"] = json!("b9");
        s["narration"][0]["cues"][1]["speed"] = json!(5);
        s["narration"][1]["text"] = json!("所以 $A^{-1}$ 存在。");
        let empty = json!({"layout": "title", "title": "封面", "narration": []});
        let issues = check_lecture_slides(&[s, empty], "general");
        let msgs: Vec<&str> = issues.iter().map(|i| i.msg.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("\"b9\"")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("speed")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("LaTeX")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("no narration")), "{msgs:?}");
        // no silent re-targeting: the fallback removes the broken cue only
        assert!(issues.iter().any(|i| matches!(
            i.fix,
            Fix::RemoveCue {
                sentence: 0,
                cue: 1
            }
        )));
    }

    #[test]
    fn display_only_rules() {
        let d = json!({"layout": "figure", "title": "图", "narration": [], "display_only": {"reason": "大图展示"}});
        assert!(check_lecture_slides(std::slice::from_ref(&d), "general").is_empty());
        let issues = check_lecture_slides(&[d.clone(), d], "general");
        assert!(issues.iter().any(|i| i.msg.contains("second display-only")));
    }

    #[test]
    fn unrevealed_and_unordered_steps_are_reported() {
        let mut s = slide();
        s["narration"][1]["cues"] = json!([]);
        let issues = check_lecture_slides(&[s.clone()], "general");
        assert!(issues.iter().any(|i| i.msg.contains("never revealed")));
        s["narration"][1]["cues"] = json!([{"type": "step", "n": 1}]);
        let issues = check_lecture_slides(&[s], "general");
        assert!(issues.iter().any(|i| i.msg.contains("out of order")));
    }

    #[test]
    fn checkpoint_needs_reason() {
        let mut s = slide();
        s["checkpoint"] = json!({"kind": "ask", "question": "为什么？"});
        assert!(check_lecture_slides(&[s.clone()], "general")
            .iter()
            .any(|i| i.msg.contains("reason")));
        s["checkpoint"]["reason"] = json!("易混淆");
        assert!(check_lecture_slides(&[s], "general").is_empty());
    }

    #[test]
    fn fallbacks_remove_only_broken_parts_and_renumber() {
        let mut s = slide();
        s["narration"][0]["cues"][1]["target"] = json!("nope");
        s["builds"][0]["targets"] = json!(["missing"]);
        let mut slides = vec![s];
        let issues = check_lecture_slides(&slides, "general");
        apply_fallbacks(&mut slides, &issues);
        let s = &slides[0];
        // laser cue removed (not re-pointed), build step 1 removed, step 2 renumbered to 1
        assert!(s["narration"][0]["cues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["type"] != "laser"));
        assert_eq!(s["builds"].as_array().unwrap().len(), 1);
        assert_eq!(s["builds"][0]["step"], 1);
        assert!(
            check_lecture_slides(&slides, "general").is_empty(),
            "{:?}",
            check_lecture_slides(&slides, "general")
        );
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
        assert!(check_lecture_slides(&[s.clone()], "general").is_empty());
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
        let issues = check_lecture_slides(&[alone], "general");
        assert!(
            issues
                .iter()
                .any(|i| i.msg.contains("no on-screen text beside")),
            "{issues:?}"
        );
        s["narration"][0]["cues"][1]["target"] = json!("figure:Q");
        assert!(!check_lecture_slides(&[s], "general").is_empty());
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

        // a derivation section only asks for a step reveal, and is satisfied by one
        assert_eq!(
            check_section_visuals(&[table_slide(3)], "suggested").len(),
            1
        );
        let mut built = table_slide(3);
        built["builds"] = json!([{"step": 1, "targets": ["table.r1"]}]);
        assert!(check_section_visuals(&[built], "suggested").is_empty());
        assert!(check_section_visuals(&[table_slide(3)], "none").is_empty());
    }

    #[test]
    fn table_counts_as_text_beside_a_figure() {
        let mut s = table_slide(2);
        s["figure"] = json!({"meta": {"parts": []}});
        let issues = check_lecture_slides(&[s], "general");
        assert!(!issues.iter().any(|i| i.msg.contains("no on-screen text")));
    }
}

#[cfg(test)]
mod figure_order_tests {
    use super::*;

    fn slide(h_anchors: Value, h_laser_first: bool) -> Value {
        let first_cues = if h_laser_first {
            json!([{"type": "step", "n": 1}, {"type": "laser", "target": "figure:h"}])
        } else {
            json!([{"type": "step", "n": 1}])
        };
        json!({
            "layout": "figure", "title": "t", "bullets": ["a", "b"],
            "figure": {"meta": {"parts": [
                {"id": "P", "bbox": [0, 0, 0, 0]},
                {"id": "V", "bbox": [0, 0, 0, 0]},
                {"id": "h", "bbox": [0, 0, 0, 0], "anchors": h_anchors},
                {"id": "lbl", "bbox": [0, 0, 0, 0], "anchors": [["h"]]}
            ]}},
            "builds": [
                {"step": 1, "targets": ["b1"], "effect": "fade"},
                {"step": 2, "targets": ["b2", "figure:V"], "effect": "draw"}
            ],
            "narration": [
                {"text": "一", "cues": first_cues},
                {"text": "二", "cues": [{"type": "step", "n": 2}]}
            ]
        })
    }

    fn step_of(s: &Value, target: &str) -> Option<i64> {
        s["builds"].as_array().unwrap().iter().find_map(|b| {
            b["targets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t == target)
                .then(|| b["step"].as_i64().unwrap())
        })
    }

    #[test]
    fn helper_and_its_label_wait_for_the_vector() {
        let mut s = slide(json!([["V"]]), false);
        let moved = order_figure_builds(&mut s);
        assert_eq!(moved, vec!["h".to_string(), "lbl".to_string()]);
        assert_eq!(step_of(&s, "figure:h"), Some(2));
        assert_eq!(step_of(&s, "figure:lbl"), Some(2));
        assert!(check_lecture_slides(std::slice::from_ref(&s), "general")
            .iter()
            .all(|i| !i.msg.contains("step")));
    }

    #[test]
    fn point_already_drawn_or_pointed_at_keeps_order() {
        // the shared point is also on the static P: nothing to wait for
        let mut s = slide(json!([["P", "V"]]), false);
        assert!(order_figure_builds(&mut s).is_empty());
        // the narration points at h before V appears: leave it visible
        let mut s = slide(json!([["V"]]), true);
        let moved = order_figure_builds(&mut s);
        assert!(!moved.contains(&"h".to_string()));
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
            check_lecture_slides(&[slide(t)], mode)
                .iter()
                .any(|i| i.msg.contains("prerequisite/review slide"))
        };
        assert!(flagged("前置知识：叉乘", "general"));
        assert!(flagged("先回顾一下转动惯量", "practice"));
        assert!(flagged("Prerequisites", "preview"));
        assert!(!flagged("前置知识：叉乘", "review"));
        assert!(!flagged("第一步：写出力矩", "general"));
    }
}
