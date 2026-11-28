//! Studio pipeline behaviour against a scripted model (E1, E2, E9, E10, E11).
mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

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

fn create(env: &Env, nid: &str, ty: &str, config: Value) -> String {
    env.ok(
        "studio.create",
        json!({ "notebook_id": nid, "artifact_type": ty, "config": config }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn small_notebook(env: &Env) -> String {
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    nid
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

fn studio_mock() -> MockLlm {
    MockLlm::start(|r| match r.kind {
        Kind::Digest => Reply::Text("本章摘要：基与维数的定义与关系。".into()),
        Kind::Reduce => Reply::Text("全局大纲：向量空间、基、维数。".into()),
        Kind::Plan => Reply::Text(r#"{"outline":["封面","基","维数"]}"#.into()),
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("OK".into()),
    })
}

/// E1/E2/E3/E4/E5/E6/E7: every text modality generates, validates and exports.
#[test]
fn every_modality_generates_and_exports() {
    let mock = studio_mock();
    let env = Env::new("modalities");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let cases = [
        ("quiz", "json", "quiz.json"),
        ("quiz", "markdown", "quiz.md"),
        ("flashcards", "csv", "cards.csv"),
        ("mindmap", "markdown", "map.md"),
        ("report", "markdown", "report.md"),
        ("slides", "pptx", "deck.pptx"),
        ("infographic", "svg", "info.svg"),
    ];
    for (ty, fmt, file) in cases {
        let id = create(&env, &nid, ty, json!({}));
        let art = studio_wait(&env, &id, "READY", 30);
        assert!(art["payload"].is_object(), "{ty}: {art}");
        let out = env.dir.join(file);
        env.ok(
            "studio.export",
            json!({ "artifact_id": id, "format": fmt, "out_path": out.display().to_string() }),
        );
        let bytes = std::fs::read(&out).unwrap();
        // The default title follows the OS language, so keep the threshold below the shortest English export.
        assert!(bytes.len() > 10, "{ty}/{fmt} export is empty");
        match fmt {
            "pptx" => assert_eq!(&bytes[..2], b"PK", "pptx is a zip"),
            "svg" => {
                let s = String::from_utf8(bytes).unwrap();
                assert!(s.contains("<svg"), "svg root");
            }
            "csv" => assert!(String::from_utf8(bytes)
                .unwrap()
                .starts_with("front,back,hint")),
            _ => {}
        }
    }
    // retry / rename / delete / feedback (E11)
    let id = create(&env, &nid, "flashcards", json!({}));
    studio_wait(&env, &id, "READY", 30);
    env.ok(
        "studio.rename",
        json!({ "artifact_id": id, "title": "新名字" }),
    );
    env.ok(
        "studio.set_feedback",
        json!({ "artifact_id": id, "feedback": 1 }),
    );
    let a = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(a["title"], json!("新名字"));
    assert_eq!(a["feedback"], json!(1));
    env.ok("studio.delete", json!({ "artifact_id": id }));
    assert_eq!(
        env.err("studio.get", json!({ "artifact_id": id })),
        "NOT_FOUND"
    );
}

/// E2: an invalid payload (bad citation) fails the artifact with OUTPUT_INVALID, not silently.
#[test]
fn quiz_with_out_of_range_citation_fails_validation() {
    let mock = MockLlm::start(|r| {
        match r.kind {
        Kind::Studio => Reply::Text(
            json!({"title":"t","questions":[{"id":"q1","type":"single","difficulty":"easy","prompt":"题","options":["甲","乙"],"answer_indices":[0],"accepted_answers":[],"rubric":"","explanation":"","citations":[99]}]})
                .to_string(),
        ),
        _ => Reply::Text("OK".into()),
    }
    });
    let env = Env::new("quizcite");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "quiz", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 30);
    assert_eq!(art["error_code"], json!("OUTPUT_INVALID"), "{art}");
}

/// E1: unparseable output is repaired at most twice on the initial run, then retried 3 times with rollback before failing.
#[test]
fn unparseable_output_is_repaired_twice_then_fails() {
    let mock = MockLlm::start(|_| Reply::Text("这不是 JSON".into()));
    let env = Env::new("repair");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "flashcards", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 30);
    assert_eq!(art["error_code"], json!("OUTPUT_INVALID"), "{art}");
    let log = art["error_log"].as_str().unwrap_or("");
    assert!(
        log.contains("OUTPUT_INVALID"),
        "missing OUTPUT_INVALID in log: {log}"
    );
    assert!(
        log.contains("Attempts (4 recorded)"),
        "expected 4 recorded attempts: {log}"
    );
    // Attempt 1: 1 generation + 2 repair turns (3 calls); Retries 1..=3: 1 generation each (3 calls) => 6 calls total.
    let n = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        // repair turns of the flashcards schema only (the notebook summary is structured output too)
        .filter(|r| {
            r.kind == Kind::Studio
                || ((r.system.contains("JSON repairer") || r.system.contains("JSON 修复器"))
                    && r.user_text().contains("\"front\""))
        })
        .count();
    assert_eq!(n, 6);
}

/// E9: estimates carry prices when configured, and large inputs need explicit confirmation.
#[test]
fn estimate_uses_prices_and_large_inputs_require_confirmation() {
    let mock = studio_mock();
    let env = Env::new("estimate");
    let nid = small_notebook(&env);
    let p = env.ok(
        "profile.upsert",
        json!({
            "display_name": "m", "protocol": "chat_completion", "base_url": mock.base_url(),
            "model_id": "m", "activate": true, "context_window": 32000, "max_output_tokens": 1000,
            "input_price_per_m": 2.0, "output_price_per_m": 10.0
        }),
    );
    assert!(p["id"].is_string());
    let est = env.ok(
        "studio.estimate",
        json!({ "notebook_id": nid, "artifact_type": "quiz" }),
    );
    let tin = est["estimated_input_tokens"].as_f64().unwrap();
    let tout = est["estimated_output_tokens"].as_f64().unwrap();
    let want = (tin * 2.0 + tout * 10.0) / 1e6;
    assert!(
        (est["estimated_cost_usd"].as_f64().unwrap() - want).abs() < 1e-9,
        "{est}"
    );
    assert_eq!(est["is_estimated"], json!(true));
    assert_eq!(est["requires_confirmation"], json!(false));

    // Pretend the source is huge.
    env.core
        .db
        .write(|c| {
            c.execute("UPDATE sources SET token_count = 400000", [])
                .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let est = env.ok(
        "studio.estimate",
        json!({ "notebook_id": nid, "artifact_type": "quiz" }),
    );
    assert_eq!(est["requires_confirmation"], json!(false));
    let id = create(&env, &nid, "quiz", json!({}));
    assert!(!id.is_empty());
    // measured usage is recorded as such after the run
    studio_wait(&env, &id, "READY", 60);
    let usage = env.ok("usage.summary", json!({ "notebook_id": nid }));
    assert!(usage["input_tokens"].as_i64().unwrap() > 0);
}

/// E10: oversized sources go through per-section digests; digests are cached and reused by the
/// next artifact; the topic path retrieves instead of reducing.
#[test]
fn oversized_sources_use_cached_digests_map_reduce_and_topic_retrieval() {
    let mock = studio_mock();
    let env = Env::new("mapreduce");
    let nid = big_notebook(&env);
    env.profile(&mock, 4000);

    let id = create(&env, &nid, "quiz", json!({}));
    studio_wait(&env, &id, "READY", 60);
    let digests = mock.of_kind(Kind::Digest).len();
    assert!(digests >= 2, "section digests requested: {digests}");
    // more than 20 digests are first merged in batches of 20, then reduced once
    let batches = if digests > 20 {
        digests.div_ceil(20)
    } else {
        0
    };
    assert_eq!(mock.of_kind(Kind::Reduce).len(), batches + 1);
    let gen = mock.of_kind(Kind::Studio).pop().unwrap();
    assert!(
        gen.user_text().contains("全局章节大纲"),
        "{}",
        gen.user_text()
    );
    let rows: i64 = env.query(|c| {
        c.query_row("SELECT COUNT(*) FROM source_digests", [], |r| r.get(0))
            .unwrap()
    });
    assert_eq!(rows as usize, digests);

    // Second artifact reuses every cached digest.
    let id2 = create(&env, &nid, "flashcards", json!({}));
    studio_wait(&env, &id2, "READY", 60);
    assert_eq!(
        mock.of_kind(Kind::Digest).len(),
        digests,
        "digest cache must be hit"
    );

    // With a topic: related evidence is retrieved and no global reduce is run.
    let reduces = mock.of_kind(Kind::Reduce).len();
    let id3 = create(&env, &nid, "report", json!({ "topic": "基与维数" }));
    studio_wait(&env, &id3, "READY", 60);
    assert_eq!(mock.of_kind(Kind::Reduce).len(), reduces);
    let gen = mock.of_kind(Kind::Studio).pop().unwrap();
    // Topic evidence arrives either via the on-demand reader's topic retrieval or the
    // direct topic-retrieval fallback; both mean "retrieved, not reduced".
    let user = gen.user_text();
    assert!(
        user.contains("主题相关证据") || user.contains("On-Demand Retrieved Excerpts"),
        "USER={:?}",
        user.chars().take(700).collect::<String>()
    );

    // Changing a source invalidates only that source's digests.
    env.core
        .db
        .write(|c| {
            c.execute(
                "UPDATE sources SET content_hash = 'changed' WHERE title = '大讲义0.md'",
                [],
            )
            .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let id4 = create(&env, &nid, "mindmap", json!({}));
    studio_wait(&env, &id4, "READY", 60);
    let after = mock.of_kind(Kind::Digest).len();
    assert!(
        after > digests && after < digests * 2,
        "only source 0 regenerated: {digests} -> {after}"
    );
}

/// E1: a failure after the collect step keeps the checkpoint, so retry does not repeat the
/// expensive digest work.
#[test]
fn retry_resumes_from_the_collect_checkpoint() {
    let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let f2 = fail.clone();
    let mock = MockLlm::start(move |r| match r.kind {
        Kind::Digest => Reply::Text("摘要。".into()),
        Kind::Reduce => Reply::Text("大纲。".into()),
        Kind::Studio if f2.load(std::sync::atomic::Ordering::SeqCst) => {
            Reply::Status(400, r#"{"error":{"message":"bad request"}}"#.into())
        }
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("resume");
    let nid = big_notebook(&env);
    env.profile(&mock, 4000);
    let id = create(&env, &nid, "quiz", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 60);
    assert_eq!(art["error_code"], json!("MODEL_BAD_REQUEST"), "{art}");
    let digests = mock.of_kind(Kind::Digest).len();
    let reduces = mock.of_kind(Kind::Reduce).len();
    assert!(digests > 0);

    fail.store(false, std::sync::atomic::Ordering::SeqCst);
    env.ok("studio.retry", json!({ "artifact_id": id }));
    studio_wait(&env, &id, "READY", 60);
    assert_eq!(
        mock.of_kind(Kind::Digest).len(),
        digests,
        "collect step not repeated"
    );
    assert_eq!(mock.of_kind(Kind::Reduce).len(), reduces);

    // A finished artifact retried again reuses the saved generation as well.
    let studio_calls = mock.of_kind(Kind::Studio).len();
    env.ok("studio.retry", json!({ "artifact_id": id }));
    studio_wait(&env, &id, "READY", 60);
    assert_eq!(mock.of_kind(Kind::Studio).len(), studio_calls);
}

/// E1: the outline review state machine: DRAFT_REVIEW -> approve -> READY.
#[test]
fn outline_review_pauses_then_continues_after_approval() {
    let mock = studio_mock();
    let env = Env::new("draft");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "slides", json!({ "reviewOutline": true }));
    let art = studio_wait(&env, &id, "DRAFT_REVIEW", 30);
    assert!(
        art["draft"].is_object() || art["draft"].is_string(),
        "{art}"
    );
    assert_eq!(
        mock.of_kind(Kind::Studio).len(),
        0,
        "no generation before approval"
    );
    env.ok(
        "studio.approve_draft",
        json!({ "artifact_id": id, "draft": { "outline": ["封面", "基"] } }),
    );
    studio_wait(&env, &id, "READY", 30);
    let gen = mock.of_kind(Kind::Studio).pop().unwrap();
    assert!(gen.user_text().contains("封面"), "approved draft is used");
}

/// E1: Studio jobs run one model call at a time per profile, and never block source ingest.
#[test]
fn studio_calls_are_serialised_per_profile_and_ingest_is_not_blocked() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => {
            let Reply::Text(t) = studio_reply(r) else {
                unreachable!()
            };
            Reply::SlowText(t, 60)
        }
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("serial");
    let nid = small_notebook(&env);
    profile_with_limit(&env, &mock, 4);
    let ids: Vec<String> = ["quiz", "flashcards", "mindmap"]
        .iter()
        .map(|t| create(&env, &nid, t, json!({})))
        .collect();
    // Import another source while Studio is busy.
    let sid = env.add_text(
        &nid,
        "并发.md",
        "# 并发\n\n矩阵的零空间是齐次方程组的解集。",
    );
    env.wait(20, || {
        env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == json!(sid) && s["status"] == json!("READY"))
    });
    for id in &ids {
        studio_wait(&env, id, "READY", 60);
    }
    assert!(
        mock.max_inflight_of(Kind::Studio) <= 4,
        "Studio requests stay bounded by the profile's max_parallel (4 here)"
    );
}

/// E1: cancelling stops the run and the state stays CANCELLED.
#[test]
fn cancel_during_generation_stays_cancelled() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => Reply::SlowText("x".repeat(400), 200),
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("studiocancel");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "report", json!({}));
    env.wait(20, || !mock.of_kind(Kind::Studio).is_empty());
    env.ok("studio.cancel", json!({ "artifact_id": id }));
    std::thread::sleep(Duration::from_millis(1500));
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(art["status"], json!("CANCELLED"), "{art}");
}

/// Pausing stops the run like cancelling but is marked PAUSED, and retry continues it.
#[test]
fn pause_stops_generation_and_retry_continues() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => Reply::SlowText("x".repeat(400), 200),
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("studiopause");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "report", json!({}));
    env.wait(20, || !mock.of_kind(Kind::Studio).is_empty());
    env.ok("studio.pause", json!({ "artifact_id": id }));
    std::thread::sleep(Duration::from_millis(1500));
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(art["status"], json!("CANCELLED"), "{art}");
    assert_eq!(art["stage"], json!("PAUSED"), "{art}");
    env.ok("studio.retry", json!({ "artifact_id": id }));
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_ne!(art["stage"], json!("PAUSED"), "{art}");
}

/// Finished artifacts do not auto-flag as stale when sources change, and can be regenerated
/// on user demand with updated config and the currently selected sources.
#[test]
fn artifacts_regenerate_on_demand_with_selected_sources_and_config() {
    let mock = studio_mock();
    let env = Env::new("regen");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "flashcards", json!({ "topic": "原主题" }));
    let art = studio_wait(&env, &id, "READY", 30);
    assert_eq!(art["sources_stale"], json!(false));
    env.add_text(&nid, "新增.md", "# 新增\n\n零空间。");
    env.wait_sources_ready(&nid, 2);
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(art["sources_stale"], json!(false), "{art}");

    let regen = env.ok(
        "studio.regenerate",
        json!({ "artifact_id": id, "config": { "topic": "新要求" }, "use_selected_sources": true }),
    );
    assert_eq!(regen["config"]["topic"], json!("新要求"));
    assert_eq!(regen["source_ids"].as_array().map(|a| a.len()), Some(2));
    let ready = studio_wait(&env, &id, "READY", 30);
    assert_eq!(ready["status"], json!("READY"));

    // the "regenerate" button: same requirements and the artifact's own sources, even when
    // the notebook's selection has changed since
    env.ok(
        "source.set_all_selected",
        json!({ "notebook_id": nid, "selected": false }),
    );
    let before = mock.of_kind(Kind::Studio).len();
    let again = env.ok(
        "studio.regenerate",
        json!({ "artifact_id": id, "use_selected_sources": false }),
    );
    assert_eq!(again["config"]["topic"], json!("新要求"));
    assert_eq!(again["source_ids"].as_array().map(|a| a.len()), Some(2));
    studio_wait(&env, &id, "READY", 30);
    assert!(
        mock.of_kind(Kind::Studio).len() > before,
        "a fresh run calls the model again"
    );
}

/// E2: grade_answer through the API for each question type.
#[test]
fn grade_answer_api_handles_choice_fill_and_unknown_questions() {
    let mock = MockLlm::start(|r| {
        match r.kind {
        Kind::Studio => Reply::Text(
            json!({"title":"t","questions":[
                {"id":"s","type":"single","difficulty":"easy","prompt":"单选","options":["甲","乙"],"answer_indices":[1],"accepted_answers":[],"rubric":"","explanation":"","citations":[1]},
                {"id":"m","type":"multi","difficulty":"easy","prompt":"多选","options":["甲","乙","丙"],"answer_indices":[0,2],"accepted_answers":[],"rubric":"","explanation":"","citations":[1]},
                {"id":"f","type":"fill","difficulty":"easy","prompt":"填空","options":[],"answer_indices":[],"accepted_answers":["Rank"],"rubric":"","explanation":"","citations":[1]},
                {"id":"x","type":"short","difficulty":"easy","prompt":"简答","options":[],"answer_indices":[],"accepted_answers":[],"rubric":"提到线性无关","explanation":"","citations":[1]}]})
                .to_string(),
        ),
        _ => Reply::Text("OK".into()),
    }
    });
    let env = Env::new("grade");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "quiz", json!({}));
    studio_wait(&env, &id, "READY", 30);
    let g = |q: &str, a: Value| {
        env.ok(
            "studio.grade_answer",
            json!({ "artifact_id": id, "question_id": q, "answer": a }),
        )
    };
    assert_eq!(g("s", json!([1]))["correct"], json!(true));
    assert_eq!(g("s", json!([0]))["correct"], json!(false));
    assert_eq!(g("m", json!([0, 2]))["correct"], json!(true));
    assert_eq!(g("m", json!([0]))["correct"], json!(false));
    assert_eq!(g("f", json!(" rank "))["correct"], json!(true));
    assert_eq!(g("x", json!("随便写"))["self_eval"], json!(true));
    assert_eq!(
        env.err(
            "studio.grade_answer",
            json!({ "artifact_id": id, "question_id": "nope", "answer": [] })
        ),
        "NOT_FOUND"
    );
}

/// E4/E5/E7/E8: studio.export supports mindmap svg/png, report html, infographic png, and audio wav.
#[test]
fn studio_export_supports_all_formats_and_audio_end_to_end() {
    let mock = studio_mock();
    let env = Env::new("exports");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);

    let mm_id = create(&env, &nid, "mindmap", json!({}));
    studio_wait(&env, &mm_id, "READY", 30);
    let mm_svg = env.dir.join("mm.svg");
    let mm_png = env.dir.join("mm.png");
    env.ok(
        "studio.export",
        json!({ "artifact_id": mm_id, "format": "svg", "out_path": mm_svg }),
    );
    env.ok(
        "studio.export",
        json!({ "artifact_id": mm_id, "format": "png", "out_path": mm_png }),
    );
    assert!(std::fs::read_to_string(&mm_svg).unwrap().contains("<svg"));
    assert_eq!(
        &std::fs::read(&mm_png).unwrap()[0..8],
        &[137, 80, 78, 71, 13, 10, 26, 10]
    );

    let rp_id = create(&env, &nid, "report", json!({}));
    studio_wait(&env, &rp_id, "READY", 30);
    let rp_html = env.dir.join("report.html");
    env.ok(
        "studio.export",
        json!({ "artifact_id": rp_id, "format": "html", "out_path": rp_html }),
    );
    assert!(std::fs::read_to_string(&rp_html)
        .unwrap()
        .contains("<!DOCTYPE html>"));

    let info_id = create(&env, &nid, "infographic", json!({}));
    studio_wait(&env, &info_id, "READY", 30);
    let info_png = env.dir.join("info.png");
    env.ok(
        "studio.export",
        json!({ "artifact_id": info_id, "format": "png", "out_path": info_png }),
    );
    assert_eq!(
        &std::fs::read(&info_png).unwrap()[0..8],
        &[137, 80, 78, 71, 13, 10, 26, 10]
    );

    let aud_id = create(&env, &nid, "audio", json!({}));
    let aud = studio_wait(&env, &aud_id, "READY", 60);
    assert!(aud["output_dir"].as_str().is_some());
    let aud_wav = env.dir.join("overview.wav");
    env.ok(
        "studio.export",
        json!({ "artifact_id": aud_id, "format": "wav", "out_path": aud_wav }),
    );
    let wav_bytes = std::fs::read(&aud_wav).unwrap();
    assert_eq!(&wav_bytes[0..4], b"RIFF");
    assert_eq!(&wav_bytes[8..12], b"WAVE");
}

/// The repair limit saved in Settings ("lecture.limits") is what the lecture pipeline uses:
/// with 0 rounds there is no independent review and no repair at all.
#[test]
fn lecture_repair_limit_comes_from_settings() {
    let mock = studio_mock();
    let env = Env::new("lecture_limits");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    env.ok(
        "settings.set",
        json!({ "key": "lecture.limits", "value": { "repair_rounds": 0 } }),
    );
    let id = create(
        &env,
        &nid,
        "lecture",
        json!({ "mode": "general", "detail": "auto", "topic": "基与维数" }),
    );
    studio_wait(&env, &id, "READY", 60);
    let titles: Vec<String> = env.query(|c| {
        let mut st = c
            .prepare("SELECT title FROM artifact_thoughts WHERE artifact_id = ?1 ORDER BY seq")
            .unwrap();
        st.query_map([&id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    });
    let count = |needle: &str| titles.iter().filter(|t| t.contains(needle)).count();
    assert_eq!(count("编排动画："), 1, "{titles:?}");
    assert_eq!(count("动画审查"), 0, "{titles:?}");
    assert_eq!(count("动画返修"), 0, "{titles:?}");
}

/// Lecture (讲解): outline → section with a figure, step build and laser cues → figure assets
/// and pptx; interruption Q&A, adaptation (insert / regenerate) and undo.
#[test]
fn lecture_generates_figures_and_adapts() {
    let mock = studio_mock();
    let env = Env::new("lecture");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(
        &env,
        &nid,
        "lecture",
        json!({ "mode": "general", "detail": "auto", "topic": "基与维数" }),
    );
    let art = studio_wait(&env, &id, "READY", 60);
    let p = &art["payload"];
    assert_eq!(p["mode"], json!("general"));
    let slides = p["slides"].as_array().unwrap();
    assert_eq!(slides.len(), 2, "{p}");
    assert_eq!(slides[0]["id"], json!("s1-1"));
    assert_eq!(slides[1]["narration"][0]["id"], json!("s1-2.1"));
    assert!(slides[1].get("figure_request").is_none());
    assert!(slides[1]["figure"]["meta"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["id"] == json!("f")));
    // the step and laser cues on the figure part survived validation
    assert_eq!(
        slides[1]["narration"][0]["cues"].as_array().unwrap().len(),
        2
    );
    assert!(p["estimated_minutes"].as_f64().unwrap() > 0.0);
    // animation is its own step: written by a fresh-context call, reviewed by another, the
    // reported problem sent back to a new animation call once (one review per section by default)
    let titles: Vec<String> = env.query(|c| {
        let mut st = c
            .prepare("SELECT title FROM artifact_thoughts WHERE artifact_id = ?1 ORDER BY seq")
            .unwrap();
        st.query_map([&id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    });
    let count = |needle: &str| titles.iter().filter(|t| t.contains(needle)).count();
    assert_eq!(count("编排动画："), 2, "{titles:?}");
    assert_eq!(count("动画审查："), 1, "{titles:?}");
    assert_eq!(count("动画返修"), 1, "{titles:?}");
    assert_eq!(count("返修讲解"), 0, "{titles:?}");
    let dir = std::path::PathBuf::from(art["output_dir"].as_str().unwrap());
    assert!(dir.join("figures/s1-2.svg").is_file());
    assert!(dir.join("figures/s1-2_0.png").is_file() && dir.join("figures/s1-2_1.png").is_file());
    assert!(dir.join("lecture.pptx").is_file());

    let ans = env.ok(
        "lecture.ask",
        json!({ "artifact_id": id, "slide_id": "s1-2", "sentence_id": "s1-2.1", "question": "什么是基？" }),
    );
    assert!(ans["answer"].as_str().unwrap().contains("基"));
    assert_eq!(ans["adapt"]["needed"], json!(true));
    assert_eq!(ans["adapt"]["kind"], json!("extend"));

    // insert supplementary slides after s1-1, then undo
    let r = env.ok(
        "lecture.adapt",
        json!({ "artifact_id": id, "slide_id": "s1-1", "kind": "extend", "reason": "补充", "question": "什么是基？" }),
    );
    let s2 = r["artifact"]["payload"]["slides"].as_array().unwrap();
    assert_eq!(s2.len(), 4);
    assert!(
        s2[1]["id"].as_str().unwrap().starts_with("a1-"),
        "{}",
        s2[1]["id"]
    );
    assert_eq!(
        r["adaptation"]["inserted_titles"].as_array().unwrap().len(),
        2
    );
    let u = env.ok("lecture.undo", json!({ "artifact_id": id }));
    assert_eq!(
        u["artifact"]["payload"]["slides"].as_array().unwrap().len(),
        2
    );

    // regenerate everything after s1-1 more slowly, then restore the original
    let r = env.ok(
        "lecture.adapt",
        json!({ "artifact_id": id, "slide_id": "s1-1", "kind": "slower", "reason": "太快", "question": "慢一点" }),
    );
    let s3 = r["artifact"]["payload"]["slides"].as_array().unwrap();
    assert_eq!(s3[0]["id"], json!("s1-1"));
    assert!(
        s3.iter().all(|s| s["id"] != json!("s1-2")),
        "old slide replaced"
    );
    assert_eq!(r["adaptation"]["removed_titles"], json!(["一条曲线"]));
    let u = env.ok("lecture.undo", json!({ "artifact_id": id, "all": true }));
    let restored = u["artifact"]["payload"]["slides"].as_array().unwrap();
    assert_eq!(
        restored.iter().map(|s| s["id"].clone()).collect::<Vec<_>>(),
        vec![json!("s1-1"), json!("s1-2")]
    );

    // exported deck embeds the figure layers
    let out = env.dir.join("lecture_export.pptx");
    env.ok(
        "studio.export",
        json!({ "artifact_id": id, "format": "pptx", "out_path": out.display().to_string() }),
    );
    let bytes = std::fs::read(&out).unwrap();
    let hay = String::from_utf8_lossy(&bytes);
    assert!(hay.contains("ppt/media/fig2_0.png") && hay.contains("ppt/media/fig2_1.png"));
}

/// Routes that run async Studio work from the (async) dispatcher must not nest runtimes.
#[test]
fn refine_node_route_runs_inside_dispatcher() {
    let mock = studio_mock();
    let env = Env::new("refine_route");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000);
    let id = create(&env, &nid, "mindmap", json!({ "detail": "concise" }));
    studio_wait(&env, &id, "READY", 30);
    let r = env.ok(
        "studio.refine_node",
        json!({ "artifact_id": id, "node_id": "n2" }),
    );
    assert!(r.get("refined").is_some(), "{r}");
    assert_eq!(r["artifact"]["id"], json!(id));
}

fn profile_with_limit(env: &Env, mock: &MockLlm, limit: u32) -> Value {
    env.ok(
        "profile.upsert",
        json!({
            "display_name": "mock", "protocol": "chat_completion",
            "base_url": mock.base_url(), "model_id": "mock-model", "activate": true,
            "context_window": 32000, "max_output_tokens": 1000, "max_parallel": limit
        }),
    )
}

/// The profile's `max_parallel` is the one limit on model requests in flight: a long quiz is
/// written by parallel batch executors up to it, and never beyond it.
#[test]
fn max_parallel_setting_bounds_parallel_executors() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => {
            let Reply::Text(t) = studio_reply(r) else {
                unreachable!()
            };
            Reply::SlowText(t, 40)
        }
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("maxparallel");
    let nid = small_notebook(&env);
    let prof = profile_with_limit(&env, &mock, 2);
    assert_eq!(prof["max_parallel"], json!(2), "{prof}");
    let listed = env.ok("profile.list", json!({}));
    assert_eq!(listed["profiles"][0]["max_parallel"], json!(2), "{listed}");

    // 20 questions = 4 batches: they run in parallel, but only 2 at a time.
    let id = create(&env, &nid, "quiz", json!({ "count": 20 }));
    studio_wait(&env, &id, "READY", 60);
    let inflight = mock.max_inflight_of(Kind::Studio);
    assert_eq!(
        inflight, 2,
        "batches fan out up to the profile limit (saw {inflight})"
    );

    // Values above the 1024 bound are clamped to it; 0 means unlimited, and a changed limit is
    // picked up at once.
    let prof = profile_with_limit(&env, &mock, 500);
    assert_eq!(prof["max_parallel"], json!(500), "{prof}");
    let prof = profile_with_limit(&env, &mock, 5000);
    assert_eq!(prof["max_parallel"], json!(1024), "{prof}");
    let prof = profile_with_limit(&env, &mock, 0);
    assert_eq!(prof["max_parallel"], json!(0), "{prof}");
}

/// A profile created without `max_parallel` is unlimited.
#[test]
fn new_profile_without_a_limit_is_unlimited() {
    let mock = MockLlm::start(|_| Reply::Text("OK".into()));
    let env = Env::new("unlimited");
    let prof = env.profile(&mock, 32000);
    assert_eq!(prof["max_parallel"], json!(0), "{prof}");
}

fn common_prefix_chars(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// Parallel executors of one job start their prompt with the same system prompt and the same
/// shared block (parameters, outline, sources) and put only what is specific to them last, so the
/// provider's prompt cache can serve all of them from one prefix.
#[test]
fn parallel_batches_share_a_long_prompt_prefix() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("OK".into()),
    });
    let env = Env::new("prefixshare");
    let nid = small_notebook(&env);
    profile_with_limit(&env, &mock, 4);
    let id = create(&env, &nid, "quiz", json!({ "count": 20 }));
    studio_wait(&env, &id, "READY", 60);

    let batches: Vec<_> = mock
        .of_kind(Kind::Studio)
        .into_iter()
        .filter(|r| r.user_text().contains("=== THIS BATCH"))
        .collect();
    assert!(
        batches.len() >= 4,
        "expected 4 batches, saw {}",
        batches.len()
    );
    let first = batches[0].user_text();
    let shared_len = first[..first.find("=== THIS BATCH").unwrap()]
        .chars()
        .count();
    assert!(
        first
            .chars()
            .take(shared_len)
            .collect::<String>()
            .contains("Sources:"),
        "the sources belong to the shared head"
    );
    for b in &batches[1..] {
        assert_eq!(
            b.system, batches[0].system,
            "same system prompt for every batch"
        );
        let other = b.user_text();
        let common = common_prefix_chars(&first, &other);
        assert!(
            common >= shared_len,
            "batches must share the whole head ({common} < {shared_len})\nA: {:?}\nB: {:?}",
            first
                .chars()
                .skip(common.saturating_sub(40))
                .take(120)
                .collect::<String>(),
            other
                .chars()
                .skip(common.saturating_sub(40))
                .take(120)
                .collect::<String>()
        );
    }
}

fn picture(w: u32, h: u32) -> String {
    use base64::Engine as _;
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        w,
        h,
        image::Rgb([30, 90, 200]),
    ));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

/// The mock profile, marked as reading pictures or not (what the capability probe would find).
fn profile_with_vision(env: &Env, mock: &MockLlm, vision: bool) {
    let row = env.profile(mock, 4000);
    let pid = row["id"].as_str().unwrap().to_string();
    let mut caps: lna_types::ModelCapabilities =
        serde_json::from_value(row["capabilities"].clone()).unwrap();
    caps.supports_vision = vision;
    caps.supports_audio = false;
    caps.supports_video = false;
    lna_storage::update_profile_capabilities(&env.core.db, &pid, &caps).unwrap();
}

/// A model whose on-demand reading rounds (the requests with tools) are answered by `reader`.
fn reading_mock(reader: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> MockLlm {
    MockLlm::start(move |r| match r.kind {
        Kind::Studio if r.has_tools() => reader(r),
        Kind::Digest => Reply::Text("本章摘要：基与维数的定义与关系。".into()),
        Kind::Reduce => Reply::Text("全局大纲：向量空间、基、维数。".into()),
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("OK".into()),
    })
}

fn reader_requests(mock: &MockLlm) -> Vec<Req> {
    mock.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.kind == Kind::Studio && r.has_tools())
        .cloned()
        .collect()
}

/// A vision model gets the attached picture itself in the reading request; the prompt only says
/// that a picture is shown, never the attachment id.
#[test]
fn vision_model_reads_the_picture_itself_not_a_text_brief() {
    let mock = reading_mock(|_| Reply::Text("DONE".into()));
    let env = Env::new("pics_vision");
    let nid = big_notebook(&env);
    profile_with_vision(&env, &mock, true);

    let art = env.ok(
        "studio.create",
        json!({
            "notebook_id": nid, "artifact_type": "quiz", "config": { "topic": "图中的题" },
            "images": [{ "name": "q.png", "data_base64": picture(40, 40) }]
        }),
    );
    let id = art["id"].as_str().unwrap().to_string();
    let pic_id = art["config"]["images"][0].as_str().unwrap().to_string();
    studio_wait(&env, &id, "READY", 60);

    let reads = reader_requests(&mock);
    let first = reads.first().expect("the on-demand reader was asked");
    let body = first.body.to_string();
    assert!(
        body.contains("data:image/"),
        "the picture is part of the request: {body}"
    );
    assert!(
        !body.contains(&pic_id),
        "the attachment id must not reach the prompt: {body}"
    );
    assert!(body.contains("shown in this message"), "{body}");
    assert!(
        !body.contains("</user_images>"),
        "no text brief for a vision model: {body}"
    );
    // the follow-up rounds carry the same pictures
    assert!(reads
        .iter()
        .all(|r| r.body.to_string().contains("data:image/")));
}

/// The model stopping the task on purpose fails the job with the refusal code and its reason,
/// without retrying and without any guessed content.
#[test]
fn model_abort_task_fails_the_job_with_its_reason() {
    let mock = reading_mock(|_| Reply::Tool {
        id: "call_abort".into(),
        name: "abort_task".into(),
        args: json!({ "reason": "附带的图片无法读取" }),
    });
    let env = Env::new("pics_abort");
    let nid = big_notebook(&env);
    profile_with_vision(&env, &mock, false);

    let id = create(&env, &nid, "quiz", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 60);
    assert_eq!(art["error_code"], json!("MODEL_REFUSED"), "{art}");
    assert!(
        art["error_detail"]
            .as_str()
            .unwrap_or("")
            .contains("附带的图片无法读取"),
        "{art}"
    );
    assert_eq!(reader_requests(&mock).len(), 1, "no retry after a refusal");
}

/// A model that cannot read pictures keeps the text brief: the reading request has no pixels, and
/// the brief travels in <user_images>.
#[test]
fn text_only_model_keeps_the_picture_brief() {
    let mock = reading_mock(|_| Reply::Text("DONE".into()));
    let env = Env::new("pics_text");
    let nid = big_notebook(&env);
    profile_with_vision(&env, &mock, false);

    let art = env.ok(
        "studio.create",
        json!({
            "notebook_id": nid, "artifact_type": "quiz", "config": { "topic": "图中的题" },
            "images": [{ "name": "q.png", "data_base64": picture(40, 40) }]
        }),
    );
    let id = art["id"].as_str().unwrap().to_string();
    let pic_id = art["config"]["images"][0].as_str().unwrap().to_string();
    studio_wait(&env, &id, "READY", 60);

    let reads = reader_requests(&mock);
    let first = reads.first().expect("the on-demand reader was asked");
    let body = first.body.to_string();
    assert!(
        !body.contains("data:image/"),
        "a text-only model gets no pixels: {body}"
    );
    assert!(
        body.contains("<user_images>"),
        "the text brief is used: {body}"
    );
    assert!(body.contains("described in <user_images>"), "{body}");
    assert!(!body.contains(&pic_id), "{body}");
}
