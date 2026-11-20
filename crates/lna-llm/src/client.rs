use crate::adapters::adapter_for;
use crate::sse::SseParser;
use crate::types::*;
use futures_util::StreamExt;
use lna_types::{ErrorCode, LnaError, LnaResult, ModelCapabilities};
use rand::Rng;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

type CapabilitySink = std::sync::Arc<dyn Fn(&str, &ModelCapabilities) + Send + Sync>;

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    /// Called when a request had to drop an unsupported parameter, so the profile can be updated (D5).
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

    /// Registers a callback that persists capability changes learned from 400 responses.
    pub fn set_capability_sink(
        &self,
        f: impl Fn(&str, &ModelCapabilities) + Send + Sync + 'static,
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

    /// Like `stream`, but a rejected parameter is reported instead of being silently dropped.
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
        let mut relaxed = !allow_relax;

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
                // D5: an "unsupported parameter" 400 -> drop that parameter and retry once.
                if status.as_u16() == 400
                    && !relaxed
                    && relax_for_error(&body, &mut profile, &mut req)
                {
                    relaxed = true;
                    continue;
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

            if relaxed && allow_relax {
                let sink = self.sink.lock().ok().and_then(|g| g.clone());
                if let Some(f) = sink {
                    f(&profile.id, &profile.capabilities);
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
        let mut text = String::new();
        let mut usage = Usage::default();
        self.stream(profile, req, cancel, |ev| match ev {
            LlmEvent::TextDelta(t) => text.push_str(&t),
            LlmEvent::Usage(u) => usage = u,
            _ => {}
        })
        .await?;
        Ok((text, usage))
    }

    /// Section 10.7: Structured output fallback (JsonSchema -> JsonObject -> plain JSON + 2 repair turns).
    pub async fn complete_structured(
        &self,
        profile: &ResolvedProfile,
        base_req: &LlmRequest,
        schema_name: &str,
        schema: &serde_json::Value,
        cancel: CancellationToken,
    ) -> LnaResult<(serde_json::Value, Usage)> {
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
            req.output = OutputMode::Text;
            req.system = format!(
                "{}\n\n只输出合法 JSON，不要包含 Markdown 代码块或任何解释。Schema：\n{}",
                req.system, schema
            );
        }

        let mut total_usage = Usage::default();
        let (raw, u) = self.complete_text(profile, &req, cancel.clone()).await?;
        add_usage(&mut total_usage, &u);

        let mut candidate = strip_code_fences(&raw);
        for repair_attempt in 0..=2 {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&candidate) {
                if validator.is_valid(&val) {
                    return Ok((val, total_usage));
                }
            }
            if repair_attempt == 2 {
                break;
            }
            let repair_req = LlmRequest {
                system: "你是 JSON 修复器，只输出符合给定 Schema 的合法 JSON。".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "以下 JSON 未通过 Schema 校验，请修复并只输出合法 JSON：\nSchema:\n{}\n\n原输出:\n{}",
                        schema, candidate
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: base_req.max_output_tokens,
                reasoning: ReasoningOptions::default(),
            };
            let (repaired, ru) = self
                .complete_text(profile, &repair_req, cancel.clone())
                .await?;
            add_usage(&mut total_usage, &ru);
            candidate = strip_code_fences(&repaired);
        }

        Err(LnaError::new(
            ErrorCode::OutputInvalid,
            "Structured output failed schema validation after 2 repair attempts",
        ))
    }
}

/// Looks at a 400 body for a rejected request parameter and removes it from the request/profile.
/// Returns true when something was changed (so a retry makes sense).
pub fn relax_for_error(body: &str, profile: &mut ResolvedProfile, req: &mut LlmRequest) -> bool {
    let lower = body.to_lowercase();
    let rejected = [
        "unsupported",
        "unknown parameter",
        "unrecognized",
        "not supported",
        "extra inputs",
        "invalid parameter",
        "does not support",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !rejected {
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
    if lower.contains("response_format") || lower.contains("json_schema") {
        match req.output {
            OutputMode::JsonSchema { .. } => {
                profile.capabilities.json_schema = false;
                req.output = OutputMode::JsonObject;
                return true;
            }
            OutputMode::JsonObject => {
                profile.capabilities.json_mode = false;
                req.output = OutputMode::Text;
                return true;
            }
            OutputMode::Text => {}
        }
    }
    if (lower.contains("tools") || lower.contains("tool_choice") || lower.contains("function"))
        && !req.tools.is_empty()
    {
        profile.capabilities.tools = false;
        req.tools.clear();
        req.tool_mode = ToolMode::None;
        return true;
    }
    false
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
