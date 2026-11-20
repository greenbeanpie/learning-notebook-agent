use lna_types::{ErrorCode, LnaError, LnaResult};
use serde_json::{json, Value};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

/// E2: Validate quiz payload business rules.
pub fn validate_quiz(payload: &Value, allowed_types: &[&str]) -> LnaResult<()> {
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

/// E4: Enforce mindmap depth <= 5 and max 8 children per node.
pub fn enforce_mindmap_limits(payload: &mut Value) {
    let Some(nodes) = payload.get("nodes").and_then(|x| x.as_array()).cloned() else {
        return;
    };
    let mut children_map: std::collections::HashMap<Option<String>, Vec<Value>> =
        std::collections::HashMap::new();
    for n in nodes {
        let pid = n
            .get("parent_id")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        children_map.entry(pid).or_default().push(n);
    }

    let mut kept = Vec::new();
    let mut queue: std::collections::VecDeque<(Option<String>, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((None, 1));

    while let Some((pid, depth)) = queue.pop_front() {
        if depth > 5 {
            continue;
        }
        if let Some(list) = children_map.get(&pid) {
            for child in list.iter().take(8) {
                kept.push(child.clone());
                if let Some(cid) = child.get("id").and_then(|x| x.as_str()) {
                    queue.push_back((Some(cid.to_string()), depth + 1));
                }
            }
        }
    }

    if let Some(obj) = payload.as_object_mut() {
        obj.insert("nodes".into(), Value::Array(kept));
    }
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

    #[test]
    fn enforces_mindmap_depth_and_width() {
        let mut nodes = vec![json!({"id":"n1","parent_id":null,"label":"Root","citations":[]})];
        for i in 0..12 {
            nodes.push(
                json!({"id":format!("c{i}"),"parent_id":"n1","label":"Child","citations":[]}),
            );
        }
        let mut mm = json!({ "title": "MM", "nodes": nodes });
        enforce_mindmap_limits(&mut mm);
        // 1 root + max 8 children = 9
        assert_eq!(mm["nodes"].as_array().unwrap().len(), 9);
    }
}
