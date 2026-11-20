use crate::types::*;
use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult, ProtocolKind};
use secrecy::ExposeSecret;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct AnthropicAdapter;

/// `anthropic-version` comes from the profile's extra headers when set, else the built-in default.
fn anthropic_version(profile: &ResolvedProfile) -> String {
    profile
        .extra_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("anthropic-version"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| profile.anthropic_version.clone())
}

#[async_trait]
impl ProtocolAdapter for AnthropicAdapter {
    fn build(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
        req: &LlmRequest,
    ) -> LnaResult<reqwest::Request> {
        let url = format!("{}/messages", profile.trimmed_base());
        let mut messages = Vec::new();

        for m in &req.messages {
            match m.role {
                Role::User => {
                    let blocks: Vec<Value> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::Text(t) => Some(json!({ "type": "text", "text": t })),
                            _ => None,
                        })
                        .collect();
                    messages.push(json!({ "role": "user", "content": blocks }));
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    for p in &m.parts {
                        match p {
                            Part::Opaque(op) if op.protocol == ProtocolKind::Anthropic => {
                                blocks.push(op.data.clone());
                            }
                            Part::Text(t) if !t.is_empty() => {
                                blocks.push(json!({ "type": "text", "text": t }));
                            }
                            Part::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                blocks.push(json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": arguments
                                }));
                            }
                            _ => {}
                        }
                    }
                    messages.push(json!({ "role": "assistant", "content": blocks }));
                }
                Role::Tool => {
                    let blocks: Vec<Value> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::ToolResult {
                                call_id,
                                content,
                                is_error,
                            } => Some(json!({
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": content,
                                "is_error": is_error,
                            })),
                            _ => None,
                        })
                        .collect();
                    messages.push(json!({ "role": "user", "content": blocks }));
                }
            }
        }

        let mut body = json!({
            "model": profile.model_id,
            "max_tokens": req.max_output_tokens,
            "messages": messages,
            "stream": true,
        });
        if !req.system.is_empty() {
            body["system"] = json!(req.system);
        }

        // Section 10.5: adaptive + display:"summarized" + output_config:{effort}
        let use_legacy =
            profile.capabilities.anthropic_thinking.as_deref() == Some("budget_tokens");
        if use_legacy {
            let budget = if req.reasoning.show_summary {
                4096
            } else {
                1024
            };
            body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
        } else {
            body["thinking"] = json!({
                "type": "adaptive",
                "display": if req.reasoning.show_summary { "summarized" } else { "omitted" }
            });
            let effort = req
                .reasoning
                .effort
                .as_deref()
                .unwrap_or(if req.reasoning.show_summary {
                    "medium"
                } else {
                    "low"
                });
            body["output_config"] = json!({ "effort": effort });
        }

        if let OutputMode::JsonSchema { schema, .. } = &req.output {
            let mut oc = body
                .get("output_config")
                .cloned()
                .unwrap_or_else(|| json!({}));
            oc["format"] = json!({
                "type": "json_schema",
                "schema": schema
            });
            body["output_config"] = oc;
        }

        if !req.tools.is_empty() && req.tool_mode == ToolMode::Auto {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                }))
                .collect::<Vec<_>>());
        }

        let mut builder = client
            .post(url)
            .header("anthropic-version", anthropic_version(profile))
            .json(&body);
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                builder = builder.header("x-api-key", s);
            }
        }
        for (k, v) in &profile.extra_headers {
            if k.eq_ignore_ascii_case("anthropic-version") {
                continue; // already applied above
            }
            builder = builder.header(k, v);
        }
        builder
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))
    }

    fn decoder(&self, model_id: &str) -> Box<dyn StreamDecoder> {
        Box::new(AnthropicDecoder {
            model_id: model_id.to_string(),
            ..Default::default()
        })
    }

    async fn discover(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
    ) -> LnaResult<Vec<ModelInfo>> {
        let url = format!("{}/models", profile.trimmed_base());
        let mut req = client
            .get(url)
            .header("anthropic-version", anthropic_version(profile));
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                req = req.header("x-api-key", s);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(map_http_status(status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| LnaError::new(ErrorCode::OutputInvalid, e.to_string()))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|x| x.as_str()) {
                    out.push(ModelInfo {
                        id: id.to_string(),
                        display_name: item
                            .get("display_name")
                            .and_then(|x| x.as_str())
                            .map(str::to_string),
                        context_window: None,
                        max_output_tokens: None,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[derive(Default)]
struct AnthropicDecoder {
    model_id: String,
    finished: bool,
    block_types: HashMap<u32, String>,
    thinking_text: HashMap<u32, String>,
    thinking_sig: HashMap<u32, String>,
    usage: Usage,
}

impl StreamDecoder for AnthropicDecoder {
    fn on_frame(&mut self, frame: SseFrame) -> LnaResult<Vec<LlmEvent>> {
        let v: Value = serde_json::from_str(frame.data.trim()).map_err(|e| {
            LnaError::new(ErrorCode::OutputInvalid, format!("Invalid JSON frame: {e}"))
        })?;

        let ev = frame
            .event
            .as_deref()
            .or_else(|| v.get("type").and_then(|t| t.as_str()))
            .unwrap_or("");

        let mut out = Vec::new();
        match ev {
            "ping" => {}
            "error" => {
                let msg = v
                    .pointer("/error/message")
                    .and_then(|x| x.as_str())
                    .unwrap_or("Anthropic stream error");
                return Err(LnaError::new(ErrorCode::ModelBadRequest, msg));
            }
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.usage.input_tokens =
                        u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    self.usage.cached_tokens = u
                        .get("cache_read_input_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                }
            }
            "content_block_start" => {
                let idx = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                if let Some(cb) = v.get("content_block") {
                    let kind = cb
                        .get("type")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.block_types.insert(idx, kind.clone());
                    if kind == "tool_use" {
                        let id = cb
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = cb
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        out.push(LlmEvent::ToolCallStart {
                            index: idx,
                            id,
                            name,
                        });
                    }
                }
            }
            "content_block_delta" => {
                let idx = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                if let Some(delta) = v.get("delta") {
                    match delta.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                        "thinking_delta" => {
                            if let Some(t) = delta.get("thinking").and_then(|x| x.as_str()) {
                                self.thinking_text.entry(idx).or_default().push_str(t);
                                out.push(LlmEvent::ThoughtDelta(t.to_string()));
                            }
                        }
                        "signature_delta" => {
                            if let Some(s) = delta.get("signature").and_then(|x| x.as_str()) {
                                self.thinking_sig.entry(idx).or_default().push_str(s);
                            }
                        }
                        "text_delta" => {
                            if let Some(t) = delta.get("text").and_then(|x| x.as_str()) {
                                out.push(LlmEvent::TextDelta(t.to_string()));
                            }
                        }
                        "input_json_delta" => {
                            if let Some(pj) = delta.get("partial_json").and_then(|x| x.as_str()) {
                                out.push(LlmEvent::ToolCallDelta {
                                    index: idx,
                                    json: pj.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let idx = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                if let Some(kind) = self.block_types.get(&idx) {
                    if kind == "tool_use" {
                        out.push(LlmEvent::ToolCallEnd { index: idx });
                    } else if kind == "thinking" {
                        let thinking = self.thinking_text.remove(&idx).unwrap_or_default();
                        let signature = self.thinking_sig.remove(&idx).unwrap_or_default();
                        out.push(LlmEvent::Opaque(ProviderOpaque {
                            protocol: ProtocolKind::Anthropic,
                            model_id: self.model_id.clone(),
                            data: json!({
                                "type": "thinking",
                                "thinking": thinking,
                                "signature": signature
                            }),
                        }));
                    }
                }
            }
            "message_delta" => {
                if let Some(u) = v.get("usage") {
                    self.usage.output_tokens =
                        u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    out.push(LlmEvent::Usage(self.usage.clone()));
                }
                if let Some(sr) = v.pointer("/delta/stop_reason").and_then(|x| x.as_str()) {
                    if sr == "refusal" {
                        return Err(LnaError::new(
                            ErrorCode::ModelRefused,
                            "Anthropic model refused to fulfill the request",
                        ));
                    }
                    let reason = match sr {
                        "end_turn" => StopReason::EndTurn,
                        "max_tokens" => StopReason::MaxTokens,
                        "tool_use" => StopReason::ToolUse,
                        other => StopReason::Other(other.to_string()),
                    };
                    self.finished = true;
                    out.push(LlmEvent::Finished { reason });
                }
            }
            "message_stop" if !self.finished => {
                self.finished = true;
                out.push(LlmEvent::Finished {
                    reason: StopReason::EndTurn,
                });
            }
            _ => {}
        }
        Ok(out)
    }

    fn on_eof(&mut self) -> LnaResult<Vec<LlmEvent>> {
        if !self.finished {
            return Err(LnaError::new(
                ErrorCode::ModelStreamInterrupted,
                "Anthropic stream ended without message_stop",
            ));
        }
        Ok(Vec::new())
    }
}
