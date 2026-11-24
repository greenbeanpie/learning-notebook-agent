//! Cloud text-to-speech for natural voices (opt-in; the text is sent to the provider).
//!
//! Providers:
//! - `qwen`: Alibaba Cloud Model Studio (DashScope) Qwen-TTS, e.g. `qwen-tts`, `qwen3-tts-flash`.
//!   `POST {base}/api/v1/services/aigc/multimodal-generation/generation`, the response carries
//!   the audio as a URL (downloaded) or base64 data.
//! - `openai`: any OpenAI-compatible `POST {base}/audio/speech` endpoint returning WAV bytes.

use crate::{parse_wav_bytes, AudioInfo};
use lna_types::{ErrorCode, LnaError, LnaResult};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct CloudTts {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

pub const QWEN_DEFAULT_BASE: &str = "https://dashscope.aliyuncs.com";
pub const QWEN_DEFAULT_MODEL: &str = "qwen3-tts-flash";

/// Voices offered in the UI; any other voice name can be typed in.
pub fn known_voices(provider: &str) -> Vec<(&'static str, &'static str)> {
    match provider {
        "openai" => vec![("alloy", "Alloy"), ("nova", "Nova"), ("echo", "Echo"), ("shimmer", "Shimmer"), ("onyx", "Onyx"), ("fable", "Fable")],
        _ => vec![
            ("Cherry", "Cherry 芊悦（女声）"),
            ("Ethan", "Ethan 晨煦（男声）"),
            ("Serena", "Serena 苏瑶（女声）"),
            ("Chelsie", "Chelsie 千雪（女声）"),
            ("Dylan", "Dylan 北京话"),
            ("Sunny", "Sunny 四川话"),
            ("Jada", "Jada 上海话"),
        ],
    }
}

fn net_err(e: impl std::fmt::Display) -> LnaError {
    LnaError::new(ErrorCode::FetchFailed, format!("TTS request failed: {e}"))
}

impl CloudTts {
    fn client() -> LnaResult<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| LnaError::internal(e.to_string()))
    }

    /// Synthesizes `text` into a WAV file at `out`.
    pub async fn synthesize(&self, text: &str, voice: &str, out: &Path, cancel: CancellationToken) -> LnaResult<AudioInfo> {
        if text.trim().is_empty() {
            return Err(LnaError::invalid("Empty text"));
        }
        if self.api_key.trim().is_empty() {
            return Err(LnaError::new(ErrorCode::ModelAuthFailed, "Cloud TTS API key is not set"));
        }
        let fut = async {
            match self.provider.as_str() {
                "openai" => self.openai(text, voice).await,
                _ => self.qwen(text, voice).await,
            }
        };
        let bytes = tokio::select! {
            r = fut => r?,
            _ = cancel.cancelled() => return Err(LnaError::new(ErrorCode::Cancelled, "TTS cancelled")),
        };
        let wav = parse_wav_bytes(&bytes).map_err(|_| LnaError::new(ErrorCode::OutputInvalid, "TTS provider did not return WAV audio"))?;
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
        }
        let tmp = out.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|e| LnaError::internal(e.to_string()))?;
        std::fs::rename(&tmp, out).map_err(|e| LnaError::internal(e.to_string()))?;
        let frames = wav.samples.len() as u64 / wav.channels.max(1) as u64;
        Ok(AudioInfo { duration_ms: frames * 1000 / wav.sample_rate.max(1) as u64, sample_rate: wav.sample_rate })
    }

    async fn qwen(&self, text: &str, voice: &str) -> LnaResult<Vec<u8>> {
        let base = if self.base_url.trim().is_empty() { QWEN_DEFAULT_BASE } else { self.base_url.trim().trim_end_matches('/') };
        let url = if base.contains("/services/") { base.to_string() } else { format!("{base}/api/v1/services/aigc/multimodal-generation/generation") };
        let model = if self.model.trim().is_empty() { QWEN_DEFAULT_MODEL } else { self.model.trim() };
        let voice = if voice.trim().is_empty() { "Cherry" } else { voice.trim() };
        let body = json!({ "model": model, "input": { "text": text, "voice": voice } });
        let client = Self::client()?;
        let resp = client.post(&url).bearer_auth(self.api_key.trim()).json(&body).send().await.map_err(net_err)?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(net_err)?;
        if !status.is_success() {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
            let code = if status.as_u16() == 401 || status.as_u16() == 403 { ErrorCode::ModelAuthFailed } else { ErrorCode::FetchFailed };
            return Err(LnaError::new(code, format!("Qwen-TTS HTTP {}: {}", status.as_u16(), msg)));
        }
        let audio = v.pointer("/output/audio").cloned().unwrap_or(Value::Null);
        if let Some(data) = audio.get("data").and_then(Value::as_str).filter(|d| !d.is_empty()) {
            use base64::Engine as _;
            return base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| LnaError::new(ErrorCode::OutputInvalid, e.to_string()));
        }
        let audio_url = audio
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| LnaError::new(ErrorCode::OutputInvalid, "Qwen-TTS response has no audio"))?;
        let r = client.get(audio_url).send().await.map_err(net_err)?;
        if !r.status().is_success() {
            return Err(LnaError::new(ErrorCode::FetchFailed, format!("audio download HTTP {}", r.status().as_u16())));
        }
        Ok(r.bytes().await.map_err(net_err)?.to_vec())
    }

    async fn openai(&self, text: &str, voice: &str) -> LnaResult<Vec<u8>> {
        let base = if self.base_url.trim().is_empty() { "https://api.openai.com/v1" } else { self.base_url.trim().trim_end_matches('/') };
        let url = if base.ends_with("/audio/speech") { base.to_string() } else { format!("{base}/audio/speech") };
        let model = if self.model.trim().is_empty() { "gpt-4o-mini-tts" } else { self.model.trim() };
        let voice = if voice.trim().is_empty() { "alloy" } else { voice.trim() };
        let body = json!({ "model": model, "input": text, "voice": voice, "response_format": "wav" });
        let resp = Self::client()?.post(&url).bearer_auth(self.api_key.trim()).json(&body).send().await.map_err(net_err)?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            let code = if status.as_u16() == 401 || status.as_u16() == 403 { ErrorCode::ModelAuthFailed } else { ErrorCode::FetchFailed };
            return Err(LnaError::new(code, format!("TTS HTTP {}: {}", status.as_u16(), txt.chars().take(300).collect::<String>())));
        }
        Ok(resp.bytes().await.map_err(net_err)?.to_vec())
    }
}
