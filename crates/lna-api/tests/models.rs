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
        // embedding endpoints skip the capability probe, so only the URL rule is exercised here
        let mut p = json!({
            "display_name": "x", "protocol": "chat_completion", "base_url": url, "model_id": "m",
            "purpose": "embedding"
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
    let prof = env.profile(&mock, 32000);
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
    env.profile(&origin, 32000);
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

fn upsert_params(mock: &MockLlm) -> Value {
    json!({
        "display_name": "probe", "protocol": "chat_completion",
        "base_url": mock.base_url(), "model_id": "mock-model", "activate": true,
        // tool / JSON flags sent by a client are ignored: only the probe decides
        "capabilities": { "tools": true, "json_schema": true, "json_mode": true }
    })
}

/// Saving a profile stores it and starts a background probe: the reply still shows it unchecked,
/// and the probe then stores tools / JSON Schema / JSON mode / stream usage and the media inputs
/// (announced by `profile.updated`). Saving again without a connection change keeps the result;
/// changing the model makes it stale and probes again. `profile.test` probes once more.
#[test]
fn saving_a_profile_starts_a_probe_and_testing_repeats_it() {
    let env = Env::new("probe");
    for (schema_ok, json_mode_ok) in [(true, true), (false, true)] {
        let mock = probe_mock(true, schema_ok);
        mock.raw_probe
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut rx = env.core.events.subscribe();
        let prof = env.ok("profile.upsert", upsert_params(&mock));
        let pid = prof["id"].as_str().unwrap().to_string();
        let caps = &prof["capabilities"];
        assert_eq!(caps["tools"], json!(false), "{prof}");
        assert_eq!(caps["json_schema"], json!(false));
        assert!(caps["probed_at"].is_null(), "{prof}");

        // the background probe stores its result and tells clients
        env.wait_probed(&pid);
        let row = listed_profile(&env, &pid);
        assert_eq!(row["probe_pending"], json!(false), "{row}");
        assert_eq!(row["capabilities"]["tools"], json!(true), "{row}");
        assert_eq!(row["capabilities"]["json_schema"], json!(schema_ok));
        assert_eq!(row["capabilities"]["json_mode"], json!(json_mode_ok));
        assert_eq!(row["capabilities"]["stream_usage"], json!(true));
        let mut announced = false;
        env.wait(10, || {
            while let Ok(ev) = rx.try_recv() {
                announced |= ev.topic == "profiles"
                    && ev.event == "profile.updated"
                    && ev.data["profile_id"] == json!(pid);
            }
            announced
        });

        let r = env.ok("profile.test", json!({ "profile_id": pid }));
        assert!(mock.total() > 0);
        assert_eq!(r["capabilities"]["tools"], json!(true), "{r}");
        assert_eq!(r["capabilities"]["json_schema"], json!(schema_ok));
        assert_eq!(r["capabilities"]["json_mode"], json!(json_mode_ok));
        assert_eq!(r["capabilities"]["stream_usage"], json!(true));
        assert!(r["capabilities"]["probed_at"].is_i64());

        // saving again without touching the connection keeps the measurements, and does not probe
        let mut again = upsert_params(&mock);
        again["id"] = json!(pid);
        let before = mock.total();
        let prof = env.ok("profile.upsert", again);
        assert_eq!(prof["capabilities"]["tools"], json!(true), "{prof}");
        assert!(prof["capabilities"]["probed_at"].is_i64());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            mock.total(),
            before,
            "an unchanged profile is not probed again"
        );

        // changing the model makes them stale; the reply says so and the probe runs again
        let mut changed = upsert_params(&mock);
        changed["id"] = json!(pid);
        changed["model_id"] = json!("another-model");
        let prof = env.ok("profile.upsert", changed);
        assert_eq!(prof["capabilities"]["tools"], json!(false), "{prof}");
        assert!(prof["capabilities"]["probed_at"].is_null());
        env.wait_probed(&pid);
        let found = listed_profile(&env, &pid);
        assert_eq!(found["eligible"], json!(true), "{found}");
    }
}

/// A model without tool calling, or without any structured output, is saved but refused once
/// tested: it is not eligible and cannot be activated.
#[test]
fn models_without_tools_or_structured_output_are_refused() {
    let env = Env::new("probe_refuse");
    let refuse = |params: Value| -> Value {
        let mut p = params;
        p["activate"] = json!(false);
        let saved = env.ok("profile.upsert", p);
        let pid = saved["id"].clone();
        let r = env.raw("profile.test", json!({ "profile_id": pid }));
        let listed = env.ok("profile.list", json!({}));
        let row = listed["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == pid)
            .unwrap()
            .clone();
        assert_eq!(row["eligible"], json!(false), "{row}");
        let a = env.raw("profile.activate", json!({ "profile_id": pid }));
        assert_eq!(a["error"]["code"], json!("CAPABILITY_MISSING"), "{a}");
        let _ = env.ok("profile.delete", json!({ "profile_id": pid }));
        r
    };
    // no tool calling
    let mock = probe_mock(false, true);
    mock.raw_probe
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = refuse(upsert_params(&mock));
    assert_eq!(r["error"]["code"], json!("CAPABILITY_MISSING"), "{r}");
    assert!(
        r["error"]["message"].as_str().unwrap().contains("工具调用"),
        "{r}"
    );
    // no JSON schema and no JSON mode
    let mock = MockLlm::start(|r| {
        if r.has_tools() {
            return Reply::Tool {
                id: "p1".into(),
                name: "lna_capability_probe".into(),
                args: json!({ "ok": true }),
            };
        }
        if r.body.get("response_format").is_some() {
            return Reply::Status(
                400,
                r#"{"error":{"message":"response_format is not supported"}}"#.into(),
            );
        }
        Reply::Text("OK".into())
    });
    mock.raw_probe
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = refuse(upsert_params(&mock));
    assert_eq!(r["error"]["code"], json!("CAPABILITY_MISSING"), "{r}");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("结构化输出"),
        "{r}"
    );
    // nothing is left over
    let stored = env.ok("profile.list", json!({}));
    assert!(
        stored["profiles"].as_array().unwrap().is_empty(),
        "{stored}"
    );
    // an embedding endpoint is not held to the chat rules
    let emb = env.ok(
        "profile.upsert",
        json!({
            "display_name": "emb", "protocol": "chat_completion", "base_url": mock.base_url(),
            "model_id": "text-emb", "purpose": "embedding"
        }),
    );
    assert_eq!(emb["capabilities"]["embedding_only"], json!(true));
    let r = env.raw("profile.activate", json!({ "profile_id": emb["id"] }));
    assert_eq!(r["error"]["code"], json!("CAPABILITY_MISSING"), "{r}");
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
    env.profile(&mock, 32000); // helper enables stream_usage
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
fn a_model_rejecting_tools_at_run_time_is_reported_not_downgraded() {
    let mock = MockLlm::start(|r| {
        if r.kind == Kind::Chat && r.body.get("tools").is_some() {
            return Reply::Status(
                400,
                r#"{"error":{"message":"tools are not supported by this model"}}"#.into(),
            );
        }
        match r.kind {
            Kind::Followups => Reply::Text(r#"{"questions":["a?","b?","c?"]}"#.into()),
            _ => Reply::Text("向量空间的基与维数相关[1]。".into()),
        }
    });
    let env = Env::new("no_downgrade");
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    env.profile(&mock, 32000);
    let r = env.raw(
        "chat.send",
        json!({ "notebook_id": nid, "text": "请详细说明基与维数之间的关系是什么" }),
    );
    let text = r.to_string();
    assert!(text.contains("CAPABILITY_MISSING"), "{text}");
    assert!(text.contains("工具调用"), "{text}");
    let chat = mock.of_kind(Kind::Chat);
    assert!(
        chat.iter().all(|c| c.body.get("tools").is_some()),
        "never retried without tools"
    );
    // the run-time rejection does not downgrade the stored profile: it stays as the probe found it
    // (the user re-tests it in the settings)
    let stored = env.ok("profile.list", json!({}));
    assert_eq!(
        stored["profiles"][0]["capabilities"]["tools"],
        json!(true),
        "{stored}"
    );
}

#[test]
fn builtin_provider_presets_include_official_and_opencode_go_headers() {
    let env = Env::new("presets");
    let res = env.ok("profile.presets", json!({}));
    let presets = res["presets"].as_array().expect("presets array");
    // media support is detected by the test, never declared by a preset
    for p in presets {
        assert!(p.get("supports_vision").is_none(), "{p}");
        assert!(p.get("supports_audio").is_none(), "{p}");
        assert!(p.get("supports_video").is_none(), "{p}");
    }
    let ids: Vec<&str> = presets.iter().filter_map(|p| p["id"].as_str()).collect();
    for expected in [
        "openai",
        "anthropic",
        "google",
        "deepseek",
        "glm",
        "mimo",
        "opencode_go",
    ] {
        assert!(
            ids.contains(&expected),
            "missing preset {expected} in {ids:?}"
        );
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

    let cf = presets
        .iter()
        .find(|p| p["id"] == "cloudflare_ai_gateway")
        .expect("cloudflare_ai_gateway preset");
    assert_eq!(cf["protocol"], "chat_completion");
    assert!(cf["base_url"].as_str().unwrap().ends_with("/compat"));
    assert_eq!(
        cf["extra_headers"]["cf-aig-authorization"],
        "Bearer {api_key}"
    );
}

/// Profiles saved before the rule: once probed and found lacking, they are refused for chat,
/// cannot be activated, and are skipped for media routing. Never-probed ones are probed at start.
#[test]
fn existing_unqualified_profiles_are_refused_after_probing() {
    let mock = MockLlm::start(|_| Reply::Text("答".into()));
    let env = Env::new("legacy_profile");
    let prof = env.profile(&mock, 32000);
    let pid = prof["id"].as_str().unwrap().to_string();
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);

    // simulate a profile saved by an older version: no tools, never probed, vision on
    let mut caps: lna_types::ModelCapabilities =
        serde_json::from_value(prof["capabilities"].clone()).unwrap();
    caps.tools = false;
    caps.supports_vision = true;
    caps.probed_at = None;
    lna_storage::update_profile_capabilities(&env.core.db, &pid, &caps).unwrap();
    let list = env.ok("profile.list", json!({}));
    assert_eq!(list["profiles"][0]["probe_pending"], json!(true), "{list}");

    // probed and lacking tool calling
    caps.probed_at = Some(1);
    lna_storage::update_profile_capabilities(&env.core.db, &pid, &caps).unwrap();
    let code = env.err(
        "chat.send",
        json!({ "notebook_id": nid, "text": "基是什么" }),
    );
    assert_eq!(code, "CAPABILITY_MISSING");
    assert_eq!(
        env.err("profile.activate", json!({ "profile_id": pid })),
        "CAPABILITY_MISSING"
    );
    assert!(
        lna_storage::resolve_modality_profile(&env.core.db, "vision")
            .unwrap()
            .is_none()
    );
    let list = env.ok("profile.list", json!({}));
    assert_eq!(list["profiles"][0]["eligible"], json!(false));
    assert!(list["profiles"][0]["unsupported_reason"]
        .as_str()
        .unwrap()
        .contains("工具调用"));

    // re-testing against a now-capable endpoint makes it usable again
    env.ok("profile.test", json!({ "profile_id": pid }));
    let list = env.ok("profile.list", json!({}));
    assert_eq!(list["profiles"][0]["eligible"], json!(true), "{list}");
}

/// Whether any user message of a Chat Completions request carries a content part of `kind`.
fn has_part(body: &Value, kind: &str) -> bool {
    body["messages"].as_array().into_iter().flatten().any(|m| {
        m["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["type"] == kind)
    })
}

/// A model that sees pictures (when `sees`), rejects audio, and cannot take video. The media
/// samples are sent to it as real requests (`raw_probe`), so their answers are what the test sees.
fn media_mock(sees: bool) -> MockLlm {
    let mock = MockLlm::start(move |r| {
        if r.has_tools() {
            return Reply::Tool {
                id: "p1".into(),
                name: "lna_capability_probe".into(),
                args: json!({ "ok": true }),
            };
        }
        if matches!(
            r.body["response_format"]["type"].as_str(),
            Some("json_schema" | "json_object")
        ) {
            return Reply::Text(r#"{"ok":true}"#.into());
        }
        if has_part(&r.body, "input_audio") {
            return Reply::Status(
                400,
                r#"{"error":{"message":"audio input is not supported"}}"#.into(),
            );
        }
        if has_part(&r.body, "image_url") {
            let t = if sees {
                "Red."
            } else {
                "I cannot see any attachment."
            };
            return Reply::Text(t.into());
        }
        Reply::Text("OK".into())
    });
    mock.raw_probe
        .store(true, std::sync::atomic::Ordering::SeqCst);
    mock
}

/// Image / audio / video support is detected by the test, whatever the profile declared: a sample
/// the model understands counts as support, and a clean "not supported" is `false` and not a
/// failure. Here the model sees pictures, rejects audio, and Chat Completions has no video input.
#[test]
fn testing_detects_the_inputs_the_model_takes_whatever_was_declared() {
    let env = Env::new("probe_media");
    for sees in [true, false] {
        for declared in [true, false] {
            let mock = media_mock(sees);
            let mut params = upsert_params(&mock);
            params["supports_vision"] = json!(declared);
            params["supports_audio"] = json!(declared);
            params["supports_video"] = json!(declared);
            let prof = env.ok("profile.upsert", params);
            let pid = prof["id"].as_str().unwrap().to_string();

            let r = env.ok("profile.test", json!({ "profile_id": pid }));
            assert_eq!(
                r["detected"],
                json!({ "vision": sees, "audio": false, "video": false }),
                "{r}"
            );
            let caps = &r["capabilities"];
            assert_eq!(caps["supports_vision"], json!(sees), "{r}");
            assert_eq!(caps["supports_audio"], json!(false), "{r}");
            assert_eq!(caps["supports_video"], json!(false), "{r}");
            assert_eq!(r["media_failed"], json!([]), "{r}");

            // the stored profile says the same
            let row = listed_profile(&env, &pid);
            assert_eq!(row["capabilities"]["supports_vision"], json!(sees), "{row}");
            assert_eq!(row["capabilities"]["supports_audio"], json!(false), "{row}");
            assert_eq!(row["capabilities"]["supports_video"], json!(false), "{row}");
        }
    }
}

/// `profile.upsert` never takes image / audio / video support from the client: a new profile
/// starts without it, and an edit that keeps the connection keeps what the probe found.
#[test]
fn upsert_ignores_client_sent_media_flags() {
    let env = Env::new("upsert_flags");
    let mock = media_mock(true);
    let mut params = upsert_params(&mock);
    params["supports_vision"] = json!(true);
    params["supports_audio"] = json!(true);
    params["supports_video"] = json!(true);
    let prof = env.ok("profile.upsert", params);
    let pid = prof["id"].as_str().unwrap().to_string();
    assert_eq!(
        prof["capabilities"]["supports_vision"],
        json!(false),
        "{prof}"
    );
    assert_eq!(
        prof["capabilities"]["supports_audio"],
        json!(false),
        "{prof}"
    );
    assert_eq!(
        prof["capabilities"]["supports_video"],
        json!(false),
        "{prof}"
    );

    env.wait_probed(&pid);
    let row = listed_profile(&env, &pid);
    assert_eq!(row["capabilities"]["supports_vision"], json!(true), "{row}");
    assert_eq!(row["capabilities"]["supports_audio"], json!(false), "{row}");

    // an edit that keeps the connection keeps the probe's result, whatever the client sends
    let mut again = upsert_params(&mock);
    again["id"] = json!(pid);
    again["supports_vision"] = json!(false);
    again["supports_audio"] = json!(true);
    again["supports_video"] = json!(true);
    let prof = env.ok("profile.upsert", again);
    assert_eq!(
        prof["capabilities"]["supports_vision"],
        json!(true),
        "{prof}"
    );
    assert_eq!(
        prof["capabilities"]["supports_audio"],
        json!(false),
        "{prof}"
    );
    assert_eq!(
        prof["capabilities"]["supports_video"],
        json!(false),
        "{prof}"
    );
    assert!(prof["capabilities"]["probed_at"].is_i64(), "{prof}");
}

fn listed_profile(env: &Env, pid: &str) -> Value {
    let listed = env.ok("profile.list", json!({}));
    listed["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pid)
        .unwrap()
        .clone()
}

/// `preset` and `shrink_media` persist per profile. "mimo" keeps shrinking on; an update without
/// the fields keeps both; `preset: ""` clears the preset and turns shrinking off; shrinking is
/// stored off for any non-MiMo preset.
#[test]
fn preset_and_shrink_media_persist_per_profile() {
    let env = Env::new("preset_shrink");
    // saving starts a background probe, so the endpoint is a local mock, not the provider
    let mock = MockLlm::start(|_| Reply::Text("OK".into()));
    let base = json!({
        "display_name": "MiMo", "protocol": "chat_completion",
        "base_url": mock.base_url(), "model_id": "mimo-v2.6-flash",
    });

    let mut create = base.clone();
    create["preset"] = json!("mimo");
    create["shrink_media"] = json!(true);
    let prof = env.ok("profile.upsert", create);
    let pid = prof["id"].as_str().unwrap().to_string();
    assert_eq!(prof["preset"], json!("mimo"), "{prof}");
    assert_eq!(prof["shrink_media"], json!(true), "{prof}");
    let row = listed_profile(&env, &pid);
    assert_eq!(row["preset"], json!("mimo"), "{row}");
    assert_eq!(row["shrink_media"], json!(true), "{row}");

    // an update without the fields keeps both stored values
    let mut again = base.clone();
    again["id"] = json!(pid);
    let prof = env.ok("profile.upsert", again);
    assert_eq!(prof["preset"], json!("mimo"), "{prof}");
    assert_eq!(prof["shrink_media"], json!(true), "{prof}");
    let row = listed_profile(&env, &pid);
    assert_eq!(row["preset"], json!("mimo"), "{row}");
    assert_eq!(row["shrink_media"], json!(true), "{row}");

    // an empty preset clears it, and shrinking goes off with it
    let mut cleared = base.clone();
    cleared["id"] = json!(pid);
    cleared["preset"] = json!("");
    let prof = env.ok("profile.upsert", cleared);
    assert!(prof["preset"].is_null(), "{prof}");
    assert_eq!(prof["shrink_media"], json!(false), "{prof}");
    let row = listed_profile(&env, &pid);
    assert!(row["preset"].is_null(), "{row}");
    assert_eq!(row["shrink_media"], json!(false), "{row}");

    // shrinking requested on a non-MiMo preset is stored off
    let mut openai = base.clone();
    openai["preset"] = json!("openai");
    openai["shrink_media"] = json!(true);
    let prof = env.ok("profile.upsert", openai);
    let other_id = prof["id"].as_str().unwrap().to_string();
    assert_eq!(prof["preset"], json!("openai"), "{prof}");
    assert_eq!(prof["shrink_media"], json!(false), "{prof}");

    // switching an existing MiMo profile to another preset turns shrinking off
    let mut mimo = base.clone();
    mimo["id"] = json!(pid);
    mimo["preset"] = json!("mimo");
    mimo["shrink_media"] = json!(true);
    env.ok("profile.upsert", mimo);
    let mut glm = base.clone();
    glm["id"] = json!(pid);
    glm["preset"] = json!("glm");
    let prof = env.ok("profile.upsert", glm);
    assert_eq!(prof["preset"], json!("glm"), "{prof}");
    assert_eq!(prof["shrink_media"], json!(false), "{prof}");
    let row = listed_profile(&env, &other_id);
    assert_eq!(row["shrink_media"], json!(false), "{row}");
}
