//! Locating and driving a local ffmpeg binary, used to shrink or convert audio/video that a
//! provider rejects as-is (e.g. MiMo's 50 MB inline Base64 limit).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// `CREATE_NO_WINDOW`: keeps a console window from flashing up for each ffmpeg run on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where the user gets ffmpeg from the app itself. Shown in every error that asks for ffmpeg, so
/// the wording stays the same in all places.
pub const FFMPEG_INSTALL_HINT: &str = "请在 设置 → 模型协议配置 中点击“下载 ffmpeg”";

fn ffmpeg_missing() -> String {
    format!("未找到 ffmpeg，{FFMPEG_INSTALL_HINT}")
}

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The tools folder the running core was given (`<data root>/tools`); set once at startup.
static TOOLS_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Called when the core opens its data folder, so the app copy of ffmpeg lives next to the
/// data it belongs to. The first call wins.
pub fn set_tools_dir(root: &Path) {
    let _ = TOOLS_DIR.set(root.join("tools"));
}

/// Fallback when no core was opened: the platform's local data folder with a `LocalNotebook`
/// subfolder, like the apps use.
fn data_dir() -> PathBuf {
    platform_local_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LocalNotebook")
}

fn platform_local_data_dir() -> Option<PathBuf> {
    let env_path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        env_path("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env_path("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env_path("XDG_DATA_HOME")
            .or_else(|| env_path("HOME").map(|h| h.join(".local").join("share")))
    }
}

/// Folder where a downloaded or copied ffmpeg is kept: `<data_dir>/tools`.
pub fn install_dir() -> PathBuf {
    match TOOLS_DIR.get() {
        Some(d) => d.clone(),
        None => data_dir().join("tools"),
    }
}

pub(crate) fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

pub(crate) fn hide_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The ffmpeg to run: `<install_dir>/ffmpeg[.exe]` if present, else `ffmpeg` on PATH when
/// `ffmpeg -version` runs. Not cached, so an ffmpeg installed while the app runs is picked up.
pub fn ffmpeg_path() -> Option<PathBuf> {
    let local = install_dir().join(exe_name());
    if local.is_file() {
        return Some(local);
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let on_path = hide_window(&mut cmd)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    on_path.then(|| PathBuf::from("ffmpeg"))
}

/// Whether an `ffmpeg -encoders` listing includes libx264 (its rows read `V..... libx264 ...`).
fn encoders_list_has_libx264(listing: &str) -> bool {
    listing
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some("libx264"))
}

/// Cached per ffmpeg path: the answer only changes when the binary changes.
static X264_CACHE: Mutex<Option<(PathBuf, bool)>> = Mutex::new(None);

fn has_libx264(ff: &Path) -> bool {
    if let Ok(guard) = X264_CACHE.lock() {
        if let Some((path, has)) = guard.as_ref() {
            if path == ff {
                return *has;
            }
        }
    }
    let mut cmd = Command::new(ff);
    cmd.args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let has = hide_window(&mut cmd)
        .output()
        .map(|o| {
            o.status.success() && encoders_list_has_libx264(&String::from_utf8_lossy(&o.stdout))
        })
        .unwrap_or(false);
    if let Ok(mut guard) = X264_CACHE.lock() {
        *guard = Some((ff.to_path_buf(), has));
    }
    has
}

/// Video encoder arguments: libx264 at `crf` when this ffmpeg has it, otherwise the built-in
/// mpeg4 encoder at a fixed quality (`-q:v 6`), which every ffmpeg build includes.
fn video_encoder_args<'a>(ff: &Path, crf: &'a str) -> Vec<&'a str> {
    if has_libx264(ff) {
        vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", crf]
    } else {
        vec!["-c:v", "mpeg4", "-q:v", "6"]
    }
}

fn temp_output(ext: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "lna_ffmpeg_{}_{}_{}.{ext}",
        std::process::id(),
        lna_types::now_ms(),
        seq
    ))
}

/// Runs `ffmpeg -i input <codec_args> output`, overwriting `output`.
fn encode(ff: &Path, input: &Path, output: &Path, codec_args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new(ff);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(codec_args)
        .arg(output)
        .stdin(Stdio::null());
    let out = hide_window(&mut cmd)
        .output()
        .map_err(|e| format!("无法启动 ffmpeg: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    Err(format!(
        "ffmpeg 失败（退出码 {:?}）: {last}",
        out.status.code()
    ))
}

/// Encodes `input` with `codec_args` into a temp file, reads the result back and deletes it.
pub(crate) fn encode_to_bytes(
    ff: &Path,
    input: &Path,
    codec_args: &[&str],
    ext: &str,
) -> Result<Vec<u8>, String> {
    let out = temp_output(ext);
    let res = encode(ff, input, &out, codec_args)
        .and_then(|()| std::fs::read(&out).map_err(|e| format!("读取 ffmpeg 输出失败: {e}")));
    let _ = std::fs::remove_file(&out);
    res
}

/// Re-encodes a video to H.264/AAC MP4 at its original resolution (container conversion).
pub fn transcode_to_mp4(input: &Path) -> Result<Vec<u8>, String> {
    let ff = ffmpeg_path().ok_or_else(ffmpeg_missing)?;
    let mut args = video_encoder_args(&ff, "23");
    args.extend(["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"]);
    encode_to_bytes(&ff, input, &args, "mp4")
}

/// Re-encodes audio to MP3 at its original sample rate (container conversion).
pub fn transcode_to_mp3(input: &Path) -> Result<Vec<u8>, String> {
    let ff = ffmpeg_path().ok_or_else(ffmpeg_missing)?;
    encode_to_bytes(
        &ff,
        input,
        &["-vn", "-c:a", "libmp3lame", "-b:a", "128k"],
        "mp3",
    )
}

/// Re-encodes a video at 480p, then 360p, then 240p (2 fps, CRF 30, 48k AAC) and returns the
/// first result within `limit_bytes`. Otherwise errors with the smallest size reached.
pub fn shrink_video(input: &Path, limit_bytes: u64) -> Result<Vec<u8>, String> {
    let ff = ffmpeg_path().ok_or_else(ffmpeg_missing)?;
    let mut smallest: Option<usize> = None;
    let mut last_err = None;
    for height in [480u32, 360, 240] {
        let scale = format!("scale=-2:{height}");
        let mut args = vec!["-vf", scale.as_str(), "-r", "2"];
        args.extend(video_encoder_args(&ff, "30"));
        args.extend(["-c:a", "aac", "-b:a", "48k", "-movflags", "+faststart"]);
        match encode_to_bytes(&ff, input, &args, "mp4") {
            Ok(bytes) if bytes.len() as u64 <= limit_bytes => return Ok(bytes),
            Ok(bytes) => {
                smallest = Some(smallest.map_or(bytes.len(), |s| s.min(bytes.len())));
            }
            Err(e) => last_err = Some(e),
        }
    }
    match smallest {
        Some(size) => Err(format!(
            "视频压缩到 240p 后仍约 {:.1} MB，超过 {:.1} MB 上限",
            size as f64 / (1024.0 * 1024.0),
            limit_bytes as f64 / (1024.0 * 1024.0)
        )),
        None => Err(last_err.unwrap_or_else(|| "视频压缩失败".to_string())),
    }
}

/// Re-encodes audio to mono 16 kHz MP3 at 64k, then 32k, then 16k, returning the first result
/// within `limit_bytes`. Otherwise errors with the smallest size reached.
pub fn shrink_audio(input: &Path, limit_bytes: u64) -> Result<Vec<u8>, String> {
    let ff = ffmpeg_path().ok_or_else(ffmpeg_missing)?;
    let mut smallest: Option<usize> = None;
    let mut last_err = None;
    for bitrate in ["64k", "32k", "16k"] {
        let args = [
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libmp3lame",
            "-b:a",
            bitrate,
        ];
        match encode_to_bytes(&ff, input, &args, "mp3") {
            Ok(bytes) if bytes.len() as u64 <= limit_bytes => return Ok(bytes),
            Ok(bytes) => {
                smallest = Some(smallest.map_or(bytes.len(), |s| s.min(bytes.len())));
            }
            Err(e) => last_err = Some(e),
        }
    }
    match smallest {
        Some(size) => Err(format!(
            "音频压缩到 16k 码率后仍约 {:.1} MB，超过 {:.1} MB 上限",
            size as f64 / (1024.0 * 1024.0),
            limit_bytes as f64 / (1024.0 * 1024.0)
        )),
        None => Err(last_err.unwrap_or_else(|| "音频压缩失败".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_is_tools_under_data_dir() {
        let dir = install_dir();
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("tools"));
    }

    #[test]
    fn libx264_is_detected_in_encoder_listing() {
        let listing = "Encoders:
 V....D libx264              libx264 H.264 / AVC
 A....D aac   AAC
";
        assert!(encoders_list_has_libx264(listing));
        assert!(!encoders_list_has_libx264(
            " V....D libx264rgb  libx264rgb
"
        ));
        assert!(!encoders_list_has_libx264(
            " V....D mpeg4  MPEG-4
"
        ));
    }

    #[test]
    fn temp_outputs_are_unique() {
        assert_ne!(temp_output("mp4"), temp_output("mp4"));
    }
}
