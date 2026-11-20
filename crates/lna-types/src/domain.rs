use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    ChatCompletion,
    Responses,
    Anthropic,
    Gemini,
}

impl ProtocolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChatCompletion => "chat_completion",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Pdf,
    Md,
    Txt,
    Web,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceStatus {
    Queued,
    Parsing,
    Chunking,
    Embedding,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MessageStatus {
    Streaming,
    Complete,
    Incomplete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Audio,
    Slides,
    Mindmap,
    Report,
    Flashcards,
    Quiz,
    Infographic,
}

impl ArtifactType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Slides => "slides",
            Self::Mindmap => "mindmap",
            Self::Report => "report",
            Self::Flashcards => "flashcards",
            Self::Quiz => "quiz",
            Self::Infographic => "infographic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArtifactStatus {
    Pending,
    Running,
    DraftReview,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub id: String,
    pub title: String,
    pub emoji: String,
    pub summary_md: Option<String>,
    pub summary_fingerprint: Option<String>,
    pub suggested_questions: Vec<String>,
    pub title_user_edited: bool,
    pub source_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    pub kind: SourceKind,
    pub origin_uri: Option<String>,
    pub blob_path: Option<String>,
    pub content_hash: String,
    pub byte_size: i64,
    pub page_count: Option<i64>,
    pub token_count: i64,
    pub language: Option<String>,
    pub is_selected: bool,
    pub status: SourceStatus,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceChunk {
    pub id: i64,
    pub source_id: String,
    pub notebook_id: String,
    pub chunk_index: i32,
    pub section_path: Option<String>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub char_start: i32,
    pub char_end: i32,
    pub content: String,
    pub token_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelCapabilities {
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub json_schema: bool,
    #[serde(default)]
    pub json_mode: bool,
    #[serde(default)]
    pub reasoning_summary: bool,
    #[serde(default)]
    pub stream_usage: bool,
    #[serde(default)]
    pub max_tokens_field: Option<String>,
    #[serde(default)]
    pub anthropic_thinking: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    pub display_name: String,
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub model_id: String,
    pub keychain_ref: Option<String>,
    pub has_secret: bool,
    pub show_thinking: bool,
    pub effort: Option<String>,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub capabilities: ModelCapabilities,
    pub extra_headers: BTreeMap<String, String>,
    pub token_ratio: f64,
    pub input_price_per_m: Option<f64>,
    pub output_price_per_m: Option<f64>,
    pub is_active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThoughtStep {
    pub step_id: String,
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationRef {
    pub no: i32,
    pub chunk_id: Option<i64>,
    pub source_id: Option<String>,
    pub source_title: String,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub section_path: Option<String>,
    pub quote: String,
    #[serde(default)]
    pub source_removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub notebook_id: String,
    pub role: String,
    pub status: MessageStatus,
    pub content: String,
    pub thoughts: Vec<ThoughtStep>,
    pub thought_summary: Option<String>,
    pub citations: Vec<CitationRef>,
    pub followups: Vec<String>,
    pub profile_id: Option<String>,
    pub stop_reason: Option<String>,
    pub error_code: Option<String>,
    pub feedback: Option<i32>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub notebook_id: String,
    pub title: String,
    pub body_md: String,
    pub origin: String,
    pub origin_message_id: Option<String>,
    pub citations_json: Option<String>,
    pub converted_source_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioArtifact {
    pub id: String,
    pub notebook_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub status: ArtifactStatus,
    pub stage: Option<String>,
    pub progress: f64,
    pub config: serde_json::Value,
    pub source_ids: Vec<String>,
    pub sources_fingerprint: String,
    pub profile_id: Option<String>,
    pub draft: Option<serde_json::Value>,
    pub payload: Option<serde_json::Value>,
    pub output_dir: Option<String>,
    pub estimated_input_tokens: Option<i64>,
    pub estimated_output_tokens: Option<i64>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub feedback: Option<i32>,
    pub sources_stale: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub estimated_cost_usd: Option<f64>,
}
