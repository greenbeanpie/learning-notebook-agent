// Rust Core Engine (`notebook-core`) — 跨平台共享协议适配器骨架
// 可通过 UniFFI 编译为 Windows (.dll) / macOS (.dylib) / Linux (.so) 供 WinUI 3 / SwiftUI / GTK4 调用

use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tokio_stream::Stream;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    ChatCompletion,
    ResponsesApi,
    Anthropic,
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub model_id: String,
    pub api_key: String,
    pub enable_thinking: bool,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentStreamEvent {
    ThoughtDelta(String),
    ToolStatus { tool: String, detail: String },
    TextDelta(String),
    Usage { input_tokens: u32, output_tokens: u32 },
    Done,
    Error(String),
}

pub type EventStream = Pin<Box<dyn Stream<Item = AgentStreamEvent> + Send>>;

#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    fn protocol(&self) -> ProtocolKind;
    async fn stream_chat(
        &self,
        profile: &ModelProfile,
        system_prompt: &str,
        messages: &[ChatMessage],
    ) -> anyhow::Result<EventStream>;
}

/// 根据 ModelProfile.protocol 分发到对应的协议适配器
pub fn resolve_endpoint(profile: &ModelProfile) -> String {
    let base = profile.base_url.trim_end_matches('/');
    match profile.protocol {
        ProtocolKind::ChatCompletion => format!("{base}/chat/completions"),
        ProtocolKind::ResponsesApi => format!("{base}/responses"),
        ProtocolKind::Anthropic => format!("{base}/messages"),
        ProtocolKind::Gemini => format!(
            "{base}/models/{}:streamGenerateContent?alt=sse",
            profile.model_id
        ),
    }
}
