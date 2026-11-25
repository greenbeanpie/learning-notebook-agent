//! `.lna` export/import (F2), Markdown export (F3), notes (F1) and the diagnostics bundle (S2).
mod common;

use common::*;
use serde_json::{json, Value};
use std::io::Write;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn source_ids(env: &Env, nid: &str) -> Vec<String> {
    env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

/// (title, content) of the evidence retrieved for `query`, in rank order.
fn retrieve(env: &Env, nid: &str, query: &str) -> Vec<(String, String)> {
    let ids = source_ids(env, nid);
    let out = rt()
        .block_on(
            env.core
                .retrieval
                .search(nid, &[query.to_string()], &ids, 4000, 8),
        )
        .unwrap();
    out.evidences
        .iter()
        .map(|e| (e.source_title.clone(), e.content.clone()))
        .collect()
}

const DOCS: [(&str, &str); 3] = [
    ("向量空间.md", "# 向量空间\n\n## 基与维数\n\n向量空间的一组基是线性无关且张成整个空间的向量组。维数定理说明任意两组基的向量个数相同。"),
    ("矩阵.md", "# 矩阵\n\n矩阵的秩等于列空间的维数，行秩与列秩相等。零空间是齐次方程组的解集。"),
    ("Eigen.txt", "An eigenvector of a matrix keeps its direction under the linear map; the eigenvalue is the scaling factor."),
];

fn seeded(env: &Env) -> String {
    let nid = env.notebook();
    for (t, d) in DOCS {
        env.add_text(&nid, t, d);
    }
    env.wait_sources_ready(&nid, 3);
    nid
}

fn mock_with_answer() -> MockLlm {
    MockLlm::start(|r| {
        match r.kind {
        Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
        Kind::NotebookSummary => Reply::Text(
            r#"{"title_suggestion":"线性代数","summary_md":"摘要：讨论基、秩与特征向量。","suggested_questions":["q1","q2","q3"]}"#.into(),
        ),
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("基是线性无关的生成组[1]，数学区间 [1,2] 不是引用。".into()),
    }
    })
}

/// F2: export then import yields a new notebook with the same content and the same retrieval
/// results; the archive holds no secrets, vectors or absolute local paths.
#[test]
fn export_import_round_trip_keeps_content_and_retrieval_results() {
    let mock = mock_with_answer();
    let env = Env::new("roundtrip");
    let nid = seeded(&env);
    env.profile(&mock, 32000);
    env.ok(
        "note.save",
        json!({ "notebook_id": nid, "title": "我的笔记", "body_md": "要点：基" }),
    );
    let before = retrieve(&env, &nid, "基与维数");
    assert!(!before.is_empty());

    let zip_path = env.dir.join("nb.lna");
    env.ok(
        "notebook.export",
        json!({ "notebook_id": nid, "format": "lna", "out_path": zip_path.display().to_string() }),
    );

    // archive hygiene: names, no data-dir path, no vectors
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(
        names.contains(&"manifest.json".to_string())
            && names.contains(&"notebook.json".to_string())
    );
    assert!(
        names.iter().all(|n| n == "manifest.json"
            || n == "notebook.json"
            || n.starts_with("blobs/")
            || n.starts_with("artifacts/")),
        "{names:?}"
    );
    let mut nbjson = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("notebook.json").unwrap(), &mut nbjson)
        .unwrap();
    assert!(
        !nbjson.contains(&env.dir.display().to_string().replace('\\', "\\\\")),
        "absolute local path leaked"
    );
    assert!(
        !nbjson.to_lowercase().contains("embedding"),
        "vectors are not exported"
    );

    let imp = env.ok(
        "notebook.import",
        json!({ "in_path": zip_path.display().to_string() }),
    );
    let new_id = imp["notebook_id"].as_str().unwrap().to_string();
    assert_ne!(new_id, nid);
    env.wait_sources_ready(&new_id, 3);
    let old_ids: std::collections::HashSet<_> = source_ids(&env, &nid).into_iter().collect();
    assert!(
        source_ids(&env, &new_id)
            .iter()
            .all(|i| !old_ids.contains(i)),
        "new source ids"
    );
    assert_eq!(
        retrieve(&env, &new_id, "基与维数"),
        before,
        "same retrieval results after import"
    );
    let notes = env.ok("note.list", json!({ "notebook_id": new_id }));
    assert!(notes["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["title"] == json!("我的笔记")));
}

/// F2: finished artifacts survive the round trip; unfinished ones do not come back as "running".
#[test]
fn artifacts_survive_import_and_unfinished_ones_are_not_left_running() {
    let mock = mock_with_answer();
    let env = Env::new("artifacts");
    let nid = seeded(&env);
    env.profile(&mock, 32000);
    let id = env.ok(
        "studio.create",
        json!({ "notebook_id": nid, "artifact_type": "quiz", "config": {} }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.wait(30, || {
        env.ok("studio.get", json!({ "artifact_id": id }))["status"] == json!("READY")
    });
    // an artifact caught mid-run
    env.core.db.write(|c| {
        c.execute(
            "INSERT INTO studio_artifacts (id, notebook_id, artifact_type, title, status, stage, progress, config_json, source_ids_json, sources_fingerprint, created_at, updated_at) VALUES ('inflight', ?1, 'report', '进行中', 'RUNNING', 'GENERATE', 0.5, '{}', '[]', '', 1, 1)",
            [&nid],
        ).map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
        Ok(())
    }).unwrap();
    let zip_path = env.dir.join("a.lna");
    env.ok(
        "notebook.export",
        json!({ "notebook_id": nid, "format": "lna", "out_path": zip_path.display().to_string() }),
    );
    let new_id = env.ok(
        "notebook.import",
        json!({ "in_path": zip_path.display().to_string() }),
    )["notebook_id"]
        .as_str()
        .unwrap()
        .to_string();
    let list = env.ok("studio.list", json!({ "notebook_id": new_id }));
    let arts = list["artifacts"].as_array().unwrap();
    assert_eq!(arts.len(), 2);
    let quiz = arts
        .iter()
        .find(|a| a["artifact_type"] == json!("quiz"))
        .unwrap();
    assert_eq!(quiz["status"], json!("READY"));
    assert!(quiz["payload"]["questions"].is_array(), "payload preserved");
    let other = arts
        .iter()
        .find(|a| a["artifact_type"] == json!("report"))
        .unwrap();
    assert_eq!(
        other["status"],
        json!("CANCELLED"),
        "no job exists for it any more"
    );
}

fn zip_with(path: &std::path::Path, f: impl FnOnce(&mut zip::ZipWriter<std::fs::File>)) {
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    f(&mut zw);
    zw.finish().unwrap();
}

fn valid_parts(zw: &mut zip::ZipWriter<std::fs::File>) {
    let o = zip::write::SimpleFileOptions::default();
    zw.start_file("manifest.json", o).unwrap();
    zw.write_all(br#"{"format":"lna","version":1}"#).unwrap();
    zw.start_file("notebook.json", o).unwrap();
    zw.write_all(br#"{"notebook":{"title":"X"},"sources":[],"notes":[],"artifacts":[]}"#)
        .unwrap();
}

/// F2: hostile or broken archives are rejected and leave nothing behind.
#[test]
fn hostile_or_broken_archives_are_rejected_without_side_effects() {
    let env = Env::new("hostile");
    let base = env.dir.clone();
    std::fs::create_dir_all(&base).unwrap();
    let o = zip::write::SimpleFileOptions::default();
    let notebooks_before = env.ok("notebook.list", json!({}))["notebooks"]
        .as_array()
        .unwrap()
        .len();

    type Build = Box<dyn Fn(&mut zip::ZipWriter<std::fs::File>)>;
    let cases: Vec<(&str, Build)> = vec![
        (
            "dotdot",
            Box::new(move |zw| {
                valid_parts(zw);
                zw.start_file("blobs/s1/../../escape.txt", o).unwrap();
                zw.write_all(b"x").unwrap();
            }),
        ),
        (
            "absolute",
            Box::new(move |zw| {
                valid_parts(zw);
                zw.start_file("/etc/passwd", o).unwrap();
                zw.write_all(b"x").unwrap();
            }),
        ),
        (
            "drive",
            Box::new(move |zw| {
                valid_parts(zw);
                zw.start_file("C:/Windows/x.txt", o).unwrap();
                zw.write_all(b"x").unwrap();
            }),
        ),
        (
            "backslash",
            Box::new(move |zw| {
                valid_parts(zw);
                zw.start_file("blobs\\s1\\x.txt", o).unwrap();
                zw.write_all(b"x").unwrap();
            }),
        ),
        (
            "symlink",
            Box::new(move |zw| {
                valid_parts(zw);
                zw.add_symlink("blobs/s1/link", "/etc/passwd", o).unwrap();
            }),
        ),
        (
            "no_manifest",
            Box::new(move |zw| {
                zw.start_file("notebook.json", o).unwrap();
                zw.write_all(b"{}").unwrap();
            }),
        ),
        (
            "bad_version",
            Box::new(move |zw| {
                zw.start_file("manifest.json", o).unwrap();
                zw.write_all(br#"{"format":"lna","version":99}"#).unwrap();
                zw.start_file("notebook.json", o).unwrap();
                zw.write_all(b"{}").unwrap();
            }),
        ),
        (
            "broken_json",
            Box::new(move |zw| {
                zw.start_file("manifest.json", o).unwrap();
                zw.write_all(br#"{"format":"lna","version":1}"#).unwrap();
                zw.start_file("notebook.json", o).unwrap();
                zw.write_all(b"{ not json").unwrap();
            }),
        ),
    ];
    for (name, build) in cases {
        let p = base.join(format!("{name}.lna"));
        zip_with(&p, |zw| build(zw));
        let code = env.err(
            "notebook.import",
            json!({ "in_path": p.display().to_string() }),
        );
        assert_eq!(code, "INVALID_ARGUMENT", "{name}");
    }
    assert!(!base.join("escape.txt").exists());
    assert_eq!(
        env.ok("notebook.list", json!({}))["notebooks"]
            .as_array()
            .unwrap()
            .len(),
        notebooks_before,
        "no half-imported notebook"
    );
    assert_eq!(
        env.err(
            "notebook.import",
            json!({ "in_path": base.join("missing.lna").display().to_string() })
        ),
        "INVALID_ARGUMENT"
    );
}

/// F2: entry-count and uncompressed-size limits (zip bombs) are enforced.
#[test]
fn entry_count_and_uncompressed_size_limits_stop_zip_bombs() {
    let env = Env::new("bomb");
    let base = env.dir.clone();
    std::fs::create_dir_all(&base).unwrap();
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let many = base.join("many.lna");
    zip_with(&many, |zw| {
        valid_parts(zw);
        for i in 0..10 {
            zw.start_file(format!("blobs/s{i}/f.txt"), o).unwrap();
            zw.write_all(b"x").unwrap();
        }
    });
    let e = lna_api::archive::import_notebook_lna_with_limits(
        &env.core.db,
        &env.core.jobs,
        &many,
        5,
        1 << 20,
    )
    .unwrap_err();
    assert!(e.message.contains("entry count"), "{}", e.message);

    let bomb = base.join("bomb.lna");
    zip_with(&bomb, |zw| {
        valid_parts(zw);
        zw.start_file("blobs/s1/big.txt", o).unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..8 {
            zw.write_all(&chunk).unwrap(); // 8 MiB of zeros compress to a few KiB
        }
    });
    assert!(
        std::fs::metadata(&bomb).unwrap().len() < 100_000,
        "really is a bomb"
    );
    let e = lna_api::archive::import_notebook_lna_with_limits(
        &env.core.db,
        &env.core.jobs,
        &bomb,
        100,
        1 << 20,
    )
    .unwrap_err();
    assert_eq!(e.code, lna_types::ErrorCode::SourceTooLarge);
    assert!(e.message.contains("zip bomb"));
    assert_eq!(
        env.ok("notebook.list", json!({}))["notebooks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

/// F3: the Markdown export turns citations into footnotes with unique, matching numbers and
/// leaves math intervals alone.
#[test]
fn markdown_export_converts_citations_to_matching_footnotes() {
    let mock = mock_with_answer();
    let env = Env::new("mdexport");
    let nid = seeded(&env);
    env.profile(&mock, 32000);
    for q in [
        "请详细说明基与维数之间的关系是什么",
        "请再详细说明矩阵的秩与维数之间的关系",
    ] {
        env.ok("chat.send", json!({ "notebook_id": nid, "text": q }));
    }
    env.ok(
        "note.save",
        json!({ "notebook_id": nid, "title": "笔记甲", "body_md": "笔记正文" }),
    );
    let qid = env.ok(
        "studio.create",
        json!({ "notebook_id": nid, "artifact_type": "flashcards", "config": {} }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.wait(30, || {
        env.ok("studio.get", json!({ "artifact_id": qid }))["status"] == json!("READY")
    });
    let md_path = env.dir.join("out.md");
    env.ok("notebook.export", json!({ "notebook_id": nid, "format": "markdown", "out_path": md_path.display().to_string() }));
    let md = std::fs::read_to_string(&md_path).unwrap();

    assert!(md.contains("## 笔记") && md.contains("笔记甲") && md.contains("笔记正文"));
    assert!(
        md.contains("基是什么") && md.contains("闪卡") || md.contains("卡片"),
        "artifact rendered as text, not raw JSON: {md}"
    );
    assert!(!md.contains("```json"), "no raw JSON blobs");
    assert!(md.contains("[1,2]"), "math interval untouched");
    // two answers, each cites [1] -> footnotes 1 and 2, defined once each
    let refs: Vec<_> = md.match_indices("[^").collect();
    assert!(refs.len() >= 4, "references + definitions: {md}");
    for n in 1..=2 {
        assert!(
            md.contains(&format!("[^{n}]: 向量空间.md")) || md.contains(&format!("[^{n}]: ")),
            "definition {n}"
        );
        assert_eq!(
            md.matches(&format!("[^{n}]:")).count(),
            1,
            "footnote {n} defined once"
        );
        assert!(
            md.matches(&format!("[^{n}]")).count() >= 2,
            "footnote {n} referenced"
        );
    }
    assert!(
        !md.contains("基是线性无关的生成组[1]"),
        "bare [1] converted"
    );
}

/// S2: the diagnostics bundle carries counts, codes and capability flags only.
#[test]
fn diagnostics_bundle_contains_no_content_prompts_answers_urls_or_keys() {
    let mock = mock_with_answer();
    let env = Env::new("diag");
    let nid = seeded(&env);
    let prof = env.profile(&mock, 32000);
    let kref = prof["keychain_ref"].as_str().unwrap().to_string();
    let key = format!("sk-diag-{:x}", std::process::id());
    env.ok("secret.set", json!({ "keychain_ref": kref, "secret": key }));
    let question = "请详细说明基与维数之间的关系是什么";
    let msg = env.ok("chat.send", json!({ "notebook_id": nid, "text": question }));
    let answer = msg["content"].as_str().unwrap().to_string();
    let out = env.dir.join("diag.json");
    env.ok(
        "diagnostics.export",
        json!({ "out_path": out.display().to_string() }),
    );
    env.ok("secret.clear", json!({ "keychain_ref": kref }));
    let text = std::fs::read_to_string(&out).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    for banned in [
        question,
        answer.as_str(),
        "线性无关",
        "Eigen.txt",
        "向量空间.md",
        &key,
        "127.0.0.1",
        "mock-model",
        "chat_system_v1",
        "你是",
    ] {
        assert!(!text.contains(banned), "diagnostics leaked {banned:?}");
    }
    assert_eq!(v["counts"]["sources"], json!(3));
    assert_eq!(v["counts"]["messages"], json!(2));
    assert!(v["breakdown"]["source_status"]["READY"].as_i64().unwrap() >= 3);
    assert_eq!(v["profiles"][0]["protocol"], json!("chat_completion"));
    assert!(v["os"].is_string() && v["version"].is_string());
}

/// F1: notes from a chat answer keep a citation snapshot; the summary note is a copy that later
/// summary refreshes do not change; notes can be read back and converted into sources.
#[test]
fn notes_keep_citation_snapshots_and_summary_notes_are_copies() {
    let mock = mock_with_answer();
    let env = Env::new("notes");
    let nid = seeded(&env);
    env.profile(&mock, 32000);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    let mid = msg["id"].as_str().unwrap().to_string();

    let n1 = env.ok("note.from_message", json!({ "message_id": mid }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let note = env.ok("note.get", json!({ "note_id": n1 }));
    assert_eq!(note["origin"], json!("chat"));
    assert_eq!(note["origin_message_id"], json!(mid));
    let cites: Value = serde_json::from_str(note["citations_json"].as_str().unwrap()).unwrap();
    assert!(
        cites
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["source_title"].is_string() || c["title"].is_string()),
        "{cites}"
    );
    // the snapshot survives deleting the original message
    env.ok("chat.clear", json!({ "notebook_id": nid }));
    assert_eq!(
        env.ok("note.get", json!({ "note_id": n1 }))["body_md"],
        note["body_md"]
    );

    env.wait(20, || {
        env.ok("notebook.get", json!({ "notebook_id": nid }))["summary_md"].is_string()
    });
    let s1 = env.ok("note.from_summary", json!({ "notebook_id": nid }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let saved = env.ok("note.get", json!({ "note_id": s1 }));
    assert_eq!(saved["origin"], json!("summary"));
    assert!(saved["body_md"].as_str().unwrap().contains("摘要：讨论基"));
    // a refreshed summary does not rewrite the saved copy
    env.core
        .db
        .write(|c| {
            c.execute(
                "UPDATE notebooks SET summary_md='全新的摘要', summary_fingerprint=NULL",
                [],
            )
            .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(env.ok("note.get", json!({ "note_id": s1 }))["body_md"]
        .as_str()
        .unwrap()
        .contains("摘要：讨论基"));

    let src = env.ok("note.to_source", json!({ "note_id": n1 }));
    assert_eq!(src["kind"], json!("note"));
    env.wait_sources_ready(&nid, 4);
    assert_eq!(
        env.err("note.get", json!({ "note_id": "nope" })),
        "NOT_FOUND"
    );
}
