use crate::{LnaError, ModelCapabilities, ProtocolKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LnaError>,
}

impl ApiResponse {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: String, error: LnaError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEvent {
    pub seq: u64,
    pub topic: String,
    pub event: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpsertParams {
    pub id: Option<String>,
    pub display_name: String,
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    pub effort: Option<String>,
    #[serde(default = "default_ctx")]
    pub context_window: u32,
    #[serde(default = "default_out")]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    pub input_price_per_m: Option<f64>,
    pub output_price_per_m: Option<f64>,
    #[serde(default)]
    pub capabilities: Option<ModelCapabilities>,
    /// Ignored: image / audio / video support is detected by the probe, never taken from a client.
    /// Kept so that older clients still deserialize.
    #[serde(default)]
    pub supports_vision: Option<bool>,
    /// Ignored (see `supports_vision`).
    #[serde(default)]
    pub supports_audio: Option<bool>,
    /// Ignored (see `supports_vision`).
    #[serde(default)]
    pub supports_video: Option<bool>,
    #[serde(default)]
    pub activate: bool,
    /// Most requests of this profile in flight at once (0 = unlimited, else 1..=1024). `None`
    /// keeps the stored value (unlimited for a new profile).
    #[serde(default)]
    pub max_parallel: Option<u32>,
    /// Provider preset id (e.g. "mimo"). `None` keeps the stored value; `Some("")` clears it.
    #[serde(default)]
    pub preset: Option<String>,
    /// `None` keeps the stored value. Forced to false unless the preset is "mimo".
    #[serde(default)]
    pub shrink_media: Option<bool>,
}

fn default_true() -> bool {
    true
}
fn default_ctx() -> u32 {
    32_000
}
/// 0 = no output-token limit.
fn default_out() -> u32 {
    0
}
