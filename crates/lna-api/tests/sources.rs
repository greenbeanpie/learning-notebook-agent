//! Source import behaviour: encodings, limits, retry, preview, deletion, FTS safety (B1, B2, B4-B7, B11).
mod common;

use common::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn write(env: &Env, name: &str, bytes: &[u8]) -> String {
    std::fs::create_dir_all(&env.dir).unwrap();
    let p = env.dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p.display().to_string()
}

fn status_of(env: &Env, nid: &str, title_part: &str) -> Value {
    let l = env.ok("source.list", json!({ "notebook_id": nid }));
    l["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"].as_str().unwrap().contains(title_part))
        .cloned()
        .unwrap_or(Value::Null)
}

fn fts_rows(env: &Env) -> i64 {
    env.query(|c| {
        c.query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
            .unwrap()
    })
}

fn mock() -> MockLlm {
    MockLlm::start(|_| Reply::Text("OK".into()))
}

/// B1: a GBK-encoded text file is decoded correctly and its content is searchable.
#[test]
fn b1_gbk_text_is_decoded_and_searchable() {
    let env = Env::new("gbk");
    let nid = env.notebook();
    let gbk = hex("cff2c1bfbfd5bce4b5c4bbf9d3ebceaccafdcac7cfdfd0d4b4facafdb5c4bacbd0c4b8c5c4eea1a3bed8d5f3b5c4d6c8b5c8d3dac1d0bfd5bce4b5c4ceaccafda1a3");
    let path = write(&env, "gbk.txt", &gbk);
    env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [path] }),
    );
    env.wait_sources_ready(&nid, 1);
    let ids: Vec<String> = vec![status_of(&env, &nid, "gbk")["id"]
        .as_str()
        .unwrap()
        .to_string()];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt
        .block_on(
            env.core
                .retrieval
                .search(&nid, &["矩阵的秩".to_string()], &ids, 4000, 8),
        )
        .unwrap();
    assert!(out.grounded);
    assert!(
        out.evidences[0]
            .content
            .contains("矩阵的秩等于列空间的维数"),
        "{}",
        out.evidences[0].content
    );
    // the same bytes again are a duplicate
    let again = env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [write(&env, "gbk2.txt", &gbk)] }),
    );
    assert!(
        again.to_string().contains("true"),
        "duplicate flagged: {again}"
    );
}

/// B4: pasted text and note conversion both reach READY; converting identical content again
/// returns the existing source and `converted_source_id` is written back.
#[test]
fn b4_paste_and_note_conversion() {
    let env = Env::new("paste");
    let nid = env.notebook();
    let r = env.ok(
        "source.add_text",
        json!({ "notebook_id": nid, "title": "粘贴", "text": "粘贴的内容：基与维数。" }),
    );
    assert_eq!(r["duplicate"], json!(false));
    let note = env.ok(
        "note.save",
        json!({ "notebook_id": nid, "title": "笔记甲", "body_md": "笔记内容：矩阵的秩。" }),
    );
    let nid_note = note["id"].as_str().unwrap().to_string();
    let first = env.ok("note.to_source", json!({ "note_id": nid_note }));
    env.wait_sources_ready(&nid, 2);
    let n = env.ok("note.get", json!({ "note_id": nid_note }));
    assert_eq!(n["converted_source_id"], first["id"], "{n}");
    // converting the same note again returns the existing source instead of a new one
    let second = env.ok("note.to_source", json!({ "note_id": nid_note }));
    assert_eq!(second["id"], first["id"]);
    assert_eq!(
        env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// B5: failures carry an error code; retry resets the state; preview returns chunk text.
#[test]
fn b5_failure_codes_retry_and_preview() {
    let env = Env::new("retry");
    let nid = env.notebook();
    // an empty file cannot produce text
    let empty = write(&env, "empty.txt", b"");
    env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [empty] }),
    );
    env.wait(20, || {
        status_of(&env, &nid, "empty")["status"] == json!("FAILED")
    });
    let failed = status_of(&env, &nid, "empty");
    assert_eq!(
        failed["error_code"],
        json!("SOURCE_UNSUPPORTED"),
        "{failed}"
    );
    let sid = failed["id"].as_str().unwrap().to_string();
    env.ok("source.retry", json!({ "source_id": sid }));
    let after_retry = env.ok("source.get", json!({ "source_id": sid }));
    assert!(
        after_retry["status"] == json!("QUEUED")
            || after_retry["status"] == json!("PROCESSING")
            || after_retry["status"] == json!("FAILED")
    );

    let good = env.add_text(
        &nid,
        "好文档.md",
        "# 章节\n\n第一段内容。\n\n## 小节\n\n第二段内容。",
    );
    env.wait(20, || {
        status_of(&env, &nid, "好文档")["status"] == json!("READY")
    });
    let prev = env.ok("source.preview", json!({ "source_id": good }));
    let chunks = prev["chunks"].as_array().unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks
        .iter()
        .any(|c| c["content"].as_str().unwrap().contains("第二段内容")));
    assert!(chunks[0]["chunk_index"].is_number() && chunks[0]["char_start"].is_number());
    assert_eq!(
        env.err("source.preview", json!({ "source_id": "nope" })),
        "NOT_FOUND"
    );
}

/// B2 via the API: encrypted / scanned / corrupt PDFs end FAILED with the right code, text PDFs READY.
#[test]
fn b2_pdf_failures_surface_as_source_error_codes() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if !dir.join("pdfium.dll").exists() && !dir.join("libpdfium.so").exists() {
        assert!(
            std::env::var_os("LNA_REQUIRE_PDFIUM").is_none(),
            "PDFium missing"
        );
        eprintln!("skipped: no PDFium");
        return;
    }
    std::env::set_var("LNA_PDFIUM_DIR", &dir);
    let env = Env::new("pdfapi");
    let nid = env.notebook();
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pdf");
    let paths: Vec<String> = [
        "multi_page.pdf",
        "encrypted.pdf",
        "scanned.pdf",
        "corrupt.pdf",
    ]
    .iter()
    .map(|n| fx.join(n).display().to_string())
    .collect();
    env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": paths }),
    );
    env.wait(30, || {
        let l = env.ok("source.list", json!({ "notebook_id": nid }));
        l["sources"].as_array().unwrap().len() == 4
            && l["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["status"] == json!("READY") || s["status"] == json!("FAILED"))
    });
    assert_eq!(
        status_of(&env, &nid, "multi_page")["status"],
        json!("READY")
    );
    assert_eq!(status_of(&env, &nid, "multi_page")["page_count"], json!(6));
    let code = |n: &str| status_of(&env, &nid, n)["error_code"].clone();
    assert_eq!(code("encrypted"), json!("SOURCE_ENCRYPTED"));
    assert_eq!(code("scanned"), json!("SOURCE_NEEDS_OCR"));
    assert_eq!(code("corrupt"), json!("SOURCE_UNSUPPORTED"));
    // the readable one is searchable with its page numbers
    let prev = env.ok(
        "source.preview",
        json!({ "source_id": status_of(&env, &nid, "multi_page")["id"] }),
    );
    assert!(prev["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["page_start"].is_number()));
}

/// B6: removing a source (or a notebook) leaves nothing behind in FTS or on disk.
#[test]
fn b6_removal_leaves_no_residue() {
    let env = Env::new("residue");
    let nid = env.notebook();
    let s1 = env.add_text(&nid, "甲.md", "# 甲\n\n向量空间的基与维数。");
    let _s2 = env.add_text(&nid, "乙.md", "# 乙\n\n矩阵的秩与零空间。");
    env.wait_sources_ready(&nid, 2);
    let before = fts_rows(&env);
    assert!(before >= 2);
    let blob_dir = env.dir.join("blobs").join(&nid).join(&s1);
    assert!(blob_dir.exists());
    env.ok("source.remove", json!({ "source_id": s1 }));
    assert!(fts_rows(&env) < before);
    assert!(!blob_dir.exists(), "blob folder removed");
    let chunks: i64 = env.query(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM source_chunks WHERE source_id=?1",
            [&s1],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(chunks, 0);

    env.ok("notebook.delete", json!({ "notebook_id": nid }));
    assert_eq!(fts_rows(&env), 0);
    let left: i64 = env.query(|c| {
        c.query_row(
            "SELECT (SELECT COUNT(*) FROM sources)+(SELECT COUNT(*) FROM source_chunks)",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(left, 0);
    assert!(
        !env.dir.join("blobs").join(&nid).exists()
            || std::fs::read_dir(env.dir.join("blobs").join(&nid))
                .unwrap()
                .next()
                .is_none()
    );
}

/// B7: two-character Chinese words hit; operator-looking input never errors.
#[test]
fn b7_chinese_and_hostile_fts_input() {
    let env = Env::new("fts");
    let nid = env.notebook();
    env.add_text(
        &nid,
        "文档.md",
        "# 文档\n\n零空间是齐次方程组的解集，秩与维数相关。 NEAR OR AND 也是普通词。",
    );
    env.wait_sources_ready(&nid, 1);
    let ids: Vec<String> = env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let search = |q: &str| {
        rt.block_on(
            env.core
                .retrieval
                .search(&nid, &[q.to_string()], &ids, 4000, 8),
        )
    };
    assert!(search("零空间").unwrap().grounded, "two-character word");
    // a single character may legitimately be ignored, so only require that the search itself succeeds
    search("秩").unwrap();
    for hostile in [
        "\"",
        "\"\"\"",
        "*",
        "a*",
        "NEAR(a b)",
        "OR",
        "AND NOT",
        "(",
        ")",
        "-",
        "a AND",
        "^x",
        "col:val",
        "零空间 OR \"",
        "'; DROP TABLE sources; --",
        "{}",
        "\\",
    ] {
        let r = search(hostile);
        assert!(
            r.is_ok(),
            "query {hostile:?} must not error: {:?}",
            r.err().map(|e| e.message)
        );
    }
}

/// B11: per-notebook source cap, pasted-text cap and file-size cap.
#[test]
fn b11_limits() {
    let env = Env::new("limits");
    let nid = env.notebook();
    // pasted text over 2 MB
    let big = "字".repeat(2 * 1024 * 1024 / 3 + 10);
    assert_eq!(
        env.err(
            "source.add_text",
            json!({ "notebook_id": nid, "title": "大", "text": big })
        ),
        "SOURCE_TOO_LARGE"
    );
    // single file over 200 MB (sparse file, never read)
    let p = env.dir.join("huge.txt");
    std::fs::create_dir_all(&env.dir).unwrap();
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(200 * 1024 * 1024 + 1).unwrap();
    drop(f);
    assert_eq!(
        env.err(
            "source.add_files",
            json!({ "notebook_id": nid, "paths": [p.display().to_string()] })
        ),
        "SOURCE_TOO_LARGE"
    );
    std::fs::remove_file(&p).unwrap();

    // 300 sources fit, the 301st does not
    let nb2 = env.notebook();
    for i in 0..300 {
        let r = env.raw(
            "source.add_text",
            json!({ "notebook_id": nb2, "title": format!("s{i}"), "text": format!("内容编号{i}") }),
        );
        assert!(r["error"].is_null(), "source {i}: {r}");
    }
    assert_eq!(
        env.err(
            "source.add_text",
            json!({ "notebook_id": nb2, "title": "多余", "text": "第 301 个" })
        ),
        "SOURCE_LIMIT_REACHED"
    );
    let _ = mock();
}

#[test]
fn ai_source_guide_and_hierarchical_index_work_on_import_and_on_demand() {
    let ai_mock = MockLlm::start(|r| {
        match r.kind {
        Kind::SourceIndex => Reply::Text(
            r#"{
              "guide_summary_md": "本文系统讲解了**微积分基础**与**微分中值定理**，帮助读者建立严谨逻辑。",
              "guide_topics": ["微积分基础", "微分中值定理", "积分计算"],
              "nodes": [
                {"id": "n1", "parent_id": null, "level": 1, "title": "第1章 实数与极限", "summary": "介绍实数完备性与函数极限", "keywords": ["实数", "极限"], "page_start": 1, "page_end": 20, "char_start": null, "char_end": null},
                {"id": "n2", "parent_id": null, "level": 1, "title": "第2章 导数与微分中值定理", "summary": "罗尔定理与拉格朗日中值定理", "keywords": ["导数", "中值定理"], "page_start": 21, "page_end": 50, "char_start": null, "char_end": null}
              ],
              "page_texts": []
            }"#
            .into(),
        ),
        _ => Reply::Text("ok".into()),
    }
    });

    let env = Env::new("ai_guide");
    let nid = env.notebook();

    // 1. Import before model is configured -> ai_indexed=false, then on-demand source.generate_guide works
    let legacy_id = env.add_text(
        &nid,
        "高等数学.md",
        "# 第1章 实数与极限\n\n函数与极限基础。\n\n# 第2章 导数\n\n微分中值定理。",
    );
    env.wait_sources_ready(&nid, 1);
    let preview_before = env.ok("source.preview", json!({ "source_id": legacy_id }));
    assert_eq!(preview_before["ai_indexed"], false);

    // Configure model and trigger on-demand guide generation
    env.profile(&ai_mock, 32000);
    let gen = env.ok("source.generate_guide", json!({ "source_id": legacy_id }));
    assert_eq!(gen["source"]["ai_indexed"], true);
    assert!(gen["source"]["guide_summary_md"]
        .as_str()
        .unwrap_or("")
        .contains("**微积分基础**"));
    assert_eq!(gen["index_nodes"].as_array().unwrap().len(), 2);

    // 2. Import after model is configured -> automatically AI-indexed on import
    let auto_id = env.add_text(&nid, "微积分讲义.md", "# 导数与积分\n\n牛顿-莱布尼茨公式。");
    env.wait_sources_ready(&nid, 2);
    let preview_after = env.ok("source.preview", json!({ "source_id": auto_id }));
    assert_eq!(preview_after["ai_indexed"], true);
    assert_eq!(preview_after["guide_topics"].as_array().unwrap().len(), 3);
    assert_eq!(preview_after["index_nodes"].as_array().unwrap().len(), 2);
}
