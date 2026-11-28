use crate::types::*;
use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult};
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

        // A tool call without a name (some relays and free models emit one) makes the provider
        // reject the whole request ("tool_calls.function.name is required"), so such calls and
        // their results are left out of the history.
        let nameless: std::collections::HashSet<&str> = req
            .messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .filter_map(|p| match p {
                Part::ToolCall { id, name, .. } if name.trim().is_empty() => Some(id.as_str()),
                _ => None,
            })
            .collect();

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
                                    } else if profile.is_mimo()
                                        && (mime_type.starts_with("audio/")
                                            || mime_type.starts_with("video/"))
                                    {
                                        content_parts.push(mimo_media_part(mime_type, b64)?);
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
                            Part::ToolCall { id, .. } if nameless.contains(id.as_str()) => {}
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
                            Part::Opaque(_) | Part::ToolResult { .. } | Part::Media { .. } => {}
                        }
                    }
                    if text.is_empty() && tool_calls.is_empty() {
                        continue;
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
                            if nameless.contains(call_id.as_str()) {
                                continue;
                            }
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
        // 0 = no limit: omit the field so the provider applies its own default.
        if req.max_output_tokens != 0 {
            body[max_field] = json!(req.max_output_tokens);
        }

        if profile.capabilities.stream_usage {
            body["stream_options"] = json!({ "include_usage": true });
        }

        // A test hook for one process (never stored): `LNA_EFFORT_OVERRIDE=low` asks the model to
        // reason less. The profile's own effort setting is not sent by this protocol.
        if let Some(effort) = std::env::var("LNA_EFFORT_OVERRIDE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            body["reasoning_effort"] = json!(effort);
        }

        // Tools stay in the request even when the model must not call them (the final answer
        // round): dropping them changes the prompt prefix and loses the provider's prompt cache.
        if !req.tools.is_empty() {
            if req.tool_mode != ToolMode::Auto {
                body["tool_choice"] = json!("none");
            }
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
        if let Some(s) = profile.default_auth_key() {
            builder = builder.bearer_auth(s);
        }
        for (k, v) in profile.resolved_extra_headers() {
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
        if let Some(s) = profile.default_auth_key() {
            req = req.bearer_auth(s);
        }
        for (k, v) in profile.resolved_extra_headers() {
            req = req.header(k, v);
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

/// MiMo takes audio and video only as inline Base64 data URLs; the encoded payload may not exceed this.
const MIMO_MEDIA_MAX_BASE64_BYTES: usize = 50 * 1024 * 1024;
const MIMO_VIDEO_MIMES: &[&str] = &[
    "video/mp4",
    "video/quicktime",
    "video/x-msvideo",
    "video/x-ms-wmv",
];
const MIMO_AUDIO_MIMES: &[&str] = &[
    "audio/mpeg",
    "audio/wav",
    "audio/x-wav",
    "audio/flac",
    "audio/mp4",
    "audio/x-m4a",
    "audio/ogg",
];

/// The MiMo content part for inline audio or video (images keep the `image_url` form).
fn mimo_media_part(mime_type: &str, b64: &str) -> LnaResult<Value> {
    let is_video = mime_type.starts_with("video/");
    let (label, allowed, names) = if is_video {
        ("视频", MIMO_VIDEO_MIMES, "MP4、MOV、AVI、WMV")
    } else {
        ("音频", MIMO_AUDIO_MIMES, "MP3、WAV、FLAC、M4A、OGG")
    };
    if !allowed.contains(&mime_type) {
        return Err(LnaError::new(
            ErrorCode::CapabilityMissing,
            format!("MiMo 模型的{label}输入仅支持 {names} 格式，当前格式（{mime_type}）不受支持，请先转换为支持的格式后再导入。"),
        ));
    }
    if b64.len() > MIMO_MEDIA_MAX_BASE64_BYTES {
        return Err(LnaError::new(
            ErrorCode::CapabilityMissing,
            format!(
                "MiMo 模型要求{label}的内联 Base64 数据不超过 50 MB（当前约 {:.1} MB）。请压缩或裁剪该文件后再导入。",
                b64.len() as f64 / (1024.0 * 1024.0)
            ),
        ));
    }
    let data_url = format!("data:{mime_type};base64,{b64}");
    Ok(if is_video {
        json!({
            "type": "video_url",
            "video_url": { "url": data_url },
            "fps": 1,
            "media_resolution": "default"
        })
    } else {
        json!({
            "type": "input_audio",
            "input_audio": { "data": data_url }
        })
    })
}

#[derive(Default)]
struct ChatCompletionDecoder {
    finished: bool,
    in_think_tag: bool,
    pending_tag_buf: String,
    /// Tool calls being streamed, by index. A call is announced to the consumer only once its
    /// name is known: relays differ in which chunk carries `id` and `function.name`.
    tool_calls: std::collections::BTreeMap<u32, ToolAcc>,
}

#[derive(Default)]
struct ToolAcc {
    id: String,
    name: String,
    started: bool,
    /// Argument text that arrived before the name did.
    pending_args: String,
}

impl ToolAcc {
    fn start(&mut self, idx: u32, out: &mut Vec<LlmEvent>) {
        if self.started || self.name.is_empty() {
            return;
        }
        self.started = true;
        if self.id.is_empty() {
            self.id = format!("call_{idx}");
        }
        out.push(LlmEvent::ToolCallStart {
            index: idx,
            id: self.id.clone(),
            name: self.name.clone(),
        });
        if !self.pending_args.is_empty() {
            out.push(LlmEvent::ToolCallDelta {
                index: idx,
                json: std::mem::take(&mut self.pending_args),
            });
        }
    }
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

    /// Closes every open tool call. A call that never got a name cannot be executed or sent
    /// back to the provider (it rejects an empty `function.name`), so it is dropped.
    fn end_tool_calls(&mut self, out: &mut Vec<LlmEvent>) {
        for (idx, mut acc) in std::mem::take(&mut self.tool_calls) {
            acc.start(idx, out);
            if acc.started {
                out.push(LlmEvent::ToolCallEnd { index: idx });
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
            self.end_tool_calls(&mut out);
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
            // OpenAI-style `prompt_tokens_details.cached_tokens`; DeepSeek reports the hit count as
            // `prompt_cache_hit_tokens` at the top of `usage`.
            let cached = u
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(|x| x.as_u64())
                .or_else(|| {
                    u.pointer("/prompt_tokens_details/cache_read_tokens")
                        .and_then(|x| x.as_u64())
                })
                .or_else(|| {
                    u.pointer("/input_tokens_details/cached_tokens")
                        .and_then(|x| x.as_u64())
                })
                .or_else(|| u.get("prompt_cache_hit_tokens").and_then(|x| x.as_u64()))
                .or_else(|| u.get("cache_read_input_tokens").and_then(|x| x.as_u64()))
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
                        let acc = self.tool_calls.entry(idx).or_default();
                        if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                            if !id.is_empty() {
                                acc.id = id.to_string();
                            }
                        }
                        if let Some(n) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
                            if !acc.started && !n.is_empty() && n != acc.name {
                                // the name may arrive in a later chunk than the id
                                acc.name.push_str(n);
                            }
                        }
                        acc.start(idx, &mut out);
                        if let Some(args) =
                            tc.pointer("/function/arguments").and_then(|x| x.as_str())
                        {
                            if !args.is_empty() {
                                if acc.started {
                                    out.push(LlmEvent::ToolCallDelta {
                                        index: idx,
                                        json: args.to_string(),
                                    });
                                } else {
                                    acc.pending_args.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
                self.flush_pending(&mut out);
                self.end_tool_calls(&mut out);
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
