//! Per-protocol stream decoding and request-building scenarios (D1-D4, D5).
//! Fixtures live in `tests/fixtures/` and are replayed in tiny chunks to exercise frame boundaries.
use lna_llm::adapters::adapter_for;
use lna_llm::sse::SseParser;
use lna_llm::{
    LlmEvent, LlmMessage, LlmRequest, OutputMode, Part, ProviderOpaque, ReasoningOptions,
    ResolvedProfile, Role, StopReason, ToolDef, ToolMode,
};
use lna_types::{ErrorCode, LnaError, ModelCapabilities, ProtocolKind};
use serde_json::{json, Value};

fn decode(kind: ProtocolKind, raw: &str) -> Result<Vec<LlmEvent>, LnaError> {
    let adapter = adapter_for(kind);
    let mut decoder = adapter.decoder("test-model");
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    for chunk in raw.as_bytes().chunks(13) {
        for frame in parser.push(chunk)? {
            events.extend(decoder.on_frame(frame)?);
        }
    }
    if let Some(last) = parser.finish() {
        events.extend(decoder.on_frame(last)?);
    }
    events.extend(decoder.flush());
    events.extend(decoder.on_eof()?);
    Ok(events)
}

fn text(events: &[LlmEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}
fn thought(events: &[LlmEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ThoughtDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}
fn tool_args(events: &[LlmEvent], index: u32) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ToolCallDelta { index: i, json } if *i == index => Some(json.as_str()),
            _ => None,
        })
        .collect()
}
fn finished(events: &[LlmEvent]) -> Option<StopReason> {
    events.iter().find_map(|e| match e {
        LlmEvent::Finished { reason } => Some(reason.clone()),
        _ => None,
    })
}
fn usage(events: &[LlmEvent]) -> Option<(u64, u64, u64)> {
    events.iter().find_map(|e| match e {
        LlmEvent::Usage(u) => Some((u.input_tokens, u.output_tokens, u.cached_tokens)),
        _ => None,
    })
}

macro_rules! fx {
    ($n:literal) => {
        include_str!(concat!("../../../tests/fixtures/", $n))
    };
}

fn profile(kind: ProtocolKind) -> ResolvedProfile {
    ResolvedProfile {
        id: "p".into(),
        display_name: "p".into(),
        protocol: kind,
        base_url: "http://127.0.0.1:1/v1".into(),
        model_id: "m".into(),
        api_key: Some(secrecy::SecretString::from("sk-test-key".to_string())),
        show_thinking: true,
        effort: None,
        context_window: 32000,
        max_output_tokens: 1000,
        capabilities: ModelCapabilities::default(),
        extra_headers: Default::default(),
        anthropic_version: "2023-06-01".into(),
    }
}

fn request(messages: Vec<LlmMessage>) -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages,
        tools: vec![ToolDef {
            name: "search_sources".into(),
            description: "d".into(),
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}}}),
        }],
        tool_mode: ToolMode::Auto,
        output: OutputMode::Text,
        max_output_tokens: 500,
        reasoning: ReasoningOptions {
            show_summary: true,
            effort: Some("low".into()),
        },
    }
}

fn user(t: &str) -> LlmMessage {
    LlmMessage {
        role: Role::User,
        parts: vec![Part::Text(t.into())],
    }
}

fn build_body(
    kind: ProtocolKind,
    p: &ResolvedProfile,
    req: &LlmRequest,
) -> (Value, reqwest::Request) {
    let client = reqwest::Client::new();
    let r = adapter_for(kind).build(&client, p, req).unwrap();
    let body: Value = serde_json::from_slice(r.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
    (body, r)
}

// ---------------------------------------------------------------- D1 Chat Completions

#[test]
fn d1_think_tags_split_across_frames_become_thoughts() {
    let ev = decode(ProtocolKind::ChatCompletion, fx!("cc_think_split.sse")).unwrap();
    assert_eq!(thought(&ev), "推理过程");
    assert_eq!(text(&ev), "最终回答[1]");
    assert_eq!(finished(&ev), Some(StopReason::EndTurn));
}

#[test]
fn d1_reasoning_content_and_tool_call_arguments_across_frames() {
    let ev = decode(ProtocolKind::ChatCompletion, fx!("cc_tool_split.sse")).unwrap();
    assert_eq!(tool_args(&ev, 0), r#"{"query":"基"}"#);
    assert_eq!(tool_args(&ev, 1), "{}");
    let starts: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ToolCallStart { index, id, name } => Some((*index, id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![
            (0, "c1".to_string(), "search_sources".to_string()),
            (1, "c2".to_string(), "list_sources".to_string())
        ]
    );
    assert_eq!(finished(&ev), Some(StopReason::ToolUse));
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, LlmEvent::ToolCallEnd { .. }))
            .count(),
        2
    );
}

#[test]
fn d1_length_finish_is_max_tokens_and_reports_cached_tokens() {
    let ev = decode(ProtocolKind::ChatCompletion, fx!("cc_length.sse")).unwrap();
    assert_eq!(finished(&ev), Some(StopReason::MaxTokens));
    assert_eq!(usage(&ev), Some((50, 1000, 20)));
}

#[test]
fn d1_stream_without_usage_frame_still_finishes() {
    let ev = decode(ProtocolKind::ChatCompletion, fx!("cc_no_usage.sse")).unwrap();
    assert_eq!(text(&ev), "没有用量帧");
    assert_eq!(usage(&ev), None);
    assert_eq!(finished(&ev), Some(StopReason::EndTurn));
}

#[test]
fn d1_cut_stream_is_an_interruption_but_buffered_text_is_flushed() {
    let adapter = adapter_for(ProtocolKind::ChatCompletion);
    let mut d = adapter.decoder("m");
    let mut p = SseParser::new();
    let mut events = Vec::new();
    for f in p.push(fx!("cc_cut.sse").as_bytes()).unwrap() {
        events.extend(d.on_frame(f).unwrap());
    }
    // 7 bytes are held back in case they start a <think> tag; flush() must release them.
    events.extend(d.flush());
    assert_eq!(text(&events), "断流前的内容");
    let err = d.on_eof().unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelStreamInterrupted);
}

#[test]
fn d1_content_filter_and_error_frame() {
    let ev = decode(ProtocolKind::ChatCompletion, fx!("cc_filter.sse")).unwrap();
    assert_eq!(finished(&ev), Some(StopReason::ContentFilter));
    let err = decode(
        ProtocolKind::ChatCompletion,
        "data: {\"error\":{\"message\":\"bad\"}}\n\n",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelBadRequest);
}

#[test]
fn d1_request_uses_configured_token_field_and_stream_usage() {
    let mut p = profile(ProtocolKind::ChatCompletion);
    let req = request(vec![user("hi")]);
    let (b, r) = build_body(ProtocolKind::ChatCompletion, &p, &req);
    assert_eq!(b["max_completion_tokens"], json!(500));
    assert!(b.get("stream_options").is_none());
    assert_eq!(b["tools"][0]["function"]["name"], json!("search_sources"));
    assert_eq!(r.headers()["authorization"], "Bearer sk-test-key");
    p.capabilities.max_tokens_field = Some("max_tokens".into());
    p.capabilities.stream_usage = true;
    let (b, _) = build_body(ProtocolKind::ChatCompletion, &p, &req);
    assert_eq!(b["max_tokens"], json!(500));
    assert!(b.get("max_completion_tokens").is_none());
    assert_eq!(b["stream_options"]["include_usage"], json!(true));
}

// ---------------------------------------------------------------- D2 Responses

#[test]
fn d2_function_call_flow_ends_with_tool_use_and_cached_usage() {
    let ev = decode(ProtocolKind::Responses, fx!("responses_tool.sse")).unwrap();
    assert_eq!(tool_args(&ev, 0), r#"{"query":"基"}"#);
    assert_eq!(finished(&ev), Some(StopReason::ToolUse));
    assert_eq!(usage(&ev), Some((30, 7, 10)));
    assert!(ev.iter().any(|e| matches!(
        e,
        LlmEvent::ToolCallStart { id, name, .. } if id == "call_1" && name == "search_sources"
    )));
}

#[test]
fn d2_failed_is_an_error_and_incomplete_is_max_tokens() {
    let err = decode(ProtocolKind::Responses, fx!("responses_failed.sse")).unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelBadRequest);
    assert!(err.message.contains("server_error"));
    let ev = decode(ProtocolKind::Responses, fx!("responses_incomplete.sse")).unwrap();
    assert_eq!(text(&ev), "未完成");
    assert_eq!(finished(&ev), Some(StopReason::MaxTokens));
}

#[test]
fn d2_request_is_stateless_and_replays_reasoning_items() {
    let p = profile(ProtocolKind::Responses);
    let reasoning_item = json!({"type":"reasoning","id":"rs_1","encrypted_content":"enc_abc"});
    let req = request(vec![
        user("q"),
        LlmMessage {
            role: Role::Assistant,
            parts: vec![
                Part::Opaque(ProviderOpaque {
                    protocol: ProtocolKind::Responses,
                    model_id: "m".into(),
                    data: reasoning_item.clone(),
                }),
                Part::ToolCall {
                    id: "call_1".into(),
                    name: "search_sources".into(),
                    arguments: json!({"query":"x"}),
                },
            ],
        },
        LlmMessage {
            role: Role::Tool,
            parts: vec![Part::ToolResult {
                call_id: "call_1".into(),
                content: "result".into(),
                is_error: false,
            }],
        },
    ]);
    let (b, r) = build_body(ProtocolKind::Responses, &p, &req);
    assert_eq!(b["store"], json!(false));
    assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(b["reasoning"]["summary"], json!("auto"));
    let input = b["input"].as_array().unwrap();
    assert!(
        input.contains(&reasoning_item),
        "reasoning item replayed verbatim"
    );
    assert!(input
        .iter()
        .any(|i| i["type"] == json!("function_call") && i["call_id"] == json!("call_1")));
    assert!(input
        .iter()
        .any(|i| i["type"] == json!("function_call_output") && i["output"] == json!("result")));
    assert_eq!(b["tools"][0]["name"], json!("search_sources"));
    assert!(r.url().path().ends_with("/responses"));
}

#[test]
fn d2_json_schema_output_is_native_and_strict() {
    let p = profile(ProtocolKind::Responses);
    let mut req = request(vec![user("q")]);
    req.output = OutputMode::JsonSchema {
        name: "s".into(),
        schema: json!({"type":"object"}),
    };
    let (b, _) = build_body(ProtocolKind::Responses, &p, &req);
    assert_eq!(b["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(b["text"]["format"]["strict"], json!(true));
}

// ---------------------------------------------------------------- D3 Anthropic

#[test]
fn d3_tool_use_streams_partial_json_and_reports_cache_reads() {
    let ev = decode(ProtocolKind::Anthropic, fx!("anthropic_tool_use.sse")).unwrap();
    assert_eq!(tool_args(&ev, 0), r#"{"query":"基"}"#);
    assert_eq!(finished(&ev), Some(StopReason::ToolUse));
    assert_eq!(usage(&ev), Some((40, 12, 8)));
}

#[test]
fn d3_refusal_maps_to_model_refused_and_error_event_is_surfaced() {
    let err = decode(ProtocolKind::Anthropic, fx!("anthropic_refusal.sse")).unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelRefused);
    let err = decode(ProtocolKind::Anthropic, fx!("anthropic_error.sse")).unwrap_err();
    assert!(err.message.contains("Overloaded"));
}

#[test]
fn d3_thinking_signature_is_captured_and_replayed_with_configurable_version() {
    let ev = decode(
        ProtocolKind::Anthropic,
        include_str!("../../../tests/fixtures/anthropic_replay.sse"),
    )
    .unwrap();
    let opaque = ev
        .iter()
        .find_map(|e| match e {
            LlmEvent::Opaque(o) => Some(o.clone()),
            _ => None,
        })
        .expect("thinking block captured");
    assert_eq!(opaque.data["signature"], json!("sig_xyz"));
    assert_eq!(opaque.data["thinking"], json!("Step 1"));

    let mut p = profile(ProtocolKind::Anthropic);
    let req = request(vec![
        user("q"),
        LlmMessage {
            role: Role::Assistant,
            parts: vec![Part::Opaque(opaque.clone()), Part::Text("答".into())],
        },
        user("next"),
    ]);
    let (b, r) = build_body(ProtocolKind::Anthropic, &p, &req);
    assert_eq!(r.headers()["anthropic-version"], "2023-06-01");
    assert_eq!(r.headers()["x-api-key"], "sk-test-key");
    assert_eq!(
        b["messages"][1]["content"][0]["signature"],
        json!("sig_xyz")
    );
    assert_eq!(b["thinking"]["type"], json!("adaptive"));
    assert_eq!(b["thinking"]["display"], json!("summarized"));
    assert_eq!(b["output_config"]["effort"], json!("low"));

    p.extra_headers
        .insert("Anthropic-Version".into(), "2099-01-01".into());
    let (_, r) = build_body(ProtocolKind::Anthropic, &p, &req);
    assert_eq!(r.headers().get_all("anthropic-version").iter().count(), 1);
    assert_eq!(r.headers()["anthropic-version"], "2099-01-01");

    p.capabilities.anthropic_thinking = Some("budget_tokens".into());
    let (b, _) = build_body(ProtocolKind::Anthropic, &p, &req);
    assert_eq!(b["thinking"]["type"], json!("enabled"));
}

// ---------------------------------------------------------------- D4 Gemini

#[test]
fn d4_function_call_with_stop_is_tool_use_and_keeps_the_signature() {
    let ev = decode(ProtocolKind::Gemini, fx!("gemini_tool.sse")).unwrap();
    assert_eq!(finished(&ev), Some(StopReason::ToolUse));
    assert_eq!(tool_args(&ev, 0), r#"{"query":"基"}"#);
    assert!(ev.iter().any(|e| matches!(
        e,
        LlmEvent::Opaque(o) if o.data["thoughtSignature"] == json!("sig_fc")
    )));
}

#[test]
fn d4_replayed_function_call_is_not_duplicated() {
    let ev = decode(ProtocolKind::Gemini, fx!("gemini_tool.sse")).unwrap();
    let opaque = ev
        .iter()
        .find_map(|e| match e {
            LlmEvent::Opaque(o) => Some(o.clone()),
            _ => None,
        })
        .unwrap();
    let p = profile(ProtocolKind::Gemini);
    let req = request(vec![
        user("q"),
        LlmMessage {
            role: Role::Assistant,
            parts: vec![
                Part::Opaque(opaque),
                Part::ToolCall {
                    id: "gemini_call_0".into(),
                    name: "search_sources".into(),
                    arguments: json!({"query":"基"}),
                },
            ],
        },
        LlmMessage {
            role: Role::Tool,
            parts: vec![Part::ToolResult {
                call_id: "gemini_call_0".into(),
                content: "r".into(),
                is_error: false,
            }],
        },
    ]);
    let (b, r) = build_body(ProtocolKind::Gemini, &p, &req);
    let model_parts = b["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(
        model_parts.len(),
        1,
        "functionCall sent once: {model_parts:?}"
    );
    assert_eq!(model_parts[0]["thoughtSignature"], json!("sig_fc"));
    // the function response is matched to the function name, not the synthetic call id
    assert_eq!(
        b["contents"][2]["parts"][0]["functionResponse"]["name"],
        json!("search_sources")
    );
    assert_eq!(r.headers()["x-goog-api-key"], "sk-test-key");
    assert!(r.url().as_str().contains("streamGenerateContent?alt=sse"));
    assert_eq!(
        b["generationConfig"]["thinkingConfig"]["includeThoughts"],
        json!(true)
    );
    assert_eq!(
        b["tools"][0]["functionDeclarations"][0]["name"],
        json!("search_sources")
    );
}

#[test]
fn d4_block_reason_is_a_refusal_safety_finish_is_content_filter_missing_finish_is_interrupted() {
    let err = decode(ProtocolKind::Gemini, fx!("gemini_blocked.sse")).unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelRefused);
    let ev = decode(ProtocolKind::Gemini, fx!("gemini_safety.sse")).unwrap();
    assert_eq!(finished(&ev), Some(StopReason::ContentFilter));
    let err = decode(ProtocolKind::Gemini, fx!("gemini_cut.sse")).unwrap_err();
    assert_eq!(err.code, ErrorCode::ModelStreamInterrupted);
}

#[test]
fn d4_json_schema_uses_response_schema() {
    let p = profile(ProtocolKind::Gemini);
    let mut req = request(vec![user("q")]);
    req.output = OutputMode::JsonSchema {
        name: "s".into(),
        schema: json!({"type":"object"}),
    };
    let (b, _) = build_body(ProtocolKind::Gemini, &p, &req);
    assert_eq!(
        b["generationConfig"]["responseMimeType"],
        json!("application/json")
    );
    assert_eq!(
        b["generationConfig"]["responseSchema"],
        json!({"type":"object"})
    );
}

// ---------------------------------------------------------------- D5 parameter relaxation

#[test]
fn d5_unsupported_parameter_errors_relax_the_request() {
    use lna_llm::relax_for_error;
    let mut p = profile(ProtocolKind::ChatCompletion);
    p.capabilities.stream_usage = true;
    let mut req = request(vec![user("q")]);
    assert!(relax_for_error(
        r#"{"error":{"message":"Unknown parameter: 'stream_options'"}}"#,
        &mut p,
        &mut req
    ));
    assert!(!p.capabilities.stream_usage);

    assert!(relax_for_error(
        "Unsupported parameter: 'max_completion_tokens'. Use 'max_tokens' instead.",
        &mut p,
        &mut req
    ));
    assert_eq!(
        p.capabilities.max_tokens_field.as_deref(),
        Some("max_tokens")
    );

    req.output = OutputMode::JsonSchema {
        name: "s".into(),
        schema: json!({}),
    };
    p.capabilities.json_schema = true;
    assert!(relax_for_error(
        "response_format json_schema is not supported",
        &mut p,
        &mut req
    ));
    assert!(matches!(req.output, OutputMode::JsonObject));
    assert!(!p.capabilities.json_schema);

    p.capabilities.tools = true;
    assert!(relax_for_error(
        "tools are not supported by this model",
        &mut p,
        &mut req
    ));
    assert!(req.tools.is_empty() && !p.capabilities.tools);

    // Unrelated 400s and unrelated parameters are left alone.
    assert!(!relax_for_error(
        "messages must not be empty",
        &mut p,
        &mut req
    ));
    assert!(!relax_for_error(
        "unsupported parameter: temperature",
        &mut p,
        &mut req
    ));
}

#[test]
fn hard_caps_reasoning_and_output_tokens_at_8192() {
    use lna_llm::{clamp_reasoning_and_output_tokens, MAX_SINGLE_REQUEST_TOKENS};
    assert_eq!(MAX_SINGLE_REQUEST_TOKENS, 8192);
    assert_eq!(clamp_reasoning_and_output_tokens(0), 8192);
    assert_eq!(clamp_reasoning_and_output_tokens(512), 512);
    assert_eq!(clamp_reasoning_and_output_tokens(8192), 8192);
    assert_eq!(clamp_reasoning_and_output_tokens(16384), 8192);
    assert_eq!(clamp_reasoning_and_output_tokens(65536), 8192);
}
