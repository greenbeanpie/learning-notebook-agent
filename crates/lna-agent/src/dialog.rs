//! Pure helpers for the chat pipeline: query-rewrite triggers (C5), follow-up parsing (C6),
//! and history budgeting (C7). Kept free of I/O so they are unit-testable.

/// Rough token estimate: CJK characters count as one token, everything else as four chars per token.
pub fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    (cjk + other.div_ceil(4)).max(1)
}

fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c)
}

/// "zh" when CJK characters make up more than a fifth of the text, otherwise "en".
pub fn language_of(text: &str) -> &'static str {
    let total = text.chars().count().max(1);
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    if cjk * 5 > total {
        "zh"
    } else {
        "en"
    }
}

const ZH_REFERENTS: &[&str] = &[
    "它",
    "它们",
    "这个",
    "那个",
    "这些",
    "那些",
    "该",
    "上述",
    "前面",
    "刚才",
    "上一",
    "那第",
    "这一点",
    "那一点",
];
const EN_REFERENTS: &[&str] = &[
    "it",
    "its",
    "they",
    "them",
    "those",
    "these",
    "this one",
    "that one",
    "the above",
    "previous",
    "earlier",
];

/// Why a question needs rewriting before retrieval (9.2), or `None` when it can be used as-is.
/// Any one condition is enough: a referring/elliptical question, a short question that follows
/// earlier turns, or a question whose language differs from every selected source.
pub fn rewrite_reason(
    question: &str,
    has_history: bool,
    source_langs: &[String],
) -> Option<&'static str> {
    let q = question.trim();
    if q.is_empty() {
        return None;
    }
    let lang = language_of(q);

    let referring = if lang == "zh" {
        q.chars().count() <= 30 && ZH_REFERENTS.iter().any(|w| q.contains(w))
            || q.ends_with('呢')
            || q.ends_with("呢？")
    } else {
        let lower = q.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .collect();
        words.len() <= 10
            && (words.iter().any(|w| EN_REFERENTS.contains(w))
                || EN_REFERENTS
                    .iter()
                    .filter(|p| p.contains(' '))
                    .any(|p| lower.contains(p)))
    };
    if referring {
        return Some("指代或省略");
    }

    let short = if lang == "zh" {
        q.chars().count() <= 8
    } else {
        q.split_whitespace().count() <= 4
    };
    if has_history && short {
        return Some("较短的追问");
    }

    if !source_langs.is_empty() && !source_langs.iter().any(|l| l == lang) {
        return Some("提问语言与来源语言不同");
    }
    None
}

/// Parses `{"standalone": "...", "queries": [...]}` into at most three distinct queries.
/// The standalone question comes first; the original question is always kept as a fallback.
pub fn parse_rewrite(raw: &str, original: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    };
    if let Some(v) = extract_json(raw) {
        if let Some(s) = v.get("standalone").and_then(|x| x.as_str()) {
            push(s);
        }
        if let Some(arr) = v.get("queries").and_then(|x| x.as_array()) {
            for q in arr.iter().filter_map(|x| x.as_str()) {
                push(q);
            }
        }
    }
    let original = original.trim();
    if out.is_empty() {
        out.push(original.to_string());
    } else if !out.iter().any(|x| x == original) {
        if out.len() >= 3 {
            out.pop();
        }
        out.push(original.to_string());
    }
    out.truncate(3);
    out
}

/// Parses `{"questions": [...]}`: at most three non-empty, distinct questions.
pub fn parse_followups(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(v) = extract_json(raw) {
        if let Some(arr) = v.get("questions").and_then(|x| x.as_array()) {
            for q in arr.iter().filter_map(|x| x.as_str()) {
                let q = q.trim();
                if !q.is_empty() && q.chars().count() <= 80 && !out.iter().any(|x| x == q) {
                    out.push(q.to_string());
                }
            }
        }
    }
    out.truncate(3);
    out
}

/// Models sometimes wrap JSON in prose or code fences; take the outermost object.
pub fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryMsg {
    pub id: String,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Default)]
pub struct HistoryPlan {
    /// Messages kept verbatim, oldest first. Always starts with a user message.
    pub kept: Vec<HistoryMsg>,
    /// Older messages that did not fit and must be summarised (or dropped), oldest first.
    pub overflow: Vec<HistoryMsg>,
}

/// Keeps the newest whole question/answer pairs that fit `budget_tokens` (9.4); everything
/// older becomes `overflow`. `messages` are complete messages, oldest first.
pub fn plan_history(messages: &[HistoryMsg], budget_tokens: usize) -> HistoryPlan {
    // Group into pairs: a user message together with the assistant reply that follows it.
    let mut pairs: Vec<Vec<HistoryMsg>> = Vec::new();
    for m in messages {
        if m.role == "user" || pairs.is_empty() {
            pairs.push(vec![m.clone()]);
        } else if let Some(last) = pairs.last_mut() {
            last.push(m.clone());
        }
    }
    let mut used = 0usize;
    let mut keep_from = pairs.len();
    for (i, pair) in pairs.iter().enumerate().rev() {
        let cost: usize = pair.iter().map(|m| estimate_tokens(&m.content)).sum();
        if used + cost > budget_tokens {
            break;
        }
        used += cost;
        keep_from = i;
    }
    let mut plan = HistoryPlan::default();
    for (i, pair) in pairs.into_iter().enumerate() {
        if i >= keep_from {
            plan.kept.extend(pair);
        } else {
            plan.overflow.extend(pair);
        }
    }
    // A leading assistant message (orphaned reply) would break role alternation; push it to overflow.
    while plan.kept.first().is_some_and(|m| m.role != "user") {
        let m = plan.kept.remove(0);
        plan.overflow.push(m);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, role: &str, content: &str) -> HistoryMsg {
        HistoryMsg {
            id: id.into(),
            role: role.into(),
            content: content.into(),
        }
    }

    #[test]
    fn rewrite_triggers_on_referring_short_and_cross_language_questions() {
        let zh = vec!["zh".to_string()];
        let en = vec!["en".to_string()];
        // referring expression
        assert_eq!(
            rewrite_reason("它的作用是什么？", false, &zh),
            Some("指代或省略")
        );
        assert_eq!(rewrite_reason("那第二点呢", true, &zh), Some("指代或省略"));
        assert_eq!(
            rewrite_reason("What does it depend on?", false, &en),
            Some("指代或省略")
        );
        // short follow-up with history, but not without
        assert_eq!(rewrite_reason("光合作用", true, &zh), Some("较短的追问"));
        assert_eq!(rewrite_reason("光合作用", false, &zh), None);
        // cross-language: Chinese question over English-only sources
        assert_eq!(
            rewrite_reason("请解释光合作用发生在叶绿体的哪个部位", false, &en),
            Some("提问语言与来源语言不同")
        );
        // self-contained same-language question is left alone
        assert_eq!(
            rewrite_reason("请解释光合作用发生在叶绿体的哪个部位", true, &zh),
            None
        );
        assert_eq!(rewrite_reason("", true, &zh), None);
    }

    #[test]
    fn parse_rewrite_dedupes_caps_at_three_and_keeps_original_as_fallback() {
        let raw = "```json\n{\"standalone\":\"光合作用的暗反应发生在哪里\",\"queries\":[\"暗反应 位置\",\"Calvin cycle location\",\"光合作用的暗反应发生在哪里\",\"extra\"]}\n```";
        let q = parse_rewrite(raw, "那暗反应呢");
        assert_eq!(q.len(), 3);
        assert_eq!(q[0], "光合作用的暗反应发生在哪里");
        assert!(q.contains(&"Calvin cycle location".to_string()));

        // garbage → original question only
        assert_eq!(
            parse_rewrite("not json", " 原问题 "),
            vec!["原问题".to_string()]
        );
        // original is appended when there is room
        let q = parse_rewrite(r#"{"standalone":"A","queries":[]}"#, "B");
        assert_eq!(q, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parse_followups_keeps_three_distinct_short_questions() {
        let q = parse_followups(r#"{"questions":["a?","a?","b?","c?","d?"]}"#);
        assert_eq!(q, vec!["a?", "b?", "c?"]);
        assert!(parse_followups("oops").is_empty());
        let long = "x".repeat(200);
        assert!(parse_followups(&format!(r#"{{"questions":["{long}"]}}"#)).is_empty());
    }

    #[test]
    fn history_keeps_newest_whole_pairs_and_overflows_the_rest() {
        let msgs = vec![
            m("1", "user", &"问".repeat(40)),
            m("2", "assistant", &"答".repeat(40)),
            m("3", "user", &"问".repeat(40)),
            m("4", "assistant", &"答".repeat(40)),
            m("5", "user", "短问"),
            m("6", "assistant", "短答"),
        ];
        // Budget fits the last two pairs (84 tokens) but not the first.
        let plan = plan_history(&msgs, 90);
        assert_eq!(
            plan.kept.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["3", "4", "5", "6"]
        );
        assert_eq!(
            plan.overflow
                .iter()
                .map(|x| x.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        // Huge budget keeps everything; zero budget keeps nothing.
        assert_eq!(plan_history(&msgs, 10_000).overflow.len(), 0);
        assert_eq!(plan_history(&msgs, 0).kept.len(), 0);
    }

    #[test]
    fn history_never_starts_with_an_orphaned_assistant_reply() {
        let msgs = vec![
            m("1", "assistant", "孤立的回答"),
            m("2", "user", "问"),
            m("3", "assistant", "答"),
        ];
        let plan = plan_history(&msgs, 1000);
        assert_eq!(plan.kept.first().map(|x| x.role.as_str()), Some("user"));
        assert_eq!(plan.overflow.len(), 1);
    }
}
