use crate::formats;
use crate::office::EmbeddedImageInfo;
use crate::parser::{self, Block, BlockKind, ParsedDoc, PdfPageMeta};
use crate::win_ocr;
use base64::Engine as _;
use lna_llm::{
    LlmClient, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions, ResolvedProfile, Role,
    ToolMode,
};
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
    let out = std::env::temp_dir().join(format!(
        "lna_aud_{}_{}.mp3",
        std::process::id(),
        lna_types::now_ms()
    ));
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-y", "-i"])
        .arg(input_path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-b:a", "64k"])
        .arg(&out);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let ok = matches!(cmd.output(), Ok(o) if o.status.success());
    let bytes = if ok { std::fs::read(&out).ok() } else { None };
    let _ = std::fs::remove_file(&out);
    bytes
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

    let req = LlmRequest {
        system: "You are a high-precision multimodal media understanding assistant. Strictly based on the actual speech and visual content in the media, output clear, searchable structured Markdown in the primary language of the media.".into(),
        messages: vec![LlmMessage {
            role: Role::User,
            parts: vec![media_part, Part::Text(prompt)],
        }],
        tools: vec![],
        tool_mode: ToolMode::None,
        output: OutputMode::Text,
        max_output_tokens: resolved.max_output_tokens.max(4096),
        reasoning: ReasoningOptions::default(),
    };

    let res = llm
        .complete_text(&resolved, &req, CancellationToken::new())
        .await;

    if let Some(ref fname) = uploaded_gemini_name {
        llm.delete_gemini_file(&resolved, fname).await;
    }

    let (md_text, usage) = res?;
    let _ = lna_storage::record_usage(
        db,
        Some(&src.notebook_id),
        Some(&resolved.id),
        "ingest_media",
        Some(&src.id),
        usage.input_tokens as i64,
        usage.output_tokens as i64,
        usage.cached_tokens as i64,
        false,
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
            let req = LlmRequest {
                system: "You are a high-precision image analysis and OCR assistant. Output only text and visual facts directly observed in the image, using Markdown tables and LaTeX formulas where appropriate.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![
                        Part::Media {
                            mime_type: mime,
                            data_base64: Some(b64),
                            file_uri: None,
                            audio_format: None,
                        },
                        Part::Text(prompt),
                    ],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: resolved.max_output_tokens.max(4096),
                reasoning: ReasoningOptions::default(),
            };
            if let Ok((txt, usage)) = llm
                .complete_text(&resolved, &req, CancellationToken::new())
                .await
            {
                let _ = lna_storage::record_usage(
                    db,
                    Some(&src.notebook_id),
                    Some(&resolved.id),
                    "ingest_media",
                    Some(&src.id),
                    usage.input_tokens as i64,
                    usage.output_tokens as i64,
                    usage.cached_tokens as i64,
                    false,
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
        for batch in ocr_pages.chunks(4) {
            let rendered = parser::render_pdf_pages_to_jpeg(pdf_path, batch, 1400)?;
            if rendered.is_empty() {
                continue;
            }
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

            let req = LlmRequest {
                system: "You are a professional document OCR assistant. Faithfully transcribe text, Markdown tables, LaTeX formulas, and brief figure annotations visible on each page without fabrication.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts,
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: resolved.max_output_tokens.max(4096),
                reasoning: ReasoningOptions::default(),
            };

            if let Ok((resp_text, usage)) = llm
                .complete_text(&resolved, &req, CancellationToken::new())
                .await
            {
                let _ = lna_storage::record_usage(
                    db,
                    Some(&src.notebook_id),
                    Some(&resolved.id),
                    "ingest_ocr",
                    Some(&src.id),
                    usage.input_tokens as i64,
                    usage.output_tokens as i64,
                    usage.cached_tokens as i64,
                    false,
                );
                let parsed_pages = split_ocr_pages_response(&resp_text, &rendered);
                for (pno, page_txt) in parsed_pages {
                    let trimmed = page_txt.trim();
                    if !trimmed.is_empty() {
                        let start = cursor;
                        cursor += trimmed.len();
                        out_blocks.push(Block {
                            kind: BlockKind::Paragraph,
                            text: trimmed.to_string(),
                            page: Some(pno),
                            section_path: sec_map.get(&pno).cloned().unwrap_or_default(),
                            char_range: start..cursor,
                        });
                    }
                }
            } else {
                // Batch AI failed: try local WinRT OCR fallback for this batch
                if let Ok(local) = run_local_win_ocr_on_pdf_pages(pdf_path, batch) {
                    for (pno, txt_opt) in local {
                        if let Some(txt) = txt_opt {
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
            }
        }
    }

    Ok(out_blocks)
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

        let req = LlmRequest {
            system: "You are a rigorous visual reading assistant. Extract only accurate facts visible in the provided images, including exact text, chart values, and LaTeX equations, without guessing missing text or numbers.".into(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts,
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens.max(4096),
            reasoning: ReasoningOptions::default(),
        };

        let (txt, usage) = llm
            .complete_text(&resolved, &req, CancellationToken::new())
            .await?;
        let _ = lna_storage::record_usage(
            db,
            Some(notebook_id),
            Some(&resolved.id),
            "agent_vision",
            None,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
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
