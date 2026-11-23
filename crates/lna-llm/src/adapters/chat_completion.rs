use crate::types::*;
use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult};
use secrecy::ExposeSecret;
use serde_json::{json, Value};

pub struct ChatCompletionAdapter;

#[async_trait]
impl ProtocolAdapter for ChatCompletionAdapter {
    fn build(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
        req: &LlmRequest,
    ) -> LnaResult<reqwest::Request> {
        let url = format!("{}/chat/completions", profile.trimmed_base());
        let mut messages = Vec::new();
        if !req.system.is_empty() {
            messages.push(json!({ "role": "system", "content": req.system }));
        }

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
                        messages.push(json!({ "role": "user", "content": text }));
                    } else {
                        let mut content_parts = Vec::new();
                        for p in &m.parts {
                            match p {
                                Part::Text(t) if !t.is_empty() => {
                                    content_parts.push(json!({ "type": "text", "text": t }));
                                }
                                Part::Media {
                                    mime_type,
                                    data_base64,
                                    audio_format,
                                    ..
                                } => {
                                    let b64 = data_base64.as_deref().ok_or_else(|| {
                                        LnaError::new(
                                            ErrorCode::CapabilityMissing,
                                            "Chat Completion 协议不支持 file_uri 引用，需要内联 base64 媒体数据",
                                        )
                                    })?;
                                    if mime_type.starts_with("image/") {
                                        content_parts.push(json!({
                                            "type": "image_url",
                                            "image_url": {
                                                "url": format!("data:{mime_type};base64,{b64}")
                                            }
                                        }));
                                    } else if mime_type.starts_with("audio/") {
                                        let fmt = audio_format.as_deref().unwrap_or("");
                                        if matches!(fmt, "wav" | "mp3") {
                                            content_parts.push(json!({
                                                "type": "input_audio",
                                                "input_audio": {
                                                    "data": b64,
                                                    "format": fmt
                                                }
                                            }));
                                        } else {
                                            return Err(LnaError::new(
                                                ErrorCode::CapabilityMissing,
                                                format!("Chat Completion 协议仅原生支持 wav/mp3 音频，当前格式（{mime_type}）请使用 Gemini 模型处理"),
                                            ));
                                        }
                                    } else {
                                        return Err(LnaError::new(
                                            ErrorCode::CapabilityMissing,
                                            format!("Chat Completion 协议不支持原生视频或此类媒体（{mime_type}），请在“模型协议配置”中为视频勾选 Gemini 模型"),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                        messages.push(json!({ "role": "user", "content": content_parts }));
                    }
                }
                Role::Assistant => {
                    let mut text = String::new();
                    let mut tool_calls = Vec::new();
                    for p in &m.parts {
                        match p {
                            Part::Text(t) => text.push_str(t),
                            Part::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                tool_calls.push(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(arguments).unwrap_or_else(|_| "{}".into())
                                    }
                                }));
                            }
                            Part::Opaque(_)
                            | Part::ToolResult { .. }
                            | Part::Media { .. } => {}
                        }
                    }
                    let mut msg = json!({ "role": "assistant" });
                    if !text.is_empty() {
                        msg["content"] = json!(text);
                    }
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = json!(tool_calls);
                    }
                    messages.push(msg);
                }
                Role::Tool => {
                    for p in &m.parts {
                        if let Part::ToolResult {
                            call_id, content, ..
                        } = p
                        {
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": call_id,
                                "content": content,
                            }));
                        }
                    }
                }
            }
        }

        let max_field = profile
            .capabilities
            .max_tokens_field
            .as_deref()
            .unwrap_or("max_completion_tokens");

        let mut body = json!({
            "model": profile.model_id,
            "messages": messages,
            "stream": true,
        });
        body[max_field] = json!(clamp_reasoning_and_output_tokens(req.max_output_tokens));

        if profile.capabilities.stream_usage {
            body["stream_options"] = json!({ "include_usage": true });
        }

        if !req.tools.is_empty() && req.tool_mode == ToolMode::Auto {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                }))
                .collect::<Vec<_>>());
        }

        match &req.output {
            OutputMode::Text => {}
            OutputMode::JsonObject => {
                body["response_format"] = json!({ "type": "json_object" });
            }
            OutputMode::JsonSchema { name, schema } => {
                body["response_format"] = json!({
                    "type": "json_schema",
                    "json_schema": {
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
        builder
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))
    }

    fn decoder(&self, _model_id: &str) -> Box<dyn StreamDecoder> {
        Box::new(ChatCompletionDecoder::default())
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
                req = req.bearer_auth(s);
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
                        display_name: None,
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
struct ChatCompletionDecoder {
    finished: bool,
    in_think_tag: bool,
    pending_tag_buf: String,
    active_tool_indices: Vec<u32>,
}

impl ChatCompletionDecoder {
    fn emit_content_with_think_split(&mut self, chunk: &str, out: &mut Vec<LlmEvent>) {
        self.pending_tag_buf.push_str(chunk);
        loop {
            if self.in_think_tag {
                if let Some(end_idx) = self.pending_tag_buf.find("</think>") {
                    let thought = self.pending_tag_buf[..end_idx].to_string();
                    if !thought.is_empty() {
                        out.push(LlmEvent::ThoughtDelta(thought));
                    }
                    self.pending_tag_buf.drain(..end_idx + "</think>".len());
                    self.in_think_tag = false;
                } else {
                    // Keep last 8 chars in case </think> is split across frames
                    let safe_len = self.pending_tag_buf.len().saturating_sub(8);
                    let mut cut = safe_len;
                    while cut > 0 && !self.pending_tag_buf.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    if cut > 0 {
                        let part = self.pending_tag_buf.drain(..cut).collect::<String>();
                        out.push(LlmEvent::ThoughtDelta(part));
                    }
                    break;
                }
            } else if let Some(start_idx) = self.pending_tag_buf.find("<think>") {
                let text = self.pending_tag_buf[..start_idx].to_string();
                if !text.is_empty() {
                    out.push(LlmEvent::TextDelta(text));
                }
                self.pending_tag_buf.drain(..start_idx + "<think>".len());
                self.in_think_tag = true;
            } else {
                let safe_len = self.pending_tag_buf.len().saturating_sub(7);
                let mut cut = safe_len;
                while cut > 0 && !self.pending_tag_buf.is_char_boundary(cut) {
                    cut -= 1;
                }
                if cut > 0 {
                    let part = self.pending_tag_buf.drain(..cut).collect::<String>();
                    out.push(LlmEvent::TextDelta(part));
                }
                break;
            }
        }
    }

    fn flush_pending(&mut self, out: &mut Vec<LlmEvent>) {
        if !self.pending_tag_buf.is_empty() {
            let rem = std::mem::take(&mut self.pending_tag_buf);
            if self.in_think_tag {
                out.push(LlmEvent::ThoughtDelta(rem));
            } else {
                out.push(LlmEvent::TextDelta(rem));
            }
        }
    }
}

impl StreamDecoder for ChatCompletionDecoder {
    fn on_frame(&mut self, frame: SseFrame) -> LnaResult<Vec<LlmEvent>> {
        let trimmed = frame.data.trim();
        if trimmed == "[DONE]" {
            let mut out = Vec::new();
            self.flush_pending(&mut out);
            for idx in self.active_tool_indices.drain(..) {
                out.push(LlmEvent::ToolCallEnd { index: idx });
            }
            if !self.finished {
                self.finished = true;
                out.push(LlmEvent::Finished {
                    reason: StopReason::EndTurn,
                });
            }
            return Ok(out);
        }

        let v: Value = serde_json::from_str(trimmed).map_err(|e| {
            LnaError::new(ErrorCode::OutputInvalid, format!("Invalid JSON frame: {e}"))
        })?;

        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Model error");
            return Err(LnaError::new(ErrorCode::ModelBadRequest, msg));
        }

        let mut out = Vec::new();

        if let Some(u) = v.get("usage") {
            let inp = u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            let out_t = u
                .get("completion_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let cached = u
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            out.push(LlmEvent::Usage(Usage {
                input_tokens: inp,
                output_tokens: out_t,
                cached_tokens: cached,
            }));
        }

        if let Some(choice) = v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(r) = delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .and_then(|x| x.as_str())
                {
                    if !r.is_empty() {
                        out.push(LlmEvent::ThoughtDelta(r.to_string()));
                    }
                }
                if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                    if !c.is_empty() {
                        self.emit_content_with_think_split(c, &mut out);
                    }
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                            let name = tc
                                .pointer("/function/name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !self.active_tool_indices.contains(&idx) {
                                self.active_tool_indices.push(idx);
                            }
                            out.push(LlmEvent::ToolCallStart {
                                index: idx,
                                id: id.to_string(),
                                name,
                            });
                        }
                        if let Some(args) =
                            tc.pointer("/function/arguments").and_then(|x| x.as_str())
                        {
                            if !args.is_empty() {
                                out.push(LlmEvent::ToolCallDelta {
                                    index: idx,
                                    json: args.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
                self.flush_pending(&mut out);
                for idx in self.active_tool_indices.drain(..) {
                    out.push(LlmEvent::ToolCallEnd { index: idx });
                }
                let reason = match fr {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    "content_filter" => StopReason::ContentFilter,
                    other => StopReason::Other(other.to_string()),
                };
                self.finished = true;
                out.push(LlmEvent::Finished { reason });
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Vec<LlmEvent> {
        let mut out = Vec::new();
        self.flush_pending(&mut out);
        out
    }

    fn on_eof(&mut self) -> LnaResult<Vec<LlmEvent>> {
        if !self.finished {
            return Err(LnaError::new(
                ErrorCode::ModelStreamInterrupted,
                "Chat Completions stream ended without finish_reason or [DONE]",
            ));
        }
        Ok(Vec::new())
    }
}
