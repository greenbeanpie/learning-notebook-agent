//! Deterministic consistency of a slide's animation (builds, step cues, laser cues).
//!
//! The model decides *what* is said and shown; this module alone decides *whether the
//! animation hangs together*. [`normalize_animation`] is idempotent and never needs a model
//! round trip; [`animation_violations`] states the invariants it establishes (and is what the
//! tests assert).
//!
//! Playback semantics the rules rely on:
//! - elements that are in no entrance build are visible from the start;
//! - a `{"type":"step","n":k}` cue reveals build step k when its sentence starts; the step cues
//!   of one sentence apply before its laser cues;
//! - a figure part may only appear once what it hangs on is visible: a label waits for the
//!   thing it labels, a projection line for the arrow it is dropped from.

use crate::lecture::slide_element_ids;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

const ENTRANCE: [&str; 4] = ["appear", "fade", "fly_up", "draw"];
const EFFECTS: [&str; 6] = ["appear", "fade", "fly_up", "draw", "emphasis", "dim_others"];
/// A part covering more than this share of the figure is context (a plane, a body outline),
/// never something a label or line waits for.
const BIG_PART_AREA: f64 = 0.06;
/// Parts whose bounding-box diagonal (share of the figure) is at most this are helpers; longer
/// ones (an axis, a body outline) are context that stays visible.
const SMALL_PART_DIAG: f64 = 0.35;
const LABEL_PREFIXES: [&str; 12] = [
    "lbl_", "label_", "lab_", "txt_", "text_", "l_", "t_", "lbl", "label", "lab", "txt", "text",
];
const LABEL_SUFFIXES: [&str; 10] = [
    "_label", "_lbl", "_lab", "_txt", "_text", "_t", "label", "lbl", "lab", "txt",
];

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_entrance(b: &Value) -> bool {
    ENTRANCE.contains(&b.get("effect").and_then(Value::as_str).unwrap_or("fade"))
}

fn builds_of(slide: &Value) -> Vec<Value> {
    slide
        .get("builds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn step_of(b: &Value) -> i64 {
    b.get("step").and_then(Value::as_i64).unwrap_or(0)
}

fn targets_of(b: &Value) -> Vec<String> {
    b.get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(String::from))
        .collect()
}

/// The step at which each target first appears (entrance builds only).
fn listed_steps(slide: &Value) -> HashMap<String, i64> {
    let mut m: HashMap<String, i64> = HashMap::new();
    for b in builds_of(slide).iter().filter(|b| is_entrance(b)) {
        let k = step_of(b);
        for t in targets_of(b) {
            let e = m.entry(t).or_insert(k);
            *e = (*e).min(k);
        }
    }
    m
}

fn spoken_sentences(slide: &Value) -> Vec<usize> {
    slide
        .get("narration")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, n)| !s(n, "text").trim().is_empty())
        .map(|(i, _)| i)
        .collect()
}

fn cues_of(n: &Value) -> Vec<Value> {
    n.get("cues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The build step in force while each sentence is spoken (its own step cues included).
fn sentence_steps(slide: &Value) -> Vec<i64> {
    let mut cur = 0;
    slide
        .get("narration")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|n| {
            for c in cues_of(n) {
                if s(&c, "type") == "step" {
                    cur = cur.max(c.get("n").and_then(Value::as_i64).unwrap_or(0));
                }
            }
            cur
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// what a figure part hangs on

/// For every figure part, the groups of parts it waits for (it may appear once at least one
/// member of every group is visible). Sources, strongest first: the figure's explicit `for`,
/// a label naming convention / matching text, then the engine's geometric attachment. Edges
/// that would make two parts wait for each other are dropped.
pub(crate) fn part_dependencies(slide: &Value) -> HashMap<String, Vec<Vec<String>>> {
    let parts = slide
        .pointer("/figure/meta/parts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids: Vec<String> = parts.iter().map(|p| s(p, "id")).collect();
    let idset: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let lower: HashMap<String, &str> = ids.iter().map(|i| (i.to_lowercase(), i.as_str())).collect();
    let big: HashSet<String> = parts
        .iter()
        .filter(|p| {
            p.get("bbox").and_then(Value::as_array).is_some_and(|b| {
                b.len() == 4
                    && b[2].as_f64().unwrap_or(0.0) * b[3].as_f64().unwrap_or(0.0) > BIG_PART_AREA
            })
        })
        .map(|p| s(p, "id"))
        .collect();

    // what the figure spec says about each element
    struct El {
        ty: String,
        text: String,
        label: String,
        owner: String,
    }
    fn walk(els: &[Value], out: &mut HashMap<String, El>) {
        for e in els {
            let id = s(e, "id");
            if !id.is_empty() {
                out.insert(
                    id,
                    El {
                        ty: s(e, "type"),
                        text: s(e, "text"),
                        label: s(e, "label"),
                        owner: s(e, "for"),
                    },
                );
            }
            if let Some(ch) = e.get("children").and_then(Value::as_array) {
                walk(ch, out);
            }
        }
    }
    let mut spec: HashMap<String, El> = HashMap::new();
    if let Some(els) = slide
        .pointer("/figure/spec/elements")
        .and_then(Value::as_array)
    {
        walk(els, &mut spec);
    }
    let has_spec = !spec.is_empty();

    let mut owner: HashMap<String, String> = HashMap::new();
    for id in &ids {
        let el = spec.get(id);
        // a. explicit
        if let Some(o) = el
            .map(|e| e.owner.as_str())
            .filter(|o| idset.contains(o) && *o != id)
        {
            owner.insert(id.clone(), o.to_string());
            continue;
        }
        let is_text = match el {
            Some(e) => e.ty == "text",
            None => !has_spec,
        };
        if !is_text {
            continue;
        }
        // b. naming convention
        let mut found: Option<String> = None;
        for p in LABEL_PREFIXES {
            if let Some(x) = id.strip_prefix(p).filter(|x| !x.is_empty()) {
                if let Some(t) = idset
                    .get(x)
                    .map(|t| t.to_string())
                    .or_else(|| lower.get(&x.to_lowercase()).map(|t| t.to_string()))
                {
                    if &t != id {
                        found = Some(t);
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            for p in LABEL_SUFFIXES {
                if let Some(x) = id.strip_suffix(p).filter(|x| !x.is_empty()) {
                    if let Some(t) = idset
                        .get(x)
                        .map(|t| t.to_string())
                        .or_else(|| lower.get(&x.to_lowercase()).map(|t| t.to_string()))
                    {
                        if &t != id {
                            found = Some(t);
                            break;
                        }
                    }
                }
            }
        }
        // c. the text reads like another part's label or id
        if found.is_none() {
            if let Some(e) = el {
                let t = e.text.trim();
                if !t.is_empty() {
                    let hits: Vec<&String> = ids
                        .iter()
                        .filter(|o| *o != id)
                        .filter(|o| {
                            o.eq_ignore_ascii_case(t)
                                || spec.get(*o).is_some_and(|x| x.label.trim() == t)
                        })
                        .collect();
                    if hits.len() == 1 {
                        found = Some(hits[0].clone());
                    }
                }
            }
        }
        if let Some(f) = found {
            owner.insert(id.clone(), f);
        }
    }

    // build the graph: explicit owners first, then geometry (skipping edges that close a cycle)
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    let mut deps: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for (p, o) in &owner {
        adj.entry(p.clone()).or_default().insert(o.clone());
        deps.insert(p.clone(), vec![vec![o.clone()]]);
    }
    let reaches = |adj: &HashMap<String, HashSet<String>>, from: &str, to: &str| -> bool {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !seen.insert(n) {
                continue;
            }
            if let Some(next) = adj.get(n) {
                stack.extend(next.iter().map(String::as_str));
            }
        }
        false
    };
    // two parts that touch each other have no natural order: the smaller one (an arrow tip's
    // projection, a mark) waits for the larger; equal size means neither waits
    let size: HashMap<String, f64> = parts
        .iter()
        .map(|p| {
            let b = p
                .get("bbox")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let (w, h) = (
                b.get(2).and_then(Value::as_f64).unwrap_or(0.0),
                b.get(3).and_then(Value::as_f64).unwrap_or(0.0),
            );
            (s(p, "id"), (w * w + h * h).sqrt())
        })
        .collect();
    let touching: HashMap<String, HashSet<String>> = parts
        .iter()
        .map(|p| {
            let all = p
                .get("anchors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|g| g.as_array().cloned().unwrap_or_default())
                .filter_map(|m| m.as_str().map(String::from))
                .collect();
            (s(p, "id"), all)
        })
        .collect();
    let listed = listed_steps(slide);
    let hidden_at_start = |x: &str| listed.get(&format!("figure:{x}")).copied().unwrap_or(0) > 0;
    let mutual_loses = |id: &str, m: &str| -> bool {
        touching.get(m).is_some_and(|t| t.contains(id)) && {
            let (a, b) = (
                size.get(id).copied().unwrap_or(0.0),
                size.get(m).copied().unwrap_or(0.0),
            );
            // equal size: one that no build lists (visible from the start) still waits for
            // a partner that is revealed later (the second leg of a right-angle mark)
            let tie = a <= b * 1.001;
            a >= b * 0.999 && !(tie && !hidden_at_start(id) && hidden_at_start(m))
        }
    };
    for p in &parts {
        let id = s(p, "id");
        if owner.contains_key(&id) {
            continue;
        }
        let Some(groups) = p.get("anchors").and_then(Value::as_array) else {
            continue;
        };
        let mut kept: Vec<Vec<String>> = Vec::new();
        for g in groups {
            let members: Vec<String> = g
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str().map(String::from))
                .filter(|m| *m != id && idset.contains(m.as_str()) && !big.contains(m))
                .filter(|m| !mutual_loses(&id, m))
                .filter(|m| !reaches(&adj, m, &id))
                .collect();
            if members.is_empty() {
                continue;
            }
            for m in &members {
                adj.entry(id.clone()).or_default().insert(m.clone());
            }
            if !kept.contains(&members) {
                kept.push(members);
            }
        }
        if !kept.is_empty() {
            deps.insert(id, kept);
        }
    }
    // a small helper part no build lists, touching only parts that are revealed later (a
    // projection line, the action line of a force), would hang in the air from the start:
    // it appears with the first of them
    for p in &parts {
        let id = s(p, "id");
        if deps.contains_key(&id)
            || hidden_at_start(&id)
            || big.contains(&id)
            || size.get(&id).copied().unwrap_or(1.0) > SMALL_PART_DIAG
        {
            continue;
        }
        let near: Vec<String> = touching
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|m| **m != id && idset.contains(m.as_str()) && !big.contains(*m))
            .cloned()
            .collect();
        if near.is_empty()
            || !near.iter().all(|m| hidden_at_start(m))
            || near.iter().any(|m| reaches(&adj, m, &id))
        {
            continue;
        }
        let mut near = near;
        near.sort();
        for m in &near {
            adj.entry(id.clone()).or_default().insert(m.clone());
        }
        deps.insert(id, vec![near]);
    }
    deps
}

/// The step a part needs at least: every group needs one visible member.
fn need_of(groups: &[Vec<String>], step: &HashMap<String, i64>) -> i64 {
    groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|m| step.get(m).copied().unwrap_or(0))
                .min()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

/// Lowers `id` to appear at `to` at the latest, and (recursively) one member of every group it
/// waits for.
fn lower_part(
    id: &str,
    to: i64,
    step: &mut HashMap<String, i64>,
    deps: &HashMap<String, Vec<Vec<String>>>,
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    if step.get(id).copied().unwrap_or(0) > to {
        step.insert(id.to_string(), to);
    }
    for g in deps.get(id).into_iter().flatten() {
        let best = g
            .iter()
            .min_by_key(|m| step.get(*m).copied().unwrap_or(0))
            .cloned();
        if let Some(m) = best {
            if step.get(&m).copied().unwrap_or(0) > to {
                lower_part(&m, to, step, deps, depth + 1);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// normalisation

/// Brings a lecture slide's animation into a consistent state. Returns what was changed
/// (empty when nothing had to be).
pub fn normalize_animation(slide: &mut Value) -> Vec<String> {
    normalize(slide, true)
}

/// Same for a deck slide (no narration, so no step cues or pointing).
pub fn normalize_deck_animation(slide: &mut Value) -> Vec<String> {
    normalize(slide, false)
}

fn normalize(slide: &mut Value, lecture: bool) -> Vec<String> {
    let mut log = Vec::new();
    clean(slide, lecture, &mut log);
    if lecture {
        reveal_pointed_text(slide, &mut log);
    }
    sequence_steps(slide, lecture, &mut log);
    if lecture {
        delay_early_reveals(slide, &mut log);
    }
    if close_dependencies(slide, lecture, &mut log) {
        // a step may have lost all its content: renumber and re-place the cues
        sequence_steps(slide, lecture, &mut log);
    }
    if lecture {
        point_at_new_content(slide, &mut log);
    }
    log
}

/// A sentence that brings something new onto the screen points the laser at it, whatever the
/// author wrote: the reveal and the pointer belong together, and a viewer stepping through by
/// hand then always sees a mark where the content just appeared. (The first revealed element
/// is the one pointed at; labels and helper lines come with it.)
fn point_at_new_content(slide: &mut Value, log: &mut Vec<String>) {
    let builds = builds_of(slide);
    let mut added: Vec<String> = Vec::new();
    let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) else {
        return;
    };
    for n in ns.iter_mut() {
        if s(n, "text").trim().is_empty() {
            continue;
        }
        let cues = cues_of(n);
        if cues.iter().any(|c| s(c, "type") == "laser") {
            continue;
        }
        let first = cues
            .iter()
            .filter(|c| s(c, "type") == "step")
            .filter_map(|c| c.get("n").and_then(Value::as_i64))
            .find_map(|k| {
                builds
                    .iter()
                    .filter(|b| step_of(b) == k && is_entrance(b))
                    .find_map(|b| targets_of(b).into_iter().next())
            });
        if let Some(t) = first {
            let mut cues = cues;
            cues.push(json!({"type": "laser", "target": t, "shape": "ellipse"}));
            n["cues"] = json!(cues);
            added.push(t);
        }
    }
    if !added.is_empty() {
        log.push(format!(
            "{} 句只有分步揭示没有指向，已补上对新出现内容的激光指向：{}",
            added.len(),
            added.join("、")
        ));
    }
}

/// A bullet, formula, callout, table row or figure part that no build lists would sit on screen
/// from the start although the narration only brings it up later. It appears on the first
/// sentence that points at it instead. A slide with builds always gets this; a slide without
/// any needs at least two such elements, otherwise it stays static (nothing to reveal in turn).
fn reveal_pointed_text(slide: &mut Value, log: &mut Vec<String>) {
    fn revealable(t: &str) -> bool {
        let digits = |r: &str| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit());
        t == "formula"
            || t == "callout"
            || t.strip_prefix('b').is_some_and(digits)
            || t.strip_prefix("table.r").is_some_and(digits)
            || t.strip_prefix("figure:").is_some_and(|r| !r.is_empty())
    }
    let listed = listed_steps(slide);
    let spoken = spoken_sentences(slide);
    if spoken.is_empty() {
        return;
    }
    let first_spoken = spoken[0];
    let mut later: Vec<(usize, String)> = Vec::new();
    let mut distinct: HashSet<String> = HashSet::new();
    let narration = slide
        .get("narration")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (j, n) in narration.iter().enumerate() {
        for c in cues_of(n) {
            let t = s(&c, "target");
            if s(&c, "type") == "laser" && revealable(&t) && !listed.contains_key(&t) {
                distinct.insert(t.clone());
                if j > first_spoken && !later.iter().any(|(_, x)| *x == t) {
                    later.push((j, t));
                }
            }
        }
    }
    if later.is_empty() || (listed.is_empty() && distinct.len() < 2) {
        return;
    }
    let mut next = builds_of(slide).iter().map(step_of).max().unwrap_or(0);
    let mut by_sentence: Vec<(usize, i64)> = Vec::new();
    if slide.get("builds").and_then(Value::as_array).is_none() {
        slide["builds"] = json!([]);
    }
    for (j, t) in later {
        let step = match by_sentence.iter().find(|(sj, _)| *sj == j) {
            Some((_, st)) => *st,
            None => {
                next += 1;
                by_sentence.push((j, next));
                if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
                    if let Some(cues) = ns[j].get_mut("cues").and_then(Value::as_array_mut) {
                        cues.insert(0, json!({"type": "step", "n": next}));
                    } else {
                        ns[j]["cues"] = json!([{"type": "step", "n": next}]);
                    }
                }
                if let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) {
                    bs.push(json!({"step": next, "targets": [], "effect": "fade"}));
                }
                next
            }
        };
        if let Some(b) = slide
            .get_mut("builds")
            .and_then(Value::as_array_mut)
            .and_then(|bs| bs.iter_mut().find(|b| step_of(b) == step))
        {
            if let Some(ts) = b.get_mut("targets").and_then(Value::as_array_mut) {
                ts.push(json!(t));
            }
        }
        log.push(format!("{t} 在旁白第一次指向它的那一句才出现"));
    }
}

fn clamp_num(c: &mut Value, k: &str, lo: f64, hi: f64) -> bool {
    let Some(x) = c.get(k).and_then(Value::as_f64) else {
        if c.get(k).is_some_and(|v| !v.is_null()) {
            if let Some(o) = c.as_object_mut() {
                o.remove(k);
            }
            return true;
        }
        return false;
    };
    let y = x.clamp(lo, hi);
    if (y - x).abs() > f64::EPSILON {
        c[k] = json!(y);
        return true;
    }
    false
}

/// Drops what cannot work (unknown targets/effects/cues) and clamps pointer parameters.
fn clean(slide: &mut Value, lecture: bool, log: &mut Vec<String>) {
    let ids: HashSet<String> = slide_element_ids(slide).into_iter().collect();
    let mut dropped = 0usize;
    let revealed: HashSet<String> = listed_steps(slide).into_keys().collect();
    if let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) {
        for b in bs.iter_mut() {
            // "emphasising" a conclusion box nothing reveals would show it from the first
            // sentence on: it is revealed instead
            if !is_entrance(b) && targets_of(b) == ["callout"] && !revealed.contains("callout") {
                b["effect"] = json!("appear");
            }
            if let Some(t) = b.get_mut("targets").and_then(Value::as_array_mut) {
                let before = t.len();
                t.retain(|x| x.as_str().is_some_and(|x| ids.contains(x)));
                dropped += before - t.len();
            }
            if let Some(e) = b.get("effect").and_then(Value::as_str) {
                if !EFFECTS.contains(&e) {
                    b["effect"] = json!("fade");
                }
            }
        }
        let before = bs.len();
        bs.retain(|b| step_of(b) >= 1 && !targets_of(b).is_empty());
        dropped += before - bs.len();
    }
    if lecture {
        if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
            for n in ns.iter_mut() {
                let Some(cues) = n.get_mut("cues").and_then(Value::as_array_mut) else {
                    continue;
                };
                let before = cues.len();
                cues.retain(|c| match s(c, "type").as_str() {
                    "laser" => ids.contains(&s(c, "target")),
                    "step" => c.get("n").and_then(Value::as_i64).is_some_and(|k| k >= 1),
                    _ => false,
                });
                dropped += before - cues.len();
                for c in cues.iter_mut().filter(|c| s(c, "type") == "laser") {
                    let shape = s(c, "shape");
                    if shape != "circle" && shape != "ellipse" {
                        c["shape"] = json!("ellipse");
                    }
                    for (k, lo, hi) in [
                        ("scale", 1.0, 1.6),
                        ("speed", 0.3, 1.5),
                        ("angle_deg", -90.0, 90.0),
                        ("duration_ms", 800.0, 6000.0),
                    ] {
                        clamp_num(c, k, lo, hi);
                    }
                }
            }
        }
    }
    if dropped > 0 {
        log.push(format!("移除了 {dropped} 处指向不存在内容的动画项"));
    }
}

fn find_step_cue(ns: &[Value], step: i64) -> Option<(usize, usize)> {
    ns.iter().enumerate().find_map(|(si, n)| {
        n.get("cues")?
            .as_array()?
            .iter()
            .position(|c| {
                s(c, "type") == "step" && c.get("n").and_then(Value::as_i64) == Some(step)
            })
            .map(|ci| (si, ci))
    })
}

fn insert_cue(ns: &mut [Value], si: usize, ci: usize, cue: Value) {
    match ns[si].get_mut("cues").and_then(Value::as_array_mut) {
        Some(cues) => cues.insert(ci.min(cues.len()), cue),
        None => ns[si]["cues"] = json!([cue]),
    }
}

/// Step numbers follow the order in which the narration reveals them (1..K, each once); a step
/// nobody cues is cued where the narration first points at its content.
fn sequence_steps(slide: &mut Value, lecture: bool, log: &mut Vec<String>) {
    let builds = builds_of(slide);
    let steps: BTreeSet<i64> = builds.iter().map(step_of).filter(|k| *k >= 1).collect();
    if !lecture {
        remap_builds(slide, &steps.iter().copied().collect::<Vec<_>>(), log);
        return;
    }
    let spoken = spoken_sentences(slide);
    if spoken.is_empty() {
        // nobody reveals anything: a display-only slide shows everything at once
        if let Some(o) = slide.as_object_mut() {
            if o.remove("builds").is_some() {
                log.push("没有旁白，分步显示已取消".into());
            }
        }
        strip_step_cues(slide);
        return;
    }
    if steps.is_empty() {
        strip_step_cues(slide);
        return;
    }
    let mut targets_by_step: HashMap<i64, HashSet<String>> = HashMap::new();
    for b in &builds {
        targets_by_step
            .entry(step_of(b))
            .or_default()
            .extend(targets_of(b));
    }
    let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) else {
        return;
    };
    let spoken_set: HashSet<usize> = spoken.iter().copied().collect();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut changed = 0usize;
    for (si, n) in ns.iter_mut().enumerate() {
        if let Some(cues) = n.get_mut("cues").and_then(Value::as_array_mut) {
            let before = cues.len();
            cues.retain(|c| {
                if s(c, "type") != "step" {
                    return true;
                }
                let k = c.get("n").and_then(Value::as_i64).unwrap_or(0);
                spoken_set.contains(&si) && steps.contains(&k) && seen.insert(k)
            });
            changed += before - cues.len();
        }
    }
    let uncued: Vec<i64> = steps.difference(&seen).copied().collect();
    if seen.is_empty() {
        let per_sentence = |k: usize| spoken[k * spoken.len() / uncued.len()];
        let mut at: HashMap<usize, Vec<i64>> = HashMap::new();
        for (k, step) in uncued.iter().enumerate() {
            at.entry(per_sentence(k)).or_default().push(*step);
        }
        for (si, list) in at {
            for (j, step) in list.into_iter().enumerate() {
                insert_cue(ns, si, j, json!({"type": "step", "n": step}));
                changed += 1;
            }
        }
    } else {
        let mut placed_front: HashMap<usize, usize> = HashMap::new();
        for u in uncued {
            let tg = targets_by_step.get(&u).cloned().unwrap_or_default();
            let pointed = spoken.iter().copied().find(|si| {
                cues_of(&ns[*si])
                    .iter()
                    .any(|c| s(c, "type") == "laser" && tg.contains(&s(c, "target")))
            });
            let cue = json!({"type": "step", "n": u});
            if let Some(si) = pointed {
                let j = placed_front.entry(si).or_insert(0);
                insert_cue(ns, si, *j, cue);
                *j += 1;
            } else {
                let prev = seen.iter().rev().find(|p| **p < u).copied();
                let next = seen.iter().find(|q| **q > u).copied();
                let at = match (prev, next) {
                    (Some(p), _) => find_step_cue(ns, p).map(|(si, ci)| (si, ci + 1)),
                    (None, Some(q)) => find_step_cue(ns, q),
                    (None, None) => None,
                };
                let (si, ci) = at.unwrap_or((spoken[0], 0));
                insert_cue(ns, si, ci, cue);
            }
            seen.insert(u);
            changed += 1;
        }
    }
    // the final order is the order the cues are met in
    let order: Vec<i64> = ns
        .iter()
        .flat_map(cues_of)
        .filter(|c| s(c, "type") == "step")
        .filter_map(|c| c.get("n").and_then(Value::as_i64))
        .collect();
    if changed > 0 {
        log.push(format!("补齐/整理了 {changed} 处分步提示"));
    }
    remap_builds(slide, &order, log);
}

/// A step is revealed no earlier than the first sentence that points at its content: showing
/// the answer of the next sentence early spoils it. The cue moves forward to that sentence,
/// never past the cue of the following step.
fn delay_early_reveals(slide: &mut Value, log: &mut Vec<String>) {
    let builds = builds_of(slide);
    let mut targets: HashMap<i64, HashSet<String>> = HashMap::new();
    for b in builds.iter().filter(|b| is_entrance(b)) {
        targets.entry(step_of(b)).or_default().extend(targets_of(b));
    }
    let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) else {
        return;
    };
    let mut steps: Vec<i64> = targets.keys().copied().collect();
    steps.sort();
    let mut moved = 0usize;
    for k in steps {
        let Some((from, ci)) = find_step_cue(ns, k) else {
            continue;
        };
        let tg = &targets[&k];
        let Some(first) = (0..ns.len()).find(|i| {
            cues_of(&ns[*i])
                .iter()
                .any(|c| s(c, "type") == "laser" && tg.contains(&s(c, "target")))
        }) else {
            continue;
        };
        if first <= from {
            continue;
        }
        // not past the next step's cue
        let next_at = find_step_cue(ns, k + 1).map(|(si, _)| si);
        let to = next_at.map_or(first, |n| first.min(n));
        if to <= from {
            continue;
        }
        let cue = match ns[from].get_mut("cues").and_then(Value::as_array_mut) {
            Some(cues) if ci < cues.len() => cues.remove(ci),
            _ => continue,
        };
        // after the step cues of earlier steps already in that sentence
        let at = cues_of(&ns[to])
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                s(c, "type") == "step" && c.get("n").and_then(Value::as_i64).unwrap_or(0) < k
            })
            .map(|(i, _)| i + 1)
            .max()
            .unwrap_or(0);
        insert_cue(ns, to, at, cue);
        moved += 1;
    }
    if moved > 0 {
        log.push(format!(
            "{moved} 个分步提示推迟到旁白开始指向其内容的那一句"
        ));
    }
}

fn strip_step_cues(slide: &mut Value) {
    if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
        for n in ns.iter_mut() {
            if let Some(cues) = n.get_mut("cues").and_then(Value::as_array_mut) {
                cues.retain(|c| s(c, "type") != "step");
            }
        }
    }
}

/// Renumbers steps so that `order` (old numbers, in reveal order) becomes 1..K; builds and
/// step cues both follow.
fn remap_builds(slide: &mut Value, order: &[i64], log: &mut Vec<String>) {
    let mut map: HashMap<i64, i64> = HashMap::new();
    for k in order {
        let next = map.len() as i64 + 1;
        map.entry(*k).or_insert(next);
    }
    if map.iter().all(|(k, v)| k == v) {
        return;
    }
    if let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) {
        for b in bs.iter_mut() {
            if let Some(k) = b.get("step").and_then(Value::as_i64) {
                if let Some(n) = map.get(&k) {
                    b["step"] = json!(n);
                }
            }
        }
    }
    if let Some(ns) = slide.get_mut("narration").and_then(Value::as_array_mut) {
        for n in ns.iter_mut() {
            if let Some(cues) = n.get_mut("cues").and_then(Value::as_array_mut) {
                for c in cues.iter_mut().filter(|c| s(c, "type") == "step") {
                    if let Some(k) = c.get("n").and_then(Value::as_i64) {
                        if let Some(n) = map.get(&k) {
                            c["n"] = json!(n);
                        }
                    }
                }
            }
        }
    }
    log.push("分步编号已按旁白出现顺序重排".into());
}

/// Moves `target` so that it first appears at `to` (0 = visible from the start).
fn retarget(slide: &mut Value, target: &str, to: i64) {
    let Some(bs) = slide.get_mut("builds").and_then(Value::as_array_mut) else {
        return;
    };
    for b in bs.iter_mut().filter(|b| is_entrance(b)) {
        if let Some(t) = b.get_mut("targets").and_then(Value::as_array_mut) {
            t.retain(|x| x.as_str() != Some(target));
        }
    }
    if to >= 1 {
        match bs.iter_mut().find(|b| is_entrance(b) && step_of(b) == to) {
            Some(b) => {
                if let Some(t) = b.get_mut("targets").and_then(Value::as_array_mut) {
                    t.push(json!(target));
                }
            }
            None => bs.push(json!({"step": to, "targets": [target], "effect": "fade"})),
        }
    }
    bs.retain(|b| !targets_of(b).is_empty());
}

/// Parts wait for what they hang on; whatever the narration points at is visible by then.
/// Returns true when a step lost all its content.
fn close_dependencies(slide: &mut Value, lecture: bool, log: &mut Vec<String>) -> bool {
    let listed = listed_steps(slide);
    if listed.is_empty() && builds_of(slide).is_empty() {
        return false;
    }
    let whole_figure = listed.contains_key("figure");
    let deps = if whole_figure {
        HashMap::new()
    } else {
        part_dependencies(slide)
    };
    let part_ids: Vec<String> = slide
        .pointer("/figure/meta/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| s(p, "id"))
        .filter(|i| i != "title" && i != "grid")
        .collect();
    // one table for figure parts (by id) and for every other element (by target string)
    let mut step: HashMap<String, i64> = HashMap::new();
    if !whole_figure {
        for id in &part_ids {
            step.insert(
                id.clone(),
                listed.get(&format!("figure:{id}")).copied().unwrap_or(0),
            );
        }
    }
    let original = step.clone();
    let mut other: HashMap<String, i64> = listed
        .iter()
        .filter(|(t, _)| whole_figure || !t.starts_with("figure:"))
        .map(|(t, k)| (t.clone(), *k))
        .collect();
    let other_original = other.clone();

    // raise: a part is never earlier than what it waits for
    for _ in 0..=part_ids.len() {
        let mut changed = false;
        for (id, groups) in &deps {
            let need = need_of(groups, &step);
            if step.get(id).copied().unwrap_or(0) < need {
                step.insert(id.clone(), need);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // lower: what a sentence points at is visible in that sentence
    if lecture {
        let cur = sentence_steps(slide);
        let ns: Vec<Value> = slide
            .get("narration")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (i, n) in ns.iter().enumerate() {
            let c = cur.get(i).copied().unwrap_or(0);
            for cue in cues_of(n).iter().filter(|c| s(c, "type") == "laser") {
                let t = s(cue, "target");
                match t.strip_prefix("figure:") {
                    Some(id) if !whole_figure => {
                        if step.get(id).copied().unwrap_or(0) > c {
                            lower_part(id, c, &mut step, &deps, 0);
                        }
                    }
                    _ => {
                        if other.get(&t).copied().unwrap_or(0) > c {
                            other.insert(t.clone(), c);
                        }
                    }
                }
            }
        }
    }
    // write back what moved
    let mut moved: Vec<String> = Vec::new();
    let mut lost = false;
    let steps_before: BTreeSet<i64> = builds_of(slide).iter().map(step_of).collect();
    for id in &part_ids {
        let (a, b) = (
            original.get(id).copied().unwrap_or(0),
            step.get(id).copied().unwrap_or(0),
        );
        if a != b {
            retarget(slide, &format!("figure:{id}"), b);
            moved.push(id.clone());
        }
    }
    for (t, k) in &other {
        if other_original.get(t).copied().unwrap_or(0) != *k {
            retarget(slide, t, *k);
            moved.push(t.clone());
        }
    }
    let steps_after: BTreeSet<i64> = builds_of(slide).iter().map(step_of).collect();
    if steps_before != steps_after {
        lost = true;
    }
    if !moved.is_empty() {
        moved.sort();
        log.push(format!("按依附关系调整了出现时机：{}", moved.join("、")));
    }
    lost
}

// ---------------------------------------------------------------------------------------------
// invariants

/// Everything still wrong with a slide's animation after [`normalize_animation`] (empty when
/// it is consistent). Used by tests and for diagnostics, never to send the model back.
pub fn animation_violations(slide: &Value) -> Vec<String> {
    let mut v = Vec::new();
    let listed = listed_steps(slide);
    let whole = listed.get("figure").copied().unwrap_or(0);
    let eff = |t: &str| -> i64 {
        let own = listed.get(t).copied().unwrap_or(0);
        if t.starts_with("figure:") {
            own.max(whole)
        } else {
            own
        }
    };
    let builds = builds_of(slide);
    let max_step = builds.iter().map(step_of).max().unwrap_or(0);

    if whole == 0 {
        let step: HashMap<String, i64> = slide
            .pointer("/figure/meta/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| s(p, "id"))
            .map(|id| {
                let k = eff(&format!("figure:{id}"));
                (id, k)
            })
            .collect();
        let mut deps: Vec<(String, Vec<Vec<String>>)> =
            part_dependencies(slide).into_iter().collect();
        deps.sort();
        for (id, groups) in deps {
            let need = need_of(&groups, &step);
            let have = step.get(&id).copied().unwrap_or(0);
            if have < need {
                v.push(format!(
                    "part {id} appears at step {have} before what it hangs on (step {need})"
                ));
            }
        }
    }

    let cur = sentence_steps(slide);
    let ns: Vec<Value> = slide
        .get("narration")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut order: Vec<i64> = Vec::new();
    for (i, n) in ns.iter().enumerate() {
        for c in cues_of(n) {
            match s(&c, "type").as_str() {
                "laser" => {
                    let t = s(&c, "target");
                    let need = eff(&t);
                    let have = cur.get(i).copied().unwrap_or(0);
                    if need > have {
                        v.push(format!(
                            "sentence {}: laser on {t} before it appears (step {need} > {have})",
                            i + 1
                        ));
                    }
                }
                "step" => order.push(c.get("n").and_then(Value::as_i64).unwrap_or(0)),
                _ => {}
            }
        }
    }
    let spoken = !spoken_sentences(slide).is_empty();
    if max_step == 0 {
        if !order.is_empty() {
            v.push("step cues without builds".into());
        }
    } else if spoken {
        let want: Vec<i64> = (1..=max_step).collect();
        if order != want {
            v.push(format!(
                "step cues {order:?} are not 1..{max_step} once each in order"
            ));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(id: &str, anchors: Value) -> Value {
        let mut p = json!({"id": id, "bbox": [0.1, 0.1, 0.1, 0.02], "angle_deg": 0.0});
        if !anchors.is_null() {
            p["anchors"] = anchors;
        }
        p
    }

    fn text(id: &str, t: &str) -> Value {
        json!({"id": id, "type": "text", "text": t})
    }

    fn seg(id: &str) -> Value {
        json!({"id": id, "type": "vector"})
    }

    fn slide(parts: Vec<Value>, elements: Vec<Value>, builds: Value, narration: Value) -> Value {
        json!({
            "title": "t",
            "figure": {"kind": "diagram", "spec": {"elements": elements}, "meta": {"parts": parts}},
            "builds": builds,
            "narration": narration,
        })
    }

    fn sent(text: &str, cues: Value) -> Value {
        json!({"text": text, "cues": cues})
    }

    fn step_of_part(slide: &Value, id: &str) -> i64 {
        listed_steps(slide)
            .get(&format!("figure:{id}"))
            .copied()
            .unwrap_or(0)
    }

    #[test]
    fn unlisted_label_follows_the_thing_it_labels() {
        // like the real lecture page: "z" and "r_iz" labels listed nowhere, their lines late
        let mut s = slide(
            vec![
                part("zaxis", Value::Null),
                part("riz", Value::Null),
                part("ri", Value::Null),
                part("l_riz", Value::Null),
                part("lbl_z", Value::Null),
            ],
            vec![
                seg("zaxis"),
                seg("riz"),
                seg("ri"),
                text("l_riz", "r_iz"),
                text("lbl_z", "z"),
            ],
            json!([
                {"step": 1, "targets": ["figure:riz"], "effect": "fade"},
                {"step": 2, "targets": ["figure:ri"], "effect": "fade"}
            ]),
            json!([
                sent("a", json!([{"type": "step", "n": 1}])),
                sent("b", json!([{"type": "step", "n": 2}])),
            ]),
        );
        let log = normalize_animation(&mut s);
        assert!(!log.is_empty());
        assert_eq!(step_of_part(&s, "l_riz"), 1, "{s}");
        assert_eq!(step_of_part(&s, "riz"), 1);
        assert_eq!(step_of_part(&s, "zaxis"), 0);
        // z is a plain text on the axis named z... no part "z": it stays visible
        assert!(
            animation_violations(&s).is_empty(),
            "{:?}",
            animation_violations(&s)
        );
        assert!(normalize_animation(&mut s.clone()).is_empty());
    }

    #[test]
    fn label_listed_too_early_waits_for_its_owner() {
        let mut s = slide(
            vec![part("v", Value::Null), part("lbl_v", Value::Null)],
            vec![seg("v"), text("lbl_v", "v")],
            json!([
                {"step": 1, "targets": ["figure:lbl_v"], "effect": "fade"},
                {"step": 2, "targets": ["figure:v"], "effect": "fade"}
            ]),
            json!([
                sent("a", json!([{"type": "step", "n": 1}])),
                sent("b", json!([{"type": "step", "n": 2}])),
            ]),
        );
        normalize_animation(&mut s);
        // it waits for v; step 1 is then empty and the steps are renumbered
        assert_eq!(step_of_part(&s, "lbl_v"), step_of_part(&s, "v"), "{s}");
        assert_eq!(step_of_part(&s, "v"), 1);
        assert!(animation_violations(&s).is_empty());
    }

    #[test]
    fn explicit_for_beats_naming_and_geometry() {
        let mut s = slide(
            vec![
                part("a", Value::Null),
                part("b", Value::Null),
                part("lbl_a", json!([["a"]])),
            ],
            vec![
                seg("a"),
                seg("b"),
                json!({"id": "lbl_a", "type": "text", "text": "x", "for": "b"}),
            ],
            json!([
                {"step": 1, "targets": ["figure:a"], "effect": "fade"},
                {"step": 2, "targets": ["figure:b"], "effect": "fade"}
            ]),
            json!([
                sent("a", json!([{"type": "step", "n": 1}])),
                sent("b", json!([{"type": "step", "n": 2}])),
            ]),
        );
        normalize_animation(&mut s);
        assert_eq!(step_of_part(&s, "lbl_a"), 2, "{s}");
    }

    #[test]
    fn pointing_pulls_the_target_and_its_deps_forward() {
        let mut s = slide(
            vec![part("v", Value::Null), part("lbl_v", Value::Null)],
            vec![seg("v"), text("lbl_v", "v")],
            json!([
                {"step": 1, "targets": ["b1"], "effect": "fade"},
                {"step": 2, "targets": ["figure:v"], "effect": "fade"}
            ]),
            json!([
                sent("a", json!([{"type": "step", "n": 1}])),
                sent(
                    "b",
                    json!([{"type": "step", "n": 2}, {"type": "laser", "target": "figure:lbl_v"}])
                ),
            ]),
        );
        s["bullets"] = json!(["x"]);
        normalize_animation(&mut s);
        assert!(
            animation_violations(&s).is_empty(),
            "{:?} {s}",
            animation_violations(&s)
        );
        // pointed at in sentence 2 (step 2): visible by then, after v
        assert_eq!(step_of_part(&s, "lbl_v"), 2);
    }

    #[test]
    fn laser_on_hidden_element_makes_it_visible_in_that_sentence() {
        let mut s = json!({
            "title": "t",
            "bullets": ["one", "two"],
            "builds": [{"step": 1, "targets": ["b1"]}, {"step": 2, "targets": ["b2"]}],
            "narration": [
                sent("a", json!([{"type": "step", "n": 1}, {"type": "laser", "target": "b2"}])),
                sent("b", json!([{"type": "step", "n": 2}])),
            ],
        });
        normalize_animation(&mut s);
        assert!(
            animation_violations(&s).is_empty(),
            "{:?} {s}",
            animation_violations(&s)
        );
    }

    #[test]
    fn forgotten_unordered_and_repeated_steps_are_sequenced() {
        let mut s = json!({
            "title": "t",
            "bullets": ["one", "two", "three"],
            "builds": [
                {"step": 1, "targets": ["b1"]},
                {"step": 2, "targets": ["b2"]},
                {"step": 3, "targets": ["b3"]}
            ],
            "narration": [
                sent("a", json!([{"type": "step", "n": 3}])),
                sent("b", json!([{"type": "step", "n": 3}, {"type": "laser", "target": "b2"}])),
                sent("c", json!([])),
            ],
        });
        normalize_animation(&mut s);
        assert!(
            animation_violations(&s).is_empty(),
            "{:?} {s}",
            animation_violations(&s)
        );
        assert!(normalize_animation(&mut s.clone()).is_empty());
    }

    #[test]
    fn builds_without_any_cue_are_spread_and_display_only_slides_show_everything() {
        let mut s = json!({
            "title": "t",
            "bullets": ["one", "two"],
            "builds": [{"step": 1, "targets": ["b1"]}, {"step": 2, "targets": ["b2"]}],
            "narration": [sent("a", json!([])), sent("b", json!([]))],
        });
        normalize_animation(&mut s);
        assert!(animation_violations(&s).is_empty());
        let mut d = json!({
            "title": "t",
            "bullets": ["one"],
            "builds": [{"step": 1, "targets": ["b1"]}],
            "narration": [],
        });
        normalize_animation(&mut d);
        assert!(d.get("builds").is_none());
    }

    #[test]
    fn dangling_targets_cues_and_bad_parameters_are_cleaned() {
        let mut s = json!({
            "title": "t",
            "bullets": ["one"],
            "builds": [{"step": 1, "targets": ["b1", "figure:nope"], "effect": "zoom"}],
            "narration": [sent("a", json!([
                {"type": "step", "n": 1},
                {"type": "laser", "target": "figure:nope"},
                {"type": "laser", "target": "b1", "shape": "star", "scale": 9, "duration_ms": 10},
                {"type": "wiggle"}
            ]))],
        });
        normalize_animation(&mut s);
        let cues = s["narration"][0]["cues"].as_array().unwrap().clone();
        assert_eq!(cues.len(), 2, "{cues:?}");
        assert_eq!(cues[1]["shape"], "ellipse");
        assert_eq!(cues[1]["scale"], json!(1.6));
        assert_eq!(cues[1]["duration_ms"], json!(800.0));
        assert_eq!(s["builds"][0]["effect"], "fade");
        assert!(animation_violations(&s).is_empty());
    }

    #[test]
    fn geometric_attachment_is_used_and_cycles_are_ignored() {
        // an angle mark between two lines, and two lines that "touch" each other
        let mut s = slide(
            vec![
                part("l1", json!([["l2"]])),
                part("l2", json!([["l1"]])),
                part("ang", json!([["l1"], ["l2"]])),
            ],
            vec![seg("l1"), seg("l2"), json!({"id": "ang", "type": "angle"})],
            json!([
                {"step": 1, "targets": ["figure:ang"], "effect": "fade"},
                {"step": 2, "targets": ["figure:l1"], "effect": "fade"},
                {"step": 3, "targets": ["figure:l2"], "effect": "fade"}
            ]),
            json!([
                sent("a", json!([{"type": "step", "n": 1}])),
                sent("b", json!([{"type": "step", "n": 2}])),
                sent("c", json!([{"type": "step", "n": 3}])),
            ]),
        );
        normalize_animation(&mut s);
        // the angle waits for the later line (step 1 empties out, so 2 and 3 become 1 and 2)
        assert_eq!(step_of_part(&s, "ang"), step_of_part(&s, "l2"), "{s}");
        // the mutual l1/l2 attachment must not drag one line to the other's step
        assert!(step_of_part(&s, "l1") < step_of_part(&s, "l2"));
        assert!(animation_violations(&s).is_empty());
    }

    #[test]
    fn big_parts_are_never_waited_for() {
        let mut plane = part("plane", Value::Null);
        plane["bbox"] = json!([0.1, 0.5, 0.7, 0.3]);
        let mut s = slide(
            vec![plane, part("lbl_x", json!([["plane"]]))],
            vec![
                json!({"id": "plane", "type": "polygon"}),
                text("lbl_x", "x"),
            ],
            json!([{"step": 1, "targets": ["figure:plane"], "effect": "fade"}]),
            json!([sent("a", json!([{"type": "step", "n": 1}]))]),
        );
        normalize_animation(&mut s);
        assert_eq!(step_of_part(&s, "lbl_x"), 0);
    }

    #[test]
    fn deck_slides_get_the_same_ordering_without_cues() {
        let mut s = slide(
            vec![part("v", Value::Null), part("lbl_v", Value::Null)],
            vec![seg("v"), text("lbl_v", "v")],
            json!([
                {"step": 4, "targets": ["figure:lbl_v"], "effect": "fade"},
                {"step": 9, "targets": ["figure:v"], "effect": "fade"}
            ]),
            json!([]),
        );
        normalize_deck_animation(&mut s);
        // the label waits for v; step 1 is then empty and the rest is renumbered
        assert_eq!(step_of_part(&s, "v"), 1, "{s}");
        assert_eq!(step_of_part(&s, "lbl_v"), 1);
    }

    #[test]
    fn unlisted_helpers_wait_for_the_parts_they_hang_on() {
        // ra1/ra2: the two legs of a right-angle mark, same size, only ra1 is in a build
        let mut s = slide(
            vec![
                part("zaxis", Value::Null),
                part("ri", Value::Null),
                part("ra1", json!([["ra2"], ["ri"]])),
                part("ra2", json!([["ra1"], ["zaxis"]])),
                part("proj", json!([["Mi"]])),
                part("Mi", Value::Null),
            ],
            vec![],
            json!([
                {"step": 1, "targets": ["figure:ri", "figure:ra1", "figure:Mi"], "effect": "fade"}
            ]),
            json!([sent("a", json!([{"type": "step", "n": 1}]))]),
        );
        normalize_animation(&mut s);
        assert_eq!(step_of_part(&s, "ra2"), 1, "{s}");
        // a helper touching only later parts appears with them; the axis stays
        assert_eq!(step_of_part(&s, "proj"), 1);
        assert_eq!(step_of_part(&s, "zaxis"), 0);
        assert!(animation_violations(&s).is_empty());
    }

    #[test]
    fn a_step_is_not_revealed_before_the_narration_points_at_it() {
        let mut s = json!({
            "title": "t",
            "bullets": ["one", "two"],
            "callout": "conclusion",
            "builds": [
                {"step": 1, "targets": ["b1"]},
                {"step": 2, "targets": ["b2"]},
                {"step": 3, "targets": ["callout"], "effect": "emphasis"}
            ],
            "narration": [
                sent("a", json!([{"type": "step", "n": 1}, {"type": "step", "n": 2}, {"type": "laser", "target": "b1"}])),
                sent("b", json!([{"type": "laser", "target": "b2"}])),
                sent("c", json!([{"type": "step", "n": 3}, {"type": "laser", "target": "callout"}])),
            ],
        });
        normalize_animation(&mut s);
        assert!(
            animation_violations(&s).is_empty(),
            "{:?} {s}",
            animation_violations(&s)
        );
        // b2 is revealed in sentence 2, where it is first pointed at
        assert_eq!(s["narration"][1]["cues"][0]["n"], 2, "{s}");
        // a conclusion box nothing reveals is revealed, not just emphasised
        assert_eq!(s["builds"][2]["effect"], "appear");
        assert!(normalize_animation(&mut s.clone()).is_empty());
    }

    /// Slide 4 of the lecture a person reviewed by hand ("z" and "r_iz"-style labels showed
    /// alone because they were in no build while their lines appeared later). Part ids and the
    /// engine's attachment groups are the real ones.
    #[test]
    fn unlisted_text_appears_when_the_narration_first_points_at_it() {
        let mut sl = json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b", "c"], "formula": "x=1",
            "builds": [{"step": 1, "targets": ["b1"], "effect": "fade"}],
            "narration": [
                {"text": "一", "cues": [{"type": "step", "n": 1}, {"type": "laser", "target": "b1"}]},
                {"text": "二", "cues": [{"type": "laser", "target": "b2"}]},
                {"text": "三", "cues": [{"type": "laser", "target": "formula"}, {"type": "laser", "target": "b3"}]},
                {"text": "四"}
            ]
        });
        let log = normalize_animation(&mut sl);
        assert!(log.iter().any(|l| l.contains("b2")), "{log:?}");
        let steps = listed_steps(&sl);
        assert!(steps["b1"] < steps["b2"] && steps["b2"] < steps["formula"]);
        assert_eq!(steps["formula"], steps["b3"], "one sentence, one step");
        assert!(
            animation_violations(&sl).is_empty(),
            "{:?}",
            animation_violations(&sl)
        );
        // a slide without any build whose narration points at several things in turn reveals
        // them in turn; with one pointed element it stays static
        let mut st = json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b", "c"],
            "narration": [{"text": "一", "cues": [{"type": "laser", "target": "b1"}]}, {"text": "二", "cues": [{"type": "laser", "target": "b2"}]}, {"text": "三", "cues": [{"type": "laser", "target": "b3"}]}]
        });
        normalize_animation(&mut st);
        let steps = listed_steps(&st);
        assert!(
            !steps.contains_key("b1") && steps["b2"] < steps["b3"],
            "{st}"
        );
        assert!(animation_violations(&st).is_empty());
        let mut one = json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b"],
            "narration": [{"text": "一"}, {"text": "二", "cues": [{"type": "laser", "target": "b2"}]}]
        });
        normalize_animation(&mut one);
        assert!(one
            .get("builds")
            .is_none_or(|b| b.as_array().is_some_and(Vec::is_empty)));
        // idempotent
        let once = sl.clone();
        normalize_animation(&mut sl);
        assert_eq!(sl, once);
    }

    #[test]
    fn a_sentence_that_reveals_something_points_at_it() {
        let mut sl = json!({
            "layout": "bullets", "title": "T", "bullets": ["a", "b", "c"],
            "builds": [
                {"step": 1, "targets": ["b1"], "effect": "fade"},
                {"step": 2, "targets": ["b2", "b3"], "effect": "fade"}
            ],
            "narration": [
                {"text": "一", "cues": [{"type": "step", "n": 1}]},
                {"text": "二", "cues": [{"type": "step", "n": 2}, {"type": "laser", "target": "b3"}]},
                {"text": "三"}
            ]
        });
        let log = normalize_animation(&mut sl);
        assert!(log.iter().any(|l| l.contains("激光")), "{log:?}");
        let laser = |j: usize| -> Vec<String> {
            sl["narration"][j]["cues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|c| c["type"] == "laser")
                .map(|c| c["target"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(laser(0), vec!["b1"]);
        // an author's own pointer is kept, nothing is added next to it
        assert_eq!(laser(1), vec!["b3"]);
        // a sentence that reveals nothing is not pointed anywhere
        assert!(laser(2).is_empty());
        let once = sl.clone();
        normalize_animation(&mut sl);
        assert_eq!(sl, once);
    }

    #[test]
    fn reviewed_lecture_page_labels_no_longer_precede_their_lines() {
        let mut plane = part("plane", Value::Null);
        plane["bbox"] = json!([0.162, 0.57, 0.6856, 0.1868]);
        let mut s = slide(
            vec![
                plane,
                part("z", Value::Null),
                part(
                    "ri",
                    json!([["plane", "z"], ["ra"], ["Fiz", "cross", "m", "ra"]]),
                ),
                part("Fiz", json!([["cross", "m", "ra", "ri"]])),
                part("cross", json!([["Fiz", "m", "ra", "ri"]])),
                part(
                    "ra",
                    json!([["Fiz", "cross", "m", "ri"], ["ri"], ["cross"]]),
                ),
                part("swirl", Value::Null),
                part("m", json!([["Fiz", "cross", "ra", "ri"]])),
                part("lbl_z", Value::Null),
                part("lbl_m", Value::Null),
                part("lbl_ri", json!([["plane"]])),
                part("lbl_Fiz", json!([["Fiz"]])),
                part("lbl_cross", json!([["plane"]])),
                part("note", Value::Null),
            ],
            vec![],
            json!([
                {"step": 1, "targets": ["figure:z"], "effect": "fade"},
                {"step": 2, "targets": ["figure:ri", "b1", "figure:m"], "effect": "fade"},
                {"step": 3, "targets": ["figure:Fiz", "figure:lbl_Fiz"], "effect": "fade"},
                {"step": 4, "targets": ["figure:cross", "figure:ra", "b2"], "effect": "fade"},
                {"step": 5, "targets": ["figure:note", "b3"], "effect": "fade"}
            ]),
            json!([
                sent(
                    "1",
                    json!([{"type": "step", "n": 1}, {"type": "laser", "target": "figure:z"}])
                ),
                sent(
                    "2",
                    json!([{"type": "step", "n": 2}, {"type": "laser", "target": "figure:ri"}])
                ),
                sent(
                    "3",
                    json!([{"type": "step", "n": 3}, {"type": "laser", "target": "figure:Fiz"}])
                ),
                sent(
                    "4",
                    json!([{"type": "step", "n": 4}, {"type": "laser", "target": "figure:cross"}])
                ),
                sent("5", json!([{"type": "step", "n": 5}])),
            ]),
        );
        s["bullets"] = json!(["a", "b", "c"]);
        normalize_animation(&mut s);
        assert!(
            animation_violations(&s).is_empty(),
            "{:?}\n{s}",
            animation_violations(&s)
        );
        let at = |id: &str| step_of_part(&s, id);
        assert_eq!(at("lbl_z"), at("z"), "z label with the z axis");
        assert_eq!(at("lbl_ri"), at("ri"));
        assert_eq!(at("lbl_m"), at("m"));
        assert_eq!(at("lbl_cross"), at("cross"));
        assert!(at("lbl_Fiz") >= at("Fiz"));
        // the big plane never held a label back
        assert!(at("lbl_ri") >= 2);
        assert!(normalize_animation(&mut s.clone()).is_empty(), "idempotent");
    }
}
