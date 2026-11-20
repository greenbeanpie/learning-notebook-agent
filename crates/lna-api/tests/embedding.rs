//! Semantic retrieval, embedding model configuration and the model download (B8, B-15, B-20).
mod common;

use common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::time::Duration;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn corpus() -> Vec<(String, String)> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/corpus");
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        out.push((
            e.file_name().to_string_lossy().to_string(),
            std::fs::read_to_string(e.path()).unwrap(),
        ));
    }
    out
}

fn ids(env: &Env, nid: &str) -> Vec<String> {
    env.ok("source.list", json!({ "notebook_id": nid }))["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn status(env: &Env) -> Value {
    env.ok("embedding.status", json!({}))
}

fn wait_embedded(env: &Env) {
    env.wait(30, || {
        let s = status(env);
        s["total_chunks"].as_i64().unwrap() > 0 && s["embedded_chunks"] == s["total_chunks"]
    });
}

fn setup_remote(env: &Env, mock: &MockLlm) {
    let prof = env.profile(mock, 32000, false);
    env.ok(
        "embedding.configure",
        json!({
            "provider": "remote", "model_name": "concept-8", "dimension": 9,
            "query_prefix": "", "passage_prefix": "", "endpoint_profile_id": prof["id"]
        }),
    );
}

/// B8: with an embedding model, vectors are created for existing and new sources, and a Chinese
/// question finds an English-only passage on its own, which keyword search cannot do.
#[test]
fn semantic_retrieval_bridges_languages_without_query_rewriting() {
    let mock = MockLlm::start_with(|_| Reply::Text("OK".into()), Some(concept_embedder()));
    let env = Env::new("semantic");
    let nid = env.notebook();
    for (title, text) in corpus() {
        env.add_text(&nid, &title, &text);
    }
    let n = corpus().len();
    env.wait_sources_ready(&nid, n);
    let all = ids(&env, &nid);
    let search = |q: &str| {
        rt().block_on(
            env.core
                .retrieval
                .search(&nid, &[q.to_string()], &all, 6000, 8),
        )
        .unwrap()
    };

    // Before any model: the Chinese question cannot reach the English document.
    let before = search("特征值和特征向量是什么意思");
    assert!(
        !before
            .evidences
            .iter()
            .any(|e| e.source_title.contains("eigenvalues")),
        "lexical search should miss it"
    );
    assert_eq!(status(&env)["embedded_chunks"], json!(0));

    // Configure the model: existing sources are embedded automatically after a rebuild.
    setup_remote(&env, &mock);
    let queued = env.ok("embedding.rebuild", json!({}));
    assert_eq!(queued["sources_queued"], json!(n));
    wait_embedded(&env);
    assert!(mock.embedded_texts.load(Ordering::SeqCst) >= n);

    let after = search("特征值和特征向量是什么意思");
    assert!(
        after
            .evidences
            .iter()
            .any(|e| e.source_title.contains("eigenvalues")),
        "semantic hit expected"
    );
    assert!(after.best_cosine > 0.35);
    assert!(after.vector_fallback_reason.is_none());

    // A source added later is embedded as part of ingest.
    env.add_text(
        &nid,
        "新增.md",
        "# 行列式\n\nThe determinant is zero exactly when the matrix is singular.",
    );
    env.wait_sources_ready(&nid, n + 1);
    wait_embedded(&env);
    let all = ids(&env, &nid);
    let hit = rt()
        .block_on(env.core.retrieval.search(
            &nid,
            &["行列式什么时候为零".to_string()],
            &all,
            6000,
            8,
        ))
        .unwrap();
    assert!(hit
        .evidences
        .iter()
        .any(|e| e.source_title.contains("新增")));
}

/// B8: forcing a rebuild recomputes every vector; a normal rebuild computes nothing new.
#[test]
fn rebuild_recomputes_only_when_forced() {
    let mock = MockLlm::start_with(|_| Reply::Text("OK".into()), Some(concept_embedder()));
    let env = Env::new("rebuild");
    let nid = env.notebook();
    env.add_text(&nid, "甲.md", "# 甲\n\n基与维数。");
    env.add_text(&nid, "乙.md", "# 乙\n\n矩阵的秩。");
    env.wait_sources_ready(&nid, 2);
    setup_remote(&env, &mock);
    env.ok("embedding.rebuild", json!({}));
    wait_embedded(&env);
    let first = mock.embedded_texts.load(Ordering::SeqCst);
    env.ok("embedding.rebuild", json!({}));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        mock.embedded_texts.load(Ordering::SeqCst),
        first,
        "nothing to do"
    );
    env.ok("embedding.rebuild", json!({ "force": true }));
    env.wait(20, || {
        mock.embedded_texts.load(Ordering::SeqCst) >= first * 2
    });
    wait_embedded(&env);
}

/// B8: removing a source also removes its vectors.
#[test]
fn removing_a_source_removes_its_vectors() {
    let mock = MockLlm::start_with(|_| Reply::Text("OK".into()), Some(concept_embedder()));
    let env = Env::new("vecremove");
    let nid = env.notebook();
    let s1 = env.add_text(&nid, "甲.md", "# 甲\n\n基与维数。");
    env.add_text(&nid, "乙.md", "# 乙\n\n矩阵的秩。");
    env.wait_sources_ready(&nid, 2);
    setup_remote(&env, &mock);
    env.ok("embedding.rebuild", json!({}));
    wait_embedded(&env);
    let model = status(&env)["active_model"].clone();
    let table = model["vec_table"].as_str().unwrap().to_string();
    let count = |env: &Env| -> i64 {
        env.query(|c| {
            c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        })
    };
    let before = count(&env);
    assert!(before >= 2);
    env.ok("source.remove", json!({ "source_id": s1 }));
    assert!(count(&env) < before);
}

/// B-20: when the embedding backend is down the chat still answers from keyword search and says so.
#[test]
fn vector_failure_degrades_to_keywords_and_is_reported() {
    let mock = MockLlm::start_with(
        |r| match r.kind {
            Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
            _ => Reply::Text("基与维数的关系[1]。".into()),
        },
        Some(concept_embedder()),
    );
    let env = Env::new("degrade");
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    setup_remote(&env, &mock);
    env.ok("embedding.rebuild", json!({}));
    wait_embedded(&env);
    mock.embeddings_fail.store(true, Ordering::SeqCst);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");
    let kinds: Vec<&str> = msg["thoughts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"retrieve_fallback"), "{kinds:?}");
    assert!(
        !msg["citations"].as_array().unwrap().is_empty(),
        "keyword evidence still used: {msg}"
    );
}

#[test]
fn embedding_configuration_is_validated() {
    let env = Env::new("embcfg");
    assert_eq!(
        env.err(
            "embedding.configure",
            json!({ "provider": "carrier_pigeon", "model_name": "x", "dimension": 8 })
        ),
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        env.err(
            "embedding.configure",
            json!({ "provider": "remote", "model_name": "x", "dimension": 0 })
        ),
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        env.err(
            "embedding.configure",
            json!({ "provider": "remote", "model_name": "x", "dimension": 99999 })
        ),
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        env.err("embedding.rebuild", json!({})),
        "CAPABILITY_MISSING"
    );
}

// ------------------------------------------------------------------------------ download

fn spec(server: &FileServer, model: &[u8], tok: &[u8]) -> Value {
    let sum = |b: &[u8]| hex::encode(Sha256::digest(b));
    json!({
        "id": "test-model", "display_name": "t", "dimension": 8, "query_prefix": "q: ", "passage_prefix": "p: ",
        "files": [
            { "name": "tokenizer.json", "url": format!("http://127.0.0.1:{}/tok", server.port), "sha256": sum(tok), "size": tok.len() },
            { "name": "model.onnx", "url": format!("http://127.0.0.1:{}/model", server.port), "sha256": sum(model), "size": model.len() }
        ]
    })
}

fn wait_download_done(
    env: &Env,
    rx: &mut tokio::sync::broadcast::Receiver<lna_types::ApiEvent>,
) -> Value {
    let mut done = None;
    env.wait(30, || {
        while let Ok(ev) = rx.try_recv() {
            if ev.event == "embedding.download_done" {
                done = Some(ev.data);
            }
        }
        done.is_some()
    });
    done.unwrap()
}

#[test]
fn download_verifies_checksums_reports_progress_and_activates_the_model() {
    let model = vec![7u8; 3 * 1024 * 1024];
    let tok = b"{\"tokenizer\":true}".to_vec();
    let server = FileServer::start(vec![("/model", model.clone()), ("/tok", tok.clone())]);
    let env = Env::new("dl_ok");
    let mut rx = env.core.events.subscribe();
    let sp = spec(&server, &model, &tok);
    assert_eq!(
        env.ok("embedding.download_status", json!({}))["running"],
        json!(false)
    );
    env.ok("embedding.download", json!({ "spec": sp }));
    let done = wait_download_done(&env, &mut rx);
    assert_eq!(done["ok"], json!(true), "{done}");

    let dir = env.dir.join("models").join("test-model");
    assert_eq!(std::fs::read(dir.join("model.onnx")).unwrap(), model);
    assert_eq!(std::fs::read(dir.join("tokenizer.json")).unwrap(), tok);
    assert!(!dir.join("model.onnx.part").exists());
    let active = status(&env)["active_model"].clone();
    assert_eq!(active["provider"], json!("local_onnx"));
    assert_eq!(active["model_name"], json!("test-model"));
    assert_eq!(active["dimension"], json!(8));
    assert_eq!(active["query_prefix"], json!("q: "));
    assert_eq!(
        env.ok("embedding.download_status", json!({}))["running"],
        json!(false)
    );

    // A second run finds everything in place and does not fetch again.
    let hits = server.hits.lock().unwrap().len();
    env.ok(
        "embedding.download",
        json!({ "spec": sp, "activate": false }),
    );
    wait_download_done(&env, &mut rx);
    assert_eq!(
        server.hits.lock().unwrap().len(),
        hits,
        "intact files are not downloaded again"
    );
}

#[test]
fn download_progress_events_are_emitted() {
    let model = vec![1u8; 4 * 1024 * 1024];
    let tok = b"tok".to_vec();
    let server = FileServer::start(vec![("/model", model.clone()), ("/tok", tok.clone())]);
    let env = Env::new("dl_progress");
    let mut rx = env.core.events.subscribe();
    env.ok(
        "embedding.download",
        json!({ "spec": spec(&server, &model, &tok), "activate": false }),
    );
    let mut seen = Vec::new();
    env.wait(30, || {
        while let Ok(ev) = rx.try_recv() {
            if ev.event == "embedding.download_progress" {
                seen.push(ev.data.clone());
            }
            if ev.event == "embedding.download_done" {
                return true;
            }
        }
        false
    });
    assert!(!seen.is_empty());
    let last = seen.last().unwrap();
    assert_eq!(last["overall_done"], last["overall_total"], "{last}");
    assert!(seen
        .windows(2)
        .all(|w| w[0]["overall_done"].as_u64() <= w[1]["overall_done"].as_u64()));
}

#[test]
fn a_corrupted_download_is_rejected_and_leaves_no_files() {
    let model = vec![9u8; 100_000];
    let tok = b"tok".to_vec();
    let server = FileServer::start(vec![("/model", vec![8u8; 100_000]), ("/tok", tok.clone())]);
    let env = Env::new("dl_bad");
    let mut rx = env.core.events.subscribe();
    env.ok(
        "embedding.download",
        json!({ "spec": spec(&server, &model, &tok) }),
    ); // hash is of the good bytes
    let done = wait_download_done(&env, &mut rx);
    assert_eq!(done["ok"], json!(false));
    assert!(
        done["error"]["message"]
            .as_str()
            .unwrap()
            .contains("checksum"),
        "{done}"
    );
    let dir = env.dir.join("models").join("test-model");
    assert!(!dir.join("model.onnx").exists() && !dir.join("model.onnx.part").exists());
    assert!(
        status(&env)["active_model"].is_null(),
        "model must not be activated"
    );
}

#[test]
fn unsafe_or_incomplete_specs_are_refused() {
    let env = Env::new("dl_spec");
    let good = json!({
        "id": "m", "display_name": "x", "dimension": 8, "query_prefix": "", "passage_prefix": "",
        "files": [
            {"name":"tokenizer.json","url":"https://example.com/t","sha256":"0".repeat(64),"size":1},
            {"name":"model.onnx","url":"https://example.com/m","sha256":"0".repeat(64),"size":1}
        ]
    });
    let mutate = |f: &dyn Fn(&mut Value)| {
        let mut v = good.clone();
        f(&mut v);
        v
    };
    for bad in [
        mutate(&|v| v["id"] = json!("../evil")),
        mutate(&|v| v["files"][0]["name"] = json!("../tokenizer.json")),
        mutate(&|v| v["files"][1]["url"] = json!("http://example.com/m")),
        mutate(&|v| v["files"][1]["url"] = json!("file:///etc/passwd")),
        mutate(&|v| v["files"][1]["sha256"] = json!("abc")),
        mutate(&|v| v["files"] = json!([v["files"][0].clone()])),
        mutate(&|v| v["dimension"] = json!(0)),
    ] {
        assert_eq!(
            env.err("embedding.download", json!({ "spec": bad.clone() })),
            "INVALID_ARGUMENT",
            "{bad}"
        );
    }
    assert!(!env.dir.join("models").join("evil").exists());
}

#[test]
fn a_running_download_can_be_cancelled_and_only_one_runs_at_a_time() {
    // never-ending-ish body: the server trickles a large file
    let model = vec![3u8; 40 * 1024 * 1024];
    let tok = b"tok".to_vec();
    let server = FileServer::start(vec![("/model", model.clone()), ("/tok", tok.clone())]);
    let env = Env::new("dl_cancel");
    let mut rx = env.core.events.subscribe();
    let sp = spec(&server, &model, &tok);
    env.ok("embedding.download", json!({ "spec": sp }));
    env.wait(10, || {
        env.ok("embedding.download_status", json!({}))["running"] == json!(true)
    });
    assert_eq!(
        env.err("embedding.download", json!({ "spec": sp })),
        "CONFLICT"
    );
    env.ok("embedding.download_cancel", json!({}));
    let done = wait_download_done(&env, &mut rx);
    assert_eq!(done["ok"], json!(false));
    assert_eq!(done["error"]["code"], json!("CANCELLED"));
    let dir = env.dir.join("models").join("test-model");
    assert!(!dir.join("model.onnx").exists() && !dir.join("model.onnx.part").exists());
    assert!(status(&env)["active_model"].is_null());
}

#[test]
fn default_model_description_is_available() {
    let env = Env::new("dl_default");
    let d = env.ok("embedding.default_model", json!({}));
    assert_eq!(d["id"], json!("multilingual-e5-small"));
    assert_eq!(d["dimension"], json!(384));
    assert_eq!(d["installed"], json!(false));
    assert!(d["total_bytes"].as_u64().unwrap() > 400_000_000);
}
