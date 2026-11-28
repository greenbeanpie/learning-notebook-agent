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

/// Default of the per-profile limit on requests in flight: 0 = unlimited (the user sets it per profile).
pub const DEFAULT_PARALLEL_REQUESTS: u32 = 0;
/// Executor count used for a profile without a configured limit: the most executors any
/// orchestrator fans out to, so `parallel_limit` reports this for "unlimited".
pub const MAX_PARALLEL_REQUESTS: u32 = 64;

/// Attempts (the first one included) of a request the endpoint throttles with 429, or 503 with
/// Retry-After, before the error is returned.
const MAX_THROTTLED_TRIES: u32 = 8;
/// Upper bound of one wait before a throttled request is retried.
const MAX_THROTTLE_WAIT: Duration = Duration::from_secs(30);
/// Successful requests in a row that raise a learned cap by one.
const SUCCESSES_TO_RAISE_CAP: u32 = 20;

/// How many requests of one profile may be in flight at once. Every request of the program goes
/// through `LlmClient::stream`, so this one gate bounds all AI work of a profile (chat, indexing,
/// Studio executors, ...) without any caller having to hold a slot across nested calls.
///
/// The configured limit (None = unlimited) is a hard cap. On top of it the gate learns the real
/// limit of the provider: a 429 while N requests are in flight caps the profile at max(1, N-1),
/// and every `SUCCESSES_TO_RAISE_CAP` successes in a row raise that cap by one (the learned cap is
/// dropped once it reaches the configured limit).
struct Gate {
    configured: Option<usize>,
    state: std::sync::Mutex<GateState>,
    /// Woken when a slot is released or the cap changes.
    freed: tokio::sync::Notify,
}

#[derive(Default)]
struct GateState {
    in_flight: usize,
    learned: Option<usize>,
    successes: u32,
}

impl Gate {
    fn new(configured: Option<usize>) -> Self {
        Self {
            configured,
            state: Default::default(),
            freed: tokio::sync::Notify::new(),
        }
    }

    fn cap(&self, st: &GateState) -> usize {
        self.configured
            .unwrap_or(usize::MAX)
            .min(st.learned.unwrap_or(usize::MAX))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        // the state is plain counters: a poisoned lock is still consistent
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A request slot of a profile, held from the start of a request until its last attempt ends.
struct GateSlot {
    gate: std::sync::Arc<Gate>,
}

impl GateSlot {
    /// The provider throttled this request while the profile had `in_flight` requests running.
    fn throttled(&self) {
        let mut st = self.gate.lock();
        let in_flight = st.in_flight.max(1);
        let learned = in_flight.saturating_sub(1).max(1);
        st.learned = Some(st.learned.map_or(learned, |c| c.min(learned)));
        st.successes = 0;
        tracing::info!(
            cap = learned,
            in_flight,
            "provider throttled; learned parallel cap"
        );
    }

    /// A request of the profile finished without being throttled.
    fn succeeded(&self) {
        let mut raised = false;
        {
            let mut st = self.gate.lock();
            st.successes += 1;
            if st.successes >= SUCCESSES_TO_RAISE_CAP {
                st.successes = 0;
                if let Some(c) = st.learned {
                    let next = c + 1;
                    st.learned = match self.gate.configured {
                        Some(k) if next >= k => None,
                        _ => Some(next),
                    };
                    raised = true;
                }
            }
        }
        if raised {
            self.gate.freed.notify_waiters();
        }
    }
}

impl Drop for GateSlot {
    fn drop(&mut self) {
        self.gate.lock().in_flight -= 1;
        self.gate.freed.notify_waiters();
    }
}

/// How long a prompt prefix counts as cached at the provider after its first request was sent.
const WARM_TTL: Duration = Duration::from_secs(600);
/// How long a request waits for the first request of the same prefix before it goes anyway.
const WARM_WAIT: Duration = Duration::from_secs(20);
/// Shorter prompts are not cached by providers (the unit is about 64 tokens, the minimum about 1k).
const WARM_MIN_CHARS: usize = 3000;

/// Exact structural identity of a cacheable shared prefix (avoids blind truncation hashes):
/// matches on profile, model, declared tools, exact system prompt, and the shared material
/// prefix of the first user message (before per-batch / per-section / per-task suffixes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PrefixKey {
    profile_id: String,
    model_id: String,
    tool_names: Vec<String>,
    system: String,
    shared_material: String,
}

enum WarmEntry {
    /// The first request with this prefix is in flight; `true` is sent once its first bytes arrived.
    Warming(tokio::sync::watch::Receiver<bool>),
    Warm(std::time::Instant),
}

type WarmMap = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<PrefixKey, WarmEntry>>>;

/// Held by the first request (or dedicated lightweight warm-up probe) of a prompt prefix.
/// Parallel siblings sharing the exact `PrefixKey` wait on this until the provider has filled
/// its prompt cache, then proceed concurrently.
struct WarmLeader {
    map: WarmMap,
    key: PrefixKey,
    tx: tokio::sync::watch::Sender<bool>,
    done: bool,
}

impl WarmLeader {
    fn release(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if let Ok(mut m) = self.map.lock() {
            m.insert(self.key.clone(), WarmEntry::Warm(std::time::Instant::now()));
        }
        let _ = self.tx.send(true);
    }
}

impl Drop for WarmLeader {
    fn drop(&mut self) {
        self.release();
    }
}

enum WarmRole {
    Leader(WarmLeader),
    Follower(tokio::sync::watch::Receiver<bool>),
    Free,
}

/// Extracts the shared material head from the first user message by trimming any trailing
/// per-batch / per-section / per-task suffix marker.
fn shared_material_slice(text: &str) -> &str {
    const MARKERS: &[&str] = &[
        "\n\n=== THIS ",
        "\n\n=== TASK ===",
        "\n\nPART ",
        "\n\nPLANNING TASK",
    ];
    let cut = MARKERS
        .iter()
        .filter_map(|m| text.find(m))
        .min()
        .unwrap_or(text.len());
    &text[..cut]
}

/// Identifies the exact shared prefix of a request (system + tools + shared material).
/// Returns `None` when the shared prefix is too short to be cached by providers.
fn prefix_key(profile: &ResolvedProfile, req: &LlmRequest) -> Option<PrefixKey> {
    let mut first_msg = String::new();
    for m in req.messages.iter().take(1) {
        for p in &m.parts {
            if let Part::Text(t) = p {
                first_msg.push_str(t);
            }
        }
    }
    let shared_material = shared_material_slice(&first_msg).to_string();
    let total_chars = req.system.chars().count() + shared_material.chars().count();
    if total_chars < WARM_MIN_CHARS {
        return None;
    }
    Some(PrefixKey {
        profile_id: profile.id.clone(),
        model_id: profile.model_id.clone(),
        tool_names: req.tools.iter().map(|t| t.name.clone()).collect(),
        system: req.system.clone(),
        shared_material,
    })
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    warm: WarmMap,
    /// Called when a request had to be rewritten for the endpoint's wire format, so the profile remembers it (D5).
    sink: std::sync::Arc<std::sync::Mutex<Option<CapabilitySink>>>,
    gates:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Gate>>>>,
    /// Wait before the first retry of a throttled request (doubles per try, capped at 30s).
    retry_base: Duration,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Separator between the Chinese and English text of a status message (see `bilingual_status`).
pub const STATUS_LANG_SEP: char = '\u{1f}';

/// Status callbacks receive one string carrying both languages; the caller shows the one for the UI language
/// with `pick_status_lang`.
fn bilingual_status(zh: &str, en: &str) -> String {
    format!("{zh}{STATUS_LANG_SEP}{en}")
}

/// Picks one language out of a status message made by the LLM client (plain messages pass through unchanged).
pub fn pick_status_lang(msg: &str, en: bool) -> &str {
    match msg.split_once(STATUS_LANG_SEP) {
        Some((zh, e)) => {
            if en {
                e
            } else {
                zh
            }
        }
        None => msg,
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
            gates: Default::default(),
            warm: Default::default(),
            retry_base: Duration::from_secs(1),
        }
    }

    /// Sets the wait before the first retry of a throttled request (1s by default; tests use less).
    pub fn with_retry_base(mut self, base: Duration) -> Self {
        self.retry_base = base;
        self
    }

    fn warm_role(&self, key: Option<PrefixKey>) -> WarmRole {
        let Some(key) = key else {
            return WarmRole::Free;
        };
        let Ok(mut m) = self.warm.lock() else {
            return WarmRole::Free;
        };
        match m.get(&key) {
            Some(WarmEntry::Warm(t)) if t.elapsed() < WARM_TTL => return WarmRole::Free,
            Some(WarmEntry::Warming(rx)) => return WarmRole::Follower(rx.clone()),
            _ => {}
        }
        // forget entries that are long expired so the map stays small
        m.retain(|_, e| !matches!(e, WarmEntry::Warm(t) if t.elapsed() >= WARM_TTL));
        let (tx, rx) = tokio::sync::watch::channel(false);
        m.insert(key.clone(), WarmEntry::Warming(rx));
        WarmRole::Leader(WarmLeader {
            map: self.warm.clone(),
            key,
            tx,
            done: false,
        })
    }

    /// Sends a minimal prefill-only warm-up request (`max_output_tokens = 1`, or `16` for OpenAI
    /// Responses API) for a shared prefix before parallel sibling requests fan out, so every
    /// parallel worker hits a warm provider KV cache immediately. Best-effort: errors are ignored.
    pub async fn warm_prefix(
        &self,
        profile: &ResolvedProfile,
        sample_req: &LlmRequest,
        cancel: &CancellationToken,
    ) {
        if cancel.is_cancelled() {
            return;
        }
        let key = prefix_key(profile, sample_req);
        match self.warm_role(key) {
            WarmRole::Free => {}
            WarmRole::Follower(mut rx) => {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = tokio::time::sleep(WARM_WAIT) => {}
                    _ = rx.wait_for(|done| *done) => {}
                }
            }
            WarmRole::Leader(mut leader) => {
                let mut probe = sample_req.clone();
                // 0 means unlimited in LlmRequest; use minimal positive cap (1 token; 16 on Responses API)
                probe.max_output_tokens = if profile.protocol == ProtocolKind::Responses {
                    16
                } else {
                    1
                };
                probe.reasoning = ReasoningOptions::default();
                let _ = tokio::time::timeout(
                    WARM_WAIT,
                    self.stream_with(profile, &probe, cancel.clone(), |_| {}, true, true),
                )
                .await;
                leader.release();
            }
        }
    }

    /// Sets how many requests the profile may have in flight at once. 0 means unlimited (the
    /// default); other values are a hard cap (clamped to 1..=1024 by the storage layer). A
    /// changed limit takes effect for requests that start afterwards; running ones finish normally.
    pub fn set_parallel_limit(&self, profile_id: &str, limit: u32) {
        let configured = (limit > 0).then_some(limit as usize);
        if let Ok(mut g) = self.gates.lock() {
            let same = g
                .get(profile_id)
                .is_some_and(|gate| gate.configured == configured);
            if !same {
                g.insert(
                    profile_id.to_string(),
                    std::sync::Arc::new(Gate::new(configured)),
                );
            }
        }
    }

    /// The configured limit of a profile as an executor count: the limit itself, or
    /// `MAX_PARALLEL_REQUESTS` when the profile is unlimited (callers fan out at most that far).
    pub fn parallel_limit(&self, profile_id: &str) -> usize {
        self.gates
            .lock()
            .ok()
            .and_then(|g| g.get(profile_id).and_then(|x| x.configured))
            .unwrap_or(MAX_PARALLEL_REQUESTS as usize)
    }

    /// Waits for a free request slot of the profile (None for profiles without an id, e.g. probes).
    async fn acquire_slot(
        &self,
        profile_id: &str,
        cancel: &CancellationToken,
    ) -> LnaResult<Option<GateSlot>> {
        if profile_id.is_empty() {
            return Ok(None);
        }
        let gate = {
            let mut g = self
                .gates
                .lock()
                .map_err(|_| LnaError::internal("request gate poisoned"))?;
            g.entry(profile_id.to_string())
                .or_insert_with(|| std::sync::Arc::new(Gate::new(None)))
                .clone()
        };
        loop {
            // created before the check, so a release between check and wait is not missed
            let freed = gate.freed.notified();
            {
                let mut st = gate.lock();
                if st.in_flight < gate.cap(&st) {
                    st.in_flight += 1;
                    return Ok(Some(GateSlot { gate: gate.clone() }));
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled while waiting for a request slot")),
                _ = freed => {}
            }
        }
    }

    /// Registers a callback that persists the request-format details learned from 400 responses.
    pub fn set_capability_sink(&self, f: impl Fn(&str, &WireParams) + Send + Sync + 'static) {
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
        self.stream_with(profile, req, cancel, on_event, true, false)
            .await
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
        self.stream_with(profile, req, cancel, on_event, false, false)
            .await
    }

    async fn stream_with<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        mut on_event: F,
        allow_relax: bool,
        skip_warm: bool,
    ) -> LnaResult<StopReason>
    where
        F: FnMut(LlmEvent),
    {
        // before taking a request slot: waiting siblings must not hold one
        let mut leader = if skip_warm {
            None
        } else {
            match self.warm_role(prefix_key(profile, req)) {
            WarmRole::Leader(l) => Some(l),
            WarmRole::Follower(mut rx) => {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled while waiting for the prompt cache"));
                    }
                    _ = tokio::time::sleep(WARM_WAIT) => {}
                    _ = rx.wait_for(|done| *done) => {}
                }
                None
            }
            WarmRole::Free => None,
            }
        };
        let slot = self.acquire_slot(&profile.id, &cancel).await?;
        let adapter = adapter_for(profile.protocol);
        let mut attempt = 0u32;
        let mut throttled_tries = 0u32;
        let mut profile = profile.clone();
        let mut req = req.clone();
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
                // Throttled: wait and send the same request again, before any byte of a stream was
                // read. 503 counts only with Retry-After (the endpoint says when to come back).
                let throttled =
                    status.as_u16() == 429 || (status.as_u16() == 503 && retry_after.is_some());
                if throttled {
                    if let Some(s) = slot.as_ref() {
                        s.throttled();
                    }
                    throttled_tries += 1;
                    if throttled_tries < MAX_THROTTLED_TRIES {
                        let delay = throttle_delay(throttled_tries, retry_after, self.retry_base);
                        tracing::info!(
                            profile = %profile.id,
                            status = status.as_u16(),
                            try_no = throttled_tries,
                            delay_ms = delay.as_millis() as u64,
                            "request throttled by the endpoint; waiting before retry"
                        );
                        sleep_cancellable(delay, &cancel).await?;
                        continue;
                    }
                    return Err(err);
                }
                if status.is_server_error() && attempt < 3 {
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
                        if let Some(l) = leader.as_mut() {
                            l.release();
                        }
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
            if let Some(s) = slot.as_ref() {
                s.succeeded();
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
        on_thinking: F,
    ) -> LnaResult<(String, Usage)>
    where
        F: FnMut(&str),
    {
        let (text, usage, _) = self
            .complete_text_with_stop(profile, req, cancel, on_thinking, |_| {})
            .await?;
        Ok((text, usage))
    }

    /// Like `complete_text_with_thinking`, but also reports why generation stopped, so a
    /// length cut-off can be told apart from malformed output.
    pub async fn complete_text_with_stop<F, T>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        mut on_thinking: F,
        mut on_text: T,
    ) -> LnaResult<(String, Usage, StopReason)>
    where
        F: FnMut(&str),
        T: FnMut(&str),
    {
        let mut text = String::new();
        let mut usage = Usage::default();
        let stop = self
            .stream(profile, req, cancel, |ev| match ev {
                LlmEvent::ThoughtDelta(t) => on_thinking(&t),
                LlmEvent::TextDelta(t) => {
                    text.push_str(&t);
                    on_text(&text);
                }
                LlmEvent::Usage(u) => usage = u,
                _ => {}
            })
            .await?;
        Ok((text, usage, stop))
    }

    /// Streams one assistant turn and collects text, tool calls (ignoring nameless entries),
    /// opaque provider parts, usage, and stop reason while forwarding thinking deltas.
    pub async fn complete_turn<F>(
        &self,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
        mut on_thinking: F,
    ) -> LnaResult<TurnOutput>
    where
        F: FnMut(&str),
    {
        let mut text = String::new();
        let mut opaque = Vec::new();
        let mut usage = Usage::default();
        let mut calls: std::collections::BTreeMap<u32, (String, String, String)> =
            std::collections::BTreeMap::new();
        let stop_reason = self
            .stream(profile, req, cancel, |ev| match ev {
                LlmEvent::ThoughtDelta(delta) => on_thinking(&delta),
                LlmEvent::TextDelta(t) => text.push_str(&t),
                LlmEvent::ToolCallStart { index, id, name } => {
                    calls.insert(index, (id, name, String::new()));
                }
                LlmEvent::ToolCallDelta { index, json } => {
                    if let Some(entry) = calls.get_mut(&index) {
                        entry.2.push_str(&json);
                    }
                }
                LlmEvent::Opaque(o) => opaque.push(Part::Opaque(o)),
                LlmEvent::Usage(u) => usage = u,
                _ => {}
            })
            .await?;
        let tool_calls = calls
            .into_values()
            .filter(|(_, name, _)| !name.trim().is_empty())
            .map(|(id, name, raw_arguments)| CollectedToolCall {
                id,
                name,
                raw_arguments,
            })
            .collect();
        Ok(TurnOutput {
            text,
            tool_calls,
            opaque,
            usage,
            stop_reason,
        })
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
        on_thinking: F,
        on_status: G,
    ) -> LnaResult<(serde_json::Value, Usage)>
    where
        F: FnMut(&str),
        G: FnMut(&str),
    {
        self.complete_structured_continuing(
            profile,
            base_req,
            schema_name,
            schema,
            cancel,
            max_repairs,
            None,
            on_thinking,
            on_status,
            |_| {},
        )
        .await
    }

    /// Structured output where the answer is an object whose `array_key` property is a flat
    /// array of items. When generation is cut off by the output limit, the items that were
    /// completed are kept and the model is asked to continue with the remaining ones (up to
    /// three times) instead of rewriting everything.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_structured_continuing<F, G, T>(
        &self,
        profile: &ResolvedProfile,
        base_req: &LlmRequest,
        schema_name: &str,
        schema: &serde_json::Value,
        cancel: CancellationToken,
        max_repairs: usize,
        array_key: Option<&str>,
        mut on_thinking: F,
        mut on_status: G,
        mut on_text: T,
    ) -> LnaResult<(serde_json::Value, Usage)>
    where
        F: FnMut(&str),
        G: FnMut(&str),
        T: FnMut(&str),
    {
        let validator = jsonschema::validator_for(schema)
            .map_err(|e| LnaError::invalid(format!("Invalid JSON schema: {e}")))?;

        let mut req = base_req.clone();
        // Every structured answer may be an abort: the model that cannot do the task says why
        // instead of inventing content that fits the schema.
        let sent_schema = with_abort_property(schema);
        let schema = &sent_schema;
        req.system.push_str(ABORT_NOTE);
        // A profile that was never checked (saving does not probe) is tried with JSON Schema;
        // once checked, what the probe found decides.
        let unchecked = profile.capabilities.probed_at.is_none() && !profile.capabilities.json_mode;
        if profile.capabilities.json_schema || unchecked {
            req.output = OutputMode::JsonSchema {
                name: schema_name.to_string(),
                schema: schema.clone(),
            };
        } else if profile.capabilities.json_mode {
            req.output = OutputMode::JsonObject;
            // The schema goes at the END of the last user message, not into the system prompt:
            // calls of one job that differ only in their schema then still share the whole
            // system + message prefix, which is what the provider's prompt cache matches on.
            let note = format!("\n\n请只输出符合以下 JSON Schema 的 JSON：\n{schema}");
            let last_text = req
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.role == Role::User)
                .and_then(|m| {
                    m.parts.iter_mut().rev().find_map(|p| match p {
                        Part::Text(t) => Some(t),
                        _ => None,
                    })
                });
            match last_text {
                Some(t) => t.push_str(&note),
                None => req.system.push_str(&note),
            }
        } else {
            return Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                "该模型不支持结构化输出（JSON Schema / JSON 模式），无法使用。",
            ));
        }

        let mut total_usage = Usage::default();
        let (raw, u, mut stop) = self
            .complete_text_with_stop(
                profile,
                &req,
                cancel.clone(),
                &mut on_thinking,
                &mut on_text,
            )
            .await?;
        total_usage.add(&u);

        trace_llm_text(schema_name, "first", &raw);
        let mut candidate = strip_code_fences(&raw);
        let mut last_reason = String::new();

        // Length cut-off on an item array: keep the finished items and continue.
        if let (Some(key), true) = (array_key, matches!(stop, StopReason::MaxTokens)) {
            if serde_json::from_str::<serde_json::Value>(&candidate).is_err() {
                let mut items = salvage_array_items(&candidate, key);
                let mut rounds = 0;
                while matches!(stop, StopReason::MaxTokens) && rounds < 3 && !items.is_empty() {
                    rounds += 1;
                    on_status(&bilingual_status(
                        &format!(
                            "输出达到长度上限被截断，已保留 {} 个完整条目，正在请求从此继续（第 {rounds}/3 次）…",
                            items.len()
                        ),
                        &format!(
                            "Output was cut off at the length limit; kept {} complete item(s) and asking the model to continue (attempt {rounds}/3)…",
                            items.len()
                        ),
                    ));
                    let mut cont = req.clone();
                    let done_list = items
                        .iter()
                        .enumerate()
                        .map(|(idx, it)| {
                            let id = it.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let label = it
                                .get("label")
                                .or_else(|| it.get("title"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            match (id.is_empty(), label.is_empty()) {
                                (false, false) => format!("[{id}] {label}"),
                                (false, true) => format!("[{id}]"),
                                (true, false) => format!("#{} {label}", idx + 1),
                                (true, true) => format!("#{}", idx + 1),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    cont.messages.push(LlmMessage {
                        role: Role::Assistant,
                        parts: vec![Part::Text(serde_json::json!({ key: items }).to_string())],
                    });
                    cont.messages.push(LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Your previous output was cut off by the output length limit. These items are already complete and kept: {done_list}. Continue: output ONLY the REMAINING items (do not repeat any item above) as the same JSON object with the \"{key}\" array. Keep them concise; if everything is already covered, return an empty array."
                        ))],
                    });
                    let (more_raw, mu, mstop) = self
                        .complete_text_with_stop(
                            profile,
                            &cont,
                            cancel.clone(),
                            &mut on_thinking,
                            |_| {},
                        )
                        .await?;
                    total_usage.add(&mu);
                    stop = mstop;
                    let more_text = strip_code_fences(&more_raw);
                    let more = match serde_json::from_str::<serde_json::Value>(&more_text) {
                        Ok(v) => v
                            .get(key)
                            .and_then(|a| a.as_array())
                            .cloned()
                            .unwrap_or_default(),
                        Err(_) => salvage_array_items(&more_text, key),
                    };
                    if more.is_empty() {
                        break;
                    }
                    items.extend(more);
                }
                if !items.is_empty() {
                    candidate = serde_json::json!({ key: items }).to_string();
                }
            }
        }

        // The whole output budget went into reasoning and no answer was written. Repairing an
        // empty text is pointless, so the original request is asked again with an instruction
        // to think briefly (the per-request budget is fixed, so it cannot be raised).
        let mut empty_retries = 0;
        while candidate.trim().is_empty()
            && matches!(stop, StopReason::MaxTokens)
            && empty_retries < 2
        {
            empty_retries += 1;
            on_status(&bilingual_status(
                &format!("模型把输出额度全部用在了思考上，没有写出任何内容，正在要求它缩短思考后重试（第 {empty_retries}/2 次）…"),
                &format!("The model spent its whole output budget on reasoning and wrote nothing; asking again with a shorter-thinking instruction (attempt {empty_retries}/2)…"),
            ));
            let mut retry = req.clone();
            let note = "

（上一次尝试把输出额度全部用在了思考上，没有输出内容。这一次请把思考压缩到最少，立刻输出完整的 JSON。）";
            let last_text = retry
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.role == Role::User)
                .and_then(|m| {
                    m.parts.iter_mut().rev().find_map(|p| match p {
                        Part::Text(t) => Some(t),
                        _ => None,
                    })
                });
            match last_text {
                Some(t) => t.push_str(note),
                None => retry.system.push_str(note),
            }
            let (again, au, astop) = self
                .complete_text_with_stop(
                    profile,
                    &retry,
                    cancel.clone(),
                    &mut on_thinking,
                    &mut on_text,
                )
                .await?;
            total_usage.add(&au);
            stop = astop;
            candidate = strip_code_fences(&again);
        }

        for repair_attempt in 0..=max_repairs {
            // Nothing was written, so there is nothing to repair: asking a "repairer" to fix an
            // empty text makes it invent content that merely fits the schema.
            if candidate.trim().is_empty() {
                last_reason = if matches!(stop, StopReason::MaxTokens) {
                    "the model used its whole output budget on reasoning and wrote nothing".into()
                } else {
                    "the model returned an empty answer".into()
                };
                break;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&candidate) {
                if let Some(reason) = abort_reason(&val) {
                    return Err(LnaError::new(
                        ErrorCode::ModelRefused,
                        format!("模型主动终止了任务：{reason}"),
                    ));
                }
            }
            match serde_json::from_str::<serde_json::Value>(&candidate) {
                Ok(mut val) => {
                    strip_empty_abort(&mut val);
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
            let truncated = matches!(stop, StopReason::MaxTokens);
            let reason_txt = last_reason.chars().take(160).collect::<String>();
            on_status(&bilingual_status(
                &format!(
                    "结构化输出未通过校验（{}{}），正在进行第 {}/{} 次修复…",
                    if truncated {
                        "输出被长度上限截断；"
                    } else {
                        ""
                    },
                    reason_txt,
                    repair_attempt + 1,
                    max_repairs
                ),
                &format!(
                    "Structured output failed validation ({}{}); repair attempt {}/{}…",
                    if truncated {
                        "output was cut off at the length limit; "
                    } else {
                        ""
                    },
                    reason_txt,
                    repair_attempt + 1,
                    max_repairs
                ),
            ));
            let repair_req = LlmRequest::simple(
                "You are a strict JSON repairer. Output only valid JSON that strictly conforms to the provided JSON Schema, with no commentary or code fences.",
                format!(
                    "The following JSON failed schema validation ({}).{} Fix it and output valid JSON only:\nSchema:\n{}\n\nOriginal output:\n{}",
                    last_reason,
                    if truncated { " It was also cut off by the output length limit, so output a SHORTER but complete version." } else { "" },
                    schema, candidate
                ),
                base_req.max_output_tokens,
            )
            .with_output(req.output.clone());
            let (repaired, ru, rstop) = self
                .complete_text_with_stop(
                    profile,
                    &repair_req,
                    cancel.clone(),
                    &mut on_thinking,
                    |_| {},
                )
                .await?;
            total_usage.add(&ru);
            stop = rstop;
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
                .header(
                    "X-Goog-Upload-Header-Content-Length",
                    bytes.len().to_string(),
                )
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

/// Debug aid: with `LNA_LLM_TRACE_DIR` set, the raw text of every structured answer is saved there.
fn trace_llm_text(schema_name: &str, stage: &str, text: &str) {
    let Ok(dir) = std::env::var("LNA_LLM_TRACE_DIR") else {
        return;
    };
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        Path::new(&dir).join(format!("{ms}-{schema_name}-{stage}.txt")),
        text,
    );
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

/// Wait before the retry after the `try_no`-th throttled attempt: Retry-After when the endpoint
/// gave one, else `base` doubled per try (1s, 2s, 4s, ...). Never more than `MAX_THROTTLE_WAIT`.
fn throttle_delay(try_no: u32, retry_after_secs: Option<u64>, base: Duration) -> Duration {
    let wait = match retry_after_secs {
        Some(s) => Duration::from_secs(s),
        None => base.saturating_mul(1u32 << try_no.saturating_sub(1).min(16)),
    };
    wait.min(MAX_THROTTLE_WAIT)
}

/// Sleeps for `delay`, or returns a Cancelled error as soon as `cancel` fires.
async fn sleep_cancellable(delay: Duration, cancel: &CancellationToken) -> LnaResult<()> {
    tokio::select! {
        _ = cancel.cancelled() => Err(LnaError::new(ErrorCode::Cancelled, "Cancelled while waiting for the endpoint to accept the request")),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

/// Complete items of the flat array under `key` in possibly truncated JSON text: parses
/// element by element and stops at the first incomplete one.
pub(crate) fn salvage_array_items(text: &str, key: &str) -> Vec<serde_json::Value> {
    let needle = format!("\"{key}\"");
    let Some(kpos) = text.find(&needle) else {
        return Vec::new();
    };
    let after = &text[kpos + needle.len()..];
    let Some(bpos) = after.find('[') else {
        return Vec::new();
    };
    let mut rest = &after[bpos + 1..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() || rest.starts_with(']') {
            break;
        }
        let mut it = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        match it.next() {
            Some(Ok(v)) => {
                let used = it.byte_offset();
                out.push(v);
                rest = &rest[used..];
            }
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod salvage_tests {
    use super::salvage_array_items;

    #[test]
    fn keeps_complete_items_of_truncated_array() {
        let t =
            r#"{"title":"x","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","lab"#;
        let items = salvage_array_items(t, "nodes");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["id"], "b");
        assert!(salvage_array_items("{}", "nodes").is_empty());
    }
}

#[cfg(test)]
mod warm_tests {
    use super::*;

    fn key(tag: &str) -> PrefixKey {
        PrefixKey {
            profile_id: "p".into(),
            model_id: "m".into(),
            tool_names: vec![],
            system: "sys".into(),
            shared_material: tag.into(),
        }
    }

    #[tokio::test]
    async fn first_request_of_a_prefix_leads_and_siblings_wait_for_it() {
        let c = LlmClient::new();
        let mut leader = match c.warm_role(Some(key("A"))) {
            WarmRole::Leader(l) => l,
            _ => panic!("the first request of a prefix leads"),
        };
        let mut sibling = match c.warm_role(Some(key("A"))) {
            WarmRole::Follower(rx) => rx,
            _ => panic!("a sibling sent at the same time waits for the leader"),
        };
        // a different material prefix never collides or waits
        assert!(matches!(c.warm_role(Some(key("B"))), WarmRole::Leader(_)));
        assert!(!*sibling.borrow());
        leader.release();
        sibling.wait_for(|d| *d).await.unwrap();
        // once released the prefix counts as cached: later requests go straight through
        assert!(matches!(c.warm_role(Some(key("A"))), WarmRole::Free));
        // prompts that are too short to be cached are never held back
        assert!(matches!(c.warm_role(None), WarmRole::Free));
    }

    #[tokio::test]
    async fn a_leader_that_fails_still_releases_its_siblings() {
        let c = LlmClient::new();
        let leader = match c.warm_role(Some(key("X"))) {
            WarmRole::Leader(l) => l,
            _ => panic!(),
        };
        let mut rx = match c.warm_role(Some(key("X"))) {
            WarmRole::Follower(rx) => rx,
            _ => panic!(),
        };
        drop(leader);
        rx.wait_for(|d| *d).await.unwrap();
    }

    #[test]
    fn shared_material_slice_splits_at_task_markers() {
        assert_eq!(
            shared_material_slice("Sources: ABC\n\n=== THIS BATCH (1/3) ==="),
            "Sources: ABC"
        );
        assert_eq!(
            shared_material_slice("Sources: ABC\n\n=== TASK ===\nDo X"),
            "Sources: ABC"
        );
        assert_eq!(
            shared_material_slice("Sources: ABC\n\nPART 1 OF 3"),
            "Sources: ABC"
        );
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    async fn acquire(c: &LlmClient, profile_id: &str) -> Option<GateSlot> {
        tokio::time::timeout(
            Duration::from_millis(100),
            c.acquire_slot(profile_id, &CancellationToken::new()),
        )
        .await
        .expect("a slot was not free")
        .unwrap()
    }

    #[tokio::test]
    async fn unlimited_profile_hands_out_slots_without_waiting() {
        let c = LlmClient::new();
        c.set_parallel_limit("p", 0);
        let mut held = Vec::new();
        for _ in 0..10 {
            held.push(acquire(&c, "p").await.expect("a gated profile"));
        }
        assert_eq!(c.parallel_limit("p"), MAX_PARALLEL_REQUESTS as usize);
    }

    #[tokio::test]
    async fn a_throttle_caps_the_profile_below_the_requests_in_flight() {
        let c = LlmClient::new();
        c.set_parallel_limit("p", 0);
        let mut held = vec![
            acquire(&c, "p").await.unwrap(),
            acquire(&c, "p").await.unwrap(),
            acquire(&c, "p").await.unwrap(),
        ];
        // throttled with 3 in flight: the cap becomes 2
        held[0].throttled();
        held.pop();
        // 2 in flight: still full
        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            c.acquire_slot("p", &CancellationToken::new()),
        )
        .await;
        assert!(
            blocked.is_err(),
            "a third request must wait for the learned cap"
        );
        held.pop();
        // 1 in flight: a slot is free again
        held.push(acquire(&c, "p").await.unwrap());
        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            c.acquire_slot("p", &CancellationToken::new()),
        )
        .await;
        assert!(blocked.is_err(), "the learned cap of 2 still holds");
    }

    #[tokio::test]
    async fn successes_raise_the_learned_cap_up_to_the_configured_limit() {
        let c = LlmClient::new();
        c.set_parallel_limit("q", 3);
        let held = vec![
            acquire(&c, "q").await.unwrap(),
            acquire(&c, "q").await.unwrap(),
            acquire(&c, "q").await.unwrap(),
        ];
        held[0].throttled(); // learned cap 2
        let gate = c.gates.lock().unwrap().get("q").unwrap().clone();
        for _ in 0..SUCCESSES_TO_RAISE_CAP {
            held[1].succeeded();
        }
        // raised to the configured 3: the learned cap is dropped
        assert_eq!(gate.lock().learned, None);
        drop(held);
        assert_eq!(gate.lock().in_flight, 0);
    }
}

/// Appended to the system prompt of every structured request.
const ABORT_NOTE: &str = "

如果这项任务确实无法完成（例如材料缺失或无法读取、图片看不见、要求自相矛盾），不要编造内容：在 JSON 根对象里只写一个字符串字段 \"abort_reason\"，说明原因，其余字段可省略。";

/// `schema` with the optional root property `abort_reason` added (when the root lists properties).
/// It goes FIRST: the model writes fields in schema order, so it decides whether to give up
/// before it writes any content (placed last it would be filled with a placeholder instead).
fn with_abort_property(schema: &serde_json::Value) -> serde_json::Value {
    let mut out = schema.clone();
    if let Some(props) = out.get_mut("properties").and_then(|p| p.as_object_mut()) {
        if !props.contains_key("abort_reason") {
            let mut ordered = serde_json::Map::new();
            ordered.insert("abort_reason".into(), serde_json::json!({"type": "string"}));
            ordered.append(props);
            *props = ordered;
        }
    }
    out
}

/// Drops an empty `abort_reason` ("I am not giving up") so the answer fits the original schema.
fn strip_empty_abort(v: &mut serde_json::Value) {
    if let Some(o) = v.as_object_mut() {
        if o.get("abort_reason")
            .and_then(|r| r.as_str())
            .is_some_and(|r| r.trim().is_empty())
        {
            o.remove("abort_reason");
        }
    }
}

/// The model's own reason for giving up, when it wrote one.
fn abort_reason(v: &serde_json::Value) -> Option<String> {
    let r = v.get("abort_reason")?.as_str()?.trim();
    (!r.is_empty()).then(|| r.to_string())
}

/// A structured answer written outside `complete_structured_continuing` (e.g. the final
/// reply of a tool loop) checked against `schema`: `Ok(Some(value))` when it is valid,
/// `Ok(None)` when it is not (the caller asks again), `Err(ModelRefused)` when it is an abort.
pub fn parse_structured(
    schema: &serde_json::Value,
    raw: &str,
) -> LnaResult<Option<serde_json::Value>> {
    let candidate = strip_code_fences(raw);
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&candidate) else {
        return Ok(None);
    };
    if let Some(reason) = abort_reason(&val) {
        return Err(LnaError::new(
            ErrorCode::ModelRefused,
            format!("模型主动终止了任务：{reason}"),
        ));
    }
    strip_empty_abort(&mut val);
    let validator = jsonschema::validator_for(schema)
        .map_err(|e| LnaError::invalid(format!("Invalid JSON schema: {e}")))?;
    Ok(validator.is_valid(&val).then_some(val))
}

#[cfg(test)]
mod abort_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn abort_property_is_optional_and_detected() {
        let s = with_abort_property(
            &json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}),
        );
        assert!(s["properties"]["abort_reason"].is_object());
        assert_eq!(s["required"], json!(["a"]));
        assert_eq!(
            abort_reason(&json!({"abort_reason":" 看不到图 "})).as_deref(),
            Some("看不到图")
        );
        assert_eq!(abort_reason(&json!({"abort_reason":"  "})), None);
        assert_eq!(abort_reason(&json!({"a":1})), None);
    }

    #[test]
    fn parse_structured_accepts_valid_rejects_invalid_and_reports_abort() {
        let schema = json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"]});
        assert_eq!(
            parse_structured(&schema, "{\"a\":\"x\"}").unwrap(),
            Some(json!({"a":"x"}))
        );
        assert_eq!(
            parse_structured(&schema, "```json\n{\"a\":\"y\"}\n```").unwrap(),
            Some(json!({"a":"y"}))
        );
        assert_eq!(parse_structured(&schema, "DONE").unwrap(), None);
        assert_eq!(parse_structured(&schema, "{\"b\":1}").unwrap(), None);
        let e = parse_structured(&schema, "{\"abort_reason\":\"图看不清\"}").unwrap_err();
        assert_eq!(e.code, ErrorCode::ModelRefused);
        assert!(e.message.contains("图看不清"));
        // an empty abort_reason is "not giving up" and must not break a closed schema
        let closed = json!({"type":"object","additionalProperties":false,"properties":{"a":{"type":"string"}},"required":["a"]});
        assert_eq!(
            parse_structured(&closed, "{\"abort_reason\":\"\",\"a\":\"x\"}").unwrap(),
            Some(json!({"a":"x"}))
        );
    }

    #[test]
    fn abort_reason_is_the_first_property_the_model_writes() {
        let s = with_abort_property(&json!({"type":"object","properties":{"z":{},"a":{}}}));
        let keys: Vec<&str> = s["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["abort_reason", "z", "a"]);
    }
}

/// One tool call collected from an assistant turn stream.
#[derive(Debug, Clone)]
pub struct CollectedToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
}

/// Aggregated result of a single streamed assistant turn.
#[derive(Debug, Clone)]
pub struct TurnOutput {
    pub text: String,
    pub tool_calls: Vec<CollectedToolCall>,
    pub opaque: Vec<Part>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

/// Next item from `stream`, or `Cancelled` as soon as `cancel` fires.
pub async fn next_or_cancel<S>(
    stream: &mut S,
    cancel: &CancellationToken,
) -> LnaResult<Option<S::Item>>
where
    S: futures_util::Stream + Unpin,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(LnaError::new(ErrorCode::Cancelled, "Cancelled")),
        item = stream.next() => Ok(item),
    }
}

/// Runs `make` over `items` concurrently with at most `limit` futures in flight,
/// returning outputs in original input order. Stops on the first error or cancellation.
pub async fn run_parallel_ordered<T, O, F, Fut>(
    items: Vec<T>,
    limit: usize,
    cancel: &CancellationToken,
    mut make: F,
) -> LnaResult<Vec<O>>
where
    F: FnMut(usize, T) -> Fut,
    Fut: std::future::Future<Output = LnaResult<O>>,
{
    let total = items.len();
    if total == 0 {
        return Ok(Vec::new());
    }
    let futs: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let fut = make(i, item);
            async move { (i, fut.await) }
        })
        .collect();
    let mut running = futures_util::stream::iter(futs).buffer_unordered(limit.clamp(1, total));
    let mut out: Vec<Option<O>> = (0..total).map(|_| None).collect();
    while let Some((i, res)) = next_or_cancel(&mut running, cancel).await? {
        out[i] = Some(res?);
    }
    Ok(out.into_iter().flatten().collect())
}

/// Runs `make` over `items` concurrently with at most `limit` futures in flight,
/// passing each result to `on_done` as soon as it finishes (ideal for incremental checkpoints).
pub async fn run_parallel_unordered<T, O, F, Fut, H>(
    items: Vec<T>,
    limit: usize,
    cancel: &CancellationToken,
    mut make: F,
    mut on_done: H,
) -> LnaResult<()>
where
    F: FnMut(usize, T) -> Fut,
    Fut: std::future::Future<Output = O>,
    H: FnMut(usize, O) -> LnaResult<()>,
{
    let total = items.len();
    if total == 0 {
        return Ok(());
    }
    let futs: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let fut = make(i, item);
            async move { (i, fut.await) }
        })
        .collect();
    let mut running = futures_util::stream::iter(futs).buffer_unordered(limit.clamp(1, total));
    while let Some((i, val)) = next_or_cancel(&mut running, cancel).await? {
        on_done(i, val)?;
    }
    Ok(())
}

#[cfg(test)]
mod parallel_and_cap_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn run_parallel_ordered_preserves_order_and_bounds_concurrency() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let items = vec![10u32, 20, 30, 40, 50];
        let out = run_parallel_ordered(items, 2, &CancellationToken::new(), |_, v| {
            let active = active.clone();
            let peak = peak.clone();
            async move {
                let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(15)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(v * 2)
            }
        })
        .await
        .unwrap();
        assert_eq!(out, vec![20, 40, 60, 80, 100]);
        assert!(peak.load(Ordering::SeqCst) <= 2);
    }

    #[test]
    fn hard_output_cap_bounds_unlimited_profiles() {
        let mut p = ResolvedProfile {
            id: "p".into(),
            display_name: "p".into(),
            protocol: ProtocolKind::Gemini,
            base_url: "https://example.com".into(),
            model_id: "m".into(),
            api_key: None,
            show_thinking: false,
            effort: None,
            context_window: 128_000,
            max_output_tokens: 0,
            capabilities: Default::default(),
            extra_headers: Default::default(),
            anthropic_version: "2023-06-01".into(),
            preset: None,
            shrink_media: false,
        };
        assert_eq!(p.bounded_output(32_768), 0);
        assert_eq!(p.hard_output_cap(32_768), 32_768);
        p.max_output_tokens = 65_536;
        assert_eq!(p.hard_output_cap(32_768), 32_768);
        p.max_output_tokens = 8_192;
        assert_eq!(p.hard_output_cap(32_768), 8_192);
    }
}
