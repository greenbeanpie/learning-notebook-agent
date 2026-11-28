//! One-off reference files and web links attached to a chat message or a Studio request.
//!
//! Nothing is parsed on upload: the file is stored as is and the agent is given a list of what is
//! attached. It then decides what to do through tools — read the raw content (text formats and
//! web pages), parse the file into an outline and chunks when it needs structure (cached, at most
//! once per file), search the parsed chunks, extract one specific item exactly (a numbered problem,
//! a table, a definition) by instruction, or ask the audio/video model about a recording. A whole
//! document is never handed to the model at once. Audio and video are never transcribed here; they are only
//! accepted when the same conditions as importing them as a source hold.

use crate::attachments::{attachments_dir, write_meta, MAX_IMAGES_PER_MESSAGE};
use crate::{chunker, formats, media, office, parser, ssrf};
use base64::Engine as _;
use lna_llm::{LlmClient, LlmMessage, LlmRequest, Part, ResolvedProfile, Role, ToolDef, ToolMode};
use lna_secrets::SecretStore;
use lna_storage::Database;
use lna_types::{new_ulid, Attachment, ErrorCode, LnaError, LnaResult, SourceKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Largest one-off file accepted.
const MAX_FILE_BYTES: usize = 200 * 1024 * 1024;
/// Characters returned by one raw read.
const READ_CHUNK_CHARS: usize = 12_000;
/// Scanned PDF pages recognised per read.
const OCR_PAGES_PER_READ: usize = 6;

/// Attachment tool names, so callers can route a tool call here.
pub const TOOL_NAMES: &[&str] = &[
    "read_attachment",
    "parse_attachment",
    "read_parsed",
    "search_attachment",
    "extract_from_attachment",
    "ask_media",
];
/// Pages looked at by one extraction when the model names them; two when they are located.
const EXTRACT_MAX_PAGES: u32 = 4;

fn source_kind_of(name: &str) -> Option<(SourceKind, String)> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    formats::classify_extension(&ext).map(|k| (k, ext))
}

fn kind_label(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Audio => "音频",
        SourceKind::Video => "视频",
        SourceKind::Pdf => "PDF",
        SourceKind::Image => "图片",
        _ => "文档",
    }
}

/// Audio/video are accepted only when they could be imported as a source: processing is switched
/// on and a model that reads that medium is configured. Same messages as the source import.
fn check_media_allowed(db: &Database, kind: SourceKind) -> LnaResult<()> {
    if !formats::is_audio_or_video(kind) {
        return Ok(());
    }
    if !media::is_audio_video_enabled(db) {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "已在设置中关闭音视频文件处理，如需导入请在“模型协议配置”中开启。",
        ));
    }
    let (modality, label) = if kind == SourceKind::Video {
        ("video", "视频")
    } else {
        ("audio", "音频")
    };
    if lna_storage::resolve_modality_profile(db, modality)?.is_none() {
        return Err(LnaError::new(
            ErrorCode::ModelNotConfigured,
            format!("未配置用于处理{label}的模型，请在“模型协议配置”中为支持{label}的模型勾选“用于处理{label}”（推荐 Gemini），或关闭音视频文件处理。"),
        ));
    }
    Ok(())
}

/// Stores the files of a request parameter `[{ "name": "a.pdf", "data_base64": "..." }]` as they
/// are. Pictures among them become picture attachments. `already` is the number of attachments
/// the message carries besides these (the per-message limit counts all of them).
pub fn save_files(db: &Database, items: &Value, already: usize) -> LnaResult<Vec<Attachment>> {
    let Some(arr) = items.as_array() else {
        return Ok(Vec::new());
    };
    if arr.len() + already > MAX_IMAGES_PER_MESSAGE {
        return Err(LnaError::invalid(format!(
            "一次最多附带 {MAX_IMAGES_PER_MESSAGE} 个图片、文件或链接"
        )));
    }
    let root = db.paths().root.clone();
    let dir = attachments_dir(&root);
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("file-{}", i + 1));
        let Some((kind, ext)) = source_kind_of(&name) else {
            return Err(LnaError::new(
                ErrorCode::SourceUnsupported,
                format!("不支持文件「{name}」的格式（支持的格式与“添加来源”相同）"),
            ));
        };
        check_media_allowed(db, kind)?;
        let b64 = item
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| LnaError::invalid("file.data_base64 required"))?;
        let b64 = b64.rsplit(',').next().unwrap_or(b64);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| LnaError::invalid(format!("文件「{name}」数据无效：{e}")))?;
        if raw.is_empty() {
            return Err(LnaError::invalid(format!("文件「{name}」是空文件")));
        }
        if raw.len() > MAX_FILE_BYTES {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                format!("文件「{name}」超过 {} MB", MAX_FILE_BYTES / 1024 / 1024),
            ));
        }
        if kind == SourceKind::Image {
            out.extend(crate::attachments::save_images(
                &root,
                &json!([{ "name": name, "data_base64": b64 }]),
            )?);
            continue;
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| LnaError::internal(format!("无法创建附件目录：{e}")))?;
        let id = new_ulid();
        let path = dir.join(format!("{id}.{ext}"));
        std::fs::write(&path, &raw)
            .map_err(|e| LnaError::internal(format!("无法保存文件：{e}")))?;
        let att = Attachment {
            id,
            name,
            mime: formats::mime_type_for_ext(&ext).to_string(),
            path: path.to_string_lossy().into_owned(),
            width: 0,
            height: 0,
            bytes: raw.len() as u64,
            kind: "file".into(),
            url: None,
        };
        write_meta(&dir, &att);
        out.push(att);
    }
    Ok(out)
}

/// Stores web links (`["https://..."]`). The page is not fetched until the agent reads it.
pub fn save_links(db: &Database, items: &Value, already: usize) -> LnaResult<Vec<Attachment>> {
    let Some(arr) = items.as_array() else {
        return Ok(Vec::new());
    };
    if arr.len() + already > MAX_IMAGES_PER_MESSAGE {
        return Err(LnaError::invalid(format!(
            "一次最多附带 {MAX_IMAGES_PER_MESSAGE} 个图片、文件或链接"
        )));
    }
    let dir = attachments_dir(&db.paths().root);
    let mut out = Vec::new();
    for raw in arr.iter().filter_map(Value::as_str) {
        let url = ssrf::validate_public_url(raw.trim())?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| LnaError::internal(format!("无法创建附件目录：{e}")))?;
        let id = new_ulid();
        let name = format!(
            "{}{}",
            url.host_str().unwrap_or(""),
            url.path().trim_end_matches('/')
        );
        let att = Attachment {
            id: id.clone(),
            name,
            mime: "text/html".into(),
            path: dir
                .join(format!("{id}.meta.json"))
                .to_string_lossy()
                .into_owned(),
            width: 0,
            height: 0,
            bytes: 0,
            kind: "link".into(),
            url: Some(url.to_string()),
        };
        write_meta(&dir, &att);
        out.push(att);
    }
    Ok(out)
}

/// Files and links among `atts`.
pub fn context_items(atts: &[Attachment]) -> Vec<Attachment> {
    atts.iter()
        .filter(|a| a.kind == "file" || a.kind == "link")
        .cloned()
        .collect()
}

fn is_text_kind(k: SourceKind) -> bool {
    matches!(
        k,
        SourceKind::Txt | SourceKind::Md | SourceKind::Note | SourceKind::Csv | SourceKind::Web
    )
}

/// `<attached_files>`: what is attached and how each item can be read. No content is included.
pub fn manifest(_profile: &ResolvedProfile, atts: &[Attachment]) -> String {
    let items = context_items(atts);
    if items.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<attached_files note=\"用户为本次对话附带的一次性参考资料（不在来源列表中）。内容尚未读取：请按需调用 read_attachment / parse_attachment / read_parsed / search_attachment / extract_from_attachment / ask_media 获取，不需要的部分不要读。要原样取出某个具体内容（如“第 3 题”“例 2.5”“表 1”）时用 extract_from_attachment。引用附件内容时写明附件名和页码。\">\n",
    );
    for a in &items {
        let (how, kind) = if a.kind == "link" {
            (
                "read_attachment 抓取网页正文".to_string(),
                "网页链接".to_string(),
            )
        } else {
            match source_kind_of(&a.name) {
                Some((k, _)) if formats::is_audio_or_video(k) => {
                    ("ask_media 向音视频模型提问".into(), kind_label(k).into())
                }
                Some((k, _)) if is_text_kind(k) => {
                    ("read_attachment 直接读取文本".into(), "文本".into())
                }
                Some((k, _)) => (
                    "parse_attachment 解析后阅读，或 extract_from_attachment 精确提取".into(),
                    kind_label(k).into(),
                ),
                None => ("不支持".into(), "未知".into()),
            }
        };
        out.push_str(&format!(
            "<file id=\"{}\" name=\"{}\" type=\"{}\" size=\"{}\"{} read=\"{}\" />\n",
            a.id,
            xml_attr(&a.name),
            kind,
            human_size(a.bytes),
            a.url
                .as_deref()
                .map(|u| format!(" url=\"{}\"", xml_attr(u)))
                .unwrap_or_default(),
            how
        ));
    }
    out.push_str("</attached_files>");
    out
}

fn human_size(b: u64) -> String {
    if b == 0 {
        "-".into()
    } else if b < 1024 * 1024 {
        format!("{} KB", b.div_ceil(1024))
    } else {
        format!("{:.1} MB", b as f64 / 1024.0 / 1024.0)
    }
}

fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Tool definitions for the attachments in play (`ask_media` only when one is audio/video).
pub fn tool_defs(atts: &[Attachment]) -> Vec<ToolDef> {
    let items = context_items(atts);
    if items.is_empty() {
        return Vec::new();
    }
    let has_media = items
        .iter()
        .any(|a| source_kind_of(&a.name).is_some_and(|(k, _)| formats::is_audio_or_video(k)));
    let mut defs = vec![
        ToolDef {
            name: "read_attachment".into(),
            description: "读取附件原始内容（不解析）：文本类文件按字符偏移分段返回；网页链接返回正文文本。PDF、Word 等其他格式请用 parse_attachment 或 extract_from_attachment。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "<attached_files> 中的附件 id" },
                    "offset": { "type": "integer", "minimum": 0, "description": "起始字符偏移，默认 0" },
                    "length": { "type": "integer", "minimum": 500, "maximum": 20000, "description": "读取字符数，默认 12000" }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "parse_attachment".into(),
            description: "解析附件（PDF、Word、PPT、EPUB 等）为页码与目录大纲，结果会缓存；只有需要结构或无法直接读取时才调用。返回页数、分块数和大纲节点。".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "read_parsed".into(),
            description: "读取已解析附件的内容：按大纲节点 node_id，或按页码范围 page_from~page_to，或按分块 start_chunk/count。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "node_id": { "type": "string" },
                    "page_from": { "type": "integer", "minimum": 1 },
                    "page_to": { "type": "integer", "minimum": 1 },
                    "start_chunk": { "type": "integer", "minimum": 0 },
                    "count": { "type": "integer", "minimum": 1, "maximum": 8 }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "search_attachment".into(),
            description: "在附件中按关键词检索最相关的段落（会在需要时先解析附件），返回分块序号与页码。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "query": { "type": "string" }
                },
                "required": ["id", "query"]
            }),
        },
        ToolDef {
            name: "extract_from_attachment".into(),
            description: "按指令从附件中原样提取一个具体内容（如“第 3 题的完整题干、选项和小问”“例 2.5”“表 1 的全部数据”“定理 4 的陈述”），返回逐字原文（公式为 LaTeX）及页码，不做解答。会先按编号和关键词定位所在页（也可用 page_from/page_to 指定，最多 4 页）；PDF 页面在有识图模型时按页面图像识别，公式和版面更准确。找不到时会说明看过哪些页。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "instruction": { "type": "string", "description": "要提取什么，写明编号和范围，例如“第 3 题（含所有小问和图）”" },
                    "page_from": { "type": "integer", "minimum": 1 },
                    "page_to": { "type": "integer", "minimum": 1 }
                },
                "required": ["id", "instruction"]
            }),
        },
    ];
    if has_media {
        defs.push(ToolDef {
            name: "ask_media".into(),
            description: "把音频/视频附件的原始媒体连同你的问题交给能听/看音视频的模型，返回它的回答（不做全文转录，请问具体的问题）。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "question": { "type": "string" }
                },
                "required": ["id", "question"]
            }),
        });
    }
    defs
}

/// What an attachment tool produced: the text of the tool result.
pub struct ToolOutcome {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(text: impl Into<String>) -> Self {
        ToolOutcome {
            text: text.into(),
            is_error: false,
        }
    }
    fn err(text: impl Into<String>) -> Self {
        ToolOutcome {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Context the tools need.
pub struct ToolCtx<'a> {
    pub db: &'a Database,
    pub secrets: &'a Arc<dyn SecretStore>,
    pub llm: &'a LlmClient,
    pub notebook_id: &'a str,
    pub profile: &'a ResolvedProfile,
    pub atts: &'a [Attachment],
}

/// Runs an attachment tool; `None` when `name` is not one.
pub async fn run_tool(ctx: &ToolCtx<'_>, name: &str, args: &Value) -> Option<ToolOutcome> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let Some(att) = ctx
        .atts
        .iter()
        .find(|a| a.id == id && (a.kind == "file" || a.kind == "link"))
    else {
        return Some(ToolOutcome::err(format!(
            "没有 id 为「{id}」的附件，请使用 <attached_files> 中列出的 id。"
        )));
    };
    let out = match name {
        "read_attachment" => read_raw(ctx, att, args).await,
        "parse_attachment" => match parsed(ctx, att).await {
            Ok(p) => Ok(ToolOutcome::ok(p.outline_text(&att.name))),
            Err(e) => Err(e),
        },
        "read_parsed" => read_parsed(ctx, att, args).await,
        "search_attachment" => search(ctx, att, args).await,
        "extract_from_attachment" => extract(ctx, att, args).await,
        "ask_media" => ask_media(ctx, att, args).await,
        _ => return None,
    };
    Some(out.unwrap_or_else(|e| {
        ToolOutcome::err(format!("处理附件「{}」失败：{}", att.name, e.message))
    }))
}

async fn read_raw(ctx: &ToolCtx<'_>, att: &Attachment, args: &Value) -> LnaResult<ToolOutcome> {
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let length = (args
        .get("length")
        .and_then(Value::as_u64)
        .unwrap_or(READ_CHUNK_CHARS as u64) as usize)
        .clamp(500, 20_000);
    let text = if att.kind == "link" {
        page_text(ctx, att).await?
    } else {
        let Some((kind, _)) = source_kind_of(&att.name) else {
            return Ok(ToolOutcome::err("不支持的文件格式。"));
        };
        if formats::is_audio_or_video(kind) {
            return Ok(ToolOutcome::err(
                "这是音视频附件，请使用 ask_media 提出具体问题。",
            ));
        }
        if !is_text_kind(kind) {
            return Ok(ToolOutcome::err(format!(
                "「{}」是{}，无法直接读取原始内容；请调用 parse_attachment 阅读，或用 extract_from_attachment 提取具体内容。",
                att.name,
                kind_label(kind)
            )));
        }
        let bytes = std::fs::read(&att.path).map_err(|e| LnaError::internal(e.to_string()))?;
        parser::decode_text_bytes(&bytes)
    };
    let total = text.chars().count();
    if offset >= total {
        return Ok(ToolOutcome::ok(format!(
            "（共 {total} 字符，偏移 {offset} 之后没有内容）"
        )));
    }
    let piece: String = text.chars().skip(offset).take(length).collect();
    let end = offset + piece.chars().count();
    let more = if end < total {
        format!("；后面还有内容，继续读取请用 offset={end}")
    } else {
        "；已读到末尾".into()
    };
    Ok(ToolOutcome::ok(format!(
        "「{}」字符 {offset}~{end}（共 {total}{more}）：\n{piece}",
        att.name
    )))
}

/// Visible text of a link attachment (fetched once, cached in the parse cache's raw text).
async fn page_text(ctx: &ToolCtx<'_>, att: &Attachment) -> LnaResult<String> {
    let p = parsed(ctx, att).await?;
    Ok(p.chunks
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ParsedCache {
    pages: Option<u32>,
    /// Scanned PDF: pages without a text layer, recognised on demand into `ocr_pages`.
    #[serde(default)]
    scanned: bool,
    #[serde(default)]
    outline: Vec<OutlineNode>,
    #[serde(default)]
    chunks: Vec<CachedChunk>,
    #[serde(default)]
    ocr_pages: Vec<(u32, String)>,
    /// Full text of each page (paged formats), so an item is never cut at a chunk boundary.
    #[serde(default)]
    page_texts: Vec<(u32, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutlineNode {
    id: String,
    title: String,
    level: usize,
    page: Option<u32>,
    first_chunk: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedChunk {
    page: Option<i32>,
    section: Option<String>,
    text: String,
}

impl ParsedCache {
    fn outline_text(&self, name: &str) -> String {
        let mut s = format!(
            "「{name}」已解析：{}共 {} 个分块。",
            self.pages.map(|p| format!("{p} 页，")).unwrap_or_default(),
            self.chunks.len()
        );
        if self.scanned {
            s.push_str(
                "这是扫描版 PDF：用 read_parsed 的 page_from/page_to 读取时会识别对应页面。",
            );
        }
        if self.outline.is_empty() {
            s.push_str(
                "\n（没有可识别的目录结构，请按页码或分块读取，或用 search_attachment 检索）",
            );
        } else {
            s.push_str("\n大纲：\n");
            for n in self.outline.iter().take(200) {
                s.push_str(&format!(
                    "{}- [{}] {}{}\n",
                    "  ".repeat(n.level.saturating_sub(1).min(4)),
                    n.id,
                    n.title,
                    n.page.map(|p| format!("（p.{p}）")).unwrap_or_default()
                ));
            }
        }
        s
    }
}

fn cache_path(att: &Attachment) -> PathBuf {
    let dir = Path::new(&att.path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    dir.join(format!("{}.parsed.json", att.id))
}

fn save_cache(att: &Attachment, c: &ParsedCache) {
    if let Ok(s) = serde_json::to_string(c) {
        let _ = std::fs::write(cache_path(att), s);
    }
}

/// The parsed form of the attachment, parsing it on first use.
async fn parsed(ctx: &ToolCtx<'_>, att: &Attachment) -> LnaResult<ParsedCache> {
    if let Ok(raw) = std::fs::read_to_string(cache_path(att)) {
        if let Ok(c) = serde_json::from_str::<ParsedCache>(&raw) {
            return Ok(c);
        }
    }
    let doc = if att.kind == "link" {
        let url = att.url.clone().unwrap_or_default();
        let (_, bytes) = ssrf::safe_fetch_url(&url).await?;
        parser::parse_html(&parser::decode_text_bytes(&bytes))
    } else {
        let (kind, ext) = source_kind_of(&att.name)
            .ok_or_else(|| LnaError::new(ErrorCode::SourceUnsupported, "不支持的文件格式"))?;
        if formats::is_audio_or_video(kind) {
            return Err(LnaError::new(
                ErrorCode::SourceUnsupported,
                "音视频附件不做解析，请用 ask_media 提问",
            ));
        }
        let path = PathBuf::from(&att.path);
        tokio::task::spawn_blocking(move || parse_local(kind, &ext, &path))
            .await
            .map_err(|e| LnaError::internal(format!("解析任务中断：{e}")))??
    };
    let scanned = doc.blocks.is_empty() && is_pdf_attachment(att);
    if doc.blocks.is_empty() && !scanned {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "附件中没有可提取的文字",
        ));
    }
    let mut page_texts: Vec<(u32, String)> = Vec::new();
    for b in &doc.blocks {
        let Some(pg) = b.page else { continue };
        match page_texts.last_mut() {
            Some((p, t)) if *p == pg => {
                t.push('\n');
                t.push_str(&b.text);
            }
            _ => page_texts.push((pg, b.text.clone())),
        }
    }
    let chunks = chunker::chunk_document(&doc);
    let mut outline = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (ci, c) in chunks.iter().enumerate() {
        if let Some(sec) = c.section_path.as_deref().filter(|s| !s.is_empty()) {
            if seen.insert(sec.to_string()) {
                let level = sec.split(" > ").count();
                let title = sec.rsplit(" > ").next().unwrap_or(sec).to_string();
                outline.push(OutlineNode {
                    id: format!("n{}", outline.len() + 1),
                    title,
                    level,
                    page: c.page_start.map(|p| p.max(0) as u32),
                    first_chunk: ci,
                });
            }
        }
    }
    let cache = ParsedCache {
        pages: doc
            .page_count
            .map(|p| p.max(0) as u32)
            .or_else(|| scanned.then(|| pdf_page_count(&att.path)).flatten()),
        scanned,
        outline,
        chunks: chunks
            .into_iter()
            .map(|c| CachedChunk {
                page: c.page_start,
                section: c.section_path,
                text: c.content,
            })
            .collect(),
        ocr_pages: Vec::new(),
        page_texts,
    };
    save_cache(att, &cache);
    let _ = ctx; // parsing itself needs no model
    Ok(cache)
}

fn pdf_page_count(path: &str) -> Option<u32> {
    // a 1-page render probe would load PDFium anyway; the parser reports the count for text PDFs
    parser::parse_pdf_detailed(Path::new(path))
        .ok()
        .map(|r| r.page_meta.len() as u32)
}

/// Parses a local file with the same parsers the source import uses (no AI indexing, no OCR).
pub fn parse_local(kind: SourceKind, ext: &str, path: &Path) -> LnaResult<parser::ParsedDoc> {
    let read = || std::fs::read(path).map_err(|e| LnaError::internal(e.to_string()));
    match kind {
        SourceKind::Pdf => parser::parse_pdf(path),
        SourceKind::Docx => office::parse_docx(path).map(|r| r.doc),
        SourceKind::Pptx => office::parse_pptx(path).map(|r| r.doc),
        SourceKind::Epub => office::parse_epub(path).map(|r| r.doc),
        SourceKind::Odt => office::parse_odt(path).map(|r| r.doc),
        SourceKind::Hwpx => office::parse_hwpx(path).map(|r| r.doc),
        SourceKind::Hwp => office::parse_hwp(path),
        SourceKind::Csv => office::parse_csv(&read()?),
        SourceKind::Rtf => office::parse_rtf(&read()?),
        SourceKind::Md | SourceKind::Note => {
            Ok(parser::parse_markdown(&parser::decode_text_bytes(&read()?)))
        }
        SourceKind::Web => Ok(parser::parse_html(&parser::decode_text_bytes(&read()?))),
        SourceKind::Txt => {
            let text = parser::decode_text_bytes(&read()?);
            Ok(match ext {
                "html" | "htm" | "xhtml" => parser::parse_html(&text),
                "md" | "markdown" | "mdx" => parser::parse_markdown(&text),
                _ => parser::parse_plain_text(&text),
            })
        }
        SourceKind::Image | SourceKind::Audio | SourceKind::Video => Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "该类型附件不做文本解析",
        )),
    }
}

async fn read_parsed(ctx: &ToolCtx<'_>, att: &Attachment, args: &Value) -> LnaResult<ToolOutcome> {
    if !cache_path(att).exists() {
        return Ok(ToolOutcome::err(format!(
            "「{}」尚未解析，请先调用 parse_attachment。",
            att.name
        )));
    }
    let mut p = parsed(ctx, att).await?;
    let node = args.get("node_id").and_then(Value::as_str);
    let page_from = args
        .get("page_from")
        .and_then(Value::as_u64)
        .map(|x| x as u32);
    let page_to = args
        .get("page_to")
        .and_then(Value::as_u64)
        .map(|x| x as u32);
    if p.scanned {
        let from = page_from.unwrap_or(1).max(1);
        let to = page_to
            .unwrap_or(from)
            .max(from)
            .min(from + OCR_PAGES_PER_READ as u32 - 1);
        let to = p.pages.map_or(to, |n| to.min(n.max(1)));
        let text = scanned_pages(ctx, att, &mut p, from, to).await?;
        return Ok(ToolOutcome::ok(text));
    }
    let selected: Vec<(usize, &CachedChunk)> = if let Some(nid) = node {
        let Some(i) = p.outline.iter().position(|n| n.id == nid) else {
            return Ok(ToolOutcome::err(format!("大纲中没有节点 {nid}。")));
        };
        let start = p.outline[i].first_chunk;
        let end = p
            .outline
            .get(i + 1)
            .map_or(p.chunks.len(), |n| n.first_chunk)
            .max(start + 1);
        let count = (args.get("count").and_then(Value::as_u64).unwrap_or(6) as usize).clamp(1, 8);
        p.chunks
            .iter()
            .enumerate()
            .skip(start)
            .take((end - start).min(count))
            .collect()
    } else if let Some(from) = page_from {
        let to = page_to.unwrap_or(from).max(from);
        p.chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.page
                    .is_some_and(|pg| pg as u32 >= from && pg as u32 <= to)
            })
            .take(12)
            .collect()
    } else {
        let start = args.get("start_chunk").and_then(Value::as_u64).unwrap_or(0) as usize;
        let count = (args.get("count").and_then(Value::as_u64).unwrap_or(4) as usize).clamp(1, 8);
        p.chunks
            .iter()
            .enumerate()
            .skip(start)
            .take(count)
            .collect()
    };
    if selected.is_empty() {
        return Ok(ToolOutcome::ok(format!(
            "「{}」在该范围内没有内容（共 {} 个分块）。",
            att.name,
            p.chunks.len()
        )));
    }
    Ok(ToolOutcome::ok(format_chunks(&att.name, &selected)))
}

fn format_chunks(name: &str, chunks: &[(usize, &CachedChunk)]) -> String {
    let mut s = String::new();
    for (i, c) in chunks {
        s.push_str(&format!(
            "[{name} · 分块 {i}{}{}]\n{}\n\n",
            c.page.map(|p| format!(" · p.{p}")).unwrap_or_default(),
            c.section
                .as_deref()
                .map(|x| format!(" · {x}"))
                .unwrap_or_default(),
            c.text
        ));
    }
    s
}

/// Recognises scanned PDF pages `from..=to` (cached per page) with the vision model or local OCR.
async fn scanned_pages(
    ctx: &ToolCtx<'_>,
    att: &Attachment,
    p: &mut ParsedCache,
    from: u32,
    to: u32,
) -> LnaResult<String> {
    let missing: Vec<u32> = (from..=to)
        .filter(|pg| !p.ocr_pages.iter().any(|(n, _)| n == pg))
        .collect();
    if !missing.is_empty() {
        let path = PathBuf::from(&att.path);
        let pages = missing.clone();
        let rendered = tokio::task::spawn_blocking(move || {
            parser::render_pdf_pages_to_jpeg(&path, &pages, 1400)
        })
        .await
        .map_err(|e| LnaError::internal(e.to_string()))??;
        for (pg, jpg) in rendered {
            let text = media::vision_inspect_images(
                ctx.db,
                ctx.secrets,
                ctx.llm,
                ctx.notebook_id,
                vec![(format!("{} 第 {pg} 页", att.name), "jpg".into(), jpg)],
                "请完整提取这一页的全部文字、公式（LaTeX）和图表要点，不要猜测。",
            )
            .await
            .unwrap_or_else(|e| format!("（第 {pg} 页识别失败：{}）", e.message));
            p.ocr_pages.push((pg, text));
        }
        save_cache(att, p);
    }
    let mut out = String::new();
    for pg in from..=to {
        if let Some((_, t)) = p.ocr_pages.iter().find(|(n, _)| *n == pg) {
            out.push_str(&format!("[{} · p.{pg}]\n{t}\n\n", att.name));
        }
    }
    Ok(out)
}

async fn search(ctx: &ToolCtx<'_>, att: &Attachment, args: &Value) -> LnaResult<ToolOutcome> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() {
        return Ok(ToolOutcome::err("query 不能为空。"));
    }
    let p = parsed(ctx, att).await?;
    if p.scanned {
        return Ok(ToolOutcome::err(
            "扫描版 PDF 无法按关键词检索，请用 read_parsed 按页码读取。",
        ));
    }
    let hits = rank(&p.chunks, query, 5);
    if hits.is_empty() {
        return Ok(ToolOutcome::ok(format!(
            "「{}」中没有找到与「{query}」相关的段落。",
            att.name
        )));
    }
    let sel: Vec<(usize, &CachedChunk)> = hits.into_iter().map(|i| (i, &p.chunks[i])).collect();
    Ok(ToolOutcome::ok(format_chunks(&att.name, &sel)))
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// Query terms: ASCII words, and bigrams within each run of CJK characters and digits, so
/// "第3题" gives "第3" and "3题" (bigrams that also occur in the text) rather than "第题".
fn terms(text: &str) -> Vec<String> {
    let lower = normalize(text);
    let mut out = Vec::new();
    let mut word = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c);
        } else if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    let mut run: Vec<char> = Vec::new();
    let flush = |run: &mut Vec<char>, out: &mut Vec<String>| {
        if run.iter().any(|c| is_cjk(*c)) {
            if run.len() == 1 {
                out.push(run[0].to_string());
            }
            for w in run.windows(2) {
                out.push(w.iter().collect());
            }
        }
        run.clear();
    };
    for c in lower.chars() {
        if is_cjk(c) || c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out.sort();
    out.dedup();
    out
}

/// Lower case, full-width ASCII folded to half-width, spaces and tabs removed (line breaks kept),
/// so "第 3 题", "第３题" and "第3题" compare equal.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let c = match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        };
        if c == ' ' || c == '\t' || c == '\r' {
            continue;
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Indices of the best matching chunks (tf-idf over query terms).
fn rank(chunks: &[CachedChunk], query: &str, k: usize) -> Vec<usize> {
    let q = terms(query);
    if q.is_empty() {
        return Vec::new();
    }
    let lowered: Vec<String> = chunks.iter().map(|c| normalize(&c.text)).collect();
    let n = chunks.len().max(1) as f64;
    let idf: Vec<f64> = q
        .iter()
        .map(|t| {
            let df = lowered.iter().filter(|c| c.contains(t.as_str())).count() as f64;
            ((n + 1.0) / (df + 0.5)).ln()
        })
        .collect();
    let mut scored: Vec<(usize, f64)> = lowered
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let s: f64 = q
                .iter()
                .zip(&idf)
                .map(|(t, w)| {
                    let tf = c.matches(t.as_str()).count() as f64;
                    if tf > 0.0 {
                        w * (1.0 + tf.ln())
                    } else {
                        0.0
                    }
                })
                .sum();
            (i, s)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

/// Hands the recording and the question to the audio/video model (the chat model itself when it
/// reads that medium) and returns its answer. Nothing is transcribed or stored.
async fn ask_media(ctx: &ToolCtx<'_>, att: &Attachment, args: &Value) -> LnaResult<ToolOutcome> {
    let Some((kind, ext)) =
        source_kind_of(&att.name).filter(|(k, _)| formats::is_audio_or_video(*k))
    else {
        return Ok(ToolOutcome::err("ask_media 只用于音频或视频附件。"));
    };
    let question = args
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if question.is_empty() {
        return Ok(ToolOutcome::err("question 不能为空。"));
    }
    check_media_allowed(ctx.db, kind)?;
    let modality = if kind == SourceKind::Video {
        "video"
    } else {
        "audio"
    };
    let own = match modality {
        "video" => ctx.profile.capabilities.supports_video,
        _ => ctx.profile.capabilities.supports_audio,
    };
    let resolved = if own {
        ctx.profile.clone()
    } else {
        media::resolve_modality_profile_with_secret(ctx.db, ctx.secrets, modality)?.ok_or_else(
            || LnaError::new(ErrorCode::ModelNotConfigured, "未配置可处理音视频的模型"),
        )?
    };
    let (part, uploaded) = media::media_part_for_file(
        ctx.llm,
        &resolved,
        Path::new(&att.path),
        &ext,
        kind,
        &att.name,
    )
    .await?;
    let req = LlmRequest::from_parts(
        "You answer questions about the attached audio or video strictly from what is actually said or shown in it. Give timestamps [MM:SS] for the parts you rely on. Do not invent content.",
        vec![part, Part::Text(format!("文件：{}\n问题：{question}", att.name))],
        resolved.output_budget().clamp(1024, 4096),
    );
    let res = ctx
        .llm
        .complete_text(&resolved, &req, CancellationToken::new())
        .await;
    if let Some(name) = uploaded {
        ctx.llm.delete_gemini_file(&resolved, &name).await;
    }
    let (answer, usage) = res?;
    crate::media::record_llm_usage(
        ctx.db,
        Some(ctx.notebook_id),
        &resolved.id,
        "attachment_media",
        None,
        &usage,
    );
    Ok(ToolOutcome::ok(format!(
        "（音视频模型对「{}」的回答）\n{answer}",
        att.name
    )))
}

/// Item labels in an instruction such as "第3题", "例2.5", "习题 1-3", "Problem 12", "(4)": the
/// forms the item can take at the start of a line in the document, strongest first.
fn item_labels(instruction: &str) -> Vec<(String, u32)> {
    const PREFIXES: &[&str] = &[
        "例题",
        "习题",
        "练习",
        "思考题",
        "问题",
        "例",
        "题",
        "定理",
        "定义",
        "引理",
        "推论",
        "表",
        "图",
        "problem",
        "exercise",
        "example",
        "question",
        "theorem",
        "definition",
        "lemma",
        "table",
        "figure",
        "fig.",
        "q",
    ];
    let norm = cn_numerals_to_digits(&normalize(instruction));
    let chars: Vec<char> = norm.chars().collect();
    let mut labels: Vec<(String, u32)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_digit()
                || (matches!(chars[i], '.' | '-' | '—' | '–')
                    && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())))
        {
            i += 1;
        }
        let num: String = chars[start..i].iter().collect();
        let before: String = chars[..start].iter().collect();
        let after: String = chars[i..].iter().take(2).collect();
        if let Some(p) = PREFIXES.iter().find(|p| before.ends_with(*p)) {
            labels.push((format!("{p}{num}"), 10));
        }
        if before.ends_with('第') && (after.starts_with('题') || after.starts_with('问')) {
            let unit = after.chars().next().unwrap_or('题');
            labels.push((format!("第{num}{unit}"), 10));
            if let Some(cn) = num.parse::<u32>().ok().and_then(digits_to_cn) {
                labels.push((format!("第{cn}{unit}"), 10));
            }
        }
        for punct in [".", "、", ")", "．"] {
            labels.push((format!("\n{num}{punct}"), 5));
        }
        labels.push((format!("\n({num})"), 5));
        labels.push((format!("\n（{num}）"), 5));
        labels.push((format!("\n{num}"), 1));
    }
    labels
}

const CN_DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// 1..=99 as Chinese numerals ("七", "十三", "二十").
fn digits_to_cn(n: u32) -> Option<String> {
    match n {
        1..=9 => Some(CN_DIGITS[n as usize].to_string()),
        10..=99 => {
            let (t, u) = (n / 10, n % 10);
            let mut s = if t == 1 {
                String::new()
            } else {
                CN_DIGITS[t as usize].to_string()
            };
            s.push('十');
            if u > 0 {
                s.push(CN_DIGITS[u as usize]);
            }
            Some(s)
        }
        _ => None,
    }
}

/// "第七题" / "第十三问" in an instruction become "第7题" / "第13问".
fn cn_numerals_to_digits(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '第' {
            let mut j = i + 1;
            while j < chars.len() && (CN_DIGITS.contains(&chars[j]) || chars[j] == '十') {
                j += 1;
            }
            if j > i + 1 && j - i <= 4 {
                let cn: String = chars[i + 1..j].iter().collect();
                if let Some(n) = (1..=99).find(|n| digits_to_cn(*n).as_deref() == Some(cn.as_str()))
                {
                    out.push('第');
                    out.push_str(&n.to_string());
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Score of one unit (a page or a chunk) for an instruction: numbered labels first, then terms.
fn label_score(unit: &str, labels: &[(String, u32)]) -> u32 {
    let text = format!("\n{}", normalize(unit));
    labels
        .iter()
        .filter(|(l, _)| {
            // a label must not run on into a longer number ("\n1." is not "\n12.")
            text.match_indices(l.as_str()).any(|(at, _)| {
                let next = text[at + l.len()..].chars().next();
                !(l.chars().last().is_some_and(|c| c.is_ascii_digit())
                    && next.is_some_and(|c| c.is_ascii_digit() || c == '.'))
            })
        })
        .map(|(_, w)| *w)
        .max()
        .unwrap_or(0)
}

/// Units (page number or chunk index, text) the extraction can choose from.
/// The stored mime of a document is often generic, so the file extension decides.
fn is_pdf_attachment(att: &Attachment) -> bool {
    att.mime == "application/pdf"
        || std::path::Path::new(&att.path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

fn extraction_units(p: &ParsedCache) -> Vec<(u32, String)> {
    if !p.page_texts.is_empty() {
        return p.page_texts.clone();
    }
    // older caches and paged chunks without page texts: group chunks by page
    if p.chunks.iter().any(|c| c.page.is_some()) {
        let mut out: Vec<(u32, String)> = Vec::new();
        for c in &p.chunks {
            let pg = c.page.unwrap_or(0).max(0) as u32;
            match out.last_mut() {
                Some((x, t)) if *x == pg => {
                    t.push('\n');
                    t.push_str(&c.text);
                }
                _ => out.push((pg, c.text.clone())),
            }
        }
        return out;
    }
    p.chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (i as u32, c.text.clone()))
        .collect()
}

/// Finds the best unit for `instruction`: numbered labels outrank keyword matches.
fn locate(units: &[(u32, String)], instruction: &str) -> Option<usize> {
    let labels = item_labels(instruction);
    let as_chunks: Vec<CachedChunk> = units
        .iter()
        .map(|(_, t)| CachedChunk {
            page: None,
            section: None,
            text: t.clone(),
        })
        .collect();
    let by_terms = rank(&as_chunks, instruction, units.len().max(1));
    let mut best: Option<(usize, u32, usize)> = None; // (unit, label score, term rank)
    for (rank_pos, &i) in by_terms.iter().enumerate() {
        let ls = label_score(&units[i].1, &labels);
        let better = match best {
            None => true,
            Some((_, bl, br)) => ls > bl || (ls == bl && rank_pos < br),
        };
        if better {
            best = Some((i, ls, rank_pos));
        }
    }
    if best.is_none() && !labels.is_empty() {
        // no term overlap at all: labels alone
        best = units
            .iter()
            .enumerate()
            .map(|(i, (_, t))| (i, label_score(t, &labels), 0))
            .filter(|x| x.1 >= 5)
            .max_by_key(|x| x.1);
    }
    best.map(|b| b.0)
}

const EXTRACT_RULES: &str = "要求：原样照录（不改写、不总结、不解答、不补全缺字）；完整保留编号、题干、选项、所有小问、条件、数据和单位；公式用 LaTeX（行内 $...$）；表格用 Markdown 表格；图只写图题和图中可读的文字/数值。材料中找不到要求的内容时，只回答“未找到”，并列出材料里出现的相近编号或标题。";

/// `extract_from_attachment`: locates the pages (or passages) holding the requested item and has
/// a model copy it out verbatim — from the page images when a vision model is set up (exact
/// formulas and layout), otherwise from the text layer / recognised text.
async fn extract(ctx: &ToolCtx<'_>, att: &Attachment, args: &Value) -> LnaResult<ToolOutcome> {
    let instruction = args
        .get("instruction")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if instruction.is_empty() {
        return Ok(ToolOutcome::err(
            "instruction 不能为空：写明要提取的内容，例如“第 3 题（含所有小问）”。",
        ));
    }
    let mut p = parsed(ctx, att).await?;
    let page_from = args
        .get("page_from")
        .and_then(Value::as_u64)
        .map(|x| x.max(1) as u32);
    let page_to = args
        .get("page_to")
        .and_then(Value::as_u64)
        .map(|x| x as u32);
    let is_pdf = is_pdf_attachment(att);

    // which pages (paged formats) or chunks (others) to look at
    let units = extraction_units(&p);
    let paged = !p.page_texts.is_empty() || p.scanned || p.chunks.iter().any(|c| c.page.is_some());
    let pages: Vec<u32> = if let Some(from) = page_from {
        let to = page_to
            .unwrap_or(from)
            .max(from)
            .min(from + EXTRACT_MAX_PAGES - 1);
        let to = p.pages.map_or(to, |n| to.min(n.max(1)));
        (from..=to).collect()
    } else if p.scanned {
        return Ok(ToolOutcome::err(format!(
            "「{}」是扫描版 PDF，无法按文字定位：请用 page_from/page_to 指定页码（可先用 read_parsed 按页浏览找到位置）。",
            att.name
        )));
    } else {
        let Some(i) = locate(&units, instruction) else {
            return Ok(ToolOutcome::ok(format!(
                "在「{}」中没有定位到与「{instruction}」相符的位置。可以先 parse_attachment 看大纲，再用 page_from/page_to 指定页码重试。",
                att.name
            )));
        };
        if paged {
            // the item may continue on the next page
            let pg = units[i].0;
            let next = units.get(i + 1).map(|u| u.0).filter(|n| *n == pg + 1);
            std::iter::once(pg).chain(next).collect()
        } else {
            // a chunk and its neighbours
            let lo = i.saturating_sub(1);
            (lo as u32..=(i + 1).min(units.len() - 1) as u32).collect()
        }
    };
    if pages.is_empty() {
        return Ok(ToolOutcome::err("没有可提取的范围。"));
    }
    let where_ = if paged {
        format!(
            "p.{}",
            if pages.len() == 1 {
                pages[0].to_string()
            } else {
                format!("{}–{}", pages[0], pages[pages.len() - 1])
            }
        )
    } else {
        format!("分块 {}–{}", pages[0], pages[pages.len() - 1])
    };

    // PDF with a vision model: read the rendered pages
    if is_pdf
        && media::resolve_modality_profile_with_secret(ctx.db, ctx.secrets, "vision")?.is_some()
    {
        let path = PathBuf::from(&att.path);
        let want = pages.clone();
        let rendered = tokio::task::spawn_blocking(move || {
            parser::render_pdf_pages_to_jpeg(&path, &want, 1600)
        })
        .await
        .map_err(|e| LnaError::internal(e.to_string()))??;
        if !rendered.is_empty() {
            let images = rendered
                .into_iter()
                .map(|(pg, jpg)| (format!("{} 第 {pg} 页", att.name), "jpg".to_string(), jpg))
                .collect();
            let text = media::vision_inspect_images(
                ctx.db,
                ctx.secrets,
                ctx.llm,
                ctx.notebook_id,
                images,
                &format!("从这些页面中提取：{instruction}\n{EXTRACT_RULES}"),
            )
            .await?;
            return Ok(ToolOutcome::ok(format!(
                "[{} · {where_} · 按页面图像提取]\n{text}",
                att.name
            )));
        }
    }

    // otherwise: the text of those pages (recognised first for a scanned PDF)
    let material = if p.scanned {
        scanned_pages(ctx, att, &mut p, pages[0], pages[pages.len() - 1]).await?
    } else if paged {
        units
            .iter()
            .filter(|(pg, _)| pages.contains(pg))
            .map(|(pg, t)| format!("[p.{pg}]\n{t}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        pages
            .iter()
            .filter_map(|i| units.get(*i as usize))
            .map(|(i, t)| format!("[分块 {i}]\n{t}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if material.trim().is_empty() {
        return Ok(ToolOutcome::ok(format!(
            "「{}」{where_} 没有文字内容。",
            att.name
        )));
    }
    let req = LlmRequest::simple(
        "You copy requested items out of document text exactly as written, without solving, paraphrasing or completing anything.",
        format!(
            "材料（「{}」{where_} 的文字层，排版和公式可能有提取误差，请按原意修正为 LaTeX，但不要改动内容）：\n<material>\n{material}\n</material>\n\n提取：{instruction}\n{EXTRACT_RULES}",
            att.name
        ),
        ctx.profile.output_budget().clamp(1024, 8192),
    );
    let (text, usage) = ctx
        .llm
        .complete_text(ctx.profile, &req, CancellationToken::new())
        .await?;
    crate::media::record_llm_usage(
        ctx.db,
        Some(ctx.notebook_id),
        &ctx.profile.id,
        "attachment_extract",
        None,
        &usage,
    );
    Ok(ToolOutcome::ok(format!(
        "[{} · {where_} · 按文字提取]\n{}",
        att.name,
        text.trim()
    )))
}

/// Lets the model read the attachments with the tools (at most `max_rounds` rounds) and returns
/// what it extracted for `task`: the relevant passages with file names and pages. Used where the
/// main request is a single structured call (Studio generation, revisions, lecture questions).
pub async fn brief(
    ctx: &ToolCtx<'_>,
    task: &str,
    max_rounds: usize,
    cancel: CancellationToken,
) -> LnaResult<String> {
    let mut tools = tool_defs(ctx.atts);
    if tools.is_empty() {
        return Ok(String::new());
    }
    tools.push(ToolDef {
        name: "abort_task".into(),
        description: "Call this only when the task cannot be done faithfully — e.g. the attached file is unreadable or missing, or the request contradicts it. It stops the whole job and shows the reason to the user. Never guess or invent content instead.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "reason": {"type": "string", "description": "Why the task cannot be done faithfully"}
            },
            "required": ["reason"]
        }),
    });
    let mut messages = vec![LlmMessage {
        role: Role::User,
        parts: vec![Part::Text(format!(
            "{}\n\n任务：{task}\n\n请用工具只读取完成这个任务需要的附件内容（不需要的部分不要读）。读完后不要再调用工具，直接输出摘录：逐条列出相关原文要点，每条注明附件名和页码/位置；不要编造附件中没有的内容。如果附件无法读取、缺失，或与任务矛盾，可以调用 abort_task 说明原因，而不是猜测。",
            manifest(ctx.profile, ctx.atts)
        ))],
    }];
    let mut usage = lna_llm::Usage::default();
    let mut final_text = String::new();
    for round in 0..=max_rounds {
        let last = round == max_rounds;
        let req = LlmRequest::conversation(
            "Read attached reference files via tools and return only task-relevant passages with file name and page.",
            messages.clone(),
            ctx.profile.output_budget().clamp(2048, 8192),
        )
        .with_tools(
            tools.clone(),
            if last { ToolMode::None } else { ToolMode::Auto },
        );
        let turn = ctx
            .llm
            .complete_turn(ctx.profile, &req, cancel.clone(), |_| {})
            .await?;
        usage.add(&turn.usage);
        if turn.tool_calls.is_empty() || last {
            final_text = turn.text;
            break;
        }
        let mut asst = turn.opaque;
        if !turn.text.trim().is_empty() {
            asst.push(Part::Text(turn.text));
        }
        let mut parsed_calls = Vec::with_capacity(turn.tool_calls.len());
        for tc in turn.tool_calls {
            let args: Value = serde_json::from_str(&tc.raw_arguments).unwrap_or_else(|_| json!({}));
            if tc.name == "abort_task" {
                let reason = args
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .unwrap_or("(no reason given)");
                crate::media::record_llm_usage(
                    ctx.db,
                    Some(ctx.notebook_id),
                    &ctx.profile.id,
                    "attachment_read",
                    None,
                    &usage,
                );
                return Err(LnaError::new(
                    ErrorCode::ModelRefused,
                    format!("模型主动终止任务：{reason}"),
                ));
            }
            asst.push(Part::ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: args.clone(),
            });
            parsed_calls.push((tc.id, tc.name, args));
        }
        // Execute independent attachment tool calls concurrently (including vision/text extraction),
        // bounded by the profile's parallel limit.
        let limit = ctx.llm.parallel_limit(&ctx.profile.id).clamp(1, 16);
        let results = lna_llm::run_parallel_ordered(
            parsed_calls,
            limit,
            &cancel,
            |_, (id, name, args)| async move {
                let o = run_tool(ctx, &name, &args)
                    .await
                    .unwrap_or_else(|| ToolOutcome::err(format!("不支持的工具：{name}")));
                Ok(Part::ToolResult {
                    call_id: id,
                    content: format!("<tool_result trust=\"untrusted\">{}</tool_result>", o.text),
                    is_error: o.is_error,
                })
            },
        )
        .await?;
        messages.push(LlmMessage {
            role: Role::Assistant,
            parts: asst,
        });
        messages.push(LlmMessage {
            role: Role::Tool,
            parts: results,
        });
    }
    crate::media::record_llm_usage(
        ctx.db,
        Some(ctx.notebook_id),
        &ctx.profile.id,
        "attachment_read",
        None,
        &usage,
    );
    Ok(final_text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_prefers_chunks_with_the_query_terms() {
        let c = |t: &str| CachedChunk {
            page: Some(1),
            section: None,
            text: t.into(),
        };
        let chunks = vec![
            c("向量空间的基与维数"),
            c("特征值与特征向量的定义"),
            c("矩阵的秩 rank"),
        ];
        assert_eq!(rank(&chunks, "特征值", 3), vec![1]);
        assert_eq!(rank(&chunks, "rank", 3), vec![2]);
        assert!(rank(&chunks, "傅里叶", 3).is_empty());
    }

    #[test]
    fn a_pdf_is_recognised_by_its_extension_when_the_stored_mime_is_generic() {
        let att = |mime: &str, path: &str| Attachment {
            id: "a".into(),
            name: "b".into(),
            mime: mime.into(),
            path: path.into(),
            width: 0,
            height: 0,
            bytes: 1,
            kind: "file".into(),
            url: None,
        };
        assert!(is_pdf_attachment(&att(
            "application/octet-stream",
            "x/01ABC.pdf"
        )));
        assert!(is_pdf_attachment(&att(
            "application/octet-stream",
            "x/01ABC.PDF"
        )));
        assert!(is_pdf_attachment(&att("application/pdf", "x/01ABC")));
        assert!(!is_pdf_attachment(&att("text/markdown", "x/01ABC.md")));
    }

    #[test]
    fn numbered_items_are_located_by_their_label_not_by_shared_digits() {
        let pages = vec![
            (
                1,
                "习题一\n1. 求极限 lim x→0 sin x / x。\n2. 证明数列 1/n 收敛。".to_string(),
            ),
            (
                2,
                "3. 设 f(x)=x^2，求 f'(3)。\n（1）写出定义式\n（2）计算结果".to_string(),
            ),
            (
                3,
                "13. 求 ∫ x dx，其中 3 < x < 5。\n例 2.5 已知数列满足 a_{n+1}=2a_n。".to_string(),
            ),
            (4, "第 7 题 讨论函数 13x+3 的单调性。".to_string()),
        ];
        assert_eq!(locate(&pages, "第3题").map(|i| pages[i].0), Some(2));
        assert_eq!(
            locate(&pages, "第 13 题的题干").map(|i| pages[i].0),
            Some(3)
        );
        assert_eq!(locate(&pages, "例2.5").map(|i| pages[i].0), Some(3));
        assert_eq!(
            locate(&pages, "第七题").map(|i| pages[i].0),
            Some(4),
            "Chinese numerals"
        );
        assert_eq!(
            locate(&pages, "第７题").map(|i| pages[i].0),
            Some(4),
            "full-width digits"
        );
        assert_eq!(
            locate(&pages, "数列收敛的证明").map(|i| pages[i].0),
            Some(1),
            "keywords when there is no number"
        );
    }

    #[test]
    fn query_terms_keep_digits_next_to_cjk() {
        let t = terms("第3题");
        assert!(
            t.contains(&"第3".to_string()) && t.contains(&"3题".to_string()),
            "{t:?}"
        );
        assert!(!t.contains(&"第题".to_string()), "{t:?}");
        assert_eq!(normalize("第 ３ 题"), "第3题");
        assert_eq!(
            cn_numerals_to_digits("第十三题和第二十问"),
            "第13题和第20问"
        );
        assert_eq!(digits_to_cn(7).as_deref(), Some("七"));
    }

    #[test]
    fn unsupported_and_supported_extensions() {
        assert!(source_kind_of("a.xyz").is_none());
        assert_eq!(
            source_kind_of("讲义.PDF").map(|x| x.0),
            Some(SourceKind::Pdf)
        );
        assert_eq!(
            source_kind_of("talk.mp3").map(|x| x.0),
            Some(SourceKind::Audio)
        );
    }
}
