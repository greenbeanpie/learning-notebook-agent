use crate::types::*;
use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult, ProtocolKind};
use secrecy::ExposeSecret;
use serde_json::{json, Value};

pub struct GeminiAdapter;

#[async_trait]
impl ProtocolAdapter for GeminiAdapter {
    fn build(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
        req: &LlmRequest,
    ) -> LnaResult<reqwest::Request> {
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            profile.trimmed_base(),
            profile.model_id
        );

        let mut call_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut contents = Vec::new();
        for m in &req.messages {
            match m.role {
                Role::User => {
                    let parts: Vec<Value> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::Text(t) => Some(json!({ "text": t })),
                            _ => None,
                        })
                        .collect();
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    // A replayed part that already carries a functionCall (with its thought signature)
                    // must not be sent a second time as a plain ToolCall.
                    let mut replayed_calls: Vec<String> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::Opaque(op) if op.protocol == ProtocolKind::Gemini => op
                                .data
                                .pointer("/functionCall/name")
                                .and_then(|x| x.as_str())
                                .map(str::to_string),
                            _ => None,
                        })
                        .collect();
                    for p in &m.parts {
                        match p {
                            Part::Opaque(op) if op.protocol == ProtocolKind::Gemini => {
                                parts.push(op.data.clone());
                            }
                            Part::ToolCall { id, name, .. } if replayed_calls.contains(name) => {
                                call_names.insert(id.clone(), name.clone());
                                if let Some(pos) = replayed_calls.iter().position(|n| n == name) {
                                    replayed_calls.remove(pos);
                                }
                            }
                            Part::Text(t) if !t.is_empty() => {
                                parts.push(json!({ "text": t }));
                            }
                            Part::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                call_names.insert(id.clone(), name.clone());
                                parts.push(json!({
                                    "functionCall": { "name": name, "args": arguments }
                                }));
                            }
                            _ => {}
                        }
                    }
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
                Role::Tool => {
                    let parts: Vec<Value> = m
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::ToolResult {
                                call_id, content, ..
                            } => {
                                let fn_name = call_names
                                    .get(call_id)
                                    .map(String::as_str)
                                    .unwrap_or(call_id.as_str());
                                Some(json!({
                                    "functionResponse": {
                                        "name": fn_name,
                                        "response": { "result": content }
                                    }
                                }))
                            }
                            _ => None,
                        })
                        .collect();
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }

        let mut gen_cfg = json!({
            "maxOutputTokens": req.max_output_tokens
        });
        if req.reasoning.show_summary {
            gen_cfg["thinkingConfig"] = json!({ "includeThoughts": true });
        }
        match &req.output {
            OutputMode::Text => {}
            OutputMode::JsonObject => {
                gen_cfg["responseMimeType"] = json!("application/json");
            }
            OutputMode::JsonSchema { schema, .. } => {
                gen_cfg["responseMimeType"] = json!("application/json");
                gen_cfg["responseSchema"] = schema.clone();
            }
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": gen_cfg,
        });
        if !req.system.is_empty() {
            body["systemInstruction"] = json!({ "parts": [{ "text": req.system }] });
        }
        if !req.tools.is_empty() && req.tool_mode == ToolMode::Auto {
            body["tools"] = json!([{
                "functionDeclarations": req.tools.iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                })).collect::<Vec<_>>()
            }]);
        }

        let mut builder = client.post(url).json(&body);
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                builder = builder.header("x-goog-api-key", s);
            }
        }
        for (k, v) in &profile.extra_headers {
            builder = builder.header(k, v);
        }
        builder
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))
    }

    fn decoder(&self, model_id: &str) -> Box<dyn StreamDecoder> {
        Box::new(GeminiDecoder {
            model_id: model_id.to_string(),
            finished: false,
            tool_idx: 0,
            saw_tool_call: false,
        })
    }

    async fn discover(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
    ) -> LnaResult<Vec<ModelInfo>> {
        let url = format!("{}/models", profile.trimmed_base());
        let mut req = client.get(url);
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                req = req.header("x-goog-api-key", s);
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
        if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|x| x.as_str()) {
                    let id = name.strip_prefix("models/").unwrap_or(name).to_string();
                    out.push(ModelInfo {
                        id,
                        display_name: item
                            .get("displayName")
                            .and_then(|x| x.as_str())
                            .map(str::to_string),
                        context_window: item
                            .get("inputTokenLimit")
                            .and_then(|x| x.as_u64())
                            .map(|n| n as u32),
                        max_output_tokens: item
                            .get("outputTokenLimit")
                            .and_then(|x| x.as_u64())
                            .map(|n| n as u32),
                    });
                }
            }
        }
        Ok(out)
    }
}

struct GeminiDecoder {
    model_id: String,
    finished: bool,
    tool_idx: u32,
    saw_tool_call: bool,
}

impl StreamDecoder for GeminiDecoder {
    fn on_frame(&mut self, frame: SseFrame) -> LnaResult<Vec<LlmEvent>> {
        let v: Value = serde_json::from_str(frame.data.trim()).map_err(|e| {
            LnaError::new(ErrorCode::OutputInvalid, format!("Invalid JSON frame: {e}"))
        })?;

        if let Some(reason) = v
            .pointer("/promptFeedback/blockReason")
            .and_then(|x| x.as_str())
        {
            return Err(LnaError::new(
                ErrorCode::ModelRefused,
                format!("Gemini blocked prompt: {reason}"),
            ));
        }

        let mut out = Vec::new();

        if let Some(u) = v.get("usageMetadata") {
            out.push(LlmEvent::Usage(Usage {
                input_tokens: u
                    .get("promptTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0),
                output_tokens: u
                    .get("candidatesTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0),
                cached_tokens: u
                    .get("cachedContentTokenCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0),
            }));
        }

        if let Some(cand) = v
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(parts) = cand.pointer("/content/parts").and_then(|p| p.as_array()) {
                for part in parts {
                    let is_thought = part
                        .get("thought")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    if part.get("thoughtSignature").is_some() {
                        out.push(LlmEvent::Opaque(ProviderOpaque {
                            protocol: ProtocolKind::Gemini,
                            model_id: self.model_id.clone(),
                            data: part.clone(),
                        }));
                    }
                    if let Some(text) = part.get("text").and_then(|x| x.as_str()) {
                        if is_thought {
                            out.push(LlmEvent::ThoughtDelta(text.to_string()));
                        } else {
                            out.push(LlmEvent::TextDelta(text.to_string()));
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                        let idx = self.tool_idx;
                        self.tool_idx += 1;
                        self.saw_tool_call = true;
                        out.push(LlmEvent::ToolCallStart {
                            index: idx,
                            id: format!("gemini_call_{idx}"),
                            name,
                        });
                        out.push(LlmEvent::ToolCallDelta {
                            index: idx,
                            json: serde_json::to_string(&args).unwrap_or_else(|_| "{}".into()),
                        });
                        out.push(LlmEvent::ToolCallEnd { index: idx });
                    }
                }
            }
            if let Some(fr) = cand.get("finishReason").and_then(|x| x.as_str()) {
                let reason = match fr {
                    // Gemini reports STOP even when the turn ended with a function call.
                    "STOP" if self.saw_tool_call => StopReason::ToolUse,
                    "STOP" => StopReason::EndTurn,
                    "MAX_TOKENS" => StopReason::MaxTokens,
                    "SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" => StopReason::ContentFilter,
                    other => StopReason::Other(other.to_string()),
                };
                self.finished = true;
                out.push(LlmEvent::Finished { reason });
            }
        }
        Ok(out)
    }

    fn on_eof(&mut self) -> LnaResult<Vec<LlmEvent>> {
        if !self.finished {
            return Err(LnaError::new(
                ErrorCode::ModelStreamInterrupted,
                "Gemini stream ended without finishReason",
            ));
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_build_maps_tool_call_id_to_function_name() {
        let client = reqwest::Client::new();
        let profile = ResolvedProfile {
            id: "p1".into(),
            display_name: "g".into(),
            protocol: ProtocolKind::Gemini,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            model_id: "gemini-2.5-flash".into(),
            api_key: None,
            show_thinking: false,
            effort: None,
            context_window: 32000,
            max_output_tokens: 1024,
            capabilities: Default::default(),
            extra_headers: Default::default(),
            anthropic_version: String::new(),
        };
        let req = LlmRequest {
            system: String::new(),
            messages: vec![
                LlmMessage {
                    role: Role::Assistant,
                    parts: vec![Part::ToolCall {
                        id: "gemini_call_0".into(),
                        name: "search_sources".into(),
                        arguments: json!({ "query": "basis" }),
                    }],
                },
                LlmMessage {
                    role: Role::Tool,
                    parts: vec![Part::ToolResult {
                        call_id: "gemini_call_0".into(),
                        content: "found".into(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: 1024,
            reasoning: ReasoningOptions::default(),
        };
        let built = GeminiAdapter.build(&client, &profile, &req).unwrap();
        let body: Value =
            serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["name"],
            json!("search_sources")
        );
    }
}
