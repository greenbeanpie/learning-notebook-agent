use async_trait::async_trait;
use lna_types::{LnaError, LnaResult, ModelCapabilities, ProtocolKind};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
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
    pub max_output_tokens: u32,
    pub capabilities: ModelCapabilities,
    pub extra_headers: BTreeMap<String, String>,
    pub anthropic_version: String,
}

impl ResolvedProfile {
    pub fn trimmed_base(&self) -> &str {
        self.base_url.trim_end_matches('/')
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
