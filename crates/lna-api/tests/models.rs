//! Model configuration, capability probing and secret handling (D5, D6).
mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

fn files_under(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                files_under(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    haystack.windows(n.len()).any(|w| w == n)
}

/// D6: plain-http endpoints off the loopback interface need explicit confirmation.
#[test]
fn non_loopback_http_endpoint_needs_explicit_confirmation() {
    let env = Env::new("insecure");
    let base = |url: &str, extra: Value| {
        let mut p = json!({
            "display_name": "x", "protocol": "chat_completion", "base_url": url, "model_id": "m"
        });
        if let Value::Object(m) = extra {
            for (k, v) in m {
                p[k] = v;
            }
        }
        p
    };
    for url in [
        "http://example.com/v1",
        "http://192.168.1.20:8080/v1",
        "http://10.0.0.5/v1",
    ] {
        assert_eq!(
            env.err("profile.upsert", base(url, json!({}))),
            "INVALID_ARGUMENT",
            "{url}"
        );
    }
    // explicit confirmation, https, and loopback hosts are accepted
    env.ok(
        "profile.upsert",
        base(
            "http://example.com/v1",
            json!({ "allow_insecure_http": true }),
        ),
    );
    env.ok(
        "profile.upsert",
        base("https://api.example.com/v1", json!({})),
    );
    env.ok(
        "profile.upsert",
        base("http://127.0.0.1:11434/v1", json!({})),
    );
    env.ok(
        "profile.upsert",
        base("http://localhost:11434/v1", json!({})),
    );
}

/// D6: a random API key reaches the model as a bearer token, and is nowhere else: not in any API
/// response, event, log, database file or the diagnostics bundle.
#[test]
fn random_api_key_is_only_ever_sent_to_the_model() {
    let key = format!(
        "sk-{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
        _ => Reply::Text("向量空间的基与维数相关[1]。".into()),
    });
    let env = Env::new("keyscan");
    let mut rx = env.core.events.subscribe();
    let mut transcript = String::new();

    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    let prof = env.profile(&mock, 32000, false);
    let kref = prof["keychain_ref"].as_str().unwrap().to_string();
    transcript.push_str(&prof.to_string());
    let r = env.raw("secret.set", json!({ "keychain_ref": kref, "secret": key }));
    transcript.push_str(&r.to_string());
    assert!(r.get("error").is_none_or(|e| e.is_null()), "{r}");

    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    transcript.push_str(&msg.to_string());
    transcript.push_str(&env.ok("profile.list", json!({})).to_string());
    transcript.push_str(
        &env.ok("settings.get", json!({ "key": "ui.theme" }))
            .to_string(),
    );
    transcript.push_str(&env.ok("usage.summary", json!({})).to_string());
    transcript.push_str(&env.ok("app.info", json!({})).to_string());
    let bundle = env.dir.join("diag.json");
    env.ok(
        "diagnostics.export",
        json!({ "out_path": bundle.display().to_string() }),
    );
    // let async work (follow-ups) finish before scanning
    std::thread::sleep(std::time::Duration::from_millis(800));
    while let Ok(ev) = rx.try_recv() {
        transcript.push_str(&serde_json::to_string(&ev.data).unwrap());
    }

    // The key was actually used.
    let chat = mock.of_kind(Kind::Chat);
    assert_eq!(
        chat[0].auth.as_deref(),
        Some(format!("Bearer {key}").as_str())
    );

    assert!(
        !transcript.contains(&key),
        "key leaked into an API response or event"
    );
    let mut files = Vec::new();
    files_under(&env.dir, &mut files);
    assert!(!files.is_empty());
    for f in files {
        let bytes = std::fs::read(&f).unwrap_or_default();
        assert!(!contains(&bytes, &key), "key found in {}", f.display());
    }

    env.ok("secret.clear", json!({ "keychain_ref": kref }));
}

/// D6: a redirect is never followed, so the Authorization header cannot reach another host.
#[test]
fn cross_origin_redirect_is_not_followed_and_does_not_forward_credentials() {
    let target = MockLlm::start(|_| Reply::Text("should never be reached".into()));
    let target_url = format!("http://127.0.0.1:{}/v1/chat/completions", target.port);
    let origin = MockLlm::start(move |_| Reply::Redirect(target_url.clone()));
    let env = Env::new("redirect");
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    env.profile(&origin, 32000, false);
    let code = env.err(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    assert!(code.starts_with("MODEL_"), "{code}");
    assert_eq!(target.total(), 0, "redirect target must not be contacted");
}

fn probe_mock(tools_ok: bool, schema_ok: bool) -> MockLlm {
    MockLlm::start(move |r| {
        let rf = r.body["response_format"]["type"].as_str().unwrap_or("");
        if r.has_tools() {
            if tools_ok {
                return Reply::Tool {
                    id: "p1".into(),
                    name: "lna_capability_probe".into(),
                    args: json!({ "ok": true }),
                };
            }
            return Reply::Status(
                400,
                r#"{"error":{"message":"tools are not supported"}}"#.into(),
            );
        }
        match rf {
            "json_schema" if !schema_ok => Reply::Status(
                400,
                r#"{"error":{"message":"response_format json_schema is not supported"}}"#.into(),
            ),
            "json_schema" | "json_object" => Reply::Text(r#"{"ok":true}"#.into()),
            _ => Reply::Text("OK".into()),
        }
    })
}

/// D5: `profile.test` measures tools / JSON Schema / JSON mode / stream usage and stores them.
#[test]
fn profile_test_probes_capabilities_and_persists_them() {
    let mock = probe_mock(true, true);
    let env = Env::new("probe");
    let prof = env.profile(&mock, 32000, false);
    let pid = prof["id"].as_str().unwrap().to_string();
    let r = env.ok("profile.test", json!({ "profile_id": pid }));
    let caps = &r["capabilities"];
    assert_eq!(caps["tools"], json!(true), "{r}");
    assert_eq!(caps["json_schema"], json!(true));
    assert_eq!(caps["json_mode"], json!(true));
    assert_eq!(caps["stream_usage"], json!(true));
    let stored = env.ok("profile.list", json!({}));
    assert_eq!(stored["profiles"][0]["capabilities"]["tools"], json!(true));
}

#[test]
fn profile_test_marks_unsupported_features_as_false() {
    let mock = probe_mock(false, false);
    let env = Env::new("probe2");
    let prof = env.profile(&mock, 32000, true);
    let pid = prof["id"].as_str().unwrap().to_string();
    let r = env.ok("profile.test", json!({ "profile_id": pid }));
    let caps = &r["capabilities"];
    assert_eq!(caps["tools"], json!(false), "{r}");
    assert_eq!(caps["json_schema"], json!(false));
    assert_eq!(caps["json_mode"], json!(true));
    let stored = env.ok("profile.list", json!({}));
    assert_eq!(stored["profiles"][0]["capabilities"]["tools"], json!(false));
}

/// D5: a 400 naming an unsupported parameter drops it, retries once, and remembers the result.
#[test]
fn unsupported_stream_options_is_dropped_retried_and_remembered() {
    let mock = MockLlm::start(|r| {
        if r.body.get("stream_options").is_some() {
            return Reply::Status(
                400,
                r#"{"error":{"message":"Unknown parameter: 'stream_options'"}}"#.into(),
            );
        }
        match r.kind {
            Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
            _ => Reply::Text("向量空间的基与维数相关[1]。".into()),
        }
    });
    let env = Env::new("relax");
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    env.profile(&mock, 32000, false); // helper enables stream_usage
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    assert_eq!(msg["status"], json!("COMPLETE"), "{msg}");
    let chat = mock.of_kind(Kind::Chat);
    assert_eq!(chat.len(), 2, "one failed attempt plus one retry");
    assert!(chat[0].body.get("stream_options").is_some());
    assert!(chat[1].body.get("stream_options").is_none());
    let stored = env.ok("profile.list", json!({}));
    assert_eq!(
        stored["profiles"][0]["capabilities"]["stream_usage"],
        json!(false),
        "{stored}"
    );
    // the next turn no longer sends the parameter at all
    env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请再详细说明基与维数之间的关系是什么" }),
    );
    let chat = mock.of_kind(Kind::Chat);
    assert_eq!(chat.len(), 3);
}

#[test]
fn builtin_provider_presets_include_official_and_opencode_go_headers() {
    let env = Env::new("presets");
    let res = env.ok("profile.presets", json!({}));
    let presets = res["presets"].as_array().expect("presets array");
    let ids: Vec<&str> = presets
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    for expected in [
        "openai",
        "anthropic",
        "google",
        "deepseek",
        "glm",
        "mimo",
        "opencode_go",
    ] {
        assert!(ids.contains(&expected), "missing preset {expected} in {ids:?}");
    }
    let opencode = presets
        .iter()
        .find(|p| p["id"] == "opencode_go")
        .expect("opencode_go preset");
    assert_eq!(opencode["base_url"], "https://opencode.ai/zen/go/v1");
    assert_eq!(opencode["protocol"], "chat_completion");
    assert_eq!(
        opencode["extra_headers"]["x-opencode-session"],
        "local-notebook-agent"
    );
    assert_eq!(opencode["extra_headers"]["User-Agent"], "opencode/1.0");
}
