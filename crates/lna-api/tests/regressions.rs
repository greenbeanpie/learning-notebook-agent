//! Regression tests for defects found while driving the Windows client with UI automation.

use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn call(core: &CoreRuntime, method: &str, params: Value) -> Value {
    let req = json!({ "id": "t", "method": method, "params": params }).to_string();
    serde_json::from_str(&core.handle_json(&req)).expect("core returned invalid JSON")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("lna_{tag}_{}_{nanos}", std::process::id()))
}

/// B-25: `has_secret` used to be `keychain_ref.is_some()`, so it was true for every profile
/// even when no key had ever been stored.
#[test]
fn profile_has_secret_reflects_the_stored_secret() {
    let dir = temp_dir("secret");
    let core = CoreRuntime::open(dir.clone(), true).unwrap();

    let saved = call(
        &core,
        "profile.upsert",
        json!({
            "display_name": "t", "protocol": "chat_completion",
            "base_url": "http://127.0.0.1:1/v1", "model_id": "m",
            // nothing listens on port 1: an embedding endpoint is saved without the chat probe
            "purpose": "embedding"
        }),
    );
    let prof = &saved["result"];
    let id = prof["id"].as_str().expect("profile id").to_string();
    let kref = prof["keychain_ref"]
        .as_str()
        .expect("keychain_ref")
        .to_string();
    assert_eq!(
        prof["has_secret"],
        json!(false),
        "no key stored yet: {saved}"
    );

    let has_secret = |core: &CoreRuntime| -> Value {
        let list = call(core, "profile.list", json!({}));
        list["result"]["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == json!(id))
            .map(|p| p["has_secret"].clone())
            .expect("profile in list")
    };
    assert_eq!(has_secret(&core), json!(false));

    call(
        &core,
        "secret.set",
        json!({ "keychain_ref": kref, "secret": "sk-regression-test" }),
    );
    assert_eq!(has_secret(&core), json!(true));

    call(&core, "secret.clear", json!({ "keychain_ref": kref }));
    assert_eq!(has_secret(&core), json!(false));

    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The client only refreshes on `source.status` events; the core never emitted them, so a
/// source stayed on `[QUEUED]` in the UI after it was already READY.
#[test]
fn ingest_emits_source_status_events() {
    let dir = temp_dir("events");
    let core = CoreRuntime::open(dir.clone(), true).unwrap();
    let mut rx = core.events.subscribe();

    let nb = call(&core, "notebook.create", json!({ "title": "t" }));
    let nid = nb["result"]["id"]
        .as_str()
        .expect("notebook id")
        .to_string();

    let file = dir.join("note.txt");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&file, "光合作用发生在叶绿体中。").unwrap();
    call(
        &core,
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [file.display().to_string()] }),
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut status_events = 0;
    let mut ready = false;
    while Instant::now() < deadline && !(ready && status_events >= 2) {
        while let Ok(ev) = rx.try_recv() {
            if ev.event == "source.status" {
                assert_eq!(ev.data["notebook_id"], json!(nid));
                status_events += 1;
            }
        }
        let list = call(&core, "source.list", json!({ "notebook_id": nid }));
        ready = list["result"]["sources"][0]["status"] == json!("READY");
        std::thread::sleep(Duration::from_millis(100));
    }
    // Drain anything delivered between the last poll and the READY observation.
    std::thread::sleep(Duration::from_millis(300));
    while let Ok(ev) = rx.try_recv() {
        if ev.event == "source.status" {
            status_events += 1;
        }
    }

    assert!(ready, "source never became READY");
    assert!(
        status_events >= 2,
        "expected a source.status event when ingest starts and when it finishes, got {status_events}"
    );

    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_retry_resets_status_and_errors_and_emits_event() {
    let dir = temp_dir("retry");
    let core = CoreRuntime::open(dir.clone(), true).unwrap();
    let nb = call(&core, "notebook.create", json!({ "title": "t" }));
    let nid = nb["result"]["id"].as_str().unwrap().to_string();

    let file = dir.join("retry.txt");
    std::fs::write(&file, "测试重试重置状态。").unwrap();
    let added = call(
        &core,
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [file.display().to_string()] }),
    );
    let sid = added["result"]["items"][0]["source"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Simulate a failed source state
    core.db
        .write(|conn| {
            conn.execute(
                "UPDATE sources SET status='FAILED', error_code='INTERNAL', error_detail='boom' WHERE id=?1",
                rusqlite::params![sid],
            )
            .map_err(lna_storage::db::map_sql_err)?;
            Ok(())
        })
        .unwrap();

    let mut rx = core.events.subscribe();
    let retried = call(&core, "source.retry", json!({ "source_id": sid }));
    assert_eq!(retried["result"]["ok"], json!(true));

    let got_ev = rx.try_recv().ok().map(|e| e.event);
    assert_eq!(got_ev.as_deref(), Some("source.status"));

    let after = lna_storage::get_source(&core.db, &sid).unwrap();
    assert_ne!(after.status, lna_types::SourceStatus::Failed);
    assert_eq!(after.error_code, None);
    assert_eq!(after.error_detail, None);

    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tool / JSON flags come only from the probe: what a client sends is ignored, and a chat
/// profile whose endpoint cannot be probed is not saved at all.
#[test]
fn profile_upsert_ignores_client_capability_flags() {
    let dir = temp_dir("caps");
    let core = CoreRuntime::open(dir.clone(), true).unwrap();
    let claimed = json!({ "streaming": true, "tools": true, "json_schema": true, "supports_vision": true });
    let refused = call(
        &core,
        "profile.upsert",
        json!({
            "display_name": "caps", "protocol": "chat_completion",
            "base_url": "http://127.0.0.1:1/v1", "model_id": "m", "capabilities": claimed
        }),
    );
    assert!(!refused["error"].is_null(), "unreachable chat endpoint must not be saved: {refused}");
    assert!(call(&core, "profile.list", json!({}))["result"]["profiles"].as_array().unwrap().is_empty());

    let saved = call(
        &core,
        "profile.upsert",
        json!({
            "display_name": "emb", "protocol": "chat_completion",
            "base_url": "http://127.0.0.1:1/v1", "model_id": "m", "capabilities": claimed,
            "purpose": "embedding"
        }),
    );
    let caps = &saved["result"]["capabilities"];
    assert_eq!(caps["tools"], json!(false), "{saved}");
    assert_eq!(caps["json_schema"], json!(false));
    assert_eq!(caps["supports_vision"], json!(true), "media flags are still the user's choice");
    drop(core);
    let _ = std::fs::remove_dir_all(&dir);
}
