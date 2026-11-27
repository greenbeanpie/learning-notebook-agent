use async_trait::async_trait;
use lna_types::{LnaError, LnaResult, ModelCapabilities, ProtocolKind};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Output-token limit used when a provider requires `max_tokens` and the profile has no limit
/// (`max_output_tokens == 0`).
pub const UNLIMITED_OUTPUT_FALLBACK_TOKENS: u32 = 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderOpaque {
    pub protocol: ProtocolKind,
    pub model_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text(String),
    Media {
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_base64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio_format: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
    Opaque(ProviderOpaque),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    None,
    Auto,
}

#[derive(Debug, Clone)]
pub enum OutputMode {
    Text,
    JsonSchema {
        name: String,
        schema: serde_json::Value,
    },
    JsonObject,
}

#[derive(Debug, Clone, Default)]
pub struct ReasoningOptions {
    pub show_summary: bool,
    pub effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system: String,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<ToolDef>,
    pub tool_mode: ToolMode,
    pub output: OutputMode,
    pub max_output_tokens: u32,
    pub reasoning: ReasoningOptions,
}

impl LlmRequest {
    /// Single-turn text request with no tools, `OutputMode::Text`, and default reasoning.
    pub fn simple(
        system: impl Into<String>,
        user_text: impl Into<String>,
        max_output_tokens: u32,
    ) -> Self {
        Self::from_parts(
            system,
            vec![Part::Text(user_text.into())],
            max_output_tokens,
        )
    }

    /// Single-turn request with arbitrary user parts (e.g. media + text).
    pub fn from_parts(system: impl Into<String>, parts: Vec<Part>, max_output_tokens: u32) -> Self {
        Self::conversation(
            system,
            vec![LlmMessage {
                role: Role::User,
                parts,
            }],
            max_output_tokens,
        )
    }

    /// Multi-turn request with no tools, `OutputMode::Text`, and default reasoning.
    pub fn conversation(
        system: impl Into<String>,
        messages: Vec<LlmMessage>,
        max_output_tokens: u32,
    ) -> Self {
        Self {
            system: system.into(),
            messages,
            tools: Vec::new(),
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens,
            reasoning: ReasoningOptions::default(),
        }
    }

    pub fn with_output(mut self, output: OutputMode) -> Self {
        self.output = output;
        self
    }

    pub fn with_reasoning(mut self, reasoning: ReasoningOptions) -> Self {
        self.reasoning = reasoning;
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDef>, tool_mode: ToolMode) -> Self {
        self.tools = tools;
        self.tool_mode = tool_mode;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(other.cached_tokens);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    ContentFilter,
    Refusal,
    Other(String),
}

#[derive(Debug, Clone)]
pub enum LlmEvent {
    ThoughtDelta(String),
    TextDelta(String),
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: u32,
        json: String,
    },
    ToolCallEnd {
        index: u32,
    },
    Opaque(ProviderOpaque),
    Usage(Usage),
    Finished {
        reason: StopReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// Resolved profile holding SecretString in memory only (never Serialize or Debug).
#[derive(Clone)]
pub struct ResolvedProfile {
    pub id: String,
    pub display_name: String,
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub model_id: String,
    pub api_key: Option<SecretString>,
    pub show_thinking: bool,
    pub effort: Option<String>,
    pub context_window: u32,
    /// 0 = no output limit (the provider decides).
    pub max_output_tokens: u32,
    pub capabilities: ModelCapabilities,
    pub extra_headers: BTreeMap<String, String>,
    pub anthropic_version: String,
    /// Provider preset id the profile was created from (e.g. "mimo"); `None` = custom.
    pub preset: Option<String>,
    /// Trade picture quality for capacity when sending media.
    pub shrink_media: bool,
}

/// Placeholder an extra-header value may contain; it is replaced by the profile's API key.
pub const API_KEY_PLACEHOLDER: &str = "{api_key}";

impl ResolvedProfile {
    pub fn trimmed_base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// True when the profile was created from the MiMo preset.
    pub fn is_mimo(&self) -> bool {
        self.preset.as_deref() == Some("mimo")
    }

    /// Output-token budget for prompt sizing and chunking. `max_output_tokens == 0` means no
    /// limit was set, so budget a quarter of the context window (at most 32000, at least 1024).
    pub fn output_budget(&self) -> u32 {
        if self.max_output_tokens != 0 {
            self.max_output_tokens
        } else {
            (self.context_window / 4).clamp(1024, UNLIMITED_OUTPUT_FALLBACK_TOKENS)
        }
    }

    /// Output limit for a request that only needs `cap` tokens of answer. A profile with a limit
    /// keeps the smaller of the two; a profile set to "no limit" (`0`) stays unlimited, because
    /// a reasoning model spends part of any cap on thinking and may write nothing at all.
    pub fn bounded_output(&self, cap: u32) -> u32 {
        if self.max_output_tokens == 0 {
            0
        } else {
            self.max_output_tokens.min(cap)
        }
    }

    /// Hard output-token ceiling: even when the profile is configured with no limit (`0`),
    /// never exceed `cap`. Prevents runaway outputs on long-form generation tasks.
    pub fn hard_output_cap(&self, cap: u32) -> u32 {
        if self.max_output_tokens == 0 {
            cap
        } else {
            self.max_output_tokens.min(cap)
        }
    }

    /// Reasoning options derived from this profile's settings.
    pub fn reasoning_options(&self) -> ReasoningOptions {
        ReasoningOptions {
            show_summary: self.show_thinking,
            effort: self.effort.clone(),
        }
    }

    /// True when some extra header carries the key itself (e.g. `cf-aig-authorization: Bearer
    /// {api_key}`). Such a profile must not also send the key in the protocol's default auth
    /// header: a gateway would forward it to the upstream provider as that provider's key.
    pub fn key_in_extra_headers(&self) -> bool {
        self.extra_headers
            .values()
            .any(|v| v.contains(API_KEY_PLACEHOLDER))
    }

    /// The key to send in the protocol's default auth header (none when it travels in a custom one).
    pub fn default_auth_key(&self) -> Option<&str> {
        use secrecy::ExposeSecret;
        if self.key_in_extra_headers() {
            return None;
        }
        self.api_key
            .as_ref()
            .map(|k| k.expose_secret())
            .filter(|s| !s.is_empty())
    }

    /// Extra headers with `{api_key}` substituted (values with the placeholder are skipped when
    /// no key is set).
    pub fn resolved_extra_headers(&self) -> Vec<(String, String)> {
        use secrecy::ExposeSecret;
        let key = self
            .api_key
            .as_ref()
            .map(|k| k.expose_secret().to_string())
            .filter(|s| !s.is_empty());
        let mut headers: Vec<(String, String)> = self
            .extra_headers
            .iter()
            .filter_map(|(k, v)| {
                if v.contains(API_KEY_PLACEHOLDER) {
                    key.as_ref()
                        .map(|key| (k.clone(), v.replace(API_KEY_PLACEHOLDER, key)))
                } else {
                    Some((k.clone(), v.clone()))
                }
            })
            .collect();
        headers.extend(self.gateway_cache_headers());
        headers
    }

    /// Cloudflare AI Gateway may cache responses by request body. A generation that failed (cut
    /// off, invalid JSON) would then come back identical on every retry, so the cache is skipped
    /// unless the profile sets the header itself.
    pub fn gateway_cache_headers(&self) -> Vec<(String, String)> {
        let on_gateway = self.base_url.contains("gateway.ai.cloudflare.com");
        let set = self.extra_headers.keys().any(|k| {
            k.eq_ignore_ascii_case("cf-aig-skip-cache")
                || k.eq_ignore_ascii_case("cf-aig-cache-ttl")
        });
        if on_gateway && !set {
            vec![("cf-aig-skip-cache".to_string(), "true".to_string())]
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

pub trait StreamDecoder: Send {
    fn on_frame(&mut self, frame: SseFrame) -> LnaResult<Vec<LlmEvent>>;
    fn on_eof(&mut self) -> LnaResult<Vec<LlmEvent>>;
    /// Text the decoder is still holding back (e.g. a possible `<think>` tag prefix). Called when
    /// the stream breaks so already-received text is not lost.
    fn flush(&mut self) -> Vec<LlmEvent> {
        Vec::new()
    }
}

#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    fn build(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
        req: &LlmRequest,
    ) -> LnaResult<reqwest::Request>;
    fn decoder(&self, model_id: &str) -> Box<dyn StreamDecoder>;
    async fn discover(
        &self,
        client: &reqwest::Client,
        profile: &ResolvedProfile,
    ) -> LnaResult<Vec<ModelInfo>>;
}

pub fn map_http_status(status: reqwest::StatusCode, body: &str) -> LnaError {
    use lna_types::ErrorCode;
    let code = match status.as_u16() {
        401 | 403 => ErrorCode::ModelAuthFailed,
        404 => ErrorCode::ModelNotFound,
        429 => ErrorCode::ModelRateLimited,
        400 | 422 => {
            let lower = body.to_lowercase();
            if lower.contains("context") || lower.contains("too long") || lower.contains("maximum")
            {
                ErrorCode::ModelContextExceeded
            } else {
                ErrorCode::ModelBadRequest
            }
        }
        500..=599 => ErrorCode::ModelOverloaded,
        _ => ErrorCode::ModelBadRequest,
    };
    LnaError::new(
        code,
        format!("HTTP {}: {}", status.as_u16(), truncate(body, 400)),
    )
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}
