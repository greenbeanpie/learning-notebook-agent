pub mod ai_indexer;
pub mod chunker;
pub mod formats;
pub mod media;
pub mod office;
pub mod parser;
pub mod ssrf;
pub mod win_ocr;

use lna_embedding::EmbeddingService;
use lna_jobs::JobQueue;
use lna_llm::LlmClient;
use lna_retrieval::tokenize_for_fts;
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, now_ms, ErrorCode, LnaError, LnaResult, Source, SourceKind};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SOURCES_PER_NOTEBOOK: i64 = 300;
const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct IngestService {
    db: Database,
    jobs: JobQueue,
    embedding: EmbeddingService,
    llm: LlmClient,
    secrets: Arc<dyn SecretStore>,
}

impl IngestService {
    pub fn new(
        db: Database,
        jobs: JobQueue,
        embedding: EmbeddingService,
        llm: LlmClient,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            db,
            jobs,
            embedding,
            llm,
            secrets,
        }
    }

    pub fn add_file(&self, notebook_id: &str, file_path: &Path) -> LnaResult<(Source, bool)> {
        let meta = std::fs::metadata(file_path).map_err(|e| LnaError::invalid(e.to_string()))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                "File exceeds 200 MB limit",
            ));
        }
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let bytes = std::fs::read(file_path).map_err(|e| LnaError::internal(e.to_string()))?;

        let kind = match formats::classify_extension(&ext) {
            Some(k) => k,
            None if formats::is_probably_plain_text(&bytes) => SourceKind::Txt,
            None => {
                return Err(LnaError::new(
                    ErrorCode::SourceUnsupported,
                    format!("Unsupported file extension: .{ext}"),
                ))
            }
        };

        if formats::is_audio_or_video(kind) && !media::is_audio_video_enabled(&self.db) {
            return Err(LnaError::new(
                ErrorCode::SourceUnsupported,
                "已在设置中关闭音视频文件处理，如需导入请在“模型协议配置”中开启。",
            ));
        }

        let title = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let blob_ext = if ext.is_empty() { "txt" } else { ext.as_str() };

        self.enqueue_bytes(notebook_id, &title, kind, blob_ext, None, &bytes)
    }

    pub fn add_text(
        &self,
        notebook_id: &str,
        title: &str,
        text: &str,
        kind: SourceKind,
    ) -> LnaResult<(Source, bool)> {
        let bytes = text.as_bytes();
        if bytes.len() > MAX_TEXT_BYTES {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                "Pasted text exceeds 2 MB limit",
            ));
        }
        let ext = if matches!(kind, SourceKind::Md | SourceKind::Note) {
            "md"
        } else {
            "txt"
        };
        self.enqueue_bytes(notebook_id, title, kind, ext, None, bytes)
    }

    pub async fn add_url(&self, notebook_id: &str, url: &str) -> LnaResult<(Source, bool)> {
        let (final_url, bytes) = ssrf::safe_fetch_url(url).await?;
        let html_str = parser::decode_text_bytes(&bytes);
        let parsed = parser::parse_html(&html_str);
        let title = parsed.title.unwrap_or_else(|| final_url.clone());
        self.enqueue_bytes(
            notebook_id,
            &title,
            SourceKind::Web,
            "html",
            Some(&final_url),
            &bytes,
        )
    }

    fn enqueue_bytes(
        &self,
        notebook_id: &str,
        title: &str,
        kind: SourceKind,
        ext: &str,
        origin_uri: Option<&str>,
        bytes: &[u8],
    ) -> LnaResult<(Source, bool)> {
        let count: i64 = self.db.read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sources WHERE notebook_id = ?1",
                [notebook_id],
                |r| r.get(0),
            )
            .map_err(map_sql_err)
        })?;
        if count >= MAX_SOURCES_PER_NOTEBOOK {
            return Err(LnaError::new(
                ErrorCode::SourceLimitReached,
                "Notebook reached 300 sources limit",
            ));
        }

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let content_hash = hex::encode(hasher.finalize());

        let existing_id: Option<String> = self.db.read(|conn| {
            conn.query_row(
                "SELECT id FROM sources WHERE notebook_id = ?1 AND content_hash = ?2",
                params![notebook_id, content_hash],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        if let Some(eid) = existing_id {
            return Ok((lna_storage::get_source(&self.db, &eid)?, true));
        }

        let source_id = new_ulid();
        let blob_dir = self.db.paths().blobs_dir.join(notebook_id).join(&source_id);
        std::fs::create_dir_all(&blob_dir).map_err(|e| LnaError::internal(e.to_string()))?;
        let blob_path = blob_dir.join(format!("original.{ext}"));
        std::fs::write(&blob_path, bytes).map_err(|e| LnaError::internal(e.to_string()))?;

        let now = now_ms();
        let kind_str = kind.as_str();
        let bp_str = blob_path.to_string_lossy().to_string();

        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO sources
                 (id, notebook_id, title, kind, origin_uri, blob_path, content_hash, byte_size, status, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'QUEUED',?9,?10)",
                params![
                    source_id,
                    notebook_id,
                    title,
                    kind_str,
                    origin_uri,
                    bp_str,
                    content_hash,
                    bytes.len() as i64,
                    now,
                    now
                ],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;

        self.jobs
            .enqueue("ingest_source", Some(notebook_id), &source_id, 10)?;

        Ok((lna_storage::get_source(&self.db, &source_id)?, false))
    }

    pub async fn process_ingest_job(&self, source_id: &str) -> LnaResult<()> {
        let src = lna_storage::get_source(&self.db, source_id)?;
        let blob_path = src
            .blob_path
            .as_deref()
            .ok_or_else(|| LnaError::internal("Source missing blob_path"))?;

        self.update_status(source_id, "PARSING", None, None)?;

        let bp = PathBuf::from(blob_path);
        let ext = bp
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = src.kind;

        let mut precomputed_ai_index: Option<ai_indexer::AiIndexOutput> = None;

        let parsed: LnaResult<parser::ParsedDoc> = match kind {
            SourceKind::Pdf => {
                let bp_clone = bp.clone();
                let ext_res = tokio::task::spawn_blocking(move || parser::parse_pdf_detailed(&bp_clone))
                    .await
                    .map_err(|e| LnaError::internal(format!("Parser worker panicked or was cancelled: {e}")))
                    .and_then(|r| r);

                match ext_res {
                    Ok(mut extract) => {
                        media::write_media_index(
                            &bp,
                            &media::MediaIndex {
                                source_id: src.id.clone(),
                                kind: "pdf".into(),
                                page_count: extract.doc.page_count,
                                ocr_pages: extract.ocr_needed_pages.clone(),
                                pdf_pages: extract.page_meta.clone(),
                                embedded_images: Vec::new(),
                            },
                        );
                        // For pure-scanned PDFs (no selectable text layer):
                        // Use low-token AI Vision TOC indexing (front pages only for large books;
                        // body pages are read on-demand during chat and permanently cached).
                        if extract.doc.blocks.is_empty() && !extract.ocr_needed_pages.is_empty() {
                            let total_pages = extract.doc.page_count.unwrap_or(extract.page_meta.len() as i64).max(1) as u32;
                            match ai_indexer::run_ai_vision_toc_index(
                                &self.db,
                                &self.secrets,
                                &self.llm,
                                &src,
                                &bp,
                                total_pages,
                                &extract.ocr_needed_pages,
                            )
                            .await
                            {
                                Ok(Some(ai_out)) => {
                                    if !ai_out.scanned_page_texts.is_empty() {
                                        let mut cur = 0usize;
                                        for (pno, txt) in &ai_out.scanned_page_texts {
                                            let start = cur;
                                            cur += txt.len();
                                            extract.doc.blocks.push(parser::Block {
                                                kind: parser::BlockKind::Paragraph,
                                                text: txt.clone(),
                                                page: Some(*pno),
                                                section_path: vec![],
                                                char_range: start..cur,
                                            });
                                        }
                                    } else {
                                        extract.doc.blocks = ai_indexer::build_scanned_stub_blocks(
                                            &ai_out.nodes,
                                            &extract.page_meta,
                                        );
                                    }
                                    precomputed_ai_index = Some(ai_out);
                                }
                                Ok(None) | Err(_) => {
                                    // Fallback to local OCR when no vision model is configured
                                    if let Ok(ocr_blocks) = media::ocr_pdf_pages(
                                        &self.db,
                                        &self.secrets,
                                        &self.llm,
                                        &src,
                                        &bp,
                                        &extract.ocr_needed_pages,
                                        &extract.page_meta,
                                    )
                                    .await
                                    {
                                        extract.doc.blocks.extend(ocr_blocks);
                                    }
                                }
                            }
                            if !extract.doc.blocks.is_empty() {
                                extract.doc.blocks.sort_by_key(|b| b.page.unwrap_or(0));
                                let sample = extract
                                    .doc
                                    .blocks
                                    .iter()
                                    .take(5)
                                    .map(|b| b.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if !sample.is_empty() {
                                    extract.doc.language = Some(parser::detect_language(&sample));
                                }
                            }
                        }
                        if extract.doc.blocks.is_empty() && !extract.ocr_needed_pages.is_empty() {
                            Err(LnaError::new(
                                ErrorCode::SourceNeedsOcr,
                                "扫描版 PDF 未提取到文字，请在“模型协议配置”中为视觉模型勾选“用于处理图文”后重试",
                            ))
                        } else {
                            Ok(extract.doc)
                        }
                    }
                    Err(e) => Err(e),
                }
            }
            SourceKind::Image => {
                media::write_media_index(
                    &bp,
                    &media::MediaIndex {
                        source_id: src.id.clone(),
                        kind: "image".into(),
                        page_count: Some(1),
                        ocr_pages: vec![],
                        pdf_pages: vec![],
                        embedded_images: vec![],
                    },
                );
                media::ingest_image(&self.db, &self.secrets, &self.llm, &src, &bp, &ext).await
            }
            SourceKind::Audio | SourceKind::Video => {
                media::ingest_audio_video(&self.db, &self.secrets, &self.llm, &src, &bp, &ext).await
            }
            SourceKind::Docx
            | SourceKind::Pptx
            | SourceKind::Epub
            | SourceKind::Odt
            | SourceKind::Hwpx => {
                let bp_clone = bp.clone();
                let rich_res = tokio::task::spawn_blocking(move || match kind {
                    SourceKind::Docx => office::parse_docx(&bp_clone),
                    SourceKind::Pptx => office::parse_pptx(&bp_clone),
                    SourceKind::Epub => office::parse_epub(&bp_clone),
                    SourceKind::Odt => office::parse_odt(&bp_clone),
                    SourceKind::Hwpx => office::parse_hwpx(&bp_clone),
                    _ => unreachable!(),
                })
                .await
                .map_err(|e| LnaError::internal(format!("Parser worker panicked: {e}")))
                .and_then(|r| r);

                match rich_res {
                    Ok(mut rich) => {
                        let imgs = rich.embedded_images.clone();
                        media::write_media_index(
                            &bp,
                            &media::MediaIndex {
                                source_id: src.id.clone(),
                                kind: kind.as_str().into(),
                                page_count: rich.doc.page_count,
                                ocr_pages: vec![],
                                pdf_pages: vec![],
                                embedded_images: imgs.clone(),
                            },
                        );
                        if rich.doc.blocks.is_empty() && !imgs.is_empty() {
                            let mut image_payloads = Vec::new();
                            for im in imgs.iter().take(6) {
                                if let Ok(bytes) =
                                    office::extract_zip_embedded_image(&bp, &im.zip_entry)
                                {
                                    let im_ext = im
                                        .zip_entry
                                        .rsplit('.')
                                        .next()
                                        .unwrap_or("png")
                                        .to_string();
                                    let label = im
                                        .page
                                        .map(|p| format!("第 {p} 页图片 {}", im.id))
                                        .unwrap_or_else(|| format!("图片 {}", im.id));
                                    image_payloads.push((label, im_ext, bytes));
                                }
                            }
                            if !image_payloads.is_empty() {
                                if let Ok(vis_txt) = media::vision_inspect_images(
                                    &self.db,
                                    &self.secrets,
                                    &self.llm,
                                    &src.notebook_id,
                                    image_payloads,
                                    "请提取以上文档图片中的全部文字与关键内容摘要。",
                                )
                                .await
                                {
                                    let parsed_md = parser::parse_markdown(&vis_txt);
                                    rich.doc.blocks = parsed_md.blocks;
                                    rich.doc.language = parsed_md.language;
                                }
                            }
                        }
                        Ok(rich.doc)
                    }
                    Err(e) => Err(e),
                }
            }
            SourceKind::Csv | SourceKind::Rtf | SourceKind::Hwp | SourceKind::Md | SourceKind::Note
            | SourceKind::Txt | SourceKind::Web => {
                let bp_clone = bp.clone();
                let ext_clone = ext.clone();
                tokio::task::spawn_blocking(move || match kind {
                    SourceKind::Csv => {
                        let bytes =
                            std::fs::read(&bp_clone).map_err(|e| LnaError::internal(e.to_string()))?;
                        office::parse_csv(&bytes)
                    }
                    SourceKind::Rtf => {
                        let bytes =
                            std::fs::read(&bp_clone).map_err(|e| LnaError::internal(e.to_string()))?;
                        office::parse_rtf(&bytes)
                    }
                    SourceKind::Hwp => office::parse_hwp(&bp_clone),
                    SourceKind::Md | SourceKind::Note => {
                        let bytes =
                            std::fs::read(&bp_clone).map_err(|e| LnaError::internal(e.to_string()))?;
                        Ok(parser::parse_markdown(&parser::decode_text_bytes(&bytes)))
                    }
                    SourceKind::Txt => {
                        let bytes =
                            std::fs::read(&bp_clone).map_err(|e| LnaError::internal(e.to_string()))?;
                        let text = parser::decode_text_bytes(&bytes);
                        if matches!(ext_clone.as_str(), "html" | "htm" | "xhtml") {
                            Ok(parser::parse_html(&text))
                        } else if matches!(ext_clone.as_str(), "md" | "markdown" | "mdx") {
                            Ok(parser::parse_markdown(&text))
                        } else {
                            Ok(parser::parse_plain_text(&text))
                        }
                    }
                    SourceKind::Web => {
                        let bytes =
                            std::fs::read(&bp_clone).map_err(|e| LnaError::internal(e.to_string()))?;
                        Ok(parser::parse_html(&parser::decode_text_bytes(&bytes)))
                    }
                    _ => unreachable!(),
                })
                .await
                .map_err(|e| LnaError::internal(format!("Parser worker panicked or was cancelled: {e}")))
                .and_then(|r| r)
            }
        };

        let doc = match parsed {
            Ok(d) => d,
            Err(e) => return Err(self.fail_ingest(source_id, e)),
        };

        self.update_status(source_id, "CHUNKING", None, None)?;

        // Stage 1: AI-driven structural indexing & Source Guide (1 low-token call)
        let ai_index = match precomputed_ai_index {
            Some(pre) => Some(pre),
            None => match ai_indexer::run_ai_index_for_doc(&self.db, &self.secrets, &self.llm, &src, &doc).await {
                Ok(opt) => opt,
                Err(e) => {
                    tracing::warn!(source_id, error = %e.message, "AI structural indexing failed; continuing with physical anchors");
                    None
                }
            },
        };

        let mut drafts = chunker::chunk_document(&doc);
        if drafts.is_empty() {
            let e = LnaError::new(
                ErrorCode::SourceUnsupported,
                "No extractable text in this source",
            );
            let _ =
                self.update_status(source_id, "FAILED", Some(e.code.as_str()), Some(&e.message));
            return Err(e);
        }

        if let Some(ref ai) = ai_index {
            ai_indexer::align_chunks_to_nodes(&mut drafts, &ai.nodes);
            let _ = lna_storage::replace_source_index_nodes(&self.db, source_id, &ai.nodes);
            let guide_md = if ai.guide_summary_md.trim().is_empty() {
                None
            } else {
                Some(ai.guide_summary_md.as_str())
            };
            let _ = lna_storage::update_source_guide(&self.db, source_id, guide_md, &ai.guide_topics, true);
        }

        let total_tokens: i64 = drafts.iter().map(|c| c.token_count as i64).sum();

        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute("DELETE FROM source_chunks WHERE source_id = ?1", [source_id])
                .map_err(map_sql_err)?;
            for c in &drafts {
                tx.execute(
                    "INSERT INTO source_chunks
                     (source_id, notebook_id, chunk_index, section_path, page_start, page_end, char_start, char_end, content, token_count, node_id)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        source_id,
                        src.notebook_id,
                        c.chunk_index,
                        c.section_path,
                        c.page_start,
                        c.page_end,
                        c.char_start,
                        c.char_end,
                        c.content,
                        c.token_count,
                        c.node_id
                    ],
                )
                .map_err(map_sql_err)?;
                let rowid = tx.last_insert_rowid();
                let fts_tokens = tokenize_for_fts(&c.content);
                tx.execute(
                    "INSERT INTO chunks_fts(rowid, tokens) VALUES (?1, ?2)",
                    params![rowid, fts_tokens],
                )
                .map_err(map_sql_err)?;
            }
            tx.execute(
                "UPDATE sources SET page_count=?1, token_count=?2, language=?3, status='EMBEDDING', updated_at=?4 WHERE id=?5",
                params![doc.page_count, total_tokens, doc.language, now_ms(), source_id],
            )
            .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })?;

        // Embedding (failure does not fail the source — BM25 remains usable per Section 7.1)
        if let Err(e) = self
            .embed_source_chunks(source_id, &src.notebook_id, &src.title)
            .await
        {
            tracing::warn!(source_id, error = %e.message, "Embedding failed; source remains lexically searchable");
            let _ = self.update_status(
                source_id,
                "READY",
                Some("EMBEDDING_INCOMPLETE"),
                Some(&e.message),
            );
        } else {
            self.update_status(source_id, "READY", None, None)?;
        }

        // Trigger notebook summary refresh
        let _ = self.jobs.enqueue(
            "refresh_summary",
            Some(&src.notebook_id),
            &src.notebook_id,
            200,
        );

        Ok(())
    }

    pub async fn embed_source_chunks(
        &self,
        source_id: &str,
        notebook_id: &str,
        source_title: &str,
    ) -> LnaResult<()> {
        let model = match self.embedding.active_model()? {
            Some(m) => m,
            None => return Ok(()),
        };
        self.embed_source_chunks_for_model(source_id, notebook_id, source_title, &model)
            .await
    }

    pub async fn embed_source_chunks_for_model(
        &self,
        source_id: &str,
        notebook_id: &str,
        source_title: &str,
        model: &lna_embedding::EmbeddingModelRecord,
    ) -> LnaResult<()> {
        let pending: Vec<(i64, Option<String>, String)> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.section_path, c.content
                     FROM source_chunks c
                     LEFT JOIN chunk_embedding_state s
                       ON s.chunk_id = c.id AND s.embedding_model_id = ?1
                     WHERE c.source_id = ?2 AND s.chunk_id IS NULL
                     ORDER BY c.chunk_index ASC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map(params![model.id, source_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;

        for batch in pending.chunks(16) {
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, sec, content)| {
                    format!(
                        "{} | {} | {}",
                        source_title,
                        sec.as_deref().unwrap_or(""),
                        content
                    )
                })
                .collect();
            let vecs = self.embedding.embed_passages(model, &texts).await?;
            let items: Vec<(i64, Vec<f32>)> = batch
                .iter()
                .zip(vecs)
                .map(|((cid, _, _), v)| (*cid, v))
                .collect();
            self.embedding
                .store_chunk_embeddings(model, notebook_id, &items)?;
        }
        Ok(())
    }

    /// B5: Return source chunks for preview & citation jump.
    pub fn source_preview(
        &self,
        source_id: &str,
        page: Option<i32>,
    ) -> LnaResult<Vec<lna_types::SourceChunk>> {
        lna_storage::get_source(&self.db, source_id)?; // NOT_FOUND for unknown ids
        self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, source_id, notebook_id, chunk_index, section_path, page_start, page_end, char_start, char_end, content, token_count
                 FROM source_chunks WHERE source_id = ?1 AND (?2 IS NULL OR (page_start <= ?2 AND page_end >= ?2))
                 ORDER BY chunk_index ASC"
            ).map_err(map_sql_err)?;
            let rows = stmt.query_map(params![source_id, page], |r| {
                Ok(lna_types::SourceChunk {
                    id: r.get(0)?, source_id: r.get(1)?, notebook_id: r.get(2)?,
                    chunk_index: r.get(3)?, section_path: r.get(4)?, page_start: r.get(5)?,
                    page_end: r.get(6)?, char_start: r.get(7)?, char_end: r.get(8)?,
                    content: r.get(9)?, token_count: r.get(10)?,
                })
            }).map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows { out.push(r.map_err(map_sql_err)?); }
            Ok(out)
        })
    }

    /// On-demand AI Source Guide & Hierarchical Index generation (useful for legacy sources
    /// imported before an AI model was configured).
    pub async fn regenerate_source_guide(&self, source_id: &str) -> LnaResult<Source> {
        let src = lna_storage::get_source(&self.db, source_id)?;
        let chunks = self.source_preview(source_id, None)?;
        if chunks.is_empty() {
            return Err(LnaError::invalid("该来源暂无已索引内容，无法生成来源指南"));
        }
        let blocks: Vec<parser::Block> = chunks
            .iter()
            .map(|c| parser::Block {
                kind: parser::BlockKind::Paragraph,
                text: c.content.clone(),
                page: c.page_start.map(|p| p.max(1) as u32),
                section_path: c
                    .section_path
                    .as_deref()
                    .map(|s| s.split(" > ").map(str::to_string).collect())
                    .unwrap_or_default(),
                char_range: (c.char_start.max(0) as usize)..(c.char_end.max(0) as usize),
            })
            .collect();
        let doc = parser::ParsedDoc {
            title: Some(src.title.clone()),
            language: src.language.clone(),
            page_count: src.page_count,
            blocks,
        };
        let ai = ai_indexer::run_ai_index_for_doc(&self.db, &self.secrets, &self.llm, &src, &doc)
            .await?
            .ok_or_else(|| {
                LnaError::new(
                    ErrorCode::ModelNotConfigured,
                    "请先在“模型协议配置”中配置并启用可用的 AI 模型",
                )
            })?;
        lna_storage::replace_source_index_nodes(&self.db, source_id, &ai.nodes)?;
        let guide_md = if ai.guide_summary_md.trim().is_empty() {
            None
        } else {
            Some(ai.guide_summary_md.as_str())
        };
        lna_storage::update_source_guide(&self.db, source_id, guide_md, &ai.guide_topics, true)?;

        // Backfill node_id on existing chunks
        let mut drafts: Vec<chunker::ChunkDraft> = chunks
            .iter()
            .map(|c| chunker::ChunkDraft {
                chunk_index: c.chunk_index,
                section_path: c.section_path.clone(),
                node_id: None,
                page_start: c.page_start,
                page_end: c.page_end,
                char_start: c.char_start,
                char_end: c.char_end,
                content: c.content.clone(),
                token_count: c.token_count,
            })
            .collect();
        ai_indexer::align_chunks_to_nodes(&mut drafts, &ai.nodes);
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            for (c, d) in chunks.iter().zip(drafts.iter()) {
                tx.execute(
                    "UPDATE source_chunks SET node_id = ?1, section_path = COALESCE(?2, section_path) WHERE id = ?3",
                    params![d.node_id, d.section_path, c.id],
                )
                .map_err(map_sql_err)?;
            }
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })?;

        lna_storage::get_source(&self.db, source_id)
    }

    /// Returns (and lazily generates) a local HTML preview path for Office / EPUB / CSV / RTF sources.
    pub fn ensure_preview_html(&self, source_id: &str) -> LnaResult<Option<PathBuf>> {
        let src = lna_storage::get_source(&self.db, source_id)?;
        let Some(bp) = src.blob_path.as_deref() else {
            return Ok(None);
        };
        let path = Path::new(bp);
        if !path.exists() {
            return Ok(None);
        }
        ai_indexer::ensure_preview_html(&src, path)
    }

    /// B6/S3: Remove orphan blob directories whose source_id no longer exists in DB.
    pub fn cleanup_orphan_files(db: &Database) -> LnaResult<usize> {
        let mut removed = 0usize;
        let blobs_root = &db.paths().blobs_dir;
        if !blobs_root.exists() {
            return Ok(0);
        }
        if let Ok(nb_dirs) = std::fs::read_dir(blobs_root) {
            for nb_entry in nb_dirs.flatten() {
                if let Ok(src_dirs) = std::fs::read_dir(nb_entry.path()) {
                    for src_entry in src_dirs.flatten() {
                        let sid = src_entry.file_name().to_string_lossy().to_string();
                        let exists: bool = db
                            .read(|conn| {
                                conn.query_row(
                                    "SELECT 1 FROM sources WHERE id = ?1",
                                    [&sid],
                                    |_| Ok(true),
                                )
                                .optional()
                                .map_err(map_sql_err)
                            })
                            .unwrap_or(Some(true))
                            .unwrap_or(false);
                        if !exists && std::fs::remove_dir_all(src_entry.path()).is_ok() {
                            removed += 1;
                        }
                    }
                }
            }
        }
        Ok(removed)
    }

    fn fail_ingest(&self, source_id: &str, e: LnaError) -> LnaError {
        let _ = self.update_status(
            source_id,
            "FAILED",
            Some(e.code.as_str()),
            Some(&e.message),
        );
        e
    }

    fn update_status(
        &self,
        source_id: &str,
        status: &str,
        err_code: Option<&str>,
        err_detail: Option<&str>,
    ) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE sources SET status=?1, error_code=?2, error_detail=?3, updated_at=?4 WHERE id=?5",
                params![status, err_code, err_detail, now_ms(), source_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }
}
