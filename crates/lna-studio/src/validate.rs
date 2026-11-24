use lna_types::{ErrorCode, LnaError, LnaResult};
use serde_json::{json, Value};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

/// E2: Validate quiz payload business rules.
pub fn validate_quiz(payload: &Value, allowed_types: &[&str]) -> LnaResult<()> {
    validate_quiz_with(payload, allowed_types, None)
}

fn invalid(msg: &str) -> LnaError {
    LnaError::new(ErrorCode::OutputInvalid, msg)
}

/// Highest evidence number `[n]` present in an evidence block (lines look like `[3] (title) ...`).
pub fn max_evidence_no(evidence: &str) -> usize {
    evidence
        .split("\n\n")
        .filter_map(|block| {
            let rest = block.trim_start().strip_prefix('[')?;
            let (num, _) = rest.split_once(']')?;
            num.parse::<usize>().ok()
        })
        .max()
        .unwrap_or(0)
}

/// Removes citation numbers that do not refer to real evidence (`1..=max`) from every
/// `citations` array in the payload. Returns how many were removed.
pub fn sanitize_citations(payload: &mut Value, max: usize) -> usize {
    let mut removed = 0;
    match payload {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == "citations" {
                    if let Value::Array(items) = v {
                        let before = items.len();
                        items.retain(|n| n.as_u64().is_some_and(|n| n >= 1 && n as usize <= max));
                        removed += before - items.len();
                        continue;
                    }
                }
                removed += sanitize_citations(v, max);
            }
        }
        Value::Array(items) => {
            for v in items {
                removed += sanitize_citations(v, max);
            }
        }
        _ => {}
    }
    removed
}

/// Like `validate_quiz`, additionally rejecting citations outside `1..=max_citation`.
pub fn validate_quiz_with(
    payload: &Value,
    allowed_types: &[&str],
    max_citation: Option<usize>,
) -> LnaResult<()> {
    let qs = payload
        .get("questions")
        .and_then(|x| x.as_array())
        .ok_or_else(|| LnaError::new(ErrorCode::OutputInvalid, "Missing questions array"))?;
    let mut seen_prompts = HashSet::new();
    for q in qs {
        let qtype = q.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if !allowed_types.is_empty() && !allowed_types.contains(&qtype) {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                format!("Disallowed question type: {qtype}"),
            ));
        }
        let prompt = q
            .get("prompt")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if prompt.is_empty() || !seen_prompts.insert(prompt.to_string()) {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Empty or duplicate question prompt",
            ));
        }
        let opts = q
            .get("options")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let ans = q
            .get("answer_indices")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        if qtype == "single" && ans.len() != 1 {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Single choice must have 1 answer",
            ));
        }
        if qtype == "multi" && ans.len() < 2 {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Multi choice must have >= 2 answers",
            ));
        }
        for idx_v in &ans {
            let idx = idx_v.as_u64().unwrap_or(usize::MAX as u64) as usize;
            if idx >= opts.len() {
                return Err(LnaError::new(
                    ErrorCode::OutputInvalid,
                    "Answer index out of bounds",
                ));
            }
        }
        let distinct_answers: HashSet<u64> = ans.iter().filter_map(|v| v.as_u64()).collect();
        if distinct_answers.len() != ans.len() {
            return Err(invalid("Duplicate answer index"));
        }
        if qtype == "single" || qtype == "multi" {
            if opts.len() < 2 {
                return Err(invalid("Choice questions need at least 2 options"));
            }
            let mut seen_opts = HashSet::new();
            for o in &opts {
                let text = o.as_str().unwrap_or("").trim().to_lowercase();
                if text.is_empty() || !seen_opts.insert(text) {
                    return Err(invalid("Empty or duplicate option"));
                }
            }
        }
        if qtype == "fill" {
            let has_answer = q
                .get("accepted_answers")
                .and_then(|x| x.as_array())
                .is_some_and(|a| {
                    a.iter()
                        .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
                });
            if !has_answer {
                return Err(invalid("Fill-in question needs accepted_answers"));
            }
        }
        if qtype == "short"
            && q.get("rubric")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(invalid("Short-answer question needs a rubric"));
        }
        if let (Some(max), Some(cites)) =
            (max_citation, q.get("citations").and_then(|x| x.as_array()))
        {
            if cites
                .iter()
                .any(|c| !c.as_u64().is_some_and(|n| n >= 1 && n as usize <= max))
            {
                return Err(invalid("Citation does not refer to real evidence"));
            }
        }
    }
    Ok(())
}

/// E2: Grade single / multi / fill / short answers.
pub fn grade_quiz_answer(question: &Value, user_answer: &Value) -> Value {
    let qtype = question
        .get("type")
        .and_then(|x| x.as_str())
        .unwrap_or("single");
    match qtype {
        "single" | "multi" => {
            let expected: HashSet<i64> = question
                .get("answer_indices")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();
            let actual: HashSet<i64> = user_answer
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                .or_else(|| user_answer.as_i64().map(|n| [n].into_iter().collect()))
                .unwrap_or_default();
            let correct = !expected.is_empty() && expected == actual;
            json!({ "correct": correct, "score": if correct { 1.0 } else { 0.0 } })
        }
        "fill" => {
            let norm = |s: &str| -> String {
                s.nfkc()
                    .collect::<String>()
                    .to_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let actual = norm(user_answer.as_str().unwrap_or(""));
            let accepted = question
                .get("accepted_answers")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .any(|e| norm(e) == actual)
                })
                .unwrap_or(false);
            json!({ "correct": accepted, "score": if accepted { 1.0 } else { 0.0 } })
        }
        _ => {
            let rubric = question
                .get("rubric")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            json!({ "correct": false, "self_eval": true, "rubric": rubric })
        }
    }
}

/// E3: Deduplicate flashcards by normalized front text.
pub fn dedup_flashcards(payload: &mut Value) {
    if let Some(cards) = payload.get_mut("cards").and_then(|x| x.as_array_mut()) {
        let mut seen = HashSet::new();
        cards.retain(|c| {
            let f = c
                .get("front")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            !f.is_empty() && seen.insert(f)
        });
    }
}

/// Strip bracketed numeric citation tags like `[1]`, `[1][17]`, `[1, 2]`, `【1】` from mindmap labels.
pub fn strip_citation_markers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '[' || ch == '【' {
            let close = if ch == '[' { ']' } else { '】' };
            let mut j = i + 1;
            let mut has_digit = false;
            let mut valid = true;
            while j < chars.len() && chars[j] != close {
                let c = chars[j];
                if c.is_ascii_digit() {
                    has_digit = true;
                } else if !matches!(c, ' ' | ',' | '，' | '-' | '–' | '—' | '、') {
                    valid = false;
                    break;
                }
                j += 1;
            }
            if valid && has_digit && j < chars.len() && chars[j] == close {
                i = j + 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalises a mindmap / knowledge-graph payload WITHOUT dropping content: no depth or
/// branching caps. Strips `[n]` citation markers from labels, clears node citations, drops
/// nodes with empty labels (re-parenting their children), makes ids unique, re-attaches
/// orphans to the root, breaks parent cycles, cleans `facets`, and converts TeX to Unicode.
pub fn normalize_mindmap(payload: &mut Value) {
    use std::collections::{HashMap, HashSet};
    let Some(nodes) = payload.get("nodes").and_then(|x| x.as_array()).cloned() else {
        return;
    };
    let mut out: Vec<Value> = Vec::with_capacity(nodes.len());
    let mut seen: HashSet<String> = HashSet::new();
    // Old id -> new id, plus parent links of dropped (empty-label) nodes for re-parenting.
    let mut rename: HashMap<String, String> = HashMap::new();
    let mut dropped_parent: HashMap<String, Option<String>> = HashMap::new();
    for (i, mut n) in nodes.into_iter().enumerate() {
        let Some(obj) = n.as_object_mut() else { continue };
        let label = obj
            .get("label")
            .and_then(Value::as_str)
            .map(strip_citation_markers)
            .unwrap_or_default();
        let old_id = obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("n{i}"));
        let parent = obj.get("parent_id").and_then(Value::as_str).map(String::from);
        if label.trim().is_empty() {
            dropped_parent.insert(old_id, parent);
            continue;
        }
        let mut id = old_id.clone();
        let mut k = 2;
        while seen.contains(&id) {
            id = format!("{old_id}_{k}");
            k += 1;
        }
        seen.insert(id.clone());
        rename.entry(old_id).or_insert_with(|| id.clone());
        obj.insert("id".into(), Value::String(id));
        obj.insert("label".into(), Value::String(label));
        obj.insert("citations".into(), Value::Array(Vec::new()));
        if let Some(Value::Array(facets)) = obj.get_mut("facets") {
            let mut uniq: HashSet<String> = HashSet::new();
            let cleaned: Vec<Value> = facets
                .iter()
                .filter_map(Value::as_str)
                .map(strip_citation_markers)
                .filter(|t| !t.trim().is_empty() && uniq.insert(t.clone()))
                .map(Value::String)
                .collect();
            *facets = cleaned;
        }
        out.push(n);
    }
    if out.is_empty() {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("nodes".into(), Value::Array(out));
        }
        return;
    }
    // Resolve a parent id through renames and dropped nodes.
    let resolve = |pid: Option<String>| -> Option<String> {
        let mut cur = pid;
        for _ in 0..64 {
            match cur {
                None => return None,
                Some(ref p) if rename.contains_key(p) => return rename.get(p).cloned(),
                Some(ref p) if dropped_parent.contains_key(p) => cur = dropped_parent[p].clone(),
                Some(_) => return None,
            }
        }
        None
    };
    let root_id = out
        .iter()
        .find(|n| n.get("parent_id").map_or(true, Value::is_null))
        .or_else(|| out.first())
        .and_then(|n| n.get("id").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_default();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for n in out.iter_mut() {
        let id = n.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let raw = n.get("parent_id").and_then(Value::as_str).map(String::from);
        let mut pid = resolve(raw);
        if id == root_id {
            pid = None;
        } else if pid.is_none() || pid.as_deref() == Some(id.as_str()) {
            // A second root or a dangling parent: hang it under the root instead of dropping it.
            pid = Some(root_id.clone());
        }
        parent_of.insert(id, pid.clone());
        if let Some(obj) = n.as_object_mut() {
            obj.insert("parent_id".into(), pid.map(Value::String).unwrap_or(Value::Null));
        }
    }
    // Break cycles: a node whose ancestor chain never reaches the root is re-attached to it.
    for n in out.iter_mut() {
        let id = n.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if id == root_id {
            continue;
        }
        let mut cur = parent_of.get(&id).cloned().flatten();
        let mut steps = 0;
        let mut reaches_root = false;
        while let Some(p) = cur {
            if p == root_id {
                reaches_root = true;
                break;
            }
            steps += 1;
            if steps > parent_of.len() {
                break;
            }
            cur = parent_of.get(&p).cloned().flatten();
        }
        if !reaches_root {
            parent_of.insert(id, Some(root_id.clone()));
            if let Some(obj) = n.as_object_mut() {
                obj.insert("parent_id".into(), Value::String(root_id.clone()));
            }
        }
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("nodes".into(), Value::Array(out));
    }
    crate::texfmt::convert_mindmap_payload(payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_quiz_and_grades_fill_normalized() {
        let q = json!({
            "title": "Test",
            "questions": [{
                "id": "q1", "type": "fill", "difficulty": "easy",
                "prompt": "矩阵的秩等于？", "options": [], "answer_indices": [],
                "accepted_answers": ["Row Rank"], "rubric": "", "explanation": "", "citations": [1]
            }]
        });
        validate_quiz(&q, &["single", "fill"]).unwrap();
        let res = grade_quiz_answer(&q["questions"][0], &json!("  row   rank "));
        assert_eq!(res["correct"], true);
    }

    fn q(extra: Value) -> Value {
        let mut base = json!({
            "id": "q1", "type": "single", "difficulty": "easy", "prompt": "题干一",
            "options": ["甲", "乙"], "answer_indices": [0], "accepted_answers": [],
            "rubric": "", "explanation": "", "citations": [1]
        });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        json!({ "title": "t", "questions": [base] })
    }

    fn rejects(payload: Value, allowed: &[&str], why: &str) {
        let e = validate_quiz_with(&payload, allowed, Some(3)).unwrap_err();
        assert_eq!(e.code, ErrorCode::OutputInvalid, "{why}");
        assert!(
            e.message.to_lowercase().contains(&why.to_lowercase()),
            "{why}: {}",
            e.message
        );
    }

    #[test]
    fn e2_each_business_rule_rejects_its_violation() {
        validate_quiz_with(&q(json!({})), &["single"], Some(3)).unwrap();
        rejects(q(json!({ "answer_indices": [0, 1] })), &[], "1 answer");
        rejects(
            q(json!({ "type": "multi", "answer_indices": [0] })),
            &[],
            ">= 2",
        );
        rejects(q(json!({ "answer_indices": [5] })), &[], "out of bounds");
        rejects(
            q(json!({ "options": ["甲", "甲"] })),
            &[],
            "duplicate option",
        );
        rejects(
            q(json!({ "options": ["甲", " "] })),
            &[],
            "duplicate option",
        );
        rejects(
            q(json!({ "options": ["甲"], "answer_indices": [0] })),
            &[],
            "at least 2 options",
        );
        rejects(
            q(json!({ "type": "multi", "answer_indices": [0, 0] })),
            &[],
            "duplicate answer",
        );
        rejects(q(json!({})), &["fill"], "disallowed question type");
        rejects(
            q(json!({ "prompt": "  " })),
            &[],
            "empty or duplicate question prompt",
        );
        rejects(
            q(
                json!({ "type": "fill", "options": [], "answer_indices": [], "accepted_answers": [] }),
            ),
            &[],
            "accepted_answers",
        );
        rejects(
            q(json!({ "type": "short", "options": [], "answer_indices": [], "rubric": " " })),
            &[],
            "rubric",
        );
        rejects(q(json!({ "citations": [9] })), &[], "citation");
        rejects(q(json!({ "citations": [0] })), &[], "citation");
    }

    #[test]
    fn e2_duplicate_prompts_across_questions_are_rejected() {
        let mut p = q(json!({}));
        let first = p["questions"][0].clone();
        p["questions"].as_array_mut().unwrap().push(first);
        assert!(validate_quiz(&p, &[]).is_err());
    }

    #[test]
    fn grading_covers_single_multi_fill_and_short() {
        let single = json!({"type":"single","answer_indices":[1]});
        assert_eq!(grade_quiz_answer(&single, &json!([1]))["correct"], true);
        assert_eq!(grade_quiz_answer(&single, &json!(1))["correct"], true);
        assert_eq!(grade_quiz_answer(&single, &json!([0]))["correct"], false);
        let multi = json!({"type":"multi","answer_indices":[0,2]});
        assert_eq!(grade_quiz_answer(&multi, &json!([2, 0]))["correct"], true);
        assert_eq!(grade_quiz_answer(&multi, &json!([0]))["correct"], false);
        assert_eq!(
            grade_quiz_answer(&multi, &json!([0, 1, 2]))["correct"],
            false
        );
        let fill = json!({"type":"fill","accepted_answers":["ＡＢＣ","第一"]});
        assert_eq!(grade_quiz_answer(&fill, &json!(" abc "))["correct"], true);
        assert_eq!(grade_quiz_answer(&fill, &json!("第 一"))["correct"], false);
        let short = json!({"type":"short","rubric":"需要提到线性无关"});
        let g = grade_quiz_answer(&short, &json!("随便"));
        assert_eq!(g["self_eval"], true);
        assert_eq!(g["rubric"], "需要提到线性无关");
    }

    #[test]
    fn citations_outside_the_evidence_are_removed_and_counted() {
        let mut p = json!({"cards":[{"citations":[1,7,0]},{"citations":[2]}],"citations":[9]});
        assert_eq!(sanitize_citations(&mut p, 3), 3);
        assert_eq!(p["cards"][0]["citations"], json!([1]));
        assert_eq!(p["citations"], json!([]));
        assert_eq!(max_evidence_no("[1] (a) x\n\n[2] (b) y\n\n[12] (c) z"), 12);
        assert_eq!(max_evidence_no("无编号"), 0);
    }

    #[test]
    fn flashcards_are_deduplicated_by_normalised_front() {
        let mut p = json!({"cards":[
            {"front":"What is X?","back":"a"},{"front":" what is x? ","back":"b"},{"front":"","back":"c"},{"front":"Y","back":"d"}]});
        dedup_flashcards(&mut p);
        assert_eq!(p["cards"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn mindmap_is_never_truncated() {
        let mut nodes = vec![json!({"id":"n0","parent_id":null,"label":"r","citations":[]})];
        for i in 1..12 {
            nodes.push(json!({"id":format!("n{i}"),"parent_id":format!("n{}", i-1),"label":"x","citations":[]}));
        }
        for i in 0..40 {
            nodes.push(json!({"id":format!("c{i}"),"parent_id":"n0","label":"Child","citations":[]}));
        }
        let mut mm = json!({"title":"m","nodes":nodes});
        normalize_mindmap(&mut mm);
        assert_eq!(mm["nodes"].as_array().unwrap().len(), 52);
    }

    #[test]
    fn mindmap_repairs_ids_orphans_and_cycles() {
        let mut mm = json!({"title":"m","nodes":[
            {"id":"r","parent_id":null,"label":"Root [3]","citations":[1]},
            {"id":"a","parent_id":"r","label":"A","citations":[],"facets":["x [2]","x","", "y"]},
            {"id":"a","parent_id":"r","label":"A2","citations":[]},
            {"id":"gone","parent_id":"r","label":"  ","citations":[]},
            {"id":"b","parent_id":"gone","label":"B","citations":[]},
            {"id":"o","parent_id":"missing","label":"O","citations":[]},
            {"id":"c1","parent_id":"c2","label":"C1","citations":[]},
            {"id":"c2","parent_id":"c1","label":"C2","citations":[]},
            {"id":"r2","parent_id":null,"label":"Second root","citations":[]}
        ]});
        normalize_mindmap(&mut mm);
        let n = mm["nodes"].as_array().unwrap();
        assert_eq!(n.len(), 8);
        let by = |id: &str| n.iter().find(|x| x["id"] == id).unwrap().clone();
        assert_eq!(by("r")["label"], "Root");
        assert_eq!(by("r")["citations"], json!([]));
        assert_eq!(by("a")["facets"], json!(["x", "y"]));
        assert_eq!(by("a_2")["parent_id"], "r");
        assert_eq!(by("b")["parent_id"], "r");
        assert_eq!(by("o")["parent_id"], "r");
        assert_eq!(by("r2")["parent_id"], "r");
        assert!(by("c1")["parent_id"] == "r" || by("c2")["parent_id"] == "r");
    }
}
