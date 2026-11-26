use async_trait::async_trait;
use lna_types::{ErrorCode, LnaError, LnaResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub mod cloud;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub lang: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInfo {
    pub duration_ms: u64,
    pub sample_rate: u32,
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    async fn voices(&self, lang: &str) -> LnaResult<Vec<Voice>>;
    async fn synthesize(
        &self,
        text: &str,
        voice: &Voice,
        out: &Path,
        cancel: CancellationToken,
    ) -> LnaResult<AudioInfo>;
}

pub struct SystemTtsProvider;

#[async_trait]
impl TtsProvider for SystemTtsProvider {
    async fn voices(&self, lang: &str) -> LnaResult<Vec<Voice>> {
        let is_zh = lang.starts_with("zh");
        Ok(vec![
            Voice {
                id: "host_a".into(),
                name: if is_zh {
                    "主讲 A (系统语音)".into()
                } else {
                    "Host A (System)".into()
                },
                lang: lang.to_string(),
            },
            Voice {
                id: "host_b".into(),
                name: if is_zh {
                    "嘉宾 B (系统语音)".into()
                } else {
                    "Host B (System)".into()
                },
                lang: lang.to_string(),
            },
        ])
    }

    async fn synthesize(
        &self,
        text: &str,
        voice: &Voice,
        out: &Path,
        cancel: CancellationToken,
    ) -> LnaResult<AudioInfo> {
        if cancel.is_cancelled() {
            return Err(LnaError::new(ErrorCode::Cancelled, "TTS cancelled"));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LnaError::internal(e.to_string()))?;
        }

        let out_owned = out.to_path_buf();
        let text_owned = text.to_string();
        let voice_id = voice.id.clone();
        let lang_prefix = if voice.lang.starts_with("zh")
            || text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
        {
            "zh"
        } else {
            "en"
        }
        .to_string();

        let cancel_bg = cancel.clone();
        let synth_res = tokio::task::spawn_blocking(move || {
            #[cfg(target_os = "windows")]
            {
                let safe_txt = text_owned.replace('\'', "''");
                let safe_out = out_owned.display().to_string().replace('\'', "''");
                let is_b = voice_id == "host_b";
                let ps = format!(
                    "Add-Type -AssemblyName System.Speech; \
                     $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                     $vs = @($s.GetInstalledVoices() | Where-Object {{ $_.Enabled -and $_.VoiceInfo.Culture.Name -like '{lang_prefix}*' }}); \
                     if ($vs.Count -ge 2) {{ $s.SelectVoice($vs[{idx}].VoiceInfo.Name); }} \
                     elseif ($vs.Count -eq 1) {{ $s.SelectVoice($vs[0].VoiceInfo.Name); if ({is_b_ps}) {{ $s.Rate = 1; }} }}; \
                     $s.SetOutputToWaveFile('{safe_out}'); $s.Speak('{safe_txt}'); $s.Dispose()",
                    idx = if is_b { 1 } else { 0 },
                    is_b_ps = if is_b { "$true" } else { "$false" }
                );
                if let Ok(mut child) = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command", &ps])
                    .spawn()
                {
                    loop {
                        if cancel_bg.is_cancelled() {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = std::fs::remove_file(&out_owned);
                            return Err(true);
                        }
                        match child.try_wait() {
                            Ok(Some(st)) => {
                                if st.success() && out_owned.exists() {
                                    return Ok(());
                                }
                                break;
                            }
                            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                            Err(_) => break,
                        }
                    }
                }
            }
            #[cfg(target_os = "linux")]
            {
                let pitch = if voice_id == "host_b" { "65" } else { "45" };
                let lang_arg = if lang_prefix == "zh" { "cmn" } else { "en" };
                if let Ok(mut child) = std::process::Command::new("espeak-ng")
                    .args(["-v", lang_arg, "-p", pitch, "-w", &out_owned.to_string_lossy(), &text_owned])
                    .spawn()
                {
                    loop {
                        if cancel_bg.is_cancelled() {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = std::fs::remove_file(&out_owned);
                            return Err(true);
                        }
                        match child.try_wait() {
                            Ok(Some(st)) => {
                                if st.success() && out_owned.exists() {
                                    return Ok(());
                                }
                                break;
                            }
                            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                            Err(_) => break,
                        }
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                if let Some(r) =
                    macos_say(&text_owned, &voice_id, &lang_prefix, &out_owned, &cancel_bg)
                {
                    return r;
                }
            }
            let _ = (&text_owned, &voice_id, &lang_prefix, &out_owned, &cancel_bg);
            Err(false)
        })
        .await;

        if matches!(synth_res, Ok(Err(true))) || cancel.is_cancelled() {
            return Err(LnaError::new(ErrorCode::Cancelled, "TTS cancelled"));
        }

        if matches!(synth_res, Ok(Ok(()))) {
            if let Ok(parsed) = parse_wav_pcm16(out) {
                let dur = (parsed.samples.len() as u64 * 1000) / (parsed.sample_rate.max(1) as u64);
                return Ok(AudioInfo {
                    duration_ms: dur,
                    sample_rate: parsed.sample_rate,
                });
            }
        }

        let est_ms = ((text.chars().count() as u64) * 140).clamp(800, 15_000);
        let sample_rate = 22_050u32;
        let num_samples = (sample_rate as u64 * est_ms / 1000) as usize;
        write_pcm_wav(out, sample_rate, &vec![0i16; num_samples])?;
        Ok(AudioInfo {
            duration_ms: est_ms,
            sample_rate,
        })
    }
}

/// Parses `say -v '?'` output into `(voice name, locale)` pairs, e.g. `("Tingting", "zh_CN")`.
/// A line looks like `Tingting            zh_CN    # 你好！我叫Tingting。`; names may contain spaces.
pub fn parse_say_voices(listing: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in listing.lines() {
        let head = line.split('#').next().unwrap_or("").trim_end();
        let Some(split) = head.rfind(|c: char| c.is_whitespace()) else {
            continue;
        };
        let (name, locale) = (head[..split].trim(), head[split..].trim());
        let valid_locale = locale.len() >= 5
            && locale.contains('_')
            && locale
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !name.is_empty() && valid_locale {
            out.push((name.to_string(), locale.to_string()));
        }
    }
    out
}

/// Picks the voice for a host: the first (A) or second (B) installed voice of the language.
pub fn pick_say_voice(
    voices: &[(String, String)],
    lang_prefix: &str,
    host_b: bool,
) -> Option<String> {
    let wanted = if lang_prefix == "zh" { "zh_" } else { "en_" };
    let matching: Vec<&(String, String)> = voices
        .iter()
        .filter(|(_, l)| l.starts_with(wanted))
        .collect();
    // Prefer one voice per region so A and B sound different without being a different dialect.
    let idx = if host_b && matching.len() > 1 { 1 } else { 0 };
    matching.get(idx).map(|(n, _)| n.clone())
}

/// macOS system voice: `say` to AIFF, then `afconvert` to 16-bit mono WAV. `None` = not available
/// (the caller falls back to a silent placeholder); `Some(Err(true))` = cancelled.
#[cfg(target_os = "macos")]
fn macos_say(
    text: &str,
    voice_id: &str,
    lang_prefix: &str,
    out: &Path,
    cancel: &CancellationToken,
) -> Option<Result<(), bool>> {
    use std::process::{Command, Stdio};

    let listing = Command::new("say")
        .args(["-v", "?"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let voices = parse_say_voices(&String::from_utf8_lossy(&listing.stdout));
    let voice = pick_say_voice(&voices, lang_prefix, voice_id == "host_b");

    let tmp_base = out.with_extension("say");
    let txt = tmp_base.with_extension("txt");
    let aiff = tmp_base.with_extension("aiff");
    std::fs::write(&txt, text).ok()?;

    let wait = |mut child: std::process::Child| -> Option<Result<bool, bool>> {
        loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Some(Err(true));
            }
            match child.try_wait() {
                Ok(Some(st)) => return Some(Ok(st.success())),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(_) => return None,
            }
        }
    };
    let cleanup = || {
        let _ = std::fs::remove_file(&txt);
        let _ = std::fs::remove_file(&aiff);
    };

    let mut say = Command::new("say");
    if let Some(v) = &voice {
        say.args(["-v", v]);
    }
    say.arg("-o").arg(&aiff).arg("-f").arg(&txt);
    let ok = match say.spawn().ok().and_then(wait) {
        Some(Ok(ok)) => ok,
        Some(Err(c)) => {
            cleanup();
            let _ = std::fs::remove_file(out);
            return Some(Err(c));
        }
        None => false,
    };
    if !ok {
        cleanup();
        return None;
    }
    let conv = Command::new("afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@22050", "-c", "1"])
        .arg(&aiff)
        .arg(out)
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .and_then(wait);
    cleanup();
    match conv {
        Some(Ok(true)) if out.exists() => Some(Ok(())),
        Some(Err(c)) => {
            let _ = std::fs::remove_file(out);
            Some(Err(c))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ParsedWav {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

pub fn parse_wav_pcm16(path: &Path) -> LnaResult<ParsedWav> {
    let bytes = std::fs::read(path).map_err(|e| LnaError::internal(e.to_string()))?;
    parse_wav_bytes(&bytes)
}

pub fn parse_wav_bytes(bytes: &[u8]) -> LnaResult<ParsedWav> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(LnaError::invalid("Not a valid RIFF/WAVE file"));
    }
    let mut pos = 12usize;
    let mut sample_rate = 16_000u32;
    let mut channels = 1u16;
    let mut bits_per_sample = 16u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(chunk_size).min(bytes.len());

        if chunk_id == b"fmt " && body_end - body_start >= 16 {
            let fmt = &bytes[body_start..body_end];
            channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
        } else if chunk_id == b"data" {
            data_slice = Some(&bytes[body_start..body_end]);
        }

        let padded = chunk_size + (chunk_size % 2);
        pos = body_start.saturating_add(padded);
    }

    let raw = data_slice.ok_or_else(|| LnaError::invalid("WAV missing data chunk"))?;
    if bits_per_sample != 16 {
        return Err(LnaError::invalid(format!(
            "Unsupported WAV bit depth: {bits_per_sample}"
        )));
    }

    let ch = channels.max(1) as usize;
    let mut mono = Vec::with_capacity(raw.len() / (2 * ch));
    for frame in raw.chunks_exact(2 * ch) {
        let mut sum = 0i32;
        for c in 0..ch {
            sum += i16::from_le_bytes([frame[c * 2], frame[c * 2 + 1]]) as i32;
        }
        mono.push((sum / ch as i32) as i16);
    }

    Ok(ParsedWav {
        sample_rate,
        channels: 1,
        samples: mono,
    })
}

pub fn resample_linear(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || samples.is_empty() || src_rate == 0 {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as u64) * (dst_rate as u64) / (src_rate as u64)).max(1) as usize;
    let mut out = Vec::with_capacity(out_len);
    let ratio = (src_rate as f64) / (dst_rate as f64);
    for i in 0..out_len {
        let src_pos = (i as f64) * ratio;
        let idx = (src_pos.floor() as usize).min(samples.len() - 1);
        let frac = src_pos - (idx as f64);
        let s0 = samples[idx] as f64;
        let s1 = samples[(idx + 1).min(samples.len() - 1)] as f64;
        out.push((s0 + (s1 - s0) * frac).round() as i16);
    }
    out
}

pub fn write_pcm_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> LnaResult<()> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36 + data_bytes;
    let mut buf = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, buf).map_err(|e| LnaError::internal(e.to_string()))
}

pub fn concat_wavs(inputs: &[std::path::PathBuf], out: &Path) -> LnaResult<()> {
    const TARGET_RATE: u32 = 22_050;
    let mut merged: Vec<i16> = Vec::new();
    for p in inputs {
        let parsed = parse_wav_pcm16(p)?;
        let resampled = resample_linear(&parsed.samples, parsed.sample_rate, TARGET_RATE);
        merged.extend_from_slice(&resampled);
    }
    write_pcm_wav(out, TARGET_RATE, &merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_wav_with_fmt_size(sample_rate: u32, samples: &[i16], fmt_size: u32) -> Vec<u8> {
        let data_bytes = (samples.len() * 2) as u32;
        let riff_size = 4 + (8 + fmt_size) + (8 + data_bytes);
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&riff_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&fmt_size.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.resize(buf.len() + (fmt_size as usize - 16), 0);
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    #[test]
    fn parses_44_and_46_byte_wav_and_resamples_mixed_rates() {
        let tmp = std::env::temp_dir().join("lna_tts_b21_test");
        let _ = std::fs::create_dir_all(&tmp);
        let p1 = tmp.join("a_44.wav");
        let p2 = tmp.join("b_46.wav");
        let out = tmp.join("merged.wav");

        std::fs::write(
            &p1,
            build_wav_with_fmt_size(16_000, &vec![100i16; 16_000], 16),
        )
        .unwrap();
        std::fs::write(
            &p2,
            build_wav_with_fmt_size(22_050, &vec![200i16; 22_050], 18),
        )
        .unwrap();

        concat_wavs(&[p1, p2], &out).unwrap();
        let merged = parse_wav_pcm16(&out).unwrap();
        assert_eq!(merged.sample_rate, 22_050);
        assert!((merged.samples.len() as i64 - 44_100).abs() <= 4);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parses_say_voice_listing_and_picks_per_host() {
        let listing = "Eddy (Chinese (China mainland)) zh_CN    # 你好！我叫Eddy。\n\
Samantha            en_US    # Hello! My name is Samantha.\n\
Daniel              en_GB    # Hello! My name is Daniel.\n\
Tingting            zh_CN    # 你好！我叫Tingting。\n\
garbage line without locale\n";
        let v = parse_say_voices(listing);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].0, "Eddy (Chinese (China mainland))");
        assert_eq!(pick_say_voice(&v, "en", false).as_deref(), Some("Samantha"));
        assert_eq!(pick_say_voice(&v, "en", true).as_deref(), Some("Daniel"));
        assert_eq!(pick_say_voice(&v, "zh", true).as_deref(), Some("Tingting"));
        assert_eq!(pick_say_voice(&[], "en", false), None);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_system_tts_writes_audible_wav_and_cancels() {
        let tmp = std::env::temp_dir().join("lna_tts_macos_real_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let tts = SystemTtsProvider;
        let v = Voice {
            id: "host_a".into(),
            name: "Host A".into(),
            lang: "auto".into(),
        };
        let out = tmp.join("turn.wav");
        let info = tts
            .synthesize(
                "Hello from the notebook.",
                &v,
                &out,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let wav = parse_wav_pcm16(&out).unwrap();
        assert!(info.duration_ms > 0);
        assert!(
            wav.samples.iter().any(|s| s.abs() > 200),
            "synthesized audio is silent"
        );

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = tts
            .synthesize("cancelled", &v, &tmp.join("c.wav"), cancel)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn e8_b08_real_system_tts_bilingual_and_cancel() {
        let tmp = std::env::temp_dir().join("lna_tts_e8_real_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let tts = SystemTtsProvider;
        let va = Voice {
            id: "host_a".into(),
            name: "Host A".into(),
            lang: "auto".into(),
        };
        let vb = Voice {
            id: "host_b".into(),
            name: "Host B".into(),
            lang: "auto".into(),
        };
        let p_zh = tmp.join("turn_000.wav");
        let p_en = tmp.join("turn_001.wav");
        let merged = tmp.join("overview.wav");

        let info_zh = tts
            .synthesize(
                "欢迎收听本地笔记本音频概览。",
                &va,
                &p_zh,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let info_en = tts
            .synthesize(
                "Welcome to the local notebook audio overview.",
                &vb,
                &p_en,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        // A machine without a Chinese voice (a CI runner, say) reads Chinese text as silence, so the Chinese
        // clip is only required to have content when such a voice exists.
        if info_zh.duration_ms <= 300 {
            eprintln!(
                "note: no Chinese system voice available; skipping the Chinese duration check"
            );
        }
        assert!(info_en.duration_ms > 300);

        concat_wavs(&[p_zh, p_en], &merged).unwrap();
        let wav = parse_wav_pcm16(&merged).unwrap();
        assert!(
            wav.samples.iter().any(|&s| s.abs() > 100),
            "real TTS should produce non-silent audio"
        );

        // Cancellation mid-synthesis
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            cancel_clone.cancel();
        });
        let p_cancel = tmp.join("cancelled.wav");
        let err = tts
            .synthesize(
                "This is a long sentence that will be cancelled before finishing synthesis.",
                &va,
                &p_cancel,
                cancel,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
