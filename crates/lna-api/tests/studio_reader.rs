//! The lecture's source reader writes the outline itself, sees the passages it read, and small
//! lectures reuse the collected evidence instead of reading again per section.
mod common;

use common::*;
use serde_json::{json, Value};

fn studio_wait(env: &Env, id: &str, status: &str, secs: u64) -> Value {
    let mut last = Value::Null;
    env.wait(secs, || {
        last = env.ok("studio.get", json!({ "artifact_id": id }));
        if status != "FAILED" && last["status"] == json!("FAILED") {
            panic!("artifact unexpectedly failed while waiting for {status}: {last}");
        }
        last["status"] == json!(status)
    });
    last
}

/// A notebook whose sources are far larger than a 4k context window allows.
fn big_notebook(env: &Env) -> String {
    let nid = env.notebook();
    for s in 0..2 {
        let mut doc = format!("# 大讲义{s}\n\n");
        for chap in 0..6 {
            doc.push_str(&format!("## 第{chap}章\n\n"));
            for para in 0..6 {
                doc.push_str(&format!(
                    "第{s}-{chap}-{para}节：向量空间的基是线性无关且张成空间的向量组，维数是基中向量的个数，这是第{para}段独有的说明文字，用于保证内容各不相同。{}\n\n",
                    "线性代数讨论向量、矩阵与线性变换。".repeat(12)
                ));
            }
        }
        env.add_text(&nid, &format!("大讲义{s}.md"), &doc);
    }
    env.wait_sources_ready(&nid, 2);
    nid
}

fn outline(n_sections: usize) -> String {
    let sections: Vec<Value> = (0..n_sections)
        .map(|i| json!({ "title": format!("第{i}部分"), "focus": format!("第{i}章：基与维数"), "visual_need": "none" }))
        .collect();
    json!({ "title": "基与维数讲解", "objectives": ["理解基"], "sections": sections }).to_string()
}

fn is_collect_reader(r: &Req) -> bool {
    r.system.contains("research planner")
}
fn is_section_reader(r: &Req) -> bool {
    r.system.contains("deepening ONE branch")
}
fn has_tool_results(r: &Req) -> bool {
    r.body.to_string().contains("\"role\":\"tool\"")
}

/// A model whose collecting reader reads once, then answers with `closing`; when asked to stop
/// reading (the follow-up request), it answers with the outline. Section readers say DONE.
fn mock(closing: String, n_sections: usize) -> MockLlm {
    MockLlm::start(move |r| match r.kind {
        Kind::Studio if r.has_tools() && is_section_reader(r) => Reply::Text("DONE".into()),
        Kind::Studio if r.has_tools() && r.user_text().contains("Stop reading now") => {
            Reply::Text(outline(n_sections))
        }
        Kind::Studio if r.has_tools() && !has_tool_results(r) => Reply::Tool {
            id: "call_read".into(),
            name: "search_sources".into(),
            args: json!({ "queries": ["基与维数"] }),
        },
        Kind::Studio if r.has_tools() => Reply::Text(closing.clone()),
        Kind::Digest => Reply::Text("本章摘要：基与维数的定义与关系。".into()),
        Kind::Reduce => Reply::Text("全局大纲：向量空间、基、维数。".into()),
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("OK".into()),
    })
}

fn requests(mock: &MockLlm) -> Vec<Req> {
    mock.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.kind == Kind::Studio)
        .cloned()
        .collect()
}

fn lecture(env: &Env, nid: &str) -> String {
    env.ok(
        "studio.create",
        json!({ "notebook_id": nid, "artifact_type": "lecture", "config": { "topic": "基与维数" } }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The reader's closing reply is the outline: no separate planner request, the reader saw the
/// passages it read, and a two-section lecture does not read the sources again per section.
#[test]
fn reader_writes_the_outline_and_small_lectures_do_not_reread() {
    let mock = mock(outline(2), 2);
    let env = Env::new("reader_outline");
    let nid = big_notebook(&env);
    // a realistic window: the collected passages fit in a section prompt
    env.profile(&mock, 128_000);

    let id = lecture(&env, &nid);
    let art = studio_wait(&env, &id, "READY", 90);

    let reqs = requests(&mock);
    assert!(
        !reqs.iter().any(|r| r.system.contains("lecture planner")),
        "no separate planner request"
    );
    let readers: Vec<&Req> = reqs.iter().filter(|r| is_collect_reader(r)).collect();
    assert_eq!(readers.len(), 2, "one reading round and the closing reply");
    let second = readers[1].body.to_string();
    assert!(
        second.contains("向量空间的基是线性无关"),
        "the tool result shows the passages read: {second}"
    );
    assert!(
        !reqs.iter().any(is_section_reader),
        "a small lecture reuses the collected evidence"
    );
    let payload = art["payload"].to_string();
    assert!(payload.contains("基与维数讲解"), "{payload}");
}

/// A closing reply that is not the outline is followed by one request in the same conversation
/// asking for it; the planner is still not needed.
#[test]
fn an_invalid_closing_reply_is_asked_again_in_the_same_conversation() {
    let mock = mock("DONE".into(), 2);
    let env = Env::new("reader_outline_again");
    let nid = big_notebook(&env);
    env.profile(&mock, 4000);

    let id = lecture(&env, &nid);
    studio_wait(&env, &id, "READY", 90);

    let reqs = requests(&mock);
    let again: Vec<&Req> = reqs
        .iter()
        .filter(|r| r.user_text().contains("Stop reading now"))
        .collect();
    assert_eq!(again.len(), 1, "asked once more");
    assert!(has_tool_results(again[0]), "in the reading conversation");
    assert!(!reqs.iter().any(|r| r.system.contains("lecture planner")));
}

/// A lecture of many sections over large sources still reads each section's passages.
#[test]
fn large_lectures_still_read_per_section() {
    let mock = mock(outline(5), 5);
    let env = Env::new("reader_outline_large");
    let nid = big_notebook(&env);
    env.profile(&mock, 4000);

    let id = lecture(&env, &nid);
    studio_wait(&env, &id, "READY", 120);

    let section_reads = requests(&mock)
        .into_iter()
        .filter(is_section_reader)
        .count();
    assert!(section_reads >= 5, "each section reads: {section_reads}");
}
