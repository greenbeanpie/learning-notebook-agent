//! Chat pipeline behaviour against a scripted mock model (C3-C10, B-10, B-19).
mod common;

use common::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A notebook with `n` distinct short sources that all match the query "基与维数".
fn notebook_with_sources(env: &Env, n: usize) -> String {
    let nid = env.notebook();
    for i in 0..n {
        env.add_text(
            &nid,
            &format!("讲义{i}.md"),
            &format!("# 讲义{i}\n\n基与维数的第{i}段说明：向量空间的基是线性无关且张成空间的向量组，维数是基的向量个数。"),
        );
    }
    env.wait_sources_ready(&nid, n);
    nid
}

const LONG_QUESTION: &str = "请详细说明向量空间中基与维数之间的关系是什么";

fn followups_ok() -> Reply {
    Reply::Text(r#"{"questions":["什么是基？","维数怎么求？","秩是什么？"]}"#.into())
}

fn answer() -> Reply {
    Reply::Text("向量空间的基与维数密切相关，任意两组基的向量个数相同[1]。".into())
}

fn thought_kinds(msg: &Value) -> Vec<String> {
    msg["thoughts"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["kind"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// C4 / B-10 / B-19: after three tool rounds the fourth request carries no tools, the message
/// ends COMPLETE (never STREAMING), and usage from every round is summed.
#[test]
fn tool_loop_stops_after_three_rounds_and_sums_usage() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat if r.has_tools() => Reply::Tool {
            id: format!("call{}", r.nth),
            name: "list_sources".into(),
            args: json!({}),
        },
        _ => answer(),
    });
    let env = Env::new("toolloop");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, true);

    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");

    let chat = mock.of_kind(Kind::Chat);
    assert_eq!(chat.len(), 4, "3 tool rounds + 1 final request");
    assert!(chat[..3].iter().all(|r| r.has_tools()));
    assert!(!chat[3].has_tools(), "4th request must not offer tools");
    // tool results are wrapped as untrusted
    assert!(chat[1].user_text().contains("untrusted"));
    let kinds = thought_kinds(&msg);
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "tool_list_sources" || *k == "tool_call")
            .count(),
        3
    );

    let usage = env.ok("usage.summary", json!({ "notebook_id": nid }));
    assert_eq!(
        usage["input_tokens"],
        json!(400),
        "100 per round x 4: {usage}"
    );
    assert_eq!(usage["output_tokens"], json!(80));
    assert!(env
        .history(&nid)
        .iter()
        .all(|m| m["status"] != json!("STREAMING")));
}

/// C4: `read_context` and `list_sources` are executed; unknown tools and bad citation numbers
/// come back as error results rather than failing the turn.
#[test]
fn read_only_tools_return_results_and_errors_without_failing_the_turn() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat if r.has_tools() && r.nth == 0 => Reply::Tool {
            id: "c1".into(),
            name: "read_context".into(),
            args: json!({ "citation_no": 1 }),
        },
        Kind::Chat if r.has_tools() && r.nth == 1 => Reply::Tool {
            id: "c2".into(),
            name: "read_context".into(),
            args: json!({ "citation_no": 99 }),
        },
        Kind::Chat if r.has_tools() && r.nth == 2 => Reply::Tool {
            id: "c3".into(),
            name: "drop_database".into(),
            args: json!({}),
        },
        _ => answer(),
    });
    let env = Env::new("tools");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, true);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");
    let chat = mock.of_kind(Kind::Chat);
    let second = chat[1].user_text();
    assert!(
        second.contains("当前片段"),
        "read_context result missing: {second}"
    );
    let third = chat[2].user_text();
    assert!(third.contains("不存在引用编号"), "{third}");
    let fourth = chat[3].user_text();
    assert!(fourth.contains("不支持的只读工具"), "{fourth}");
}

/// C7: a context-exceeded error on the first request halves the evidence and retries once.
#[test]
fn context_exceeded_halves_evidence_and_retries_once() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat if r.nth == 0 => Reply::Status(
            400,
            r#"{"error":{"message":"maximum context length exceeded"}}"#.into(),
        ),
        _ => answer(),
    });
    let env = Env::new("ctxretry");
    let nid = notebook_with_sources(&env, 6);
    env.profile(&mock, 32000, false);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");
    let chat = mock.of_kind(Kind::Chat);
    assert_eq!(chat.len(), 2, "exactly one retry");
    let count = |r: &Req| r.user_text().matches("<source ").count();
    assert!(count(&chat[0]) >= 2, "need several evidences to halve");
    assert_eq!(count(&chat[1]), (count(&chat[0]) / 2).max(1));
    assert!(thought_kinds(&msg).contains(&"context_retry".to_string()));
}

/// C7: a second context-exceeded error is not retried again.
#[test]
fn context_exceeded_twice_fails_without_a_third_attempt() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Chat => Reply::Status(400, r#"{"error":{"message":"context too long"}}"#.into()),
        _ => followups_ok(),
    });
    let env = Env::new("ctxtwice");
    let nid = notebook_with_sources(&env, 4);
    env.profile(&mock, 32000, false);
    let code = env.err(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(code, "MODEL_CONTEXT_EXCEEDED");
    assert_eq!(mock.of_kind(Kind::Chat).len(), 2);
    let h = env.history(&nid);
    assert_eq!(h.last().unwrap()["status"], json!("FAILED"));
}

/// C7: history beyond its budget is summarised into `conversation_summaries`, the summary is
/// injected into the system prompt, and the kept history never exceeds the budget.
#[test]
fn history_over_budget_is_summarised_and_injected() {
    let long_answer = format!("答：{}[1]", "向量空间的基与维数".repeat(30));
    let mock = {
        let la = long_answer.clone();
        MockLlm::start(move |r| match r.kind {
            Kind::HistorySummary => {
                Reply::Text(r#"{"summary":"用户在询问基与维数的关系。"}"#.into())
            }
            Kind::Followups => followups_ok(),
            Kind::Chat => Reply::Text(la.clone()),
            _ => answer(),
        })
    };
    let env = Env::new("histsum");
    let nid = notebook_with_sources(&env, 2);
    // context 4000: available = 4000 - (1000 + 800 + 200) = 2000 -> history budget 600 tokens.
    env.profile(&mock, 4000, false);
    for i in 0..4 {
        env.ok(
            "chat.send",
            json!({ "notebook_id": nid, "text": format!("{LONG_QUESTION}（第{i}问）") }),
        );
    }
    let sums = mock.of_kind(Kind::HistorySummary);
    assert!(!sums.is_empty(), "history should have been summarised");
    let stored: Option<String> = env.query(|c| {
        c.query_row(
            "SELECT summary FROM conversation_summaries WHERE notebook_id=?1",
            [&nid],
            |r| r.get(0),
        )
        .ok()
    });
    assert_eq!(stored.as_deref(), Some("用户在询问基与维数的关系。"));
    let last_chat = mock.of_kind(Kind::Chat).pop().unwrap();
    assert!(last_chat.system.contains("[历史摘要]"));
    assert!(last_chat.system.contains("用户在询问基与维数的关系。"));
    // kept history (messages between system and the final question) fits the 600-token budget
    let msgs = last_chat.body["messages"].as_array().unwrap();
    let kept_tokens: usize = msgs[1..msgs.len() - 1]
        .iter()
        .map(|m| lna_agent_estimate(m["content"].as_str().unwrap_or("")))
        .sum();
    assert!(kept_tokens <= 600, "kept {kept_tokens} tokens of history");
}

fn lna_agent_estimate(s: &str) -> usize {
    lna_agent::dialog::estimate_tokens(s)
}

/// C7: when summarising fails the oldest turns are dropped and the chat still completes; the
/// failed attempt leaves no summary row.
#[test]
fn history_summary_failure_drops_oldest_and_chat_still_completes() {
    let long_answer = format!("答：{}[1]", "向量空间的基与维数".repeat(30));
    let mock = {
        let la = long_answer.clone();
        MockLlm::start(move |r| match r.kind {
            Kind::HistorySummary => Reply::Status(400, "{}".into()),
            Kind::Followups => followups_ok(),
            Kind::Chat => Reply::Text(la.clone()),
            _ => answer(),
        })
    };
    let env = Env::new("histfail");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 4000, false);
    for i in 0..4 {
        let m = env.ok(
            "chat.send",
            json!({ "notebook_id": nid, "text": format!("{LONG_QUESTION}（第{i}问）") }),
        );
        assert_eq!(m["status"], json!("COMPLETE"));
    }
    assert!(!mock.of_kind(Kind::HistorySummary).is_empty());
    let rows: i64 = env.query(|c| {
        c.query_row("SELECT COUNT(*) FROM conversation_summaries", [], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert_eq!(rows, 0);
    let last = mock.of_kind(Kind::Chat).pop().unwrap();
    assert!(!last.system.contains("[历史摘要]"));
    // oldest turn is gone from the prompt
    assert!(!last.user_text().contains("第0问"));
}

/// C5: when the rewrite call fails the original question is used and the turn continues.
#[test]
fn rewrite_failure_falls_back_to_the_original_question() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Rewrite => Reply::Status(400, r#"{"error":{"message":"bad"}}"#.into()),
        Kind::Followups => followups_ok(),
        _ => answer(),
    });
    let env = Env::new("rewritefail");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "它的定理呢" }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");
    assert_eq!(mock.of_kind(Kind::Rewrite).len(), 1);
    let step = msg["thoughts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == json!("query_rewrite") && t["state"] != json!("running"))
        .expect("finished rewrite step");
    assert_eq!(step["state"], json!("skipped"));
    assert!(step["detail"].as_str().unwrap().contains("改写失败"));
}

/// C5: a rewrite that takes longer than 3 seconds is abandoned.
#[test]
fn rewrite_slower_than_three_seconds_is_abandoned() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Rewrite => Reply::SlowText(r#"{"standalone":"x","queries":[]}"#.into(), 1500),
        Kind::Followups => followups_ok(),
        _ => answer(),
    });
    let env = Env::new("rewriteslow");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let t0 = std::time::Instant::now();
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "它的定理呢" }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"));
    assert!(
        t0.elapsed() < Duration::from_secs(9),
        "took {:?}",
        t0.elapsed()
    );
    let step = msg["thoughts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == json!("query_rewrite") && t["state"] != json!("running"))
        .unwrap();
    assert!(
        step["detail"].as_str().unwrap().contains("超过 3 秒"),
        "{step}"
    );
}

/// C5: self-contained questions are not rewritten at all.
#[test]
fn self_contained_question_skips_rewrite() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        _ => answer(),
    });
    let env = Env::new("norewrite");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(mock.of_kind(Kind::Rewrite).len(), 0);
}

/// C6: a failing follow-up generation leaves the answer COMPLETE and emits no event.
#[test]
fn followup_failure_keeps_message_complete() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => Reply::Status(400, "{}".into()),
        _ => answer(),
    });
    let env = Env::new("fufail");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let mut rx = env.core.events.subscribe();
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"));
    env.wait(5, || !mock.of_kind(Kind::Followups).is_empty());
    std::thread::sleep(Duration::from_millis(500));
    while let Ok(ev) = rx.try_recv() {
        assert_ne!(ev.event, "chat.followups");
    }
    let h = env.history(&nid);
    let last = h.last().unwrap();
    assert_eq!(last["status"], json!("COMPLETE"));
    assert!(last["followups"].as_array().is_none_or(|a| a.is_empty()));
}

/// C3: a substantive answer with no citation is flagged but not retried.
#[test]
fn answer_without_citation_is_flagged_and_not_retried() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        _ => Reply::Text(
            "这是一个没有任何引用标记的、足够长的回答内容，用来触发未找到引用的提示。".into(),
        ),
    });
    let env = Env::new("nocite");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"));
    assert_eq!(mock.of_kind(Kind::Chat).len(), 1);
    let h = env.history(&nid);
    assert!(thought_kinds(h.last().unwrap()).contains(&"citation_missing".to_string()));
}

/// C9: content is persisted while the answer is still streaming.
#[test]
fn streamed_content_is_checkpointed_before_the_stream_ends() {
    let body = "向量空间的基与维数。".repeat(12);
    let mock = {
        let b = body.clone();
        MockLlm::start(move |r| match r.kind {
            Kind::Followups => followups_ok(),
            Kind::Chat => Reply::SlowText(b.clone(), 220),
            _ => answer(),
        })
    };
    let env = Env::new("checkpoint");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let core = Arc::clone(&env.core);
    let n2 = nid.clone();
    let worker = std::thread::spawn(move || {
        let req = json!({ "id": "t", "method": "chat.send", "params": { "notebook_id": n2, "text": LONG_QUESTION } });
        core.handle_json(&req.to_string())
    });
    let mut seen = None;
    env.wait(20, || {
        let h = env.history(&nid);
        if let Some(m) = h.iter().find(|m| m["role"] == json!("assistant")) {
            if m["status"] == json!("STREAMING") && !m["content"].as_str().unwrap_or("").is_empty()
            {
                seen = Some(m["content"].as_str().unwrap().to_string());
                return true;
            }
        }
        false
    });
    let partial = seen.unwrap();
    assert!(
        partial.chars().count() < body.chars().count(),
        "should be partial"
    );
    worker.join().unwrap();
    let h = env.history(&nid);
    assert_eq!(h.last().unwrap()["status"], json!("COMPLETE"));
}

/// C9: after an abrupt restart a STREAMING message becomes INCOMPLETE and keeps its text.
#[test]
fn crash_recovery_marks_streaming_incomplete_and_keeps_content() {
    let dir = temp_dir("recover");
    let nid;
    {
        let core = lna_api::CoreRuntime::open(dir.clone(), true).unwrap();
        let call = |m: &str, p: Value| -> Value {
            serde_json::from_str(
                &core.handle_json(&json!({ "id": "t", "method": m, "params": p }).to_string()),
            )
            .unwrap()
        };
        nid = call("notebook.create", json!({ "title": "t" }))["result"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        core.db
            .write(|c| {
                c.execute(
                    "INSERT INTO chat_messages (id, notebook_id, role, status, content, created_at) VALUES ('m1', ?1, 'assistant', 'STREAMING', '已生成的部分', 1)",
                    [&nid],
                )
                .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                Ok(())
            })
            .unwrap();
    }
    let core = lna_api::CoreRuntime::open(dir.clone(), true).unwrap();
    let r: Value = serde_json::from_str(
        &core.handle_json(
            &json!({ "id": "t", "method": "chat.history", "params": { "notebook_id": nid } })
                .to_string(),
        ),
    )
    .unwrap();
    let m = &r["result"]["messages"][0];
    assert_eq!(m["status"], json!("INCOMPLETE"));
    assert_eq!(m["content"], json!("已生成的部分"));
    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C10: a stream that dies after content was received is INCOMPLETE (not retried) and keeps text.
#[test]
fn interrupted_stream_after_content_is_incomplete_and_not_retried() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat => Reply::Truncated("向量空间的基与维数部分回答".into()),
        _ => answer(),
    });
    let env = Env::new("interrupt");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let code = env.err(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(code, "MODEL_STREAM_INTERRUPTED");
    assert_eq!(
        mock.of_kind(Kind::Chat).len(),
        1,
        "no automatic retry after content"
    );
    let h = env.history(&nid);
    let last = h.last().unwrap();
    assert_eq!(last["status"], json!("INCOMPLETE"));
    assert!(
        last["content"].as_str().unwrap().contains("部分回答"),
        "{last}"
    );
}

/// C10: 429 before any content is retried (honouring Retry-After); auth errors are not retried.
#[test]
fn rate_limit_is_retried_but_auth_failure_is_not() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat if r.nth == 0 => {
            Reply::RetryAfter(429, 2, r#"{"error":{"message":"slow down"}}"#.into())
        }
        _ => answer(),
    });
    let env = Env::new("retry429");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let t0 = std::time::Instant::now();
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    let elapsed = t0.elapsed();
    assert_eq!(msg["status"], json!("COMPLETE"));
    assert_eq!(mock.of_kind(Kind::Chat).len(), 2);
    assert!(
        elapsed >= std::time::Duration::from_millis(1800),
        "expected Retry-After: 2 to delay retry, elapsed={elapsed:?}"
    );

    let mock2 = MockLlm::start(|_| Reply::Status(401, r#"{"error":{"message":"bad key"}}"#.into()));
    env.profile(&mock2, 32000, false);
    let code = env.err(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    assert_eq!(code, "MODEL_AUTH_FAILED");
    assert_eq!(
        mock2.of_kind(Kind::Chat).len(),
        1,
        "auth failures are not retried"
    );
}

/// C8: clearing while a reply is streaming is rejected; clearing afterwards removes messages and
/// the stored summary; regenerate replaces the last answer.
#[test]
fn clear_conflicts_while_streaming_then_removes_messages_and_summary() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat => Reply::SlowText("向量空间的基与维数。".repeat(8), 150),
        _ => answer(),
    });
    let env = Env::new("clear");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let core = Arc::clone(&env.core);
    let n2 = nid.clone();
    let worker = std::thread::spawn(move || {
        core.handle_json(
            &json!({ "id": "t", "method": "chat.send", "params": { "notebook_id": n2, "text": LONG_QUESTION } })
                .to_string(),
        )
    });
    env.wait(10, || {
        env.history(&nid)
            .iter()
            .any(|m| m["status"] == json!("STREAMING"))
    });
    assert_eq!(
        env.err("chat.clear", json!({ "notebook_id": nid })),
        "CONFLICT"
    );
    assert_eq!(
        env.err("chat.regenerate", json!({ "notebook_id": nid })),
        "CONFLICT"
    );
    worker.join().unwrap();

    let before = env.history(&nid);
    let regen = env.ok("chat.regenerate", json!({ "notebook_id": nid }));
    assert_eq!(regen["status"], json!("COMPLETE"));
    assert_eq!(env.history(&nid).len(), before.len());
    assert_ne!(
        env.history(&nid).last().unwrap()["id"],
        before.last().unwrap()["id"]
    );

    env.core
        .db
        .write(|c| {
            c.execute(
                "INSERT OR REPLACE INTO conversation_summaries (notebook_id, through_message_id, summary, token_count, updated_at) VALUES (?1,'x','s',1,1)",
                [&nid],
            )
            .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    env.ok("chat.clear", json!({ "notebook_id": nid }));
    assert!(env.history(&nid).is_empty());
    let rows: i64 = env.query(|c| {
        c.query_row("SELECT COUNT(*) FROM conversation_summaries", [], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert_eq!(rows, 0);
}

/// C8: citation details include neighbouring chunks, and a removed source falls back to the
/// stored quote flagged as removed.
#[test]
fn citation_detail_survives_source_removal_with_a_snapshot() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        _ => answer(),
    });
    let env = Env::new("citedetail");
    let nid = env.notebook();
    let sid = env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    env.profile(&mock, 32000, false);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": LONG_QUESTION }),
    );
    let mid = msg["id"].as_str().unwrap().to_string();
    let live = env.ok(
        "chat.citation_detail",
        json!({ "message_id": mid, "citation_no": 1 }),
    );
    assert_eq!(live["source_removed"], json!(false));
    assert!(live["content"].as_str().unwrap().contains("线性无关"));
    env.ok("source.remove", json!({ "source_id": sid }));
    let gone = env.ok(
        "chat.citation_detail",
        json!({ "message_id": mid, "citation_no": 1 }),
    );
    assert_eq!(gone["source_removed"], json!(true), "{gone}");
    assert!(
        !gone["content"].as_str().unwrap().is_empty(),
        "quote snapshot kept"
    );
    assert_eq!(
        env.err(
            "chat.citation_detail",
            json!({ "message_id": mid, "citation_no": 9 })
        ),
        "NOT_FOUND"
    );
}

/// Cancelling a streaming answer keeps what was generated and ends CANCELLED.
#[test]
fn cancel_keeps_partial_text_and_ends_cancelled() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => followups_ok(),
        Kind::Chat => Reply::SlowText("向量空间的基与维数。".repeat(20), 200),
        _ => answer(),
    });
    let env = Env::new("cancel");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    let core = Arc::clone(&env.core);
    let n2 = nid.clone();
    let worker = std::thread::spawn(move || {
        core.handle_json(
            &json!({ "id": "t", "method": "chat.send", "params": { "notebook_id": n2, "text": LONG_QUESTION } })
                .to_string(),
        )
    });
    let mut rx = env.core.events.subscribe();
    env.wait(10, || {
        while let Ok(ev) = rx.try_recv() {
            if ev.event == "chat.text_delta" {
                return true;
            }
        }
        false
    });
    env.ok("chat.cancel", json!({ "notebook_id": nid }));
    worker.join().unwrap();
    let h = env.history(&nid);
    let last = h.last().unwrap();
    assert_eq!(last["status"], json!("CANCELLED"));
    assert!(!last["content"].as_str().unwrap().is_empty());
}

/// B-13: the notebook summary is generated once the model is configured, and is not regenerated
/// while the source set is unchanged.
#[test]
fn notebook_summary_generated_after_model_configured_and_cached_by_fingerprint() {
    let mock = MockLlm::start(|r| {
        match r.kind {
        Kind::NotebookSummary => Reply::Text(
            r#"{"title_suggestion":"线性代数笔记","summary_md":"本笔记本讨论基与维数。","suggested_questions":["什么是基？","维数如何定义？","秩是什么？"]}"#.into(),
        ),
        _ => answer(),
    }
    });
    let env = Env::new("nbsummary");
    let nid = notebook_with_sources(&env, 2);
    env.profile(&mock, 32000, false);
    env.wait(15, || {
        let nb = env.ok("notebook.get", json!({ "notebook_id": nid }));
        nb["summary_md"].as_str().is_some_and(|s| !s.is_empty())
    });
    let n = mock.of_kind(Kind::NotebookSummary).len();
    assert_eq!(n, 1);
    // Re-activating with the same sources must not call the model again.
    let pid = env.ok("profile.list", json!({}))["profiles"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.ok("profile.activate", json!({ "profile_id": pid }));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(mock.of_kind(Kind::NotebookSummary).len(), 1);
    // Changing the source set invalidates the fingerprint and regenerates once.
    env.add_text(
        &nid,
        "新增.md",
        "# 新增

矩阵的秩等于列空间的维数。",
    );
    env.wait(15, || mock.of_kind(Kind::NotebookSummary).len() == 2);
}
