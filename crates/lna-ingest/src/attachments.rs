//! Images the user attaches to a chat message or a Studio request.
//!
//! The client sends raw bytes (base64). They are decoded, downscaled and re-encoded once and
//! stored under `<data root>/attachments/`; the stored [`Attachment`] carries the file path so
//! every client can show a thumbnail without another round trip. When the model that will read
//! the message cannot see images, the pictures are described by the configured vision model
//! (or read by local OCR) and the description is sent as text instead.

use base64::Engine as _;
use image::{imageops::FilterType, DynamicImage, ImageFormat};
use lna_llm::{
    LlmClient, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions, ResolvedProfile, Role,
    ToolMode,
};
use lna_secrets::SecretStore;
use lna_storage::Database;
use lna_types::{new_ulid, Attachment, LnaError, LnaResult};
use serde_json::Value;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Pictures per message. More than this is rarely useful and multiplies the token cost.
pub const MAX_IMAGES_PER_MESSAGE: usize = 6;
/// Largest raw upload accepted for one picture.
const MAX_INPUT_BYTES: usize = 25 * 1024 * 1024;
/// Longest edge kept after downscaling; larger is wasted tokens for every provider.
const MAX_EDGE: u32 = 1568;
const JPEG_QUALITY: u8 = 85;

pub fn attachments_dir(root: &Path) -> PathBuf {
    root.join("attachments")
}

/// Decodes, downscales and stores the images of a request parameter such as
/// `[{ "name": "a.png", "data_base64": "..." }]`. Returns the stored attachments.
pub fn save_images(root: &Path, items: &Value) -> LnaResult<Vec<Attachment>> {
    let Some(arr) = items.as_array() else {
        return Ok(Vec::new());
    };
    if arr.len() > MAX_IMAGES_PER_MESSAGE {
        return Err(LnaError::invalid(format!(
            "一次最多附带 {MAX_IMAGES_PER_MESSAGE} 张图片"
        )));
    }
    let dir = attachments_dir(root);
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let b64 = item
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| LnaError::invalid("image.data_base64 required"))?;
        let b64 = b64.rsplit(',').next().unwrap_or(b64); // tolerate a data: URL prefix
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| LnaError::invalid(format!("图片数据无效：{e}")))?;
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("image-{}", i + 1));
        out.push(store_one(&dir, &name, &raw)?);
    }
    Ok(out)
}

fn store_one(dir: &Path, name: &str, raw: &[u8]) -> LnaResult<Attachment> {
    if raw.is_empty() {
        return Err(LnaError::invalid(format!("图片「{name}」是空文件")));
    }
    if raw.len() > MAX_INPUT_BYTES {
        return Err(LnaError::invalid(format!(
            "图片「{name}」超过 {} MB",
            MAX_INPUT_BYTES / 1024 / 1024
        )));
    }
    let img = image::load_from_memory(raw).map_err(|_| {
        LnaError::invalid(format!(
            "无法识别图片「{name}」的格式（支持 PNG、JPEG、WebP、GIF、BMP）"
        ))
    })?;
    let img = if img.width().max(img.height()) > MAX_EDGE {
        img.resize(MAX_EDGE, MAX_EDGE, FilterType::Triangle)
    } else {
        img
    };
    let has_alpha = img.color().has_alpha();
    let (bytes, mime, ext) = encode(&img, has_alpha)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| LnaError::internal(format!("无法创建附件目录：{e}")))?;
    let id = new_ulid();
    let path = dir.join(format!("{id}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| LnaError::internal(format!("无法保存图片：{e}")))?;
    let att = Attachment {
        id,
        name: name.to_string(),
        mime: mime.to_string(),
        path: path.to_string_lossy().into_owned(),
        width: img.width(),
        height: img.height(),
        bytes: bytes.len() as u64,
        kind: "image".into(),
        url: None,
    };
    write_meta(dir, &att);
    Ok(att)
}

/// `<id>.meta.json`: the attachment record without its path, so Studio configs (which keep only
/// ids) can find name / kind / url again.
pub(crate) fn write_meta(dir: &Path, att: &Attachment) {
    let mut v = serde_json::to_value(att).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("path");
    }
    let _ = std::fs::write(dir.join(format!("{}.meta.json", att.id)), v.to_string());
}

/// The id part of a file name in the attachments directory (`<id>.<ext>`, `<id>.meta.json`, ...).
fn id_of_file_name(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// Opaque pictures become JPEG (small); anything with transparency stays PNG.
fn encode(img: &DynamicImage, has_alpha: bool) -> LnaResult<(Vec<u8>, &'static str, &'static str)> {
    let mut buf = Cursor::new(Vec::new());
    if has_alpha {
        img.write_to(&mut buf, ImageFormat::Png)
            .map_err(|e| LnaError::internal(format!("图片编码失败：{e}")))?;
        Ok((buf.into_inner(), "image/png", "png"))
    } else {
        let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
        rgb.write_with_encoder(enc)
            .map_err(|e| LnaError::internal(format!("图片编码失败：{e}")))?;
        Ok((buf.into_inner(), "image/jpeg", "jpg"))
    }
}

/// Attachments stored as JSON (`chat_messages.attachments_json`, an artifact's `config.images`).
pub fn parse_stored(value: &Value) -> Vec<Attachment> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

pub fn parse_stored_str(s: &str) -> Vec<Attachment> {
    serde_json::from_str::<Vec<Attachment>>(s).unwrap_or_default()
}

/// Studio artifacts keep only attachment ids in their config (it is echoed into prompts, and a
/// path would leak the local user name); this finds the stored files again.
pub fn resolve(root: &Path, ids: &Value) -> Vec<Attachment> {
    let dir = attachments_dir(root);
    let mut out = Vec::new();
    for (i, id) in ids
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
    {
        // ids are ULIDs we generated; reject anything that could escape the directory
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if let Some(a) = find_by_id(&dir, id) {
            out.push(a);
            continue;
        }
        for (ext, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
            let path = dir.join(format!("{id}.{ext}"));
            if let Ok(meta) = std::fs::metadata(&path) {
                out.push(Attachment {
                    id: id.to_string(),
                    name: format!("image-{}", i + 1),
                    mime: mime.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    width: 0,
                    height: 0,
                    bytes: meta.len(),
                    kind: "image".into(),
                    url: None,
                });
                break;
            }
        }
    }
    out
}

/// The stored attachment with this id, from its `.meta.json` sidecar.
fn find_by_id(dir: &Path, id: &str) -> Option<Attachment> {
    let raw = std::fs::read_to_string(dir.join(format!("{id}.meta.json"))).ok()?;
    let mut v: Value = serde_json::from_str(&raw).ok()?;
    let entries = std::fs::read_dir(dir).ok()?;
    let data_file = entries.flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        id_of_file_name(n) == id && !n.ends_with(".meta.json") && !n.ends_with(".parsed.json")
    });
    let path = data_file.unwrap_or_else(|| dir.join(format!("{id}.meta.json")));
    v["path"] = Value::String(path.to_string_lossy().into_owned());
    serde_json::from_value(v).ok()
}

pub fn ids_of(atts: &[Attachment]) -> Value {
    Value::Array(atts.iter().map(|a| Value::String(a.id.clone())).collect())
}

/// Removes stored pictures no chat message or Studio artifact refers to and that are older than
/// `min_age` (pictures of lecture questions, of failed sends, of deleted notebooks). Returns the
/// number of files removed.
pub fn sweep_orphans(db: &Database, min_age: std::time::Duration) -> usize {
    let dir = attachments_dir(&db.paths().root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let _ = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT attachments_json FROM chat_messages WHERE attachments_json IS NOT NULL")
            .map_err(lna_storage::db::map_sql_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(lna_storage::db::map_sql_err)?;
        for r in rows.flatten() {
            used.extend(parse_stored_str(&r).into_iter().map(|a| a.id));
        }
        for key in ["images", "files"] {
            let sql = format!(
                "SELECT json_extract(config_json, '$.{key}') FROM studio_artifacts WHERE json_extract(config_json, '$.{key}') IS NOT NULL"
            );
            let mut stmt = conn.prepare(&sql).map_err(lna_storage::db::map_sql_err)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(lna_storage::db::map_sql_err)?;
            for r in rows.flatten() {
                if let Ok(Value::Array(ids)) = serde_json::from_str::<Value>(&r) {
                    used.extend(ids.iter().filter_map(Value::as_str).map(String::from));
                }
            }
        }
        Ok(())
    });
    let mut removed = 0;
    for e in entries.flatten() {
        let path = e.path();
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if used.contains(id_of_file_name(fname)) {
            continue;
        }
        let old_enough = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= min_age);
        if old_enough && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Deletes the stored files with their sidecars (metadata, parse cache), best effort.
pub fn delete_files(atts: &[Attachment]) {
    for a in atts {
        let _ = std::fs::remove_file(&a.path);
        let Some(dir) = Path::new(&a.path).parent() else {
            continue;
        };
        for suffix in ["meta.json", "parsed.json"] {
            let _ = std::fs::remove_file(dir.join(format!("{}.{suffix}", a.id)));
        }
    }
}

/// Only the pictures among `atts` (files and links are read by the agent through tools).
pub fn images(atts: &[Attachment]) -> Vec<Attachment> {
    atts.iter().filter(|a| a.is_image()).cloned().collect()
}

fn read_base64(a: &Attachment) -> Option<String> {
    std::fs::read(&a.path)
        .ok()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
}

/// `[图片 n：name]` label + image part for each stored picture (lost files are noted in text).
fn image_parts(atts: &[Attachment]) -> Vec<Part> {
    let mut parts = Vec::with_capacity(atts.len() * 2);
    for (i, a) in atts.iter().enumerate() {
        let Some(b64) = read_base64(a) else {
            parts.push(Part::Text(format!("[图片「{}」已丢失]", a.name)));
            continue;
        };
        parts.push(Part::Text(format!("[图片 {}：{}]", i + 1, a.name)));
        parts.push(Part::Media {
            mime_type: a.mime.clone(),
            data_base64: Some(b64),
            file_uri: None,
            audio_format: None,
        });
    }
    parts
}

/// The user message parts for `text` plus its pictures: image parts followed by the text when the
/// model can see images, otherwise the text with a description of the pictures appended.
pub async fn user_parts(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    notebook_id: &str,
    profile: &ResolvedProfile,
    text: String,
    atts: &[Attachment],
) -> Vec<Part> {
    let pics = images(atts);
    let atts = pics.as_slice();
    if atts.is_empty() {
        return vec![Part::Text(text)];
    }
    if profile.capabilities.supports_vision {
        let mut parts = image_parts(atts);
        parts.push(Part::Text(text));
        return parts;
    }
    let described = describe(db, secrets, llm, notebook_id, profile, atts, &text).await;
    vec![Part::Text(format!(
        "{text}\n\n<attached_images note=\"用户附带了图片；当前模型无法直接看图，以下是图片内容的识别结果\">\n{described}\n</attached_images>"
    ))]
}

/// Text rendering of the pictures. The given model reads them itself when it can see images;
/// otherwise the configured vision model does, falling back to local OCR.
pub async fn describe(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    notebook_id: &str,
    profile: &ResolvedProfile,
    atts: &[Attachment],
    question: &str,
) -> String {
    // Only the content goes into the result: remarks about picture quality or the reading process
    // would otherwise be taken for material and end up in what the student is taught.
    const CONTENT_ONLY: &str = "只输出图片里的内容本身，不要评论图片质量、清晰度、噪声或识别过程；个别看不清的字符按上下文推断，实在无法确定的写成 [?]。";
    let focus = if question.trim().is_empty() {
        format!("请完整描述这些图片的内容，提取其中的文字、公式和图表数据。{CONTENT_ONLY}")
    } else {
        format!("用户的问题是：{question}\n请描述这些图片中与该问题相关的全部内容，并提取其中的文字、公式和图表数据。{CONTENT_ONLY}")
    };
    if profile.capabilities.supports_vision {
        let mut parts = image_parts(atts);
        parts.push(Part::Text(format!(
            "{focus}\n要求：只报告图片中确实存在的文字、图表数值、公式（用 LaTeX）和视觉细节，不要猜测。"
        )));
        let req = LlmRequest {
            system: "You are a rigorous visual reading assistant. Extract only accurate facts visible in the provided images, including exact text, chart values and equations, without guessing.".into(),
            messages: vec![LlmMessage { role: Role::User, parts }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: profile.max_output_tokens.clamp(1024, 4096),
            reasoning: ReasoningOptions::default(),
        };
        if let Ok((txt, usage)) = llm
            .complete_text(profile, &req, CancellationToken::new())
            .await
        {
            let _ = lna_storage::record_usage(
                db,
                Some(notebook_id),
                Some(&profile.id),
                "attachment_vision",
                None,
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cached_tokens as i64,
                false,
            );
            return txt;
        }
    }
    let mut images = Vec::new();
    for (i, a) in atts.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(&a.path) {
            let ext = Path::new(&a.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg")
                .to_string();
            images.push((format!("图片 {}：{}", i + 1, a.name), ext, bytes));
        }
    }
    match crate::media::vision_inspect_images(db, secrets, llm, notebook_id, images, &focus).await {
        Ok(t) => t,
        Err(e) => format!("（图片识别失败：{}）", e.message),
    }
}

/// Parts for an earlier user turn. Pictures are re-sent (vision models only) when `with_media`;
/// otherwise the turn just notes that pictures were attached.
pub fn history_parts(
    profile: &ResolvedProfile,
    text: String,
    atts: &[Attachment],
    with_media: bool,
) -> Vec<Part> {
    let pics = images(atts);
    let atts = pics.as_slice();
    if atts.is_empty() {
        return vec![Part::Text(text)];
    }
    if with_media && profile.capabilities.supports_vision {
        let mut parts = image_parts(atts);
        parts.push(Part::Text(text));
        return parts;
    }
    let names: Vec<&str> = atts.iter().map(|a| a.name.as_str()).collect();
    vec![Part::Text(format!(
        "{text}\n[该问题附带了图片：{}]",
        names.join("、")
    ))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([200, 30, 30, 128]),
            ))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                w,
                h,
                image::Rgb([20, 120, 220]),
            ))
        };
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn item(name: &str, bytes: &[u8]) -> Value {
        serde_json::json!({ "name": name, "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes) })
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("lna_att_{}", new_ulid()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn downscales_large_opaque_images_to_jpeg() {
        let root = tmp();
        let items = serde_json::json!([item("big.png", &png_bytes(3200, 1600, false))]);
        let out = save_images(&root, &items).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mime, "image/jpeg");
        assert_eq!(out[0].width, MAX_EDGE);
        assert_eq!(out[0].height, MAX_EDGE / 2);
        assert!(Path::new(&out[0].path).exists());
        assert_eq!(out[0].name, "big.png");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn keeps_transparency_as_png_and_small_size() {
        let root = tmp();
        let out = save_images(
            &root,
            &serde_json::json!([item("a.png", &png_bytes(64, 48, true))]),
        )
        .unwrap();
        assert_eq!(out[0].mime, "image/png");
        assert_eq!((out[0].width, out[0].height), (64, 48));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_bad_input() {
        let root = tmp();
        assert!(save_images(&root, &serde_json::json!([item("x.png", b"not an image")])).is_err());
        assert!(save_images(&root, &serde_json::json!([{ "name": "x" }])).is_err());
        let many: Vec<Value> = (0..MAX_IMAGES_PER_MESSAGE + 1)
            .map(|i| item(&format!("{i}.png"), &png_bytes(4, 4, false)))
            .collect();
        assert!(save_images(&root, &Value::Array(many)).is_err());
        assert!(save_images(&root, &Value::Null).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_finds_stored_files_by_id_and_ignores_unsafe_ids() {
        let root = tmp();
        let out = save_images(
            &root,
            &serde_json::json!([item("a.png", &png_bytes(8, 8, false))]),
        )
        .unwrap();
        let ids = ids_of(&out);
        let found = resolve(&root, &ids);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, out[0].path);
        // ids come from client-visible config: path tricks must not resolve to anything
        let evil = serde_json::json!(["../../secret", "a/b", "", out[0].id.clone() + "x"]);
        assert!(resolve(&root, &evil).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stored_json_round_trips() {
        let root = tmp();
        let out = save_images(
            &root,
            &serde_json::json!([item("a.png", &png_bytes(8, 8, false))]),
        )
        .unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(parse_stored_str(&json), out);
        assert_eq!(parse_stored(&serde_json::to_value(&out).unwrap()), out);
        delete_files(&out);
        assert!(!Path::new(&out[0].path).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
