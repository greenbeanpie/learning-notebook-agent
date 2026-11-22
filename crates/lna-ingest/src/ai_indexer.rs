use crate::chunker::{estimate_tokens, ChunkDraft};
use crate::media;
use crate::office;
use crate::parser::{self, Block, BlockKind, ParsedDoc, PdfPageMeta};
use base64::Engine as _;
use lna_llm::{
    LlmClient, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions, ResolvedProfile, Role,
    ToolMode,
};
use lna_retrieval::tokenize_for_fts;
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, LnaError, LnaResult, Source, SourceIndexNode, SourceKind};
use rusqlite::params;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const SMALL_DOC_TOKEN_THRESHOLD: i64 = 12_000;
const PROBE_MAX_CHARS: usize = 9_000;

#[derive(Debug, Clone)]
pub struct AiIndexOutput {
    pub guide_summary_md: String,
    pub guide_topics: Vec<String>,
    pub nodes: Vec<SourceIndexNode>,
    pub scanned_page_texts: Vec<(u32, String)>,
}

#[derive(Debug, Deserialize)]
struct RawAiIndexJson {
    #[serde(default)]
    guide_summary_md: String,
    #[serde(default)]
    guide_topics: Vec<String>,
    #[serde(default)]
    nodes: Vec<RawAiNode>,
    #[serde(default)]
    page_texts: Vec<RawPageText>,
}

#[derive(Debug, Deserialize)]
struct RawAiNode {
    #[serde(default)]
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default = "default_level")]
    level: i32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    page_start: Option<i32>,
    #[serde(default)]
    page_end: Option<i32>,
    #[serde(default)]
    char_start: Option<i32>,
    #[serde(default)]
    char_end: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawPageText {
    page: u32,
    #[serde(default)]
    text: String,
}

fn default_level() -> i32 {
    1
}

fn resolve_active_chat_profile(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
) -> LnaResult<Option<ResolvedProfile>> {
    let Some(prof) = lna_storage::get_active_profile(db)? else {
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

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

/// Build a compact structural probe for AI indexing.
/// Guarantees <= ~4,000 input tokens even for a 600-page textbook.
pub fn build_structural_probe(src: &Source, doc: &ParsedDoc) -> (String, bool) {
    let total_tokens: i64 = doc
        .blocks
        .iter()
        .map(|b| estimate_tokens(&b.text) as i64)
        .sum();
    let is_small = total_tokens <= SMALL_DOC_TOKEN_THRESHOLD;

    let mut out = String::new();
    out.push_str(&format!(
        "文档标题: {}\n文件类型: {}\n总页数: {}\n估算总词元: {}\n\n",
        src.title,
        src.kind.as_str(),
        doc.page_count
            .map(|p| p.to_string())
            .unwrap_or_else(|| "未知".into()),
        total_tokens
    ));

    if is_small {
        out.push_str("=== 完整正文 ===\n");
        let mut used = out.chars().count();
        for b in &doc.blocks {
            if used >= PROBE_MAX_CHARS {
                break;
            }
            let prefix = match (b.page, &b.kind) {
                (Some(p), BlockKind::Heading { level }) => format!("[P.{p}][H{level}] "),
                (Some(p), _) => format!("[P.{p}] "),
                (None, BlockKind::Heading { level }) => format!("[H{level}] "),
                _ => String::new(),
            };
            let remain = PROBE_MAX_CHARS.saturating_sub(used);
            let piece = truncate_chars(&b.text, remain);
            out.push_str(&prefix);
            out.push_str(&piece);
            out.push('\n');
            used += prefix.chars().count() + piece.chars().count() + 1;
        }
        return (out, true);
    }

    // Large document: Heading/Outline skeleton + Front-matter + Stratified samples
    let mut outline_lines: Vec<String> = Vec::new();
    let mut seen_sections: Vec<String> = Vec::new();
    for b in &doc.blocks {
        if matches!(b.kind, BlockKind::Heading { .. }) {
            let line = match b.page {
                Some(p) => format!("- P.{p}: {}", truncate_chars(b.text.trim(), 90)),
                None => format!("- {}", truncate_chars(b.text.trim(), 90)),
            };
            if outline_lines.last().map(|s| s.as_str()) != Some(line.as_str()) {
                outline_lines.push(line);
            }
        } else if !b.section_path.is_empty() {
            let sec = b.section_path.join(" > ");
            if seen_sections.last().map(|s| s.as_str()) != Some(sec.as_str()) {
                seen_sections.push(sec.clone());
                let line = match b.page {
                    Some(p) => format!("- P.{p}: {}", truncate_chars(&sec, 90)),
                    None => format!("- {}", truncate_chars(&sec, 90)),
                };
                if outline_lines.last().map(|s| s.as_str()) != Some(line.as_str()) {
                    outline_lines.push(line);
                }
            }
        }
        if outline_lines.len() >= 120 {
            break;
        }
    }

    if !outline_lines.is_empty() {
        out.push_str("=== 原始目录/标题骨架 ===\n");
        for l in outline_lines.iter().take(100) {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }

    // Front matter (first ~1,800 chars, usually preface/abstract/TOC)
    out.push_str("=== 开头/前言/目录摘录 ===\n");
    let mut front_used = 0usize;
    for b in &doc.blocks {
        if front_used >= 1800 {
            break;
        }
        let pfx = b.page.map(|p| format!("[P.{p}] ")).unwrap_or_default();
        let take = truncate_chars(b.text.trim(), 1800usize.saturating_sub(front_used));
        if !take.is_empty() {
            out.push_str(&pfx);
            out.push_str(&take);
            out.push('\n');
            front_used += take.chars().count();
        }
    }
    out.push('\n');

    // Stratified middle & tail samples (up to 8 evenly spaced positions, ~260 chars each)
    let n = doc.blocks.len();
    if n > 12 {
        out.push_str("=== 全文均匀抽样片段 ===\n");
        let sample_count = 8usize;
        for i in 1..=sample_count {
            let idx = (n * i) / (sample_count + 1);
            if let Some(b) = doc.blocks.get(idx) {
                let pfx = b.page.map(|p| format!("[P.{p}] ")).unwrap_or_default();
                let sec = if b.section_path.is_empty() {
                    String::new()
                } else {
                    format!("[{}] ", b.section_path.join(" > "))
                };
                out.push_str(&format!(
                    "{pfx}{sec}{}\n",
                    truncate_chars(b.text.trim(), 260)
                ));
            }
        }
    }

    (truncate_chars(&out, PROBE_MAX_CHARS), false)
}

const AI_INDEX_SYSTEM_PROMPT: &str = r#"You are a document structural indexing and knowledge guide engine.
Based on the provided document content (or structural probe), output a single strictly valid JSON object with no extra text outside the JSON.
JSON schema:
{
  "guide_summary_md": "A concise 120-220 character/word overview matching the document's primary language, highlighting 5-8 core concepts, themes, or methods in **bold** like a textbook study guide.",
  "guide_topics": ["Topic 1", "Topic 2", "Topic 3", "Topic 4", "Topic 5"],
  "nodes": [
    {
      "id": "n1",
      "parent_id": null,
      "level": 1,
      "title": "Section or chapter title",
      "summary": "One concise sentence summarizing this section's core content",
      "keywords": ["keyword1", "keyword2", "keyword3"],
      "page_start": 1,
      "page_end": 12
    }
  ]
}
Requirements:
1. Output 4-6 high-value topic phrases in guide_topics that users can click to ask follow-up questions.
2. Build a clean hierarchical tree in nodes (8-30 major chapter/section nodes for long books, 2-8 topic nodes for short documents). Fill integer page_start and page_end when page numbers exist, otherwise null.
3. Output strictly valid JSON and never fabricate nonexistent sections."#;

/// Runs a single low-token AI indexing pass to generate Source Guide + Hierarchical Index Tree.
pub async fn run_ai_index_for_doc(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    src: &Source,
    doc: &ParsedDoc,
) -> LnaResult<Option<AiIndexOutput>> {
    let Some(resolved) = resolve_active_chat_profile(db, secrets)? else {
        return Ok(None);
    };
    if doc.blocks.is_empty() {
        return Ok(None);
    }

    let (probe, _is_small) = build_structural_probe(src, doc);
    let req = LlmRequest {
        system: AI_INDEX_SYSTEM_PROMPT.to_string(),
        messages: vec![LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(probe)],
        }],
        tools: vec![],
        tool_mode: ToolMode::None,
        output: OutputMode::JsonObject,
        max_output_tokens: 2048.min(resolved.max_output_tokens.max(1024)),
        reasoning: ReasoningOptions::default(),
    };

    let (raw, usage) = llm
        .complete_text(&resolved, &req, CancellationToken::new())
        .await?;
    let _ = lna_storage::record_usage(
        db,
        Some(&src.notebook_id),
        Some(&resolved.id),
        "ingest_ai_index",
        Some(&src.id),
        usage.input_tokens as i64,
        usage.output_tokens as i64,
        usage.cached_tokens as i64,
        false,
    );

    Ok(parse_ai_index_response(&src.id, &raw))
}

/// For pure-scanned PDFs (no text layer): inspect only front-matter/TOC pages (up to 8 pages)
/// via Vision model to build Source Guide + Chapter/Page Index Tree at minimal token cost.
/// If the PDF is short (<= 8 pages), also extract clean Markdown text in the same pass!
pub async fn run_ai_vision_toc_index(
    db: &Database,
    secrets: &Arc<dyn SecretStore>,
    llm: &LlmClient,
    src: &Source,
    pdf_path: &Path,
    page_count: u32,
    ocr_pages: &[u32],
) -> LnaResult<Option<AiIndexOutput>> {
    let Some(resolved) = media::resolve_modality_profile_with_secret(db, secrets, "vision")? else {
        return Ok(None);
    };

    let is_short_pdf = page_count <= 8;
    let probe_pages: Vec<u32> = if is_short_pdf {
        ocr_pages.iter().copied().take(8).collect()
    } else {
        // Front matter & TOC pages only (pages 1..=10, up to 8 pages)
        ocr_pages
            .iter()
            .copied()
            .filter(|&p| p <= 12)
            .take(8)
            .collect()
    };
    if probe_pages.is_empty() {
        return Ok(None);
    }

    let rendered = parser::render_pdf_pages_to_jpeg(pdf_path, &probe_pages, 1100)?;
    if rendered.is_empty() {
        return Ok(None);
    }

    let mut parts: Vec<Part> = Vec::with_capacity(rendered.len() * 2 + 1);
    for (pno, jpg) in &rendered {
        parts.push(Part::Text(format!("[PDF 第 {pno} 页（共 {page_count} 页）]")));
        parts.push(Part::Media {
            mime_type: "image/jpeg".into(),
            data_base64: Some(base64::engine::general_purpose::STANDARD.encode(jpg)),
            file_uri: None,
            audio_format: None,
        });
    }

    let extra_instr = if is_short_pdf {
        format!(
            "这是一份共 {page_count} 页的扫描版 PDF（标题：《{}》）。请一次性输出 JSON，除了 guide_summary_md、guide_topics、nodes 外，请在 \"page_texts\": [{{\"page\": 1, \"text\": \"...\"}}] 中准确转录每页的文字与 LaTeX 公式。",
            src.title
        )
    } else {
        format!(
            "这是一份共 {page_count} 页的扫描版书籍/长文档（标题：《{}》），以上是其前部封面/前言/目录页。请根据目录与前言一次性输出 JSON（guide_summary_md、guide_topics、以及覆盖全书主要章节与 PDF 页码范围 page_start~page_end 的 nodes）。page_texts 可留空数组 []，正文页将在用户提问时按需精读。",
            src.title
        )
    };
    parts.push(Part::Text(extra_instr));

    let req = LlmRequest {
        system: AI_INDEX_SYSTEM_PROMPT.to_string(),
        messages: vec![LlmMessage {
            role: Role::User,
            parts,
        }],
        tools: vec![],
        tool_mode: ToolMode::None,
        output: OutputMode::JsonObject,
        max_output_tokens: resolved.max_output_tokens.clamp(2048, 4096),
        reasoning: ReasoningOptions::default(),
    };

    let (raw, usage) = llm
        .complete_text(&resolved, &req, CancellationToken::new())
        .await?;
    let _ = lna_storage::record_usage(
        db,
        Some(&src.notebook_id),
        Some(&resolved.id),
        "ingest_vision_toc_index",
        Some(&src.id),
        usage.input_tokens as i64,
        usage.output_tokens as i64,
        usage.cached_tokens as i64,
        false,
    );

    if let Some(parsed) = parse_ai_index_response(&src.id, &raw) {
        return Ok(Some(parsed));
    }

    // Graceful fallback if the vision model replied with plain text (e.g. `=== PAGE N ===`) instead of JSON
    let trimmed = raw.trim();
    if !trimmed.is_empty() {
        let mut page_texts = Vec::new();
        let mut cur_page: Option<u32> = None;
        let mut buf = String::new();
        for line in trimmed.lines() {
            let l = line.trim();
            if l.starts_with("=== PAGE ") && l.ends_with(" ===") {
                if let Some(p) = cur_page.take() {
                    if !buf.trim().is_empty() {
                        page_texts.push((p, buf.trim().to_string()));
                    }
                    buf.clear();
                }
                let mid = l.trim_start_matches("=== PAGE ").trim_end_matches(" ===").trim();
                cur_page = mid.parse::<u32>().ok();
            } else {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(line);
            }
        }
        if let Some(p) = cur_page {
            if !buf.trim().is_empty() {
                page_texts.push((p, buf.trim().to_string()));
            }
        }
        if page_texts.is_empty() {
            let first_page = probe_pages.first().copied().unwrap_or(1);
            page_texts.push((first_page, trimmed.to_string()));
        }
        return Ok(Some(AiIndexOutput {
            guide_summary_md: String::new(),
            guide_topics: Vec::new(),
            nodes: Vec::new(),
            scanned_page_texts: page_texts,
        }));
    }

    Ok(None)
}

pub fn parse_ai_index_response(source_id: &str, raw: &str) -> Option<AiIndexOutput> {
    let trimmed = raw.trim();
    let json_slice = if let Some(s) = trimmed.strip_prefix("```json") {
        s.trim().strip_suffix("```").unwrap_or(s).trim()
    } else if let Some(s) = trimmed.strip_prefix("```") {
        s.trim().strip_suffix("```").unwrap_or(s).trim()
    } else if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        &trimmed[start..=end]
    } else {
        trimmed
    };

    let parsed: RawAiIndexJson = serde_json::from_str(json_slice).ok()?;
    let summary = parsed.guide_summary_md.trim().to_string();
    let topics: Vec<String> = parsed
        .guide_topics
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(6)
        .collect();

    // Map temporary node ids (like "n1") to globally unique node ids scoped by source_id
    let mut id_map: HashMap<String, String> = HashMap::new();
    for (idx, n) in parsed.nodes.iter().enumerate() {
        let local_id = if n.id.trim().is_empty() {
            format!("n{}", idx + 1)
        } else {
            n.id.trim().to_string()
        };
        let scoped = format!("{source_id}::{local_id}");
        id_map.insert(local_id, scoped);
    }

    let mut nodes = Vec::new();
    for (idx, n) in parsed.nodes.into_iter().enumerate() {
        let title = n.title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let local_id = if n.id.trim().is_empty() {
            format!("n{}", idx + 1)
        } else {
            n.id.trim().to_string()
        };
        let id = id_map
            .get(&local_id)
            .cloned()
            .unwrap_or_else(|| format!("{source_id}::{}", new_ulid()));
        let parent_id = n
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|pid| id_map.get(pid).cloned());
        let keywords: Vec<String> = n
            .keywords
            .into_iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .take(10)
            .collect();
        nodes.push(SourceIndexNode {
            id,
            source_id: source_id.to_string(),
            parent_id,
            ordinal: idx as i32,
            level: n.level.clamp(1, 4),
            title,
            summary: n.summary.trim().to_string(),
            keywords,
            page_start: n.page_start,
            page_end: n.page_end,
            char_start: n.char_start,
            char_end: n.char_end,
        });
    }

    let scanned_page_texts = parsed
        .page_texts
        .into_iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| (p.page, p.text.trim().to_string()))
        .collect();

    if summary.is_empty() && nodes.is_empty() {
        return None;
    }

    Some(AiIndexOutput {
        guide_summary_md: summary,
        guide_topics: topics,
        nodes,
        scanned_page_texts,
    })
}

/// Synthesize lazy page-stub blocks from AI TOC nodes for large scanned PDFs whose body pages
/// have not yet been visually read. Zero OCR tokens spent at import time!
pub fn build_scanned_stub_blocks(nodes: &[SourceIndexNode], page_meta: &[PdfPageMeta]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    if !nodes.is_empty() {
        for n in nodes {
            let p_range = match (n.page_start, n.page_end) {
                (Some(s), Some(e)) if e >= s => format!("第 {s}-{e} 页"),
                (Some(s), _) => format!("第 {s} 页"),
                _ => "扫描页".into(),
            };
            let kw = if n.keywords.is_empty() {
                String::new()
            } else {
                format!("；关键词：{}", n.keywords.join("、"))
            };
            let text = format!(
                "【AI 章节索引 · {p_range}】{}\n摘要：{}{}（本节为扫描图片页，提问涉及细节时系统将自动调用视觉模型按需精读并缓存）",
                n.title, n.summary, kw
            );
            let start = cursor;
            cursor += text.len();
            blocks.push(Block {
                kind: BlockKind::Paragraph,
                text,
                page: n.page_start.map(|p| p.max(1) as u32),
                section_path: vec![n.title.clone()],
                char_range: start..cursor,
            });
        }
    } else if !page_meta.is_empty() {
        let text = format!(
            "【扫描版 PDF 共 {} 页】当前尚未提取正文文本层，提问时系统可按页码调用视觉模型按需精读并缓存。",
            page_meta.len()
        );
        blocks.push(Block {
            kind: BlockKind::Paragraph,
            text: text.clone(),
            page: Some(1),
            section_path: vec![],
            char_range: 0..text.len(),
        });
    }
    blocks
}

/// Aligns chunks to AI-generated `source_index_nodes`, setting `node_id` and clean `section_path`.
pub fn align_chunks_to_nodes(chunks: &mut [ChunkDraft], nodes: &[SourceIndexNode]) {
    if nodes.is_empty() {
        return;
    }
    let parent_titles: HashMap<&str, &str> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.title.as_str()))
        .collect();

    let full_path = |n: &SourceIndexNode| -> String {
        if let Some(pid) = n.parent_id.as_deref() {
            if let Some(pt) = parent_titles.get(pid) {
                return format!("{pt} > {}", n.title);
            }
        }
        n.title.clone()
    };

    let total_chunks = chunks.len().max(1);
    for (idx, chunk) in chunks.iter_mut().enumerate() {
        // 1. Prefer exact page range match (pick deepest/narrowest node)
        let mut matched: Option<&SourceIndexNode> = None;
        if let Some(ps) = chunk.page_start {
            let mut best_span = i32::MAX;
            for n in nodes {
                if let Some(ns) = n.page_start {
                    let ne = n.page_end.unwrap_or(ns);
                    if ps >= ns && ps <= ne {
                        let span = (ne - ns).max(0);
                        if span < best_span
                            || (span == best_span
                                && matched.map(|m| n.level > m.level).unwrap_or(true))
                        {
                            best_span = span;
                            matched = Some(n);
                        }
                    }
                }
            }
        }

        // 2. Fallback: match by section_path substring
        if matched.is_none() {
            if let Some(sec) = chunk.section_path.as_deref() {
                let sec_l = sec.to_lowercase();
                matched = nodes
                    .iter()
                    .find(|n| {
                        let nt = n.title.to_lowercase();
                        !nt.is_empty() && (sec_l.contains(&nt) || nt.contains(&sec_l))
                    });
            }
        }

        // 3. Fallback: proportional ordinal assignment
        if matched.is_none() {
            let node_idx = (idx * nodes.len()) / total_chunks;
            matched = nodes.get(node_idx.min(nodes.len() - 1));
        }

        if let Some(n) = matched {
            chunk.node_id = Some(n.id.clone());
            chunk.section_path = Some(full_path(n));
        }
    }
}

/// Permanently caches on-demand visually-read page text into `source_chunks` + `chunks_fts`,
/// so future questions about the same page spend 0 vision tokens.
pub fn cache_visual_page_result(
    db: &Database,
    source_id: &str,
    notebook_id: &str,
    page_no: u32,
    text: &str,
) -> LnaResult<Option<i64>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let nodes = lna_storage::list_source_index_nodes(db, source_id).unwrap_or_default();
    let p_i32 = page_no as i32;
    let matched_node = nodes.iter().find(|n| {
        n.page_start
            .map(|s| p_i32 >= s && p_i32 <= n.page_end.unwrap_or(s))
            .unwrap_or(false)
    });
    let node_id = matched_node.map(|n| n.id.clone());
    let section_path = matched_node
        .map(|n| n.title.clone())
        .or_else(|| Some(format!("第 {page_no} 页（AI 视觉精读）")));
    let token_count = estimate_tokens(trimmed);
    let fts_tokens = tokenize_for_fts(trimmed);

    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        let next_idx: i32 = tx
            .query_row(
                "SELECT COALESCE(MAX(chunk_index), -1) + 1 FROM source_chunks WHERE source_id = ?1",
                [source_id],
                |r| r.get(0),
            )
            .map_err(map_sql_err)?;
        tx.execute(
            "INSERT INTO source_chunks
             (source_id, notebook_id, chunk_index, section_path, page_start, page_end, char_start, char_end, content, token_count, node_id)
             VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8,?9,?10)",
            params![
                source_id,
                notebook_id,
                next_idx,
                section_path,
                p_i32,
                p_i32,
                trimmed.len() as i32,
                trimmed,
                token_count,
                node_id
            ],
        )
        .map_err(map_sql_err)?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO chunks_fts(rowid, tokens) VALUES (?1, ?2)",
            params![rowid, fts_tokens],
        )
        .map_err(map_sql_err)?;
        tx.commit().map_err(map_sql_err)?;
        Ok(Some(rowid))
    })
}

/// Generates a self-contained dark-mode HTML preview file for Office / EPUB / CSV / RTF sources
/// so the Windows client can render the original document cleanly in WebView2.
pub fn ensure_preview_html(src: &Source, blob_path: &Path) -> LnaResult<Option<PathBuf>> {
    if !matches!(
        src.kind,
        SourceKind::Docx
            | SourceKind::Pptx
            | SourceKind::Epub
            | SourceKind::Odt
            | SourceKind::Rtf
            | SourceKind::Hwp
            | SourceKind::Hwpx
            | SourceKind::Csv
    ) {
        return Ok(None);
    }

    let html_path = blob_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("preview.html");
    if html_path.exists() {
        return Ok(Some(html_path));
    }

    let mut body = String::new();
    match src.kind {
        SourceKind::Csv => {
            let bytes = std::fs::read(blob_path).map_err(|e| LnaError::internal(e.to_string()))?;
            let text = parser::decode_text_bytes(&bytes);
            body.push_str("<div class=\"table-wrap\"><table>");
            for (r_idx, line) in text.lines().take(500).enumerate() {
                let tag = if r_idx == 0 { "th" } else { "td" };
                body.push_str("<tr>");
                for cell in line.split(',') {
                    body.push_str(&format!("<{tag}>{}</{tag}>", html_escape(cell.trim())));
                }
                body.push_str("</tr>");
            }
            body.push_str("</table></div>");
        }
        SourceKind::Docx | SourceKind::Pptx | SourceKind::Epub | SourceKind::Odt | SourceKind::Hwpx => {
            let rich = match src.kind {
                SourceKind::Docx => office::parse_docx(blob_path)?,
                SourceKind::Pptx => office::parse_pptx(blob_path)?,
                SourceKind::Epub => office::parse_epub(blob_path)?,
                SourceKind::Odt => office::parse_odt(blob_path)?,
                SourceKind::Hwpx => office::parse_hwpx(blob_path)?,
                _ => unreachable!(),
            };
            render_blocks_to_html(&rich.doc.blocks, &mut body);
            // Append embedded images (up to 24) if present
            if !rich.embedded_images.is_empty() {
                body.push_str("<h3>文档内嵌图片</h3><div class=\"img-grid\">");
                for im in rich.embedded_images.iter().take(24) {
                    if let Ok(bytes) = office::extract_zip_embedded_image(blob_path, &im.zip_entry) {
                        let ext = im.zip_entry.rsplit('.').next().unwrap_or("png");
                        let mime = crate::formats::mime_type_for_ext(ext);
                        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                        body.push_str(&format!(
                            "<figure><img src=\"data:{mime};base64,{b64}\" /><figcaption>{}</figcaption></figure>",
                            html_escape(&im.id)
                        ));
                    }
                }
                body.push_str("</div>");
            }
        }
        SourceKind::Rtf | SourceKind::Hwp => {
            let doc = if src.kind == SourceKind::Rtf {
                let bytes = std::fs::read(blob_path).map_err(|e| LnaError::internal(e.to_string()))?;
                office::parse_rtf(&bytes)?
            } else {
                office::parse_hwp(blob_path)?
            };
            render_blocks_to_html(&doc.blocks, &mut body);
        }
        _ => return Ok(None),
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{}</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{
    margin: 0; padding: 18px 20px 36px;
    background: #1B1E28; color: #E8EAED;
    font: 14px/1.75 "Microsoft YaHei UI", "Segoe UI", sans-serif;
    word-break: break-word;
  }}
  h1, h2, h3, h4 {{ color: #F1F3F4; margin: 1.1em 0 0.45em; line-height: 1.35; }}
  h1 {{ font-size: 1.45em; border-bottom: 1px solid #2E3448; padding-bottom: 6px; }}
  h2 {{ font-size: 1.25em; }}
  h3 {{ font-size: 1.1em; }}
  p {{ margin: 0.55em 0; white-space: pre-wrap; }}
  .page-tag {{
    display: inline-block; font-size: 11px; color: #9AA0A6;
    background: #252A38; padding: 1px 8px; border-radius: 10px; margin: 12px 0 4px;
  }}
  pre {{ background: #141720; padding: 10px 12px; border-radius: 8px; overflow-x: auto; }}
  .table-wrap {{ overflow-x: auto; }}
  table {{ border-collapse: collapse; width: 100%; font-size: 13px; }}
  th, td {{ border: 1px solid #30374B; padding: 6px 10px; text-align: left; }}
  th {{ background: #252A38; font-weight: 600; }}
  .img-grid {{ display: flex; flex-direction: column; gap: 12px; margin-top: 12px; }}
  figure {{ margin: 0; background: #232836; padding: 10px; border-radius: 8px; }}
  img {{ max-width: 100%; height: auto; border-radius: 4px; display: block; }}
  figcaption {{ font-size: 11px; color: #9AA0A6; margin-top: 6px; }}
</style>
</head>
<body>
{}
</body>
</html>"#,
        html_escape(&src.title),
        body
    );

    std::fs::write(&html_path, html).map_err(|e| LnaError::internal(e.to_string()))?;
    Ok(Some(html_path))
}

fn render_blocks_to_html(blocks: &[Block], out: &mut String) {
    let mut last_page: Option<u32> = None;
    for b in blocks {
        if b.page.is_some() && b.page != last_page {
            last_page = b.page;
            if let Some(p) = b.page {
                out.push_str(&format!("<div class=\"page-tag\" id=\"page-{p}\">第 {p} 页</div>\n"));
            }
        }
        let esc = html_escape(&b.text);
        match b.kind {
            BlockKind::Heading { level } => {
                let h = level.clamp(1, 4);
                out.push_str(&format!("<h{h}>{esc}</h{h}>\n"));
            }
            BlockKind::Code => {
                out.push_str(&format!("<pre><code>{esc}</code></pre>\n"));
            }
            _ => {
                out.push_str(&format!("<p>{esc}</p>\n"));
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lna_types::SourceStatus;

    fn dummy_source(kind: SourceKind) -> Source {
        Source {
            id: "src_test".into(),
            notebook_id: "nb_test".into(),
            title: "Linear Algebra 5th Ed".into(),
            kind,
            origin_uri: None,
            blob_path: None,
            content_hash: "abc".into(),
            byte_size: 1000,
            page_count: Some(500),
            token_count: 300_000,
            language: Some("zh".into()),
            is_selected: true,
            status: SourceStatus::Ready,
            error_code: None,
            error_detail: None,
            guide_summary_md: None,
            guide_topics: vec![],
            ai_indexed: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn large_doc_probe_is_strictly_bounded() {
        let mut blocks = Vec::new();
        for p in 1..=500u32 {
            blocks.push(Block {
                kind: if p % 50 == 1 {
                    BlockKind::Heading { level: 1 }
                } else {
                    BlockKind::Paragraph
                },
                text: format!("第 {p} 页内容：向量空间、矩阵分解、特征值与奇异值分解。").repeat(20),
                page: Some(p),
                section_path: vec![format!("第 {} 章", (p - 1) / 50 + 1)],
                char_range: 0..100,
            });
        }
        let doc = ParsedDoc {
            title: Some("Linear Algebra".into()),
            language: Some("zh".into()),
            page_count: Some(500),
            blocks,
        };
        let (probe, is_small) = build_structural_probe(&dummy_source(SourceKind::Pdf), &doc);
        assert!(!is_small);
        assert!(probe.chars().count() <= PROBE_MAX_CHARS);
        assert!(estimate_tokens(&probe) <= 10_500);
        assert!(probe.contains("原始目录/标题骨架"));
    }

    #[test]
    fn parse_ai_index_response_extracts_guide_topics_and_nodes() {
        let raw = r#"```json
        {
          "guide_summary_md": "本书系统讲解了**向量空间**与**矩阵分解**。",
          "guide_topics": ["向量空间", "特征值", "奇异值分解"],
          "nodes": [
            {"id": "c1", "parent_id": null, "level": 1, "title": "第1章 向量引论", "summary": "介绍点积与长度", "keywords": ["点积", "施瓦茨不等式"], "page_start": 1, "page_end": 45},
            {"id": "c1_2", "parent_id": "c1", "level": 2, "title": "1.2 长度与点积", "summary": "余弦公式与施瓦茨不等式", "keywords": ["余弦", "不等式"], "page_start": 30, "page_end": 40}
          ]
        }
        ```"#;
        let out = parse_ai_index_response("src_1", raw).expect("parsed");
        assert!(out.guide_summary_md.contains("**向量空间**"));
        assert_eq!(out.guide_topics.len(), 3);
        assert_eq!(out.nodes.len(), 2);
        assert_eq!(out.nodes[1].parent_id.as_deref(), Some("src_1::c1"));
    }
}
