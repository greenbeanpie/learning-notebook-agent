use crate::types::*;
use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult, ProtocolKind};
use secrecy::ExposeSecret;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct ResponsesAdapter;

#[async_trait]
impl ProtocolAdapter for ResponsesAdapter {
    fn build(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
        req: &LlmRequest,
    ) -> LnaResult<reqwest::Request> {
        let url = format!("{}/responses", profile.trimmed_base());
        let mut input = Vec::new();

        for m in &req.messages {
            match m.role {
                Role::User => {
                    let has_media = m.parts.iter().any(|p| matches!(p, Part::Media { .. }));
                    if !has_media {
                        let text = m
                            .parts
                            .iter()
                            .filter_map(|p| match p {
                                Part::Text(t) => Some(t.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        input.push(json!({ "role": "user", "content": text }));
                    } else {
                        let mut content_parts = Vec::new();
                        for p in &m.parts {
                            match p {
                                Part::Text(t) if !t.is_empty() => {
                                    content_parts.push(json!({ "type": "input_text", "text": t }));
                                }
                                Part::Media {
                                    mime_type,
                                    data_base64,
                                    ..
                                } => {
                                    if mime_type.starts_with("image/") {
                                        let b64 = data_base64.as_deref().ok_or_else(|| {
                                            LnaError::new(
                                                ErrorCode::CapabilityMissing,
                                                "Responses 协议需要 base64 图片数据",
                                            )
                                        })?;
                                        content_parts.push(json!({
                                            "type": "input_image",
                                            "image_url": format!("data:{mime_type};base64,{b64}")
                                        }));
                                    } else {
                                        return Err(LnaError::new(
                                            ErrorCode::CapabilityMissing,
                                            format!("Responses 协议暂不支持原生音视频输入（{mime_type}），请在“模型协议配置”中为音视频勾选 Gemini 等原生支持的模型"),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                        input.push(json!({ "role": "user", "content": content_parts }));
                    }
                }
                Role::Assistant => {
                    for p in &m.parts {
                        match p {
                            Part::Opaque(op) if op.protocol == ProtocolKind::Responses => {
                                input.push(op.data.clone());
                            }
                            Part::Text(t) if !t.is_empty() => {
                                input.push(json!({ "role": "assistant", "content": t }));
                            }
                            Part::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                input.push(json!({
                                    "type": "function_call",
                                    "call_id": id,
                                    "name": name,
                                    "arguments": serde_json::to_string(arguments).unwrap_or_else(|_| "{}".into())
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                Role::Tool => {
                    for p in &m.parts {
                        if let Part::ToolResult {
                            call_id, content, ..
                        } = p
                        {
                            input.push(json!({
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": content
                            }));
                        }
                    }
                }
            }
        }

        let mut body = json!({
            "model": profile.model_id,
            "instructions": req.system,
            "input": input,
            "stream": true,
            "store": false,
        });
        // 0 = no limit: omit the field so the provider applies its own default.
        if req.max_output_tokens != 0 {
            body["max_output_tokens"] = json!(req.max_output_tokens);
        }

        if req.reasoning.show_summary || req.reasoning.effort.is_some() {
            let mut r = json!({});
            if let Some(ref eff) = req.reasoning.effort {
                r["effort"] = json!(eff);
            }
            if req.reasoning.show_summary {
                r["summary"] = json!("auto");
            }
            body["reasoning"] = r;
            // With store:false the reasoning item can only be replayed from its encrypted content.
            body["include"] = json!(["reasoning.encrypted_content"]);
        }

        if !req.tools.is_empty() && req.tool_mode == ToolMode::Auto {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }))
                .collect::<Vec<_>>());
        }

        match &req.output {
            OutputMode::Text => {}
            OutputMode::JsonObject => {
                body["text"] = json!({ "format": { "type": "json_object" } });
            }
            OutputMode::JsonSchema { name, schema } => {
                body["text"] = json!({
                    "format": {
                        "type": "json_schema",
                        "name": name,
                        "strict": true,
                        "schema": schema
                    }
                });
            }
        }

        let mut builder = client.post(url).json(&body);
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                builder = builder.bearer_auth(s);
            }
        }
        for (k, v) in &profile.extra_headers {
            builder = builder.header(k, v);
        }
        for (k, v) in profile.gateway_cache_headers() {
            builder = builder.header(k, v);
        }
        builder
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))
    }

    fn decoder(&self, model_id: &str) -> Box<dyn StreamDecoder> {
        Box::new(ResponsesDecoder {
            model_id: model_id.to_string(),
            ..Default::default()
        })
    }

    async fn discover(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
    ) -> LnaResult<Vec<ModelInfo>> {
        crate::adapters::chat_completion::ChatCompletionAdapter
            .discover(client, profile)
            .await
    }
}

#[derive(Default)]
struct ResponsesDecoder {
    model_id: String,
    finished: bool,
    item_to_index: HashMap<String, u32>,
    next_tool_idx: u32,
}

impl StreamDecoder for ResponsesDecoder {
    fn on_frame(&mut self, frame: SseFrame) -> LnaResult<Vec<LlmEvent>> {
        let v: Value = serde_json::from_str(frame.data.trim()).map_err(|e| {
            LnaError::new(ErrorCode::OutputInvalid, format!("Invalid JSON frame: {e}"))
        })?;

        let ev_type = frame
            .event
            .as_deref()
            .or_else(|| v.get("type").and_then(|t| t.as_str()))
            .unwrap_or("");

        let mut out = Vec::new();
        match ev_type {
            "response.reasoning_summary_text.delta" => {
                if let Some(d) = v.get("delta").and_then(|x| x.as_str()) {
                    out.push(LlmEvent::ThoughtDelta(d.to_string()));
                }
            }
            "response.output_text.delta" => {
                if let Some(d) = v.get("delta").and_then(|x| x.as_str()) {
                    out.push(LlmEvent::TextDelta(d.to_string()));
                }
            }
            "response.output_item.added" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(|x| x.as_str()) == Some("function_call") {
                        let item_id = item
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let call_id = item
                            .get("call_id")
                            .and_then(|x| x.as_str())
                            .unwrap_or(&item_id)
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let idx = self.next_tool_idx;
                        self.next_tool_idx += 1;
                        self.item_to_index.insert(item_id, idx);
                        out.push(LlmEvent::ToolCallStart {
                            index: idx,
                            id: call_id,
                            name,
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = v.get("item_id").and_then(|x| x.as_str()).unwrap_or("");
                if let Some(&idx) = self.item_to_index.get(item_id) {
                    if let Some(d) = v.get("delta").and_then(|x| x.as_str()) {
                        out.push(LlmEvent::ToolCallDelta {
                            index: idx,
                            json: d.to_string(),
                        });
                    }
                }
            }
            "response.function_call_arguments.done" => {
                let item_id = v.get("item_id").and_then(|x| x.as_str()).unwrap_or("");
                if let Some(&idx) = self.item_to_index.get(item_id) {
                    out.push(LlmEvent::ToolCallEnd { index: idx });
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(|x| x.as_str()) == Some("reasoning") {
                        out.push(LlmEvent::Opaque(ProviderOpaque {
                            protocol: ProtocolKind::Responses,
                            model_id: self.model_id.clone(),
                            data: item.clone(),
                        }));
                    }
                }
            }
            "response.completed" => {
                if let Some(u) = v.pointer("/response/usage") {
                    out.push(LlmEvent::Usage(Usage {
                        input_tokens: u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                        output_tokens: u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                        cached_tokens: u
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                    }));
                }
                let has_tools = !self.item_to_index.is_empty();
                self.finished = true;
                out.push(LlmEvent::Finished {
                    reason: if has_tools {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    },
                });
            }
            "response.incomplete" => {
                self.finished = true;
                out.push(LlmEvent::Finished {
                    reason: StopReason::MaxTokens,
                });
            }
            "response.failed" | "error" => {
                let msg = v
                    .pointer("/response/error/message")
                    .or_else(|| v.pointer("/error/message"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("Responses API failed");
                return Err(LnaError::new(ErrorCode::ModelBadRequest, msg));
            }
            _ => {}
        }
        Ok(out)
    }

    fn on_eof(&mut self) -> LnaResult<Vec<LlmEvent>> {
        if !self.finished {
            return Err(LnaError::new(
                ErrorCode::ModelStreamInterrupted,
                "Responses API stream ended without response.completed",
            ));
        }
        Ok(Vec::new())
    }
}
