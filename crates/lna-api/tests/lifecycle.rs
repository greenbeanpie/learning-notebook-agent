//! Start-up / shut-down behaviour (S3) and the API contract (S1).
mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

/// S3: after a crash the leftovers (RUNNING job, RUNNING artifact) are picked up again and the
/// artifact finishes on the next start.
#[test]
fn interrupted_studio_job_resumes_after_restart() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Studio => studio_reply(r),
        Kind::NotebookSummary => Reply::Text("{}".into()),
        _ => Reply::Text("OK".into()),
    });
    let dir = temp_dir("resume");
    let (nid, aid) = {
        let env = Env {
            core: lna_api::CoreRuntime::open(dir.clone(), true).unwrap(),
            dir: temp_dir("unused"),
        };
        let nid = env.notebook();
        env.add_text(&nid, "讲义.md", ZH_DOC);
        env.wait_sources_ready(&nid, 1);
        // Configure a model that is unreachable so the job does not finish before the "crash".
        env.ok(
            "profile.upsert",
            json!({"display_name":"x","protocol":"chat_completion","base_url":mock.base_url(),"model_id":"m","activate":true,"context_window":32000,"max_output_tokens":1000}),
        );
        let aid = env.ok(
            "studio.create",
            json!({"notebook_id": nid, "artifact_type": "quiz", "config": {}}),
        )["id"]
            .as_str()
            .unwrap()
            .to_string();
        env.wait(30, || {
            env.ok("studio.get", json!({"artifact_id": aid}))["status"] == json!("READY")
        });
        // Rewind to what a kill -9 mid-run leaves behind.
        env.core
            .db
            .write(|c| {
                c.execute("UPDATE studio_artifacts SET status='RUNNING', stage='GENERATE', payload_json=NULL, finished_at=NULL", [])
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                c.execute("DELETE FROM artifact_checkpoints", [])
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                c.execute("UPDATE jobs SET status='RUNNING' WHERE kind='studio_generate'", [])
                    .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
                Ok(())
            })
            .unwrap();
        // drop without a graceful shutdown
        let (n, a) = (nid.clone(), aid.clone());
        std::mem::forget(env.core.clone());
        drop(env);
        (n, a)
    };
    // The leaked Arc keeps the first runtime alive; give a fresh process-like open its own dir copy.
    let dir2 = temp_dir("resume2");
    copy_dir(&dir, &dir2);
    let env2 = Env {
        core: lna_api::CoreRuntime::open(dir2.clone(), true).unwrap(),
        dir: dir2,
    };
    let _ = nid;
    env2.wait(40, || {
        env2.ok("studio.get", json!({"artifact_id": aid}))["status"] == json!("READY")
    });
    let art = env2.ok("studio.get", json!({"artifact_id": aid}));
    assert!(art["payload"]["questions"].is_array());
    let _ = std::fs::remove_dir_all(&dir);
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let dest = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            let _ = std::fs::copy(&p, &dest);
        }
    }
}

/// S3: a graceful shutdown folds the WAL into the main database file.
#[test]
fn graceful_shutdown_truncates_the_wal() {
    let env = Env::new("wal");
    let nid = env.notebook();
    for i in 0..20 {
        env.add_text(
            &nid,
            &format!("文档{i}.md"),
            &format!("# 文档{i}\n\n内容{i}：线性代数。"),
        );
    }
    env.wait_sources_ready(&nid, 20);
    env.core.shutdown();
    let wal = env.dir.join("notebook.db-wal");
    let len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    assert_eq!(len, 0, "WAL should be empty after a graceful shutdown");
}

/// S3: data written in one session is there in the next.
#[test]
fn data_survives_a_restart() {
    let dir = temp_dir("persist");
    let nid;
    {
        let core = lna_api::CoreRuntime::open(dir.clone(), true).unwrap();
        let env = Env {
            core,
            dir: temp_dir("unused2"),
        };
        nid = env.notebook();
        env.add_text(&nid, "讲义.md", ZH_DOC);
        env.wait_sources_ready(&nid, 1);
        env.ok(
            "note.save",
            json!({"notebook_id": nid, "title": "n", "body_md": "b"}),
        );
        env.core.shutdown();
    }
    let env = Env {
        core: lna_api::CoreRuntime::open(dir.clone(), true).unwrap(),
        dir: dir.clone(),
    };
    assert_eq!(
        env.ok("source.list", json!({"notebook_id": nid}))["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        env.ok("note.list", json!({"notebook_id": nid}))["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// S3: orphan blob folders are removed about five seconds after start, not immediately.
#[test]
fn orphan_blob_directories_are_cleaned_up_after_a_delay() {
    let dir = temp_dir("orphan");
    {
        let core = lna_api::CoreRuntime::open(dir.clone(), true).unwrap();
        core.shutdown();
    }
    let orphan = dir.join("blobs").join("nb1").join("ghost-source");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("original.txt"), "x").unwrap();
    let env = Env {
        core: lna_api::CoreRuntime::open(dir.clone(), true).unwrap(),
        dir: dir.clone(),
    };
    std::thread::sleep(Duration::from_millis(1500));
    assert!(orphan.exists(), "not removed during start-up");
    env.wait(12, || !orphan.exists());
}

/// S1: every method in the dispatcher answers an empty request with a structured error or a
/// result: never INTERNAL, never a hang. Method names are read from the source so a new method
/// is covered automatically.
#[test]
fn every_api_method_handles_empty_params_without_internal_errors() {
    let src = include_str!("../src/lib.rs");
    let start = src.find("match req.method.as_str() {").expect("dispatcher");
    let mut methods: Vec<String> = Vec::new();
    for line in src[start..].lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix('"') {
            if let Some((name, tail)) = rest.split_once('"') {
                if tail.trim_start().starts_with("=>") && name.contains('.') {
                    methods.push(name.to_string());
                }
            }
        }
    }
    assert!(methods.len() >= 60, "found only {} methods", methods.len());
    let env = Env::new("contract");
    // Methods that open files or take a long time without real input are given dummy-safe params.
    for m in &methods {
        if m == "profile.test" || m == "profile.discover_models" {
            continue; // need a real profile; covered in models.rs
        }
        let r = env.raw(m, json!({}));
        if let Some(code) = r["error"]["code"].as_str() {
            assert_ne!(
                code, "INTERNAL",
                "{m} returned INTERNAL for empty params: {r}"
            );
            assert!(r["error"]["message"].is_string(), "{m}: {r}");
        } else {
            assert!(r.get("result").is_some(), "{m}: {r}");
        }
    }
    assert!(env.raw("no.such.method", json!({}))["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Unknown method"));
    // Garbage JSON is a clean error, not a crash.
    let out: Value = serde_json::from_str(&env.core.handle_json("{not json")).unwrap();
    assert_eq!(out["error"]["code"], json!("INVALID_ARGUMENT"));
    // Unknown ids are NOT_FOUND, not INTERNAL.
    for (m, p) in [
        ("notebook.get", json!({"notebook_id": "nope"})),
        ("source.get", json!({"source_id": "nope"})),
        ("studio.get", json!({"artifact_id": "nope"})),
        ("note.get", json!({"note_id": "nope"})),
    ] {
        assert_eq!(env.err(m, p), "NOT_FOUND", "{m}");
    }
}

/// The deferred and cancelled scopes stay closed.
#[test]
fn deferred_and_cancelled_features_are_rejected() {
    let env = Env::new("scope");
    let nid = env.notebook();
    assert_eq!(
        env.err("research.start", json!({"notebook_id": nid})),
        "CAPABILITY_MISSING"
    );
    assert_eq!(
        env.err(
            "studio.create",
            json!({"notebook_id": nid, "artifact_type": "video", "config": {}})
        ),
        "INVALID_ARGUMENT"
    );
}

/// S1: Exported API DTO JSON Schema matches Rust DTO serialization and C# JsonContext.cs properties.
#[test]
fn s1_dto_schema_matches_rust_and_csharp() {
    let schema: Value = serde_json::from_str(include_str!("../../../schemas/api.schema.json"))
        .expect("valid api.schema.json");
    let defs = schema["definitions"].as_object().expect("definitions");
    for def in [
        "OpenConfig",
        "ApiRequest",
        "ApiError",
        "ApiResponse",
        "ApiEvent",
    ] {
        assert!(defs.contains_key(def), "missing definition {def}");
    }

    // Verify Rust DTOs serialize with matching keys.
    let req_val = serde_json::to_value(lna_types::ApiRequest {
        id: "1".into(),
        method: "app.health".into(),
        params: json!({}),
    })
    .unwrap();
    for k in defs["ApiRequest"]["properties"].as_object().unwrap().keys() {
        assert!(req_val.get(k).is_some(), "ApiRequest missing {k}");
    }

    let resp_val = serde_json::to_value(lna_types::ApiResponse::err(
        "1".into(),
        lna_types::LnaError::invalid("bad"),
    ))
    .unwrap();
    assert!(resp_val.get("id").is_some());
    assert!(resp_val.get("error").is_some());
    for k in defs["ApiError"]["properties"].as_object().unwrap().keys() {
        assert!(resp_val["error"].get(k).is_some(), "ApiError missing {k}");
    }

    let evt_val = serde_json::to_value(lna_types::ApiEvent {
        seq: 1,
        topic: "chat".into(),
        event: "chat.text_delta".into(),
        data: json!({}),
    })
    .unwrap();
    for k in defs["ApiEvent"]["properties"].as_object().unwrap().keys() {
        assert!(evt_val.get(k).is_some(), "ApiEvent missing {k}");
    }

    // Verify C# JsonContext.cs declares every property from the schema.
    let cs = include_str!("../../../apps/windows/LocalNotebook/Interop/JsonContext.cs");
    for def in [
        "OpenConfig",
        "ApiRequest",
        "ApiError",
        "ApiResponse",
        "ApiEvent",
    ] {
        for k in defs[def]["properties"].as_object().unwrap().keys() {
            let attr = format!("[JsonPropertyName(\"{k}\")]");
            assert!(
                cs.contains(&attr),
                "JsonContext.cs missing {attr} for {def}"
            );
        }
    }
}
