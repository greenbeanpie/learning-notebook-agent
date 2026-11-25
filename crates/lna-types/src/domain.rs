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
    Docx,
    Csv,
    Pptx,
    Epub,
    Odt,
    Rtf,
    Hwp,
    Hwpx,
    Image,
    Audio,
    Video,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Web => "web",
            Self::Note => "note",
            Self::Docx => "docx",
            Self::Csv => "csv",
            Self::Pptx => "pptx",
            Self::Epub => "epub",
            Self::Odt => "odt",
            Self::Rtf => "rtf",
            Self::Hwp => "hwp",
            Self::Hwpx => "hwpx",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "pdf" => Self::Pdf,
            "md" => Self::Md,
            "txt" => Self::Txt,
            "web" => Self::Web,
            "note" => Self::Note,
            "docx" => Self::Docx,
            "csv" => Self::Csv,
            "pptx" => Self::Pptx,
            "epub" => Self::Epub,
            "odt" => Self::Odt,
            "rtf" => Self::Rtf,
            "hwp" => Self::Hwp,
            "hwpx" => Self::Hwpx,
            "image" => Self::Image,
            "audio" => Self::Audio,
            "video" => Self::Video,
            _ => Self::Note,
        }
    }
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
    Lecture,
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
            Self::Lecture => "lecture",
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
    #[serde(default)]
    pub guide_summary_md: Option<String>,
    #[serde(default)]
    pub guide_topics: Vec<String>,
    #[serde(default)]
    pub ai_indexed: bool,
    #[serde(default)]
    pub recommend_knowledge_graph: bool,
    #[serde(default)]
    pub knowledge_graph_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceIndexNode {
    pub id: String,
    pub source_id: String,
    pub parent_id: Option<String>,
    pub ordinal: i32,
    pub level: i32,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub char_start: Option<i32>,
    pub char_end: Option<i32>,
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
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_audio: bool,
    #[serde(default)]
    pub supports_video: bool,
    /// When the tool/structured-output probe last ran (ms). `None`: never probed.
    #[serde(default)]
    pub probed_at: Option<i64>,
    /// Profile only serves an embedding endpoint; chat requirements do not apply.
    #[serde(default)]
    pub embedding_only: bool,
}

impl ModelCapabilities {
    /// Chat models must call tools and produce structured output (JSON schema or JSON mode).
    /// Returns the missing abilities in user-facing wording, or `None` when the model qualifies.
    pub fn chat_missing(&self) -> Option<String> {
        if self.embedding_only {
            return None;
        }
        let mut miss = Vec::new();
        if !self.tools {
            miss.push("工具调用（tool / function call）");
        }
        if !self.json_schema && !self.json_mode {
            miss.push("结构化输出（JSON Schema / JSON 模式）");
        }
        if miss.is_empty() {
            None
        } else {
            Some(format!("该模型不支持{}，无法使用。", miss.join("和")))
        }
    }

    /// Usable for chat-type work: qualifies, or has not been probed yet (probe pending).
    pub fn chat_usable(&self) -> bool {
        self.embedding_only || self.probed_at.is_none() || self.chat_missing().is_none()
    }
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
    #[serde(default)]
    pub artifacts: Vec<StudioArtifact>,
    /// Images the user attached to this message (user messages only).
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: i64,
}

/// Something the user attached to a chat message or a Studio request: a picture (downscaled), a
/// one-off reference file (kept as uploaded, read by the agent on demand) or a web link. Files
/// live under `<data root>/attachments/`; clients show or open them from `path` / `url`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub path: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub bytes: u64,
    /// `image` | `file` | `link` (older rows have no kind: they are images).
    #[serde(default = "default_attachment_kind")]
    pub kind: String,
    /// Web address of a `link` attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn default_attachment_kind() -> String {
    "image".into()
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.kind == "image"
    }
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
pub struct ArtifactThought {
    pub seq: i32,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSourceRef {
    pub id: String,
    pub title: String,
    pub kind: String,
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
    #[serde(default)]
    pub source_refs: Vec<ArtifactSourceRef>,
    pub sources_fingerprint: String,
    pub profile_id: Option<String>,
    pub draft: Option<serde_json::Value>,
    pub payload: Option<serde_json::Value>,
    pub output_dir: Option<String>,
    pub estimated_input_tokens: Option<i64>,
    pub estimated_output_tokens: Option<i64>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    #[serde(default)]
    pub error_log: Option<String>,
    #[serde(default)]
    pub thoughts: Vec<ArtifactThought>,
    pub feedback: Option<i32>,
    pub sources_stale: bool,
    #[serde(default = "default_true")]
    pub in_studio: bool,
    #[serde(default)]
    pub origin_message_id: Option<String>,
    #[serde(default)]
    pub progress_state: Option<serde_json::Value>,
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
