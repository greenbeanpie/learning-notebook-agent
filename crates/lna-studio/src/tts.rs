//! Text-to-speech settings and cached synthesis for the lecture player.
//!
//! The system voice is spoken directly by each client (WinRT / Android / AVSpeech / speech-
//! dispatcher). The cloud engine (Qwen-TTS or an OpenAI-compatible endpoint) is synthesized
//! here and cached by (provider, model, voice, text), so replays never call the service again.

use super::*;
use lna_tts::cloud::{known_voices, CloudTts};
use secrecy::{ExposeSecret, SecretString};

const SETTINGS_KEY: &str = "tts.settings";
const SECRET_REF: &str = "tts.cloud.api_key";

impl StudioService {
    /// `{engine: "system"|"cloud", rate, system_voice, cloud: {provider, base_url, model, voice},
    ///   has_api_key, voices: [...]}`
    pub fn tts_settings(&self) -> LnaResult<Value> {
        let mut s = lna_storage::get_setting_json(&self.db, SETTINGS_KEY)?.unwrap_or_else(|| json!({}));
        if !s.is_object() {
            s = json!({});
        }
        let provider = s.pointer("/cloud/provider").and_then(Value::as_str).unwrap_or("qwen").to_string();
        let o = s.as_object_mut().expect("object");
        o.entry("engine").or_insert_with(|| json!("system"));
        o.entry("rate").or_insert_with(|| json!(1.0));
        o.entry("system_voice").or_insert_with(|| json!(""));
        o.entry("cloud").or_insert_with(|| json!({ "provider": "qwen", "base_url": "", "model": "", "voice": "Cherry" }));
        o.insert("has_api_key".into(), json!(self.secrets.exists(SECRET_REF).unwrap_or(false)));
        o.insert(
            "voices".into(),
            json!(known_voices(&provider).iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>()),
        );
        Ok(s)
    }

    /// Merges the given fields; `api_key` goes to the OS keychain ("" clears it).
    pub fn set_tts_settings(&self, args: &Value) -> LnaResult<Value> {
        let mut cur = self.tts_settings()?;
        if let Some(o) = cur.as_object_mut() {
            o.remove("has_api_key");
            o.remove("voices");
            if let Some(e) = args.get("engine").and_then(Value::as_str) {
                if e != "system" && e != "cloud" {
                    return Err(LnaError::invalid("engine must be system or cloud"));
                }
                o.insert("engine".into(), json!(e));
            }
            if let Some(r) = args.get("rate").and_then(Value::as_f64) {
                o.insert("rate".into(), json!(r.clamp(0.5, 2.0)));
            }
            if let Some(v) = args.get("system_voice").and_then(Value::as_str) {
                o.insert("system_voice".into(), json!(v));
            }
            let cloud = o.entry("cloud").or_insert_with(|| json!({}));
            for k in ["provider", "base_url", "model", "voice"] {
                if let Some(v) = args.get(k).and_then(Value::as_str) {
                    cloud[k] = json!(v.trim());
                }
            }
        }
        lna_storage::set_setting_json(&self.db, SETTINGS_KEY, &cur)?;
        if let Some(key) = args.get("api_key").and_then(Value::as_str) {
            if key.trim().is_empty() {
                self.secrets.clear(SECRET_REF)?;
            } else {
                self.secrets.set(SECRET_REF, SecretString::from(key.trim().to_string()))?;
            }
        }
        self.tts_settings()
    }

    fn cloud_tts(&self) -> LnaResult<(CloudTts, String)> {
        let s = self.tts_settings()?;
        let c = s.get("cloud").cloned().unwrap_or_else(|| json!({}));
        let api_key = self.secrets.get(SECRET_REF)?.map(|k| k.expose_secret().to_string()).unwrap_or_default();
        let str_of = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok((
            CloudTts { provider: str_of("provider"), base_url: str_of("base_url"), model: str_of("model"), api_key },
            str_of("voice"),
        ))
    }

    /// Synthesizes one sentence with the cloud engine (cached). Returns `{path, duration_ms, cached}`.
    pub async fn tts_synthesize(&self, text: &str, voice: Option<&str>) -> LnaResult<Value> {
        let (tts, default_voice) = self.cloud_tts()?;
        let voice = voice.filter(|v| !v.trim().is_empty()).map(String::from).unwrap_or(default_voice);
        let key = hex::encode(Sha256::digest(format!("{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}", tts.provider, tts.base_url, tts.model, voice, text.trim()).as_bytes()));
        let dir = self.db.paths().root.join("tts_cache");
        let path = dir.join(format!("{}.wav", &key[..32]));
        if path.is_file() {
            let dur = lna_tts::parse_wav_pcm16(&path)
                .map(|w| (w.samples.len() as u64 / w.channels.max(1) as u64) * 1000 / w.sample_rate.max(1) as u64)
                .unwrap_or(0);
            return Ok(json!({ "path": path.to_string_lossy(), "duration_ms": dur, "cached": true }));
        }
        let info = tts.synthesize(text, &voice, &path, CancellationToken::new()).await?;
        Ok(json!({ "path": path.to_string_lossy(), "duration_ms": info.duration_ms, "cached": false }))
    }

    /// Deletes cached cloud audio; returns the number of files removed.
    pub fn tts_clear_cache(&self) -> LnaResult<usize> {
        let dir = self.db.paths().root.join("tts_cache");
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if std::fs::remove_file(e.path()).is_ok() {
                    n += 1;
                }
            }
        }
        Ok(n)
    }
}
