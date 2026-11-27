//! Downloads a static ffmpeg build into the app's tools folder (`install_dir()`), so the app can
//! shrink and convert media without a system-wide ffmpeg. Only the fixed release URLs below are
//! ever fetched.

use crate::ffmpeg::{exe_name, hide_window, install_dir};
use futures_util::StreamExt;
use lna_types::{ErrorCode, LnaError, LnaResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

// --- Download sources (change here) ---------------------------------------------------------

const WIN64_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
const LINUX64_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz";
const LINUX_ARM64_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz";
/// x86_64 binary; runs on Apple Silicon through Rosetta.
const MACOS_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/zip";
/// SHA-256 sums published next to the BtbN archives. evermeet publishes none.
const BTBN_CHECKSUMS_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/checksums.sha256";

const USER_AGENT: &str = "LocalNotebookAgent";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest silence allowed while receiving data; the whole transfer has no overall limit.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Progress is reported at most once per this many downloaded bytes (plus the final total).
const PROGRESS_STEP: u64 = 256 * 1024;
/// Upper bound for the extracted executable, so a bad archive cannot fill the disk.
const MAX_FFMPEG_BYTES: u64 = 512 * 1024 * 1024;
const ARCHIVE_PART_NAME: &str = "ffmpeg-download.part";

static BUSY: AtomicBool = AtomicBool::new(false);

/// What `status()` reports. `source` is `"app"` for the copy this module installs, `"system"` for
/// an ffmpeg found on PATH, and `None` when there is none.
#[derive(Debug, Clone, Serialize)]
pub struct FfmpegStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: Option<&'static str>,
    pub supported: bool,
}

#[derive(Clone, Copy)]
enum ArchiveFormat {
    Zip,
    TarXz,
}

struct Source {
    url: &'static str,
    format: ArchiveFormat,
    checksums_url: Option<&'static str>,
}

fn source_for_platform() -> Option<Source> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(Source {
            url: WIN64_URL,
            format: ArchiveFormat::Zip,
            checksums_url: Some(BTBN_CHECKSUMS_URL),
        }),
        ("linux", "x86_64") => Some(Source {
            url: LINUX64_URL,
            format: ArchiveFormat::TarXz,
            checksums_url: Some(BTBN_CHECKSUMS_URL),
        }),
        ("linux", "aarch64") => Some(Source {
            url: LINUX_ARM64_URL,
            format: ArchiveFormat::TarXz,
            checksums_url: Some(BTBN_CHECKSUMS_URL),
        }),
        ("macos", _) => Some(Source {
            url: MACOS_URL,
            format: ArchiveFormat::Zip,
            checksums_url: None,
        }),
        _ => None,
    }
}

fn unsupported_platform() -> LnaError {
    LnaError::new(ErrorCode::CapabilityMissing, "本平台不支持下载 ffmpeg")
}

/// Whether this platform has a download source at all.
pub fn supported() -> bool {
    source_for_platform().is_some()
}

/// Holds the single install/remove slot; released on drop, including when the future is dropped.
struct Busy;

impl Busy {
    fn acquire() -> LnaResult<Busy> {
        BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Busy)
            .map_err(|_| LnaError::new(ErrorCode::Conflict, "ffmpeg 正在下载或删除中，请稍候"))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

/// Deletes the listed files when dropped (on failure, or when the install future is dropped).
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn disk_error(what: &str, e: io::Error) -> LnaError {
    LnaError::internal(format!("{what}，请检查磁盘空间和写入权限：{e}"))
}

fn network_error(e: reqwest::Error) -> LnaError {
    LnaError::new(
        ErrorCode::FetchFailed,
        format!("下载 ffmpeg 失败，请检查网络连接后重试（{e}）"),
    )
    .with_retryable(true)
}

fn archive_error(e: impl std::fmt::Display) -> LnaError {
    LnaError::internal(format!("ffmpeg 安装包解析失败：{e}"))
}

/// The file name of a download URL (its last path segment).
fn file_name_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// Finds `file_name` in a `sha256sum`-style listing (`<hex>  <name>` or `<hex> *<name>`) and
/// returns its digest in lowercase hex.
pub(crate) fn expected_sha256(listing: &str, file_name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let mut parts = line.trim().splitn(2, char::is_whitespace);
        let hash = parts.next()?;
        let name = parts.next()?.trim().trim_start_matches('*');
        let name = name.rsplit('/').next().unwrap_or(name);
        let well_formed = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (well_formed && name == file_name).then(|| hash.to_ascii_lowercase())
    })
}

/// Compares the digest of the downloaded archive with the published one.
pub(crate) fn check_sha256(actual_hex: &str, expected_hex: &str) -> LnaResult<()> {
    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        return Ok(());
    }
    Err(LnaError::internal(
        "ffmpeg 安装包校验失败（SHA-256 与发布的校验值不一致），文件可能下载不完整或被篡改，已删除。",
    ))
}

/// A path names the executable when its file name matches and it sits at the top of the archive
/// or in a `bin` folder (`ffmpeg-<build>/bin/ffmpeg`).
fn is_ffmpeg_entry(path: &Path, target: &str) -> bool {
    if path.file_name().and_then(|n| n.to_str()) != Some(target) {
        return false;
    }
    match path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        None => true,
        Some(dir) => dir == "bin",
    }
}

fn copy_limited<R: Read, W: Write>(src: R, dst: &mut W) -> LnaResult<()> {
    let mut limited = src.take(MAX_FFMPEG_BYTES + 1);
    let n = io::copy(&mut limited, dst).map_err(archive_error)?;
    if n > MAX_FFMPEG_BYTES {
        return Err(archive_error("解压出的 ffmpeg 超过 512 MB 上限"));
    }
    Ok(())
}

fn missing_in_archive(target: &str) -> LnaError {
    LnaError::internal(format!(
        "安装包中找不到 {target}，请稍后重试或改用系统 ffmpeg"
    ))
}

/// Copies the `target` executable out of a zip archive into `dst`.
pub(crate) fn extract_from_zip<R: Read + Seek, W: Write>(
    src: R,
    target: &str,
    dst: &mut W,
) -> LnaResult<()> {
    let mut zip = zip::ZipArchive::new(src).map_err(archive_error)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(archive_error)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().map_err(archive_error)?.into_owned();
        if is_ffmpeg_entry(Path::new(&name), target) {
            return copy_limited(&mut entry, dst);
        }
    }
    Err(missing_in_archive(target))
}

/// Copies the `target` executable out of a tar.xz archive into `dst`.
pub(crate) fn extract_from_tar_xz<R: Read, W: Write>(
    src: R,
    target: &str,
    dst: &mut W,
) -> LnaResult<()> {
    let xz = lzma_rust2::XzReader::new(src, true);
    let mut tar = tar::Archive::new(xz);
    for entry in tar.entries().map_err(archive_error)? {
        let mut entry = entry.map_err(archive_error)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(archive_error)?.into_owned();
        if is_ffmpeg_entry(&path, target) {
            return copy_limited(&mut entry, dst);
        }
    }
    Err(missing_in_archive(target))
}

/// Pulls `version` out of `ffmpeg -version` output ("ffmpeg version 7.1-essentials ...").
fn parse_version(output: &str) -> Option<String> {
    let first = output.lines().next()?.trim();
    let rest = first.strip_prefix("ffmpeg version ")?;
    rest.split_whitespace().next().map(str::to_string)
}

/// Runs `<path> -version`. `Ok(version)` when it runs and exits successfully (the version is
/// `None` when the banner is not recognised); `Err(reason)` otherwise.
fn run_version(path: &Path) -> Result<Option<String>, String> {
    let mut cmd = Command::new(path);
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let out = hide_window(&mut cmd)
        .output()
        .map_err(|e| format!("无法启动：{e}"))?;
    if !out.status.success() {
        return Err(format!("退出码 {:?}", out.status.code()));
    }
    Ok(parse_version(&String::from_utf8_lossy(&out.stdout)))
}

/// The app-installed copy, if present, otherwise `ffmpeg` on PATH.
pub fn status() -> FfmpegStatus {
    let supported = supported();
    let local = install_dir().join(exe_name());
    let Some(path) = crate::ffmpeg::ffmpeg_path() else {
        return FfmpegStatus {
            installed: false,
            path: None,
            version: None,
            source: None,
            supported,
        };
    };
    let source = if path == local { "app" } else { "system" };
    let version = run_version(&path).ok().flatten();
    FfmpegStatus {
        installed: true,
        path: Some(path.to_string_lossy().into_owned()),
        version,
        source: Some(source),
        supported,
    }
}

/// Deletes the app-installed copy. A system ffmpeg is never touched.
pub fn remove() -> LnaResult<()> {
    let _busy = Busy::acquire()?;
    let local = install_dir().join(exe_name());
    match std::fs::remove_file(&local) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(disk_error("无法删除已安装的 ffmpeg", e)),
    }
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> LnaResult<String> {
    client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(network_error)?
        .text()
        .await
        .map_err(network_error)
}

/// Streams `url` into `dest`, reporting progress, and returns the SHA-256 of what was written.
async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    progress: &mut impl FnMut(u64, Option<u64>),
) -> LnaResult<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(network_error)?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| disk_error("无法创建下载文件", e))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut reported = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network_error)?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| disk_error("写入 ffmpeg 安装包失败", e))?;
        downloaded += chunk.len() as u64;
        if downloaded - reported >= PROGRESS_STEP {
            reported = downloaded;
            progress(downloaded, total);
        }
    }
    file.flush()
        .await
        .map_err(|e| disk_error("写入 ffmpeg 安装包失败", e))?;
    file.sync_all()
        .await
        .map_err(|e| disk_error("写入 ffmpeg 安装包失败", e))?;
    if let Some(total) = total {
        if downloaded != total {
            return Err(LnaError::new(
                ErrorCode::FetchFailed,
                format!("下载不完整（已收到 {downloaded} / {total} 字节），请重试"),
            )
            .with_retryable(true));
        }
    }
    progress(downloaded, total);
    Ok(hex::encode(hasher.finalize()))
}

/// Extracts the executable from `archive` into `staged` and makes it runnable.
fn extract(format: ArchiveFormat, archive: &Path, staged: &Path, target: &str) -> LnaResult<()> {
    let src = std::fs::File::open(archive).map_err(|e| disk_error("无法读取安装包", e))?;
    let mut dst = std::fs::File::create(staged).map_err(|e| disk_error("无法写入 ffmpeg", e))?;
    match format {
        ArchiveFormat::Zip => extract_from_zip(src, target, &mut dst)?,
        ArchiveFormat::TarXz => extract_from_tar_xz(BufReader::new(src), target, &mut dst)?,
    }
    dst.sync_all()
        .map_err(|e| disk_error("写入 ffmpeg 失败", e))?;
    drop(dst);
    make_executable(staged)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> LnaResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| disk_error("无法设置 ffmpeg 执行权限", e))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> LnaResult<()> {
    Ok(())
}

/// Downloads, verifies, unpacks and checks the ffmpeg build for this platform, then installs it
/// into `install_dir()` (replacing an earlier app copy). `progress` receives `(downloaded, total)`
/// in bytes. Dropping the future cancels the install and removes the partial files.
pub async fn install(mut progress: impl FnMut(u64, Option<u64>)) -> LnaResult<PathBuf> {
    let source = source_for_platform().ok_or_else(unsupported_platform)?;
    let _busy = Busy::acquire()?;

    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| disk_error("无法创建 ffmpeg 目录", e))?;
    let target = exe_name();
    let archive = dir.join(ARCHIVE_PART_NAME);
    let staged = dir.join(format!("{target}.new"));
    let installed = dir.join(target);
    let _cleanup = Cleanup(vec![archive.clone(), staged.clone()]);

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| LnaError::internal(format!("无法初始化下载组件：{e}")))?;

    let archive_name = file_name_of(source.url);
    let expected = match source.checksums_url {
        Some(url) => {
            let listing = fetch_text(&client, url).await?;
            Some(expected_sha256(&listing, archive_name).ok_or_else(|| {
                LnaError::new(
                    ErrorCode::FetchFailed,
                    format!("校验信息中没有 {archive_name} 的 SHA-256，已取消安装"),
                )
            })?)
        }
        None => None,
    };

    let digest = download(&client, source.url, &archive, &mut progress).await?;
    if let Some(expected) = expected {
        check_sha256(&digest, &expected)?;
    }

    let (archive_c, staged_c) = (archive.clone(), staged.clone());
    tokio::task::spawn_blocking(move || extract(source.format, &archive_c, &staged_c, target))
        .await
        .map_err(|e| LnaError::internal(format!("解压 ffmpeg 失败：{e}")))??;
    let _ = std::fs::remove_file(&archive);

    let staged_c = staged.clone();
    let version = tokio::task::spawn_blocking(move || run_version(&staged_c))
        .await
        .map_err(|e| LnaError::internal(format!("检查 ffmpeg 失败：{e}")))?;
    if let Err(reason) = version {
        return Err(LnaError::internal(format!(
            "下载的 ffmpeg 无法运行（{reason}），已删除，请稍后重试"
        )));
    }

    std::fs::rename(&staged, &installed).map_err(|e| disk_error("无法安装 ffmpeg", e))?;
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TARGET: &str = "ffmpeg";

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn tar_xz_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut xz =
            lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
        xz.write_all(&tar_bytes).unwrap();
        xz.finish().unwrap()
    }

    fn sha256_hex(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    #[test]
    fn checksum_listing_is_parsed_by_file_name() {
        let hash = sha256_hex(b"archive");
        let listing = format!(
            "{}  ffmpeg-master-latest-linux64-gpl.tar.xz\n{} *ffmpeg-master-latest-win64-gpl.zip\n",
            "a".repeat(64),
            hash.to_uppercase()
        );
        assert_eq!(
            expected_sha256(&listing, "ffmpeg-master-latest-win64-gpl.zip"),
            Some(hash)
        );
        assert_eq!(expected_sha256(&listing, "missing.zip"), None);
        assert_eq!(
            expected_sha256("short  ffmpeg.zip\n", "ffmpeg.zip"),
            None,
            "malformed digests are ignored"
        );
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let good = sha256_hex(b"archive bytes");
        assert!(check_sha256(&good, &good).is_ok());
        assert!(check_sha256(&good.to_uppercase(), &good).is_ok());
        let tampered = sha256_hex(b"archive bytes, altered");
        let err = check_sha256(&tampered, &good).unwrap_err();
        assert!(err.message.contains("校验失败"), "{}", err.message);
    }

    #[test]
    fn zip_extraction_takes_only_the_bin_executable() {
        let archive = zip_with(&[
            ("ffmpeg-build/", b""),
            ("ffmpeg-build/doc/ffmpeg.html", b"<html>"),
            ("ffmpeg-build/bin/ffprobe", b"probe"),
            ("ffmpeg-build/bin/ffmpeg", b"the ffmpeg binary"),
        ]);
        let mut out = Vec::new();
        extract_from_zip(Cursor::new(archive), TARGET, &mut out).unwrap();
        assert_eq!(out, b"the ffmpeg binary");
    }

    #[test]
    fn zip_extraction_accepts_a_top_level_executable() {
        let archive = zip_with(&[("ffmpeg", b"flat build")]);
        let mut out = Vec::new();
        extract_from_zip(Cursor::new(archive), TARGET, &mut out).unwrap();
        assert_eq!(out, b"flat build");
    }

    #[test]
    fn zip_without_ffmpeg_is_an_error() {
        let archive = zip_with(&[("readme.txt", b"nothing here"), ("lib/ffmpeg/x", b"no")]);
        let mut out = Vec::new();
        let err = extract_from_zip(Cursor::new(archive), TARGET, &mut out).unwrap_err();
        assert!(err.message.contains("找不到"), "{}", err.message);
    }

    #[test]
    fn tar_xz_extraction_takes_only_the_bin_executable() {
        let archive = tar_xz_with(&[
            ("ffmpeg-build/bin/ffprobe", b"probe"),
            ("ffmpeg-build/bin/ffmpeg", b"linux ffmpeg"),
            ("ffmpeg-build/doc/ffmpeg", b"not this one"),
        ]);
        let mut out = Vec::new();
        extract_from_tar_xz(Cursor::new(archive), TARGET, &mut out).unwrap();
        assert_eq!(out, b"linux ffmpeg");
    }

    #[test]
    fn corrupt_archives_are_reported_not_panicked() {
        let mut out = Vec::new();
        assert!(extract_from_zip(Cursor::new(b"PK not a zip".to_vec()), TARGET, &mut out).is_err());
        assert!(
            extract_from_tar_xz(Cursor::new(b"\xfd7zXZ garbage".to_vec()), TARGET, &mut out)
                .is_err()
        );
    }

    #[test]
    fn version_banner_is_parsed() {
        assert_eq!(
            parse_version("ffmpeg version 7.1-essentials_build-www.gyan.dev Copyright ...\nbuilt"),
            Some("7.1-essentials_build-www.gyan.dev".to_string())
        );
        assert_eq!(parse_version("not ffmpeg"), None);
    }

    #[test]
    fn busy_slot_is_exclusive_and_released_on_drop() {
        let first = Busy::acquire().unwrap();
        assert!(Busy::acquire().is_err());
        drop(first);
        assert!(Busy::acquire().is_ok());
    }

    #[test]
    fn file_name_is_the_last_url_segment() {
        assert_eq!(
            file_name_of("https://example.com/a/b/ffmpeg.zip"),
            "ffmpeg.zip"
        );
    }
}
