use crate::adapters::adapter_for;
use crate::sse::SseParser;
use crate::types::*;
use futures_util::StreamExt;
use lna_types::{ErrorCode, LnaError, LnaResult, ProtocolKind};
use rand::Rng;
use secrecy::ExposeSecret;
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Request-format details learned at run time (D5): whether the endpoint accepts `stream_options`
/// and which max-tokens field it wants. They never change what a model can do (tools, structured
/// output), only how a request is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireParams {
    pub stream_usage: bool,
    pub max_tokens_field: Option<String>,
}

type CapabilitySink = std::sync::Arc<dyn Fn(&str, &WireParams) + Send + Sync>;

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    /// Called when a request had to be rewritten for the endpoint's wire format, so the profile remembers it (D5).
    sink: std::sync::Arc<std::sync::Mutex<Option<CapabilitySink>>>,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self {
            http,
            sink: Default::default(),
        }
    }

    /// Registers a callback that persists the request-format details learned from 400 responses.
    pub fn set_capability_sink(
        &self,
        f: impl Fn(&str, &WireParams) + Send + Sync + 'static,
    ) {
        if let Ok(mut g) = self.sink.lock() {
            *g = Some(std::sync::Arc::new(f));
        }
    }

    pub async fn discover_models(&self, profile: &ResolvedProfile) -> LnaResult<Vec<ModelInfo>> {
        adapter_for(profile.protocol)
            .discover(&self.http, profile)
            .await
    }

    pub async fn stream<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        on_event: F,
    ) -> LnaResult<StopReason>
    where
        F: FnMut(LlmEvent),
    {
        self.stream_with(profile, req, cancel, on_event, true).await
    }

    /// Like `stream`, but a rejected request-format parameter is reported instead of being rewritten.
    /// Used by capability probes, which must see the real answer.
    pub async fn stream_strict<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        on_event: F,
    ) -> LnaResult<StopReason>
    where
        F: FnMut(LlmEvent),
    {
        self.stream_with(profile, req, cancel, on_event, false)
            .await
    }

    async fn stream_with<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        mut on_event: F,
        allow_relax: bool,
    ) -> LnaResult<StopReason>
    where
        F: FnMut(LlmEvent),
    {
        let adapter = adapter_for(profile.protocol);
        let mut attempt = 0u32;
        let mut profile = profile.clone();
        let mut req = req.clone();
        req.max_output_tokens = clamp_reasoning_and_output_tokens(req.max_output_tokens);
        // at most two rewrites (stream_options and the max-tokens field can both be rejected)
        let mut relaxes_left = if allow_relax { 2u32 } else { 0 };
        let mut relaxed = false;

        loop {
            if cancel.is_cancelled() {
                return Err(LnaError::new(
                    ErrorCode::Cancelled,
                    "Cancelled before request",
                ));
            }

            let http_req = adapter.build(&self.http, &profile, &req)?;
            let resp_res = tokio::select! {
                _ = cancel.cancelled() => return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled")),
                r = self.http.execute(http_req) => r,
            };

            let resp = match resp_res {
                Ok(r) => r,
                Err(e) if e.is_timeout() && attempt < 3 => {
                    attempt += 1;
                    sleep_backoff(attempt, None, &cancel).await?;
                    continue;
                }
                Err(e) => {
                    // Connection refused / DNS failure is non-retryable (B-22)
                    let retryable = e.is_timeout();
                    return Err(LnaError::new(ErrorCode::FetchFailed, e.to_string())
                        .with_retryable(retryable));
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok());
                let body = resp.text().await.unwrap_or_default();
                let err = map_http_status(status, &body);
                if status.as_u16() == 400 {
                    // A model that stops accepting tools or structured output is not quietly
                    // downgraded: it is reported, and the profile is left for a re-check.
                    if let Some(e) = capability_rejection(&body, &req) {
                        return Err(e);
                    }
                    // D5: a wire-format parameter the endpoint does not take -> rewrite and retry.
                    if relaxes_left > 0 && relax_for_error(&body, &mut profile, &mut req) {
                        relaxes_left -= 1;
                        relaxed = true;
                        continue;
                    }
                }
                if (status.as_u16() == 429 || status.is_server_error()) && attempt < 3 {
                    attempt += 1;
                    sleep_backoff(attempt, retry_after, &cancel).await?;
                    continue;
                }
                return Err(err);
            }

            let mut decoder = adapter.decoder(&profile.model_id);
            let mut parser = SseParser::new();
            let mut byte_stream = resp.bytes_stream();
            let mut stop_reason = None;

            loop {
                let next_chunk = tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(LnaError::new(ErrorCode::Cancelled, "Stream cancelled"));
                    }
                    _ = tokio::time::sleep(STREAM_IDLE_TIMEOUT) => {
                        return Err(LnaError::new(ErrorCode::ModelStreamInterrupted, "Stream idle timeout (120s)"));
                    }
                    c = byte_stream.next() => c,
                };

                match next_chunk {
                    Some(Ok(bytes)) => {
                        for frame in parser.push(&bytes)? {
                            for ev in decoder.on_frame(frame)? {
                                if let LlmEvent::Finished { ref reason } = ev {
                                    stop_reason = Some(reason.clone());
                                }
                                on_event(ev);
                            }
                        }
                    }
                    Some(Err(e)) => {
                        for ev in decoder.flush() {
                            on_event(ev);
                        }
                        return Err(LnaError::new(
                            ErrorCode::ModelStreamInterrupted,
                            e.to_string(),
                        ));
                    }
                    None => {
                        if let Some(last) = parser.finish() {
                            for ev in decoder.on_frame(last)? {
                                if let LlmEvent::Finished { ref reason } = ev {
                                    stop_reason = Some(reason.clone());
                                }
                                on_event(ev);
                            }
                        }
                        for ev in decoder.flush() {
                            on_event(ev);
                        }
                        for ev in decoder.on_eof()? {
                            if let LlmEvent::Finished { ref reason } = ev {
                                stop_reason = Some(reason.clone());
                            }
                            on_event(ev);
                        }
                        break;
                    }
                }
            }

            if relaxed {
                let sink = self.sink.lock().ok().and_then(|g| g.clone());
                if let Some(f) = sink {
                    f(
                        &profile.id,
                        &WireParams {
                            stream_usage: profile.capabilities.stream_usage,
                            max_tokens_field: profile.capabilities.max_tokens_field.clone(),
                        },
                    );
                }
            }
            return Ok(stop_reason.unwrap_or(StopReason::EndTurn));
        }
    }

    pub async fn complete_text_strict(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
    ) -> LnaResult<(String, Usage)> {
        let mut text = String::new();
        let mut usage = Usage::default();
        self.stream_strict(profile, req, cancel, |ev| match ev {
            LlmEvent::TextDelta(t) => text.push_str(&t),
            LlmEvent::Usage(u) => usage = u,
            _ => {}
        })
        .await?;
        Ok((text, usage))
    }

    pub async fn complete_text(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
    ) -> LnaResult<(String, Usage)> {
        self.complete_text_with_thinking(profile, req, cancel, |_| {})
            .await
    }

    pub async fn complete_text_with_thinking<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        mut on_thinking: F,
    ) -> LnaResult<(String, Usage)>
    where
        F: FnMut(&str),
    {
        let mut text = String::new();
        let mut usage = Usage::default();
        self.stream(profile, req, cancel, |ev| match ev {
            LlmEvent::ThoughtDelta(t) => on_thinking(&t),
            LlmEvent::TextDelta(t) => text.push_str(&t),
            LlmEvent::Usage(u) => usage = u,
            _ => {}
        })
        .await?;
        Ok((text, usage))
    }

    /// Structured output: JSON schema when supported, else JSON mode with the schema in the prompt,
    /// plus schema-repair turns. Models with neither are refused.
    pub async fn complete_structured(
        &self,
        profile: &ResolvedProfile,
        base_req: &LlmRequest,
        schema_name: &str,
        schema: &serde_json::Value,
        cancel: CancellationToken,
    ) -> LnaResult<(serde_json::Value, Usage)> {
        self.complete_structured_with_thinking(
            profile,
            base_req,
            schema_name,
            schema,
            cancel,
            2,
            |_| {},
            |_| {},
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete_structured_with_thinking<F, G>(
        &self,
        profile: &ResolvedProfile,
        base_req: &LlmRequest,
        schema_name: &str,
        schema: &serde_json::Value,
        cancel: CancellationToken,
        max_repairs: usize,
        mut on_thinking: F,
        mut on_status: G,
    ) -> LnaResult<(serde_json::Value, Usage)>
    where
        F: FnMut(&str),
        G: FnMut(&str),
    {
        let validator = jsonschema::validator_for(schema)
            .map_err(|e| LnaError::invalid(format!("Invalid JSON schema: {e}")))?;

        let mut req = base_req.clone();
        if profile.capabilities.json_schema {
            req.output = OutputMode::JsonSchema {
                name: schema_name.to_string(),
                schema: schema.clone(),
            };
        } else if profile.capabilities.json_mode {
            req.output = OutputMode::JsonObject;
            req.system = format!(
                "{}\n\n请只输出符合以下 JSON Schema 的 JSON：\n{}",
                req.system, schema
            );
        } else {
            return Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                "该模型不支持结构化输出（JSON Schema / JSON 模式），无法使用。",
            ));
        }

        let mut total_usage = Usage::default();
        let (raw, u) = self
            .complete_text_with_thinking(profile, &req, cancel.clone(), &mut on_thinking)
            .await?;
        add_usage(&mut total_usage, &u);

        let mut candidate = strip_code_fences(&raw);
        let mut last_reason = String::new();
        for repair_attempt in 0..=max_repairs {
            match serde_json::from_str::<serde_json::Value>(&candidate) {
                Ok(val) => {
                    let errs: Vec<String> = validator
                        .iter_errors(&val)
                        .take(3)
                        .map(|e| format!("{}: {}", e.instance_path, e))
                        .collect();
                    if errs.is_empty() {
                        return Ok((val, total_usage));
                    }
                    last_reason = format!("Schema validation error: {}", errs.join("; "));
                }
                Err(e) => {
                    last_reason = format!("JSON parse error: {e}");
                }
            }
            if repair_attempt == max_repairs {
                break;
            }
            on_status(&format!(
                "结构化输出未通过 Schema 校验，正在进行第 {}/{} 次修复…",
                repair_attempt + 1,
                max_repairs
            ));
            let repair_req = LlmRequest {
                system: "You are a strict JSON repairer. Output only valid JSON that strictly conforms to the provided JSON Schema, with no commentary or code fences.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "The following JSON failed schema validation ({}). Fix it and output valid JSON only:\nSchema:\n{}\n\nOriginal output:\n{}",
                        last_reason, schema, candidate
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: req.output.clone(),
                max_output_tokens: base_req.max_output_tokens,
                reasoning: ReasoningOptions::default(),
            };
            let (repaired, ru) = self
                .complete_text_with_thinking(profile, &repair_req, cancel.clone(), &mut on_thinking)
                .await?;
            add_usage(&mut total_usage, &ru);
            candidate = strip_code_fences(&repaired);
        }

        let snippet: String = last_reason.chars().take(400).collect();
        Err(LnaError::new(
            ErrorCode::OutputInvalid,
            if snippet.is_empty() {
                format!("Structured output failed schema validation after {max_repairs} repair attempts")
            } else {
                format!("Structured output failed schema validation after {max_repairs} repair attempts: {snippet}")
            },
        ))
    }

    /// Uploads a large audio/video file via the Gemini Files resumable upload API and waits until `ACTIVE`.
    /// Returns `(file_uri, file_name)` so the caller can clean up via `delete_gemini_file`.
    pub async fn upload_gemini_file(
        &self,
        profile: &ResolvedProfile,
        path: &Path,
        mime_type: &str,
        display_name: &str,
    ) -> LnaResult<(String, String)> {
        if profile.protocol != ProtocolKind::Gemini {
            return Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                "Files API upload is only supported for the Gemini protocol",
            ));
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| LnaError::internal(format!("Failed to read media file: {e}")))?;
        let trimmed = profile.trimmed_base();
        let upload_url = gemini_upload_url(trimmed);
        let apply_headers = |mut b: reqwest::RequestBuilder| -> reqwest::RequestBuilder {
            if let Some(ref key) = profile.api_key {
                let s = key.expose_secret();
                if !s.is_empty() {
                    b = b.header("x-goog-api-key", s);
                }
            }
            for (k, v) in &profile.extra_headers {
                b = b.header(k, v);
            }
            b
        };

        let meta_body = serde_json::json!({
            "file": { "display_name": display_name }
        });
        let start_resp = apply_headers(
            self.http
                .post(&upload_url)
                .header("X-Goog-Upload-Protocol", "resumable")
                .header("X-Goog-Upload-Command", "start")
                .header("X-Goog-Upload-Header-Content-Length", bytes.len().to_string())
                .header("X-Goog-Upload-Header-Content-Type", mime_type)
                .json(&meta_body),
        )
        .send()
        .await
        .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;

        if !start_resp.status().is_success() {
            let st = start_resp.status();
            let err_body = start_resp.text().await.unwrap_or_default();
            return Err(map_http_status(st, &err_body));
        }

        let session_uri = start_resp
            .headers()
            .get("x-goog-upload-url")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| {
                LnaError::new(
                    ErrorCode::OutputInvalid,
                    "Gemini Files API did not return x-goog-upload-url header",
                )
            })?;

        let up_resp = apply_headers(
            self.http
                .post(&session_uri)
                .header("X-Goog-Upload-Offset", "0")
                .header("X-Goog-Upload-Command", "upload, finalize")
                .header("Content-Type", mime_type)
                .body(bytes),
        )
        .send()
        .await
        .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;

        let up_status = up_resp.status();
        let up_text = up_resp.text().await.unwrap_or_default();
        if !up_status.is_success() {
            return Err(map_http_status(up_status, &up_text));
        }

        let val: serde_json::Value = serde_json::from_str(&up_text)
            .map_err(|e| LnaError::new(ErrorCode::OutputInvalid, e.to_string()))?;
        let file_obj = val.get("file").unwrap_or(&val);
        let file_name = file_obj
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let mut file_uri = file_obj
            .get("uri")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let mut state = file_obj
            .get("state")
            .and_then(|x| x.as_str())
            .unwrap_or("ACTIVE")
            .to_string();

        if !file_name.is_empty() && state == "PROCESSING" {
            let poll_url = format!("{trimmed}/{}", file_name.trim_start_matches('/'));
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let pr = apply_headers(self.http.get(&poll_url))
                    .send()
                    .await
                    .map_err(|e| LnaError::new(ErrorCode::FetchFailed, e.to_string()))?;
                if !pr.status().is_success() {
                    continue;
                }
                if let Ok(pv) = pr.json::<serde_json::Value>().await {
                    let fo = pv.get("file").unwrap_or(&pv);
                    if let Some(u) = fo.get("uri").and_then(|x| x.as_str()) {
                        file_uri = u.to_string();
                    }
                    state = fo
                        .get("state")
                        .and_then(|x| x.as_str())
                        .unwrap_or("ACTIVE")
                        .to_string();
                    if state != "PROCESSING" {
                        break;
                    }
                }
            }
        }

        if state == "FAILED" {
            return Err(LnaError::new(
                ErrorCode::ModelBadRequest,
                "Gemini Files API reported FAILED processing state",
            ));
        }
        if file_uri.is_empty() {
            return Err(LnaError::new(
                ErrorCode::OutputInvalid,
                "Gemini Files API did not return a file uri",
            ));
        }
        Ok((file_uri, file_name))
    }

    pub async fn delete_gemini_file(&self, profile: &ResolvedProfile, file_name: &str) {
        if file_name.is_empty() || profile.protocol != ProtocolKind::Gemini {
            return;
        }
        let url = format!(
            "{}/{}",
            profile.trimmed_base(),
            file_name.trim_start_matches('/')
        );
        let mut req = self.http.delete(url);
        if let Some(ref key) = profile.api_key {
            let s = key.expose_secret();
            if !s.is_empty() {
                req = req.header("x-goog-api-key", s);
            }
        }
        for (k, v) in &profile.extra_headers {
            req = req.header(k, v);
        }
        let _ = req.send().await;
    }
}

fn gemini_upload_url(trimmed_base: &str) -> String {
    if let Some(prefix) = trimmed_base.strip_suffix("/v1beta") {
        format!("{prefix}/upload/v1beta/files")
    } else if let Some(prefix) = trimmed_base.strip_suffix("/v1") {
        format!("{prefix}/upload/v1/files")
    } else {
        format!("{trimmed_base}/upload/v1beta/files")
    }
}

/// Looks at a 400 body for a rejected request parameter and removes it from the request/profile.
/// Returns true when something was changed (so a retry makes sense).
fn says_unsupported(lower: &str) -> bool {
    [
        "unsupported",
        "unknown parameter",
        "unrecognized",
        "not supported",
        "extra inputs",
        "invalid parameter",
        "does not support",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// D5: rewrites the request for an endpoint that rejected a wire-format parameter
/// (`stream_options`, `max_completion_tokens` / `max_tokens`). Returns false when the error is
/// about anything else. Never touches tools or the output mode: see [`capability_rejection`].
pub fn relax_for_error(body: &str, profile: &mut ResolvedProfile, _req: &mut LlmRequest) -> bool {
    let lower = body.to_lowercase();
    if !says_unsupported(&lower) {
        return false;
    }
    if lower.contains("stream_options") && profile.capabilities.stream_usage {
        profile.capabilities.stream_usage = false;
        return true;
    }
    if lower.contains("max_completion_tokens")
        && profile.capabilities.max_tokens_field.as_deref() != Some("max_tokens")
    {
        profile.capabilities.max_tokens_field = Some("max_tokens".into());
        return true;
    }
    if lower.contains("max_tokens")
        && profile.capabilities.max_tokens_field.as_deref() == Some("max_tokens")
    {
        profile.capabilities.max_tokens_field = Some("max_completion_tokens".into());
        return true;
    }
    false
}

/// A 400 saying the model does not take the tools or the structured-output format of this
/// request. Such a model no longer meets the requirements, so this is an error (CAPABILITY_MISSING)
/// instead of a retry without tools / with a weaker output mode; the stored profile is unchanged
/// and the user is asked to re-check the model in the settings.
pub fn capability_rejection(body: &str, req: &LlmRequest) -> Option<LnaError> {
    let lower = body.to_lowercase();
    if !says_unsupported(&lower) {
        return None;
    }
    let output = !matches!(req.output, OutputMode::Text)
        && (lower.contains("response_format")
            || lower.contains("json_schema")
            || lower.contains("json_object"));
    let tools = !req.tools.is_empty()
        && (lower.contains("tools") || lower.contains("tool_choice") || lower.contains("function"));
    let what = match (tools, output) {
        (true, _) => "工具调用（tool / function call）",
        (false, true) => "结构化输出（JSON Schema / JSON 模式）",
        _ => return None,
    };
    let detail: String = body.chars().take(300).collect();
    Some(LnaError::new(
        ErrorCode::CapabilityMissing,
        format!(
            "模型服务拒绝了{what}请求，可能已不再支持该能力。请在“设置 → AI 模型”中重新检测或更换模型。（服务返回：{detail}）"
        ),
    ))
}

fn add_usage(dst: &mut Usage, src: &Usage) {
    dst.input_tokens += src.input_tokens;
    dst.output_tokens += src.output_tokens;
    dst.cached_tokens += src.cached_tokens;
}

fn strip_code_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    if let Some(rest) = t.strip_prefix("```") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    t.to_string()
}

async fn sleep_backoff(
    attempt: u32,
    retry_after_secs: Option<u64>,
    cancel: &CancellationToken,
) -> LnaResult<()> {
    let base_ms = match retry_after_secs {
        Some(s) => s.saturating_mul(1000).min(30_000),
        None => {
            let exp = 1000u64 * (1u64 << (attempt - 1));
            let jitter: u64 = rand::thread_rng().gen_range(0..400);
            exp + jitter
        }
    };
    tokio::select! {
        _ = cancel.cancelled() => Err(LnaError::new(ErrorCode::Cancelled, "Cancelled during backoff")),
        _ = tokio::time::sleep(Duration::from_millis(base_ms)) => Ok(()),
    }
}
