//! D7: Real-API smoke test across all four model protocols (openai_chat, openai_responses, anthropic, gemini).
//! Run manually or nightly with API keys provided via environment variables; never included in default CI.
//!
//! Example:
//!   $env:LNA_OPENAI_API_KEY="..."
//!   $env:LNA_ANTHROPIC_API_KEY="..."
//!   $env:LNA_GEMINI_API_KEY="..."
//!   cargo test -p lna-api --test real_api_smoke -- --ignored --nocapture

mod common;

use common::Env;
use serde_json::json;

fn run_protocol_smoke(name: &str, protocol: &str, base_url: &str, model: &str, api_key: &str) {
    let env = Env::new(&format!("d7_{name}"));
    let prof = env.ok(
        "profile.upsert",
        json!({
            "name": name,
            "protocol": protocol,
            "base_url": base_url,
            "model_name": model,
            "context_window": 32000,
            "max_output_tokens": 512,
            "is_active": true,
            "api_key": api_key
        }),
    );
    let pid = prof["id"].as_str().unwrap();
    let tested = env.ok("profile.test", json!({ "profile_id": pid }));
    assert_eq!(
        tested["ok"],
        json!(true),
        "{name} profile.test failed: {tested}"
    );

    let nid = env.ok("notebook.create", json!({ "title": format!("D7 {name}") }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.add_text(
        &nid,
        "facts.md",
        "# Facts\n\nThe capital of France is Paris. The boiling point of water at 1 atm is 100 degrees Celsius.",
    );
    env.wait_sources_ready(&nid, 1);

    let msg = env.ok(
        "chat.send",
        json!({
            "notebook_id": nid,
            "text": "According to the source, what is the boiling point of water at 1 atm?"
        }),
    );
    assert_eq!(
        msg["status"],
        json!("COMPLETE"),
        "{name} chat failed: {msg}"
    );
    let content = msg["content"].as_str().unwrap_or("");
    assert!(
        content.contains("100"),
        "{name} expected answer to mention 100, got: {content}"
    );
}

#[test]
#[ignore = "D7 real API smoke: requires LNA_OPENAI_API_KEY / LNA_ANTHROPIC_API_KEY / LNA_GEMINI_API_KEY"]
fn d7_real_api_smoke_all_four_protocols() {
    let mut ran = 0usize;

    if let Ok(key) = std::env::var("LNA_OPENAI_API_KEY") {
        let base = std::env::var("LNA_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let model = std::env::var("LNA_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        run_protocol_smoke("chat_completion", "chat_completion", &base, &model, &key);
        run_protocol_smoke("responses", "responses", &base, &model, &key);
        ran += 2;
    }

    if let Ok(key) = std::env::var("LNA_ANTHROPIC_API_KEY") {
        let base = std::env::var("LNA_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".into());
        let model = std::env::var("LNA_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| "claude-3-5-haiku-latest".into());
        run_protocol_smoke("anthropic", "anthropic", &base, &model, &key);
        ran += 1;
    }

    if let Ok(key) = std::env::var("LNA_GEMINI_API_KEY") {
        let base = std::env::var("LNA_GEMINI_BASE_URL")
            .unwrap_or_else(|_| "https://generativelanguage.googleapis.com/v1beta".into());
        let model = std::env::var("LNA_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        run_protocol_smoke("gemini", "gemini", &base, &model, &key);
        ran += 1;
    }

    assert!(
        ran > 0,
        "Set at least one of LNA_OPENAI_API_KEY, LNA_ANTHROPIC_API_KEY, LNA_GEMINI_API_KEY to run D7 smoke"
    );
}
