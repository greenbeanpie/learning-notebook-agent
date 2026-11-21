//! Performance baselines (G7). Slow, so `#[ignore]`d: run with
//! `cargo test -p lna-api --test bench --release -- --ignored --nocapture`.
//! Numbers are written to `target/bench-report.json` and copied into `docs/benchmarks.md`.
mod common;

use common::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn report(key: &str, value: Value) {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/bench-report.json");
    let mut all: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}));
    all[key] = value;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap()).unwrap();
}

/// Fills one notebook with `n` synthetic chunks of Chinese/English text directly in SQL, then
/// measures end-to-end hybrid retrieval (BM25 + groundedness + evidence assembly).
fn retrieval_p95(n: usize) -> (f64, f64) {
    let env = Env::new("bench_retrieval");
    let nid = env.notebook();
    let sid = env.add_text(&nid, "基准.md", "# 基准\n\n占位。");
    env.wait_sources_ready(&nid, 1);
    let words = [
        "向量空间",
        "线性无关",
        "特征值",
        "矩阵的秩",
        "零空间",
        "正交投影",
        "奇异值分解",
        "最小二乘",
        "行列式",
        "基与维数",
        "eigenvalue",
        "projection",
        "orthogonal",
        "determinant",
    ];
    env.core
        .db
        .write(|c| {
            let tx = c.transaction().map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            {
                let mut st = tx
                    .prepare("INSERT INTO source_chunks (source_id, notebook_id, chunk_index, char_start, char_end, content, token_count) VALUES (?1,?2,?3,0,0,?4,?5)")
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                for i in 0..n {
                    let text = format!(
                        "第{i}段：{}与{}的关系，涉及{}。{} {} 编号{}。这是用于基准测试的合成内容，包含若干重复词汇。",
                        words[i % words.len()],
                        words[(i * 7 + 3) % words.len()],
                        words[(i * 11 + 5) % words.len()],
                        words[(i * 13 + 1) % words.len()],
                        words[(i * 17 + 2) % words.len()],
                        i
                    );
                    st.execute(rusqlite::params![sid, nid, (i + 1) as i64, text, 80])
                        .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                    let rowid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO chunks_fts (rowid, tokens) VALUES (?1, ?2)",
                        rusqlite::params![rowid, lna_retrieval::tokenize_for_fts(&text)],
                    )
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                }
            }
            tx.commit().map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let ids = vec![sid];
    let queries = [
        "特征值与矩阵的秩",
        "零空间和正交投影的关系",
        "奇异值分解 最小二乘",
        "eigenvalue projection",
        "行列式 基与维数",
        "向量空间的线性无关",
    ];
    let runtime = rt();
    // warm up
    for q in queries {
        let _ = runtime.block_on(
            env.core
                .retrieval
                .search(&nid, &[q.to_string()], &ids, 6000, 8),
        );
    }
    let mut lat = Vec::new();
    for i in 0..120 {
        let q = queries[i % queries.len()];
        let t = Instant::now();
        let out = runtime
            .block_on(
                env.core
                    .retrieval
                    .search(&nid, &[q.to_string()], &ids, 6000, 8),
            )
            .unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(out.grounded);
    }
    (percentile(lat.clone(), 0.5), percentile(lat, 0.95))
}

#[test]
#[ignore]
fn retrieval_latency_on_10k_and_100k_chunks() {
    let (p50a, p95a) = retrieval_p95(10_000);
    println!("10k chunks: p50 {p50a:.1} ms, p95 {p95a:.1} ms");
    let (p50b, p95b) = retrieval_p95(100_000);
    println!("100k chunks: p50 {p50b:.1} ms, p95 {p95b:.1} ms");
    report(
        "retrieval_bm25_ms",
        json!({ "chunks_10k": { "p50": p50a, "p95": p95a }, "chunks_100k": { "p50": p50b, "p95": p95b } }),
    );
    assert!(p95b < 300.0, "P95 target is 300 ms, got {p95b:.1}");
}

/// Time from `chat.send` to the first `chat.thought_step` event.
#[test]
#[ignore]
fn first_thought_step_latency() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
        _ => Reply::Text("基与维数的关系[1]。".into()),
    });
    let env = Env::new("bench_first_step");
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    env.profile(&mock, 32000, false);
    let mut lat = Vec::new();
    for i in 0..20 {
        let mut rx = env.core.events.subscribe();
        let t = Instant::now();
        let core = std::sync::Arc::clone(&env.core);
        let nid2 = nid.clone();
        let h = std::thread::spawn(move || {
            core.handle_json(
                &json!({ "id": "t", "method": "chat.send", "params": { "notebook_id": nid2, "text": format!("请详细说明基与维数之间的关系是什么 {i}") } })
                    .to_string(),
            )
        });
        let mut first = None;
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && first.is_none() {
            while let Ok(ev) = rx.try_recv() {
                if ev.event == "chat.thought_step" && first.is_none() {
                    first = Some(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        h.join().unwrap();
        lat.push(first.expect("a thought step"));
    }
    let (p50, p95) = (percentile(lat.clone(), 0.5), percentile(lat, 0.95));
    println!("first thought_step: p50 {p50:.1} ms, p95 {p95:.1} ms");
    report("first_thought_step_ms", json!({ "p50": p50, "p95": p95 }));
    assert!(p95 < 100.0, "target is 100 ms, got {p95:.1}");
}

/// A 500-page PDF from import to READY. Needs PDFium and PyMuPDF to build the file.
#[test]
#[ignore]
fn pdf_500_pages_ingest_time() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if !dir.join("pdfium.dll").exists() && !dir.join("libpdfium.so").exists() {
        eprintln!("skipped: no PDFium in {}", dir.display());
        return;
    }
    std::env::set_var("LNA_PDFIUM_DIR", &dir);
    let env = Env::new("bench_pdf");
    std::fs::create_dir_all(&env.dir).unwrap();
    let pdf = env.dir.join("big.pdf");
    let script = env.dir.join("mk.py");
    let py = r#"import pymupdf
d = pymupdf.open()
for i in range(500):
    p = d.new_page(width=420, height=595)
    p.insert_text((40, 40), "Benchmark Document - Draft", fontsize=9)
    y = 80
    for j in range(32):
        p.insert_text((40, y), "Page %d line %d: vector spaces, bases, dimension and rank of matrices." % (i, j), fontsize=10)
        y += 16
    p.insert_text((40, 570), "Page %d of 500" % i, fontsize=9)
d.save(r"__OUT__")
"#;
    std::fs::write(&script, py.replace("__OUT__", &pdf.display().to_string())).unwrap();
    let ok = std::process::Command::new("python")
        .arg(&script)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipped: could not build the PDF (PyMuPDF missing?)");
        return;
    }
    let nid = env.notebook();
    let t = Instant::now();
    env.ok(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [pdf.display().to_string()] }),
    );
    env.wait(120, || {
        env.ok("source.list", json!({ "notebook_id": nid }))["sources"][0]["status"]
            == json!("READY")
    });
    let secs = t.elapsed().as_secs_f64();
    let chunks: i64 = env.query(|c| {
        c.query_row("SELECT COUNT(*) FROM source_chunks", [], |r| r.get(0))
            .unwrap()
    });
    println!("500-page PDF: {secs:.1} s to READY, {chunks} chunks");
    report(
        "pdf_500_pages",
        json!({ "seconds": secs, "chunks": chunks }),
    );
    assert!(secs < 30.0, "target is 30 s, got {secs:.1}");
}

#[test]
#[ignore]
fn semantic_query_p95_on_100k_vectors() {
    let env = Env::new("bench_vec100k");
    let nid = env.notebook();
    let rec = env
        .core
        .embedding
        .configure("local_onnx", "bench-384", 384, "", "", None)
        .unwrap();
    let vec_table = rec.vec_table;

    // Insert 100,000 normalized 384-dim vectors into vec0.
    let mut base_vec = vec![0.0f32; 384];
    env.core
        .db
        .write(|c| {
            let tx = c
                .transaction()
                .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            {
                let sql = format!(
                    "INSERT INTO {vec_table} (chunk_id, notebook_id, embedding) VALUES (?1, ?2, ?3)"
                );
                let mut st = tx
                    .prepare(&sql)
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                for i in 1..=100_000i64 {
                    for (d, slot) in base_vec.iter_mut().enumerate() {
                        *slot = (((i as usize + d * 17) % 97) as f32) / 97.0;
                    }
                    let bytes: Vec<u8> = base_vec.iter().flat_map(|f| f.to_le_bytes()).collect();
                    st.execute(rusqlite::params![i, nid, bytes])
                        .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                }
            }
            tx.commit()
                .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
            Ok(())
        })
        .unwrap();

    let query_sql = format!(
        "SELECT chunk_id, distance FROM {vec_table}
         WHERE notebook_id = ?1 AND embedding MATCH ?2 AND k = 20
         ORDER BY distance"
    );
    let mut qvec = vec![0.0f32; 384];
    for (d, slot) in qvec.iter_mut().enumerate() {
        *slot = ((d % 31) as f32) / 31.0;
    }
    let qbytes: Vec<u8> = qvec.iter().flat_map(|f| f.to_le_bytes()).collect();

    // Warm up
    for _ in 0..5 {
        env.core
            .db
            .read(|c| {
                let mut st = c
                    .prepare(&query_sql)
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                let rows = st
                    .query_map(rusqlite::params![nid, qbytes], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                    })
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                for r in rows {
                    let _ = r.map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                }
                Ok(())
            })
            .unwrap();
    }

    let mut lat = Vec::with_capacity(60);
    for i in 0..60 {
        qvec[0] = (i as f32) / 60.0;
        let qb: Vec<u8> = qvec.iter().flat_map(|f| f.to_le_bytes()).collect();
        let t = Instant::now();
        let count = env
            .core
            .db
            .read(|c| {
                let mut st = c
                    .prepare(&query_sql)
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                let rows = st
                    .query_map(rusqlite::params![nid, qb], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                    })
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                let mut cnt = 0usize;
                for r in rows {
                    let _ = r.map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                    cnt += 1;
                }
                Ok(cnt)
            })
            .unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(count, 20);
    }
    let p50 = percentile(lat.clone(), 0.50);
    let p95 = percentile(lat, 0.95);
    println!("100k vectors semantic KNN (384-dim): p50={p50:.1} ms, p95={p95:.1} ms");
    report("semantic_100k", json!({ "p50_ms": p50, "p95_ms": p95 }));
    assert!(p95 < 300.0, "target p95 < 300 ms, got {p95:.1} ms");
}
