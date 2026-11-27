use crate::formats;
use crate::office::EmbeddedImageInfo;
use crate::parser::{self, Block, BlockKind, ParsedDoc, PdfPageMeta};
use crate::win_ocr;
use base64::Engine as _;
use lna_llm::{LlmClient, LlmRequest, Part, ResolvedProfile};
use lna_secrets::SecretStore;
use lna_storage::Database;
use lna_types::{ErrorCode, LnaError, LnaResult, ProtocolKind, Source, SourceKind};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const MEDIA_SETTINGS_KEY: &str = "media_processing";
const GEMINI_INLINE_MAX_BYTES: u64 = 15 * 1024 * 1024;
const IMAGE_RESIZE_THRESHOLD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaIndex {
    pub source_id: String,
    pub kind: String,
    pub page_count: Option<i64>,
    pub ocr_pages: Vec<u32>,
    pub pdf_pages: Vec<PdfPageMeta>,
    pub embedded_images: Vec<EmbeddedImageInfo>,
}

pub fn media_index_path(blob_path: &Path) -> std::path::PathBuf {
    blob_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("media_index.json")
}

pub fn write_media_index(blob_path: &Path, idx: &MediaIndex) {
    let p = media_index_path(blob_path);
    if let Ok(json) = serde_json::to_string_pretty(idx) {
        let _ = std::fs::write(p, json);
    }
}

pub fn read_media_index(blob_path: &Path) -> Option<MediaIndex> {
    let p = media_index_path(blob_path);
    let raw = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn is_audio_video_enabled(db: &Database) -> bool {
    lna_storage::get_setting_json(db, MEDIA_SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.get("audio_video_enabled").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

pub fn resolve_modality_profile_with_secret(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    modality: &str,
) -> LnaResult<Option<ResolvedProfile>> {
    let Some(prof) = lna_storage::resolve_modality_profile(db, modality)? else {
        return Ok(None);
    };
    let api_key = match prof.keychain_ref.as_deref() {
        Some(kref) => secrets.get(kref)?,
        None => None,
    };
    Ok(Some(ResolvedProfile {
        id: prof.id,
        display_name: prof.display_name,
        protocol: prof.protocol,
        base_url: prof.base_url,
        model_id: prof.model_id,
        api_key,
        show_thinking: prof.show_thinking,
        effort: prof.effort,
        context_window: prof.context_window,
        max_output_tokens: prof.max_output_tokens,
        capabilities: prof.capabilities,
        extra_headers: prof.extra_headers,
        anthropic_version: "2023-06-01".into(),
        preset: prof.preset.clone(),
        shrink_media: prof.shrink_media,
    }))
}

fn transcode_to_png_via_image_crate(bytes: &[u8]) -> LnaResult<Vec<u8>> {
    use image::codecs::png::PngEncoder;
    use image::ImageEncoder;
    let img = image::load_from_memory(bytes).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Failed to decode image: {e}"),
        )
    })?;
    let mut img = img;
    if img.width() > 2400 || img.height() > 2400 {
        img = img.resize(2400, 2400, image::imageops::FilterType::Lanczos3);
    }
    let rgba = img.to_rgba8();
    let mut png = Vec::new();
    let enc = PngEncoder::new(Cursor::new(&mut png));
    enc.write_image(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| LnaError::internal(format!("Failed to encode PNG: {e}")))?;
    Ok(png)
}

/// Normalizes an image into `(mime_type, raw_bytes)` suitable for the target LLM protocol.
pub fn prepare_image_for_protocol(
    bytes: &[u8],
    ext: &str,
    path_hint: Option<&Path>,
    protocol: ProtocolKind,
) -> LnaResult<(String, Vec<u8>)> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match e.as_str() {
        "png" | "jpeg" | "jpg" | "jpe" | "webp" | "gif" => {
            if bytes.len() > IMAGE_RESIZE_THRESHOLD_BYTES {
                if let Ok(png) = transcode_to_png_via_image_crate(bytes) {
                    return Ok(("image/png".to_string(), png));
                }
            }
            Ok((formats::mime_type_for_ext(&e).to_string(), bytes.to_vec()))
        }
        "bmp" | "ico" | "tif" | "tiff" => {
            let png = transcode_to_png_via_image_crate(bytes)?;
            Ok(("image/png".to_string(), png))
        }
        "heic" | "heif" => {
            if protocol == ProtocolKind::Gemini {
                return Ok((formats::mime_type_for_ext(&e).to_string(), bytes.to_vec()));
            }
            if let Some(png) = try_wic_transcode(bytes, &e, path_hint) {
                return Ok(("image/png".to_string(), png));
            }
            Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                format!(
                    "图片格式 .{e} 无法被当前模型直接解析，请改用 Gemini 模型或安装系统 HEIF 扩展"
                ),
            ))
        }
        "avif" | "jp2" => {
            if let Some(png) = try_wic_transcode(bytes, &e, path_hint) {
                return Ok(("image/png".to_string(), png));
            }
            if protocol == ProtocolKind::Gemini {
                return Ok((formats::mime_type_for_ext(&e).to_string(), bytes.to_vec()));
            }
            Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                format!("图片格式 .{e} 需要系统的 AVIF/JP2 解码器或切换到原生支持的模型"),
            ))
        }
        _ => Ok((formats::mime_type_for_ext(&e).to_string(), bytes.to_vec())),
    }
}

fn try_wic_transcode(bytes: &[u8], ext: &str, path_hint: Option<&Path>) -> Option<Vec<u8>> {
    if let Some(p) = path_hint {
        if let Some(png) = win_ocr::windows_wic_to_png(p) {
            return Some(png);
        }
    }
    let tmp = std::env::temp_dir().join(format!(
        "lna_img_in_{}_{}.{}",
        std::process::id(),
        lna_types::now_ms(),
        ext
    ));
    let res = if std::fs::write(&tmp, bytes).is_ok() {
        win_ocr::windows_wic_to_png(&tmp)
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    res
}

fn try_ffmpeg_to_mp3(input_path: &Path) -> Option<Vec<u8>> {
    let ff = crate::ffmpeg::ffmpeg_path()?;
    crate::ffmpeg::encode_to_bytes(
        &ff,
        input_path,
        &["-vn", "-ac", "1", "-ar", "16000", "-b:a", "64k"],
        "mp3",
    )
    .ok()
}

/// MiMo accepts audio and video only as inline Base64 data URLs (at most 50 MB encoded). A raw
/// file of up to 35 MiB stays inside that once encoded (about 46.7 MiB of Base64).
const MIMO_RAW_MAX_BYTES: u64 = 35 * 1024 * 1024;
const MIMO_VIDEO_EXTS: &[&str] = &["mp4", "mov", "avi", "wmv"];
const MIMO_AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "ogg"];

/// MIME type for a file MiMo should receive; falls back to `formats` for the common cases.
fn mimo_mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flac" => "audio/flac",
        other => formats::mime_type_for_ext(other),
    }
}

fn mimo_size_error(label: &str, size: u64) -> LnaError {
    LnaError::new(
        ErrorCode::CapabilityMissing,
        format!(
            "{label}文件约 {:.1} MB，超过 MiMo 的大小限制：MiMo 仅接受 Base64 内联数据，编码后不超过 50 MB（约合原文件 35 MB）。可在 MiMo 预设中开启“超限时压缩媒体画质”以降低体积，或先自行压缩该文件。",
            size as f64 / (1024.0 * 1024.0)
        ),
    )
}

fn mimo_inline_part(mime: &str, bytes: &[u8]) -> Part {
    Part::Media {
        mime_type: mime.to_string(),
        data_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        file_uri: None,
        audio_format: None,
    }
}

/// The MiMo media part for an audio or video file: the file as-is when its container and size
/// fit, otherwise converted and/or shrunk with ffmpeg (shrinking only when `shrink_media` is on).
async fn mimo_media_part(
    resolved: &ResolvedProfile,
    path: &Path,
    ext: &str,
    kind: SourceKind,
) -> LnaResult<Part> {
    let limit = MIMO_RAW_MAX_BYTES;
    let is_video = kind == SourceKind::Video;
    let label = if is_video { "视频" } else { "音频" };
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let accepted_exts = if is_video {
        MIMO_VIDEO_EXTS
    } else {
        MIMO_AUDIO_EXTS
    };
    let accepted = accepted_exts.contains(&e.as_str());
    let size = std::fs::metadata(path)
        .map_err(|err| LnaError::internal(err.to_string()))?
        .len();

    if accepted && size <= limit {
        let bytes = std::fs::read(path).map_err(|err| LnaError::internal(err.to_string()))?;
        return Ok(mimo_inline_part(mimo_mime_for_ext(&e), &bytes));
    }

    // Too big as-is, or an unsupported container: only ffmpeg can help, and only when the user
    // has allowed re-encoding (for size) or the container must change.
    if accepted && !resolved.shrink_media {
        return Err(mimo_size_error(label, size));
    }
    if crate::ffmpeg::ffmpeg_path().is_none() {
        return Err(if accepted {
            LnaError::new(
                ErrorCode::CapabilityMissing,
                format!(
                    "{label}文件超过 MiMo 的大小限制（约 35 MB），需要 ffmpeg 压缩。{}后重试。",
                    crate::ffmpeg::FFMPEG_INSTALL_HINT
                ),
            )
        } else {
            LnaError::new(
                ErrorCode::CapabilityMissing,
                format!(
                    "MiMo 不直接接受 .{e} 格式的{label}（仅支持 {}），需要 ffmpeg 转换。{}后重试。",
                    accepted_exts.join("/"),
                    crate::ffmpeg::FFMPEG_INSTALL_HINT
                ),
            )
        });
    }

    let (out_mime, converted_to) = if is_video {
        ("video/mp4", "mp4")
    } else {
        ("audio/mpeg", "mp3")
    };
    if !accepted {
        let src = path.to_path_buf();
        let converted = tokio::task::spawn_blocking(move || {
            if is_video {
                crate::ffmpeg::transcode_to_mp4(&src)
            } else {
                crate::ffmpeg::transcode_to_mp3(&src)
            }
        })
        .await
        .map_err(|e| LnaError::internal(e.to_string()))?
        .map_err(|err| {
            LnaError::new(
                ErrorCode::SourceUnsupported,
                format!("无法将 .{e} {label}转换为 {converted_to} 格式以供 MiMo 处理：{err}"),
            )
        })?;
        if converted.len() as u64 <= limit {
            return Ok(mimo_inline_part(out_mime, &converted));
        }
        if !resolved.shrink_media {
            return Err(mimo_size_error(label, converted.len() as u64));
        }
    }

    // Oversized: shrink from the original file with ffmpeg.
    let src = path.to_path_buf();
    let shrunk = tokio::task::spawn_blocking(move || {
        if is_video {
            crate::ffmpeg::shrink_video(&src, limit)
        } else {
            crate::ffmpeg::shrink_audio(&src, limit)
        }
    })
    .await
    .map_err(|e| LnaError::internal(e.to_string()))?
    .map_err(|err| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("{label}压缩失败，无法满足 MiMo 大小限制：{err}"),
        )
    })?;
    Ok(mimo_inline_part(out_mime, &shrunk))
}

/// The media part for an audio or video file as `resolved` accepts it: inline or uploaded
/// (Gemini; large files go through the Files API and the returned name must be deleted after
/// use), or wav/mp3 audio for Chat Completion. Other protocols cannot take raw audio/video.
pub async fn media_part_for_file(
    llm: &LlmClient,
    resolved: &ResolvedProfile,
    path: &Path,
    ext: &str,
    kind: SourceKind,
    title: &str,
) -> LnaResult<(Part, Option<String>)> {
    let label = if kind == SourceKind::Video {
        "视频"
    } else {
        "音频"
    };
    let mime = formats::mime_type_for_ext(ext).to_string();
    let meta = std::fs::metadata(path).map_err(|e| LnaError::internal(e.to_string()))?;
    match resolved.protocol {
        ProtocolKind::Gemini => {
            if meta.len() > GEMINI_INLINE_MAX_BYTES {
                let (uri, name) = llm.upload_gemini_file(resolved, path, &mime, title).await?;
                Ok((
                    Part::Media { mime_type: mime, data_base64: None, file_uri: Some(uri), audio_format: None },
                    Some(name),
                ))
            } else {
                let bytes = std::fs::read(path).map_err(|e| LnaError::internal(e.to_string()))?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                Ok((
                    Part::Media { mime_type: mime, data_base64: Some(b64), file_uri: None, audio_format: None },
                    None,
                ))
            }
        }
        ProtocolKind::ChatCompletion
            if resolved.is_mimo() && matches!(kind, SourceKind::Audio | SourceKind::Video) =>
        {
            Ok((mimo_media_part(resolved, path, ext, kind).await?, None))
        }
        ProtocolKind::ChatCompletion if kind == SourceKind::Audio => {
            let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            if e == "wav" || e == "mp3" {
                let bytes = std::fs::read(path).map_err(|err| LnaError::internal(err.to_string()))?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                Ok((
                    Part::Media { mime_type: mime, data_base64: Some(b64), file_uri: None, audio_format: Some(e) },
                    None,
                ))
            } else if let Some(mp3_bytes) = try_ffmpeg_to_mp3(path) {
                let b64 = base64::engine::general_purpose::STANDARD.encode(mp3_bytes);
                Ok((
                    Part::Media {
                        mime_type: "audio/mpeg".into(),
                        data_base64: Some(b64),
                        file_uri: None,
                        audio_format: Some("mp3".into()),
                    },
                    None,
                ))
            } else {
                Err(LnaError::new(
                    ErrorCode::CapabilityMissing,
                    format!("Chat Completion 协议仅支持原生 wav/mp3；格式 .{e} 请在“模型协议配置”中为音频勾选 Gemini 模型"),
                ))
            }
        }
        _ => Err(LnaError::new(
            ErrorCode::CapabilityMissing,
            format!("当前勾选处理{label}的模型协议（{}）不支持原生{label}输入，请在“模型协议配置”中使用 Gemini 协议模型。", resolved.protocol.as_str()),
        )),
    }
}

pub(crate) fn record_llm_usage(
    db: &Database,
    notebook_id: Option<&str>,
    profile_id: &str,
    scope: &str,
    subject_id: Option<&str>,
    usage: &lna_llm::Usage,
) {
    let _ = lna_storage::record_usage(
        db,
        notebook_id,
        Some(profile_id),
        scope,
        subject_id,
        usage.input_tokens as i64,
        usage.output_tokens as i64,
        usage.cached_tokens as i64,
        false,
    );
}

pub async fn ingest_audio_video(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    src: &Source,
    blob_path: &Path,
    ext: &str,
) -> LnaResult<ParsedDoc> {
    if !is_audio_video_enabled(db) {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "已在设置中关闭音视频文件处理，如需导入请在“模型协议配置”中开启。",
        ));
    }

    let modality = match src.kind {
        SourceKind::Video => "video",
        _ => "audio",
    };
    let label = if src.kind == SourceKind::Video {
        "视频"
    } else {
        "音频"
    };

    let resolved = resolve_modality_profile_with_secret(db, secrets, modality)?.ok_or_else(|| {
        LnaError::new(
            ErrorCode::ModelNotConfigured,
            format!("未配置用于处理{label}的模型，请在“模型协议配置”中为支持{label}的模型勾选“用于处理{label}”（推荐 Gemini），或关闭音视频文件处理。"),
        )
    })?;

    let (media_part, uploaded_gemini_name) =
        media_part_for_file(llm, &resolved, blob_path, ext, src.kind, &src.title).await?;

    let prompt = if src.kind == SourceKind::Video {
        format!(
            "请完整解析这段视频（文件名：{}），输出结构化的 Markdown 文档：\n\
            1. 使用一级标题 `# 内容概要` 总结核心主题与结论。\n\
            2. 使用二级标题 `## 详细内容与时间轴` 按时间顺序以 `[MM:SS]` 标记关键节点，完整转录语音内容，并补充画面中的重要文字、图表、公式或操作步骤。\n\
            3. 如有多位说话人请加以区分；不要编造未出现的信息。",
            src.title
        )
    } else {
        format!(
            "请完整转录并整理这段音频（文件名：{}），输出结构化的 Markdown 文档：\n\
            1. 使用一级标题 `# 内容概要` 总结核心主题与要点。\n\
            2. 使用二级标题 `## 逐段转录` 按 `[MM:SS]` 时间戳分段忠实转录全部语音内容；如能区分说话人请标明；数学公式用 LaTeX 表示。\n\
            3. 不要省略关键论据、数字或专有名词。",
            src.title
        )
    };

    let req = LlmRequest::from_parts(
        "You are a high-precision multimodal media understanding assistant. Strictly based on the actual speech and visual content in the media, output clear, searchable structured Markdown in the primary language of the media.",
        vec![media_part, Part::Text(prompt)],
        resolved.output_budget().max(4096),
    );

    let res = llm
        .complete_text(&resolved, &req, CancellationToken::new())
        .await;

    if let Some(ref fname) = uploaded_gemini_name {
        llm.delete_gemini_file(&resolved, fname).await;
    }

    let (md_text, usage) = res?;
    record_llm_usage(
        db,
        Some(&src.notebook_id),
        &resolved.id,
        "ingest_media",
        Some(&src.id),
        &usage,
    );

    let trimmed = md_text.trim();
    if trimmed.is_empty() {
        return Err(LnaError::new(
            ErrorCode::OutputInvalid,
            "模型未返回任何音视频解析内容",
        ));
    }
    Ok(parser::parse_markdown(trimmed))
}

pub async fn ingest_image(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    src: &Source,
    blob_path: &Path,
    ext: &str,
) -> LnaResult<ParsedDoc> {
    let raw = std::fs::read(blob_path).map_err(|e| LnaError::internal(e.to_string()))?;

    if let Some(resolved) = resolve_modality_profile_with_secret(db, secrets, "vision")? {
        if let Ok((mime, prepared)) =
            prepare_image_for_protocol(&raw, ext, Some(blob_path), resolved.protocol)
        {
            let b64 = base64::engine::general_purpose::STANDARD.encode(prepared);
            let prompt = format!(
                "请完整识别并整理这张图片（文件名：{}），输出结构化 Markdown：\n\
                - 忠实提取图片中的全部文字，表格用 Markdown 表格表示，公式用 LaTeX（`$...$` / `$$...$$`）表示；\n\
                - 若图片包含图表、示意图、流程图或照片，请用清晰条目描述其中的数据趋势、结构关系与关键视觉细节。",
                src.title
            );
            let req = LlmRequest::from_parts(
                "You are a high-precision image analysis and OCR assistant. Output only text and visual facts directly observed in the image, using Markdown tables and LaTeX formulas where appropriate.",
                vec![
                    Part::Media {
                        mime_type: mime,
                        data_base64: Some(b64),
                        file_uri: None,
                        audio_format: None,
                    },
                    Part::Text(prompt),
                ],
                resolved.output_budget().max(4096),
            );
            if let Ok((txt, usage)) = llm
                .complete_text(&resolved, &req, CancellationToken::new())
                .await
            {
                record_llm_usage(
                    db,
                    Some(&src.notebook_id),
                    &resolved.id,
                    "ingest_media",
                    Some(&src.id),
                    &usage,
                );
                if !txt.trim().is_empty() {
                    return Ok(parser::parse_markdown(&txt));
                }
            }
        }
    }

    // Offline fallback: Windows built-in WinRT OCR
    let (ocr_bytes, ocr_ext) = if matches!(
        ext.to_ascii_lowercase().as_str(),
        "avif" | "heic" | "heif" | "jp2"
    ) {
        if let Some(png) = try_wic_transcode(&raw, ext, Some(blob_path)) {
            (png, "png".to_string())
        } else {
            (raw, ext.to_string())
        }
    } else if matches!(
        ext.to_ascii_lowercase().as_str(),
        "bmp" | "ico" | "tif" | "tiff"
    ) {
        if let Ok(png) = transcode_to_png_via_image_crate(&raw) {
            (png, "png".to_string())
        } else {
            (raw, ext.to_string())
        }
    } else {
        (raw, ext.to_string())
    };

    if let Some(ocr_txt) = win_ocr::windows_ocr_image_bytes(&ocr_bytes, &ocr_ext) {
        return Ok(parser::parse_plain_text(&ocr_txt));
    }

    Err(LnaError::new(
        ErrorCode::SourceNeedsOcr,
        "未配置支持图文的 AI 模型且本地 OCR 未能提取文字，请在“模型协议配置”中为视觉模型勾选“用于处理图文”。",
    ))
}

/// OCRs scanned / garbled-CMap PDF pages into page-numbered Blocks.
/// Uses Vision AI batches for <= 20 OCR pages; uses fast local Windows WinRT OCR first for large scanned books (>20 pages)
/// while still allowing the Agent to inspect individual pages visually on demand during chat.
pub async fn ocr_pdf_pages(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    src: &Source,
    pdf_path: &Path,
    ocr_pages: &[u32],
    page_meta: &[PdfPageMeta],
) -> LnaResult<Vec<Block>> {
    if ocr_pages.is_empty() {
        return Ok(Vec::new());
    }

    let vision_prof = resolve_modality_profile_with_secret(db, secrets, "vision")?;
    let sec_map: std::collections::HashMap<u32, Vec<String>> = page_meta
        .iter()
        .map(|m| (m.page, m.section_path.clone()))
        .collect();

    let mut out_blocks = Vec::new();
    let mut cursor = 0usize;

    let prefer_local_first = ocr_pages.len() > 20 || vision_prof.is_none();

    if prefer_local_first {
        let local_texts = run_local_win_ocr_on_pdf_pages(pdf_path, ocr_pages)?;
        for (page_no, txt_opt) in local_texts {
            if let Some(txt) = txt_opt {
                let start = cursor;
                cursor += txt.len();
                out_blocks.push(Block {
                    kind: BlockKind::Paragraph,
                    text: txt,
                    page: Some(page_no),
                    section_path: sec_map.get(&page_no).cloned().unwrap_or_default(),
                    char_range: start..cursor,
                });
            }
        }
        if !out_blocks.is_empty() || vision_prof.is_none() {
            return Ok(out_blocks);
        }
    }

    if let Some(resolved) = vision_prof {
        // Orchestrator: split the pages into batches; every batch is an executor (render, model
        // call, local-OCR fallback) and up to the profile's request limit of them run at once.
        // Results are put back in page order.
        let batches: Vec<Vec<u32>> = ocr_pages.chunks(4).map(|c| c.to_vec()).collect();
        let limit = llm.parallel_limit(&resolved.id).clamp(1, 64);
        let resolved_ref = &resolved;
        let batch_pages = lna_llm::run_parallel_ordered(
            batches,
            limit,
            &CancellationToken::new(),
            |_, batch| async move {
                Ok(ocr_pdf_batch(db, llm, resolved_ref, src, pdf_path, batch).await)
            },
        )
        .await?;
        for pages in batch_pages {
            for (pno, txt) in pages {
                let start = cursor;
                cursor += txt.len();
                out_blocks.push(Block {
                    kind: BlockKind::Paragraph,
                    text: txt,
                    page: Some(pno),
                    section_path: sec_map.get(&pno).cloned().unwrap_or_default(),
                    char_range: start..cursor,
                });
            }
        }
    }

    Ok(out_blocks)
}

/// One OCR executor: renders up to four pages, asks the vision model and falls back to the local
/// OCR when the model fails. Returns the recognised `(page, text)` pairs.
async fn ocr_pdf_batch(
    db: &Database,
    llm: &LlmClient,
    resolved: &ResolvedProfile,
    src: &Source,
    pdf_path: &Path,
    batch: Vec<u32>,
) -> Vec<(u32, String)> {
    let (pdf, pages) = (pdf_path.to_path_buf(), batch.clone());
    let rendered = match tokio::task::spawn_blocking(move || {
        parser::render_pdf_pages_to_jpeg(&pdf, &pages, 1400)
    })
    .await
    {
        Ok(Ok(r)) if !r.is_empty() => r,
        _ => return Vec::new(),
    };
    let page_list: Vec<String> = rendered.iter().map(|(p, _)| p.to_string()).collect();
    let mut parts: Vec<Part> = Vec::with_capacity(rendered.len() * 2 + 1);
    for (pno, jpg) in &rendered {
        parts.push(Part::Text(format!("[第 {pno} 页图片]")));
        parts.push(Part::Media {
            mime_type: "image/jpeg".into(),
            data_base64: Some(base64::engine::general_purpose::STANDARD.encode(jpg)),
            file_uri: None,
            audio_format: None,
        });
    }
    parts.push(Part::Text(format!(
        "请逐页准确识别以上 PDF 页面（页码：{}）中的文字、表格与公式（公式用 LaTeX，表格用 Markdown）。\n\
        务必严格按如下分隔符输出每页内容，空白页可只输出分隔符不加正文：\n\
        === PAGE <页码> ===",
        page_list.join(", ")
    )));
    let req = LlmRequest::from_parts(
        "You are a professional document OCR assistant. Faithfully transcribe text, Markdown tables, LaTeX formulas, and brief figure annotations visible on each page without fabrication.",
        parts,
        resolved.output_budget().max(4096),
    );
    match llm
        .complete_text(resolved, &req, CancellationToken::new())
        .await
    {
        Ok((resp_text, usage)) => {
            record_llm_usage(
                db,
                Some(&src.notebook_id),
                &resolved.id,
                "ingest_ocr",
                Some(&src.id),
                &usage,
            );
            split_ocr_pages_response(&resp_text, &rendered)
                .into_iter()
                .map(|(pno, t)| (pno, t.trim().to_string()))
                .filter(|(_, t)| !t.is_empty())
                .collect()
        }
        Err(_) => {
            // Batch AI failed: try local WinRT OCR fallback for this batch
            let (pdf, pages) = (pdf_path.to_path_buf(), batch);
            tokio::task::spawn_blocking(move || run_local_win_ocr_on_pdf_pages(&pdf, &pages))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|v| {
                    v.into_iter()
                        .filter_map(|(pno, t)| t.map(|t| (pno, t)))
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

fn run_local_win_ocr_on_pdf_pages(
    pdf_path: &Path,
    pages: &[u32],
) -> LnaResult<Vec<(u32, Option<String>)>> {
    let mut results = Vec::with_capacity(pages.len());
    for batch in pages.chunks(16) {
        let rendered = parser::render_pdf_pages_to_jpeg(pdf_path, batch, 1400)?;
        let mut tmp_paths = Vec::with_capacity(rendered.len());
        let batch_tag = format!("{}_{}", std::process::id(), lna_types::now_ms());
        for (pno, jpg) in &rendered {
            let p = std::env::temp_dir().join(format!("lna_pdfocr_{batch_tag}_{pno}.jpg"));
            if std::fs::write(&p, jpg).is_ok() {
                tmp_paths.push((*pno, p));
            }
        }
        let path_refs: Vec<&Path> = tmp_paths.iter().map(|(_, p)| p.as_path()).collect();
        let recognized = win_ocr::windows_ocr_image_files(&path_refs);
        for ((pno, p), txt) in tmp_paths.into_iter().zip(recognized) {
            let _ = std::fs::remove_file(p);
            results.push((pno, txt));
        }
    }
    Ok(results)
}

fn split_ocr_pages_response(resp: &str, batch: &[(u32, Vec<u8>)]) -> Vec<(u32, String)> {
    let mut result: Vec<(u32, String)> = Vec::new();
    let mut current_page: Option<u32> = None;
    let mut buf = String::new();

    for line in resp.lines() {
        let trimmed = line.trim();
        if let Some(pno) = parse_page_marker(trimmed) {
            if let Some(prev) = current_page {
                let s = buf.trim().to_string();
                if !s.is_empty() {
                    result.push((prev, s));
                }
            }
            buf.clear();
            current_page = Some(pno);
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        }
    }
    if let Some(last) = current_page {
        let s = buf.trim().to_string();
        if !s.is_empty() {
            result.push((last, s));
        }
    }

    if result.is_empty() && !resp.trim().is_empty() {
        if let Some((first_pno, _)) = batch.first() {
            result.push((*first_pno, resp.trim().to_string()));
        }
    }
    result
}

fn parse_page_marker(line: &str) -> Option<u32> {
    if !(line.contains("===") || line.starts_with("---")) {
        return None;
    }
    let upper = line.to_ascii_uppercase();
    if let Some(pos) = upper.find("PAGE") {
        let after = &upper[pos + 4..];
        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return digits.parse::<u32>().ok();
    }
    None
}

/// Visual inspection helper invoked by `lna-agent` tools (`inspect_source_pages`, `inspect_source_images`).
/// Routes the provided images + optional text context to the user's configured `"vision"` model
/// (or falls back to local WinRT OCR when no vision model is configured).
pub async fn vision_inspect_images(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    notebook_id: &str,
    images: Vec<(String, String, Vec<u8>)>,
    focus_question: &str,
) -> LnaResult<String> {
    if images.is_empty() {
        return Ok("未找到可查看的图片。".into());
    }

    if let Some(resolved) = resolve_modality_profile_with_secret(db, secrets, "vision")? {
        let mut parts: Vec<Part> = Vec::with_capacity(images.len() * 2 + 1);
        for (label, ext, raw) in &images {
            match prepare_image_for_protocol(raw, ext, None, resolved.protocol) {
                Ok((mime, prepared)) => {
                    parts.push(Part::Text(format!("[{label}]")));
                    parts.push(Part::Media {
                        mime_type: mime,
                        data_base64: Some(
                            base64::engine::general_purpose::STANDARD.encode(prepared),
                        ),
                        file_uri: None,
                        audio_format: None,
                    });
                }
                Err(e) => {
                    parts.push(Part::Text(format!("[{label} 图片转码跳过：{}]", e.message)));
                }
            }
        }
        parts.push(Part::Text(format!(
            "请结合以上提供的图片与标记，精确回答或提取以下所需信息：\n{focus_question}\n\
            要求：只报告图片中确实存在的文字、图表数值、公式（用 LaTeX）和视觉细节，标注对应页码或图片名称。"
        )));

        let req = LlmRequest::from_parts(
            "You are a rigorous visual reading assistant. Extract only accurate facts visible in the provided images, including exact text, chart values, and LaTeX equations, without guessing missing text or numbers.",
            parts,
            resolved.output_budget().max(4096),
        );

        let (txt, usage) = llm
            .complete_text(&resolved, &req, CancellationToken::new())
            .await?;
        record_llm_usage(
            db,
            Some(notebook_id),
            &resolved.id,
            "agent_vision",
            None,
            &usage,
        );
        return Ok(txt);
    }

    // No vision model configured: run local WinRT OCR fallback
    let mut out =
        String::from("（提示：当前未勾选任何支持“图文”的模型，仅执行本地文字 OCR 回退识别）\n\n");
    let mut any_text = false;
    for (label, ext, raw) in images {
        let ocr = win_ocr::windows_ocr_image_bytes(&raw, &ext);
        if let Some(txt) = ocr {
            any_text = true;
            out.push_str(&format!("### {label}\n{txt}\n\n"));
        }
    }
    if !any_text {
        out.push_str("未能通过本地 OCR 识别出文字；如需分析图表或复杂版面，请在“模型协议配置”中勾选一个支持“图文”的多模态模型。");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn profile(preset: Option<&str>, shrink_media: bool) -> ResolvedProfile {
        ResolvedProfile {
            id: "p_media".into(),
            display_name: "media".into(),
            protocol: ProtocolKind::ChatCompletion,
            base_url: "https://example.com/v1".into(),
            model_id: "media-test".into(),
            api_key: None,
            show_thinking: false,
            effort: None,
            context_window: 32000,
            max_output_tokens: 1024,
            capabilities: lna_types::ModelCapabilities::default(),
            extra_headers: BTreeMap::new(),
            anthropic_version: "2023-06-01".into(),
            preset: preset.map(str::to_string),
            shrink_media,
        }
    }

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("lna_media_test_{}_{}", std::process::id(), name));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn mimo_detection_follows_preset() {
        assert!(profile(Some("mimo"), false).is_mimo());
        assert!(!profile(None, false).is_mimo());
        assert!(!profile(Some("gemini"), false).is_mimo());
    }

    #[tokio::test]
    async fn mimo_within_limit_passes_bytes_through_as_base64() {
        let bytes = b"fake mp3 payload";
        let path = temp_file("pass.mp3", bytes);
        let part = mimo_media_part(
            &profile(Some("mimo"), false),
            &path,
            "mp3",
            SourceKind::Audio,
        )
        .await
        .unwrap();
        let _ = std::fs::remove_file(&path);
        match part {
            Part::Media {
                mime_type,
                data_base64,
                file_uri,
                audio_format,
            } => {
                assert_eq!(mime_type, "audio/mpeg");
                assert_eq!(
                    data_base64.as_deref(),
                    Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(bytes)
                            .as_str()
                    )
                );
                assert!(file_uri.is_none());
                assert!(audio_format.is_none());
            }
            other => panic!("unexpected part: {other:?}"),
        }
    }

    #[tokio::test]
    async fn mimo_video_mov_uses_quicktime_mime() {
        let path = temp_file("pass.mov", b"mov bytes");
        let part = mimo_media_part(
            &profile(Some("mimo"), false),
            &path,
            ".MOV",
            SourceKind::Video,
        )
        .await
        .unwrap();
        let _ = std::fs::remove_file(&path);
        match part {
            Part::Media { mime_type, .. } => assert_eq!(mime_type, "video/quicktime"),
            other => panic!("unexpected part: {other:?}"),
        }
    }

    #[tokio::test]
    async fn mimo_over_limit_without_shrink_reports_size_and_limit() {
        // Sparse file just over the raw limit: nothing is read, so no real disk use.
        let path =
            std::env::temp_dir().join(format!("lna_media_test_{}_big.mp4", std::process::id()));
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MIMO_RAW_MAX_BYTES + 1).unwrap();
        drop(f);

        let err = mimo_media_part(
            &profile(Some("mimo"), false),
            &path,
            "mp4",
            SourceKind::Video,
        )
        .await
        .unwrap_err();
        let _ = std::fs::remove_file(&path);

        assert_eq!(err.code, ErrorCode::CapabilityMissing);
        assert!(err.message.contains("35.0 MB"), "{}", err.message);
        assert!(err.message.contains("50 MB"), "{}", err.message);
        assert!(
            err.message.contains("超限时压缩媒体画质"),
            "{}",
            err.message
        );
    }

    #[tokio::test]
    async fn mimo_unsupported_container_without_ffmpeg_or_shrink_is_an_error() {
        // Whether ffmpeg exists on this machine decides the message, not the outcome: either
        // way a .mkv must not be sent to MiMo unconverted.
        let path = temp_file("clip.mkv", b"not really mkv");
        let res = mimo_media_part(
            &profile(Some("mimo"), false),
            &path,
            "mkv",
            SourceKind::Video,
        )
        .await;
        let _ = std::fs::remove_file(&path);
        assert!(res.is_err());
    }
}
