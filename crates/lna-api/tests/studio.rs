//! Studio pipeline behaviour against a scripted model (E1, E2, E9, E10, E11).
mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

fn studio_wait(env: &Env, id: &str, status: &str, secs: u64) -> Value {
    let mut last = Value::Null;
    env.wait(secs, || {
        last = env.ok("studio.get", json!({ "artifact_id": id }));
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
    env.profile(&mock, 32000, false);
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
        assert!(bytes.len() > 20, "{ty}/{fmt} export is empty");
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
    env.profile(&mock, 32000, false);
    let id = create(&env, &nid, "quiz", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 30);
    assert_eq!(art["error_code"], json!("OUTPUT_INVALID"), "{art}");
}

/// E1: unparseable output is repaired at most twice, then fails with OUTPUT_INVALID.
#[test]
fn unparseable_output_is_repaired_twice_then_fails() {
    let mock = MockLlm::start(|_| Reply::Text("这不是 JSON".into()));
    let env = Env::new("repair");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000, false);
    let id = create(&env, &nid, "flashcards", json!({}));
    let art = studio_wait(&env, &id, "FAILED", 30);
    assert_eq!(art["error_code"], json!("OUTPUT_INVALID"), "{art}");
    // 1 generation + 2 repair turns
    let n = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.kind == Kind::Studio || r.system.contains("JSON 修复器"))
        .count();
    assert_eq!(n, 3);
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
    assert_eq!(est["requires_confirmation"], json!(true));
    assert_eq!(
        env.err(
            "studio.create",
            json!({ "notebook_id": nid, "artifact_type": "quiz", "config": {} })
        ),
        "INVALID_ARGUMENT"
    );
    let id = create(&env, &nid, "quiz", json!({ "confirmed": true }));
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
    env.profile(&mock, 4000, false);

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
    assert!(gen.user_text().contains("主题相关证据"));

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
    env.profile(&mock, 4000, false);
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
    env.profile(&mock, 32000, false);
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
    env.profile(&mock, 32000, false);
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
    assert_eq!(
        mock.max_inflight_of(Kind::Studio),
        1,
        "never more than one concurrent Studio request for one profile"
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
    env.profile(&mock, 32000, false);
    let id = create(&env, &nid, "report", json!({}));
    env.wait(20, || !mock.of_kind(Kind::Studio).is_empty());
    env.ok("studio.cancel", json!({ "artifact_id": id }));
    std::thread::sleep(Duration::from_millis(1500));
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(art["status"], json!("CANCELLED"), "{art}");
}

/// E11: after a source is added, finished artifacts report that their sources changed.
#[test]
fn artifacts_report_stale_sources_after_the_source_set_changes() {
    let mock = studio_mock();
    let env = Env::new("stale");
    let nid = small_notebook(&env);
    env.profile(&mock, 32000, false);
    let id = create(&env, &nid, "flashcards", json!({}));
    let art = studio_wait(&env, &id, "READY", 30);
    assert_eq!(art["sources_stale"], json!(false));
    env.add_text(&nid, "新增.md", "# 新增\n\n零空间。");
    env.wait_sources_ready(&nid, 2);
    let art = env.ok("studio.get", json!({ "artifact_id": id }));
    assert_eq!(art["sources_stale"], json!(true), "{art}");
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
    env.profile(&mock, 32000, false);
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
    env.profile(&mock, 32000, false);

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
