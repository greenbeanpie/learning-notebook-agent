pub mod chunker;
pub mod parser;
pub mod ssrf;

use lna_embedding::EmbeddingService;
use lna_jobs::JobQueue;
use lna_retrieval::tokenize_for_fts;
use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, now_ms, ErrorCode, LnaError, LnaResult, Source, SourceKind};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAX_SOURCES_PER_NOTEBOOK: i64 = 300;
const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct IngestService {
    db: Database,
    jobs: JobQueue,
    embedding: EmbeddingService,
}

impl IngestService {
    pub fn new(db: Database, jobs: JobQueue, embedding: EmbeddingService) -> Self {
        Self {
            db,
            jobs,
            embedding,
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
        let kind = match ext.as_str() {
            "pdf" => SourceKind::Pdf,
            "md" | "markdown" => SourceKind::Md,
            "txt" => SourceKind::Txt,
            _ => {
                return Err(LnaError::new(
                    ErrorCode::SourceUnsupported,
                    format!("Unsupported file extension: .{ext}"),
                ))
            }
        };

        let bytes = std::fs::read(file_path).map_err(|e| LnaError::internal(e.to_string()))?;
        let title = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();

        self.enqueue_bytes(notebook_id, &title, kind, &ext, None, &bytes)
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
        let kind_str = match kind {
            SourceKind::Pdf => "pdf",
            SourceKind::Md => "md",
            SourceKind::Txt => "txt",
            SourceKind::Web => "web",
            SourceKind::Note => "note",
        };
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
        let kind = src.kind;
        let parsed = tokio::task::spawn_blocking(move || match kind {
            SourceKind::Pdf => parser::parse_pdf(&bp),
            SourceKind::Md | SourceKind::Note => {
                let bytes = std::fs::read(&bp).map_err(|e| LnaError::internal(e.to_string()))?;
                Ok(parser::parse_markdown(&parser::decode_text_bytes(&bytes)))
            }
            SourceKind::Txt => {
                let bytes = std::fs::read(&bp).map_err(|e| LnaError::internal(e.to_string()))?;
                Ok(parser::parse_plain_text(&parser::decode_text_bytes(&bytes)))
            }
            SourceKind::Web => {
                let bytes = std::fs::read(&bp).map_err(|e| LnaError::internal(e.to_string()))?;
                Ok(parser::parse_html(&parser::decode_text_bytes(&bytes)))
            }
        })
        .await
        .map_err(|e| {
            let msg = format!("Parser worker panicked or was cancelled: {e}");
            let _ = self.update_status(source_id, "FAILED", Some("INTERNAL"), Some(&msg));
            LnaError::internal(msg)
        })?;

        let doc = match parsed {
            Ok(d) => d,
            Err(e) => {
                let _ = self.update_status(
                    source_id,
                    "FAILED",
                    Some(e.code.as_str()),
                    Some(&e.message),
                );
                return Err(e);
            }
        };

        self.update_status(source_id, "CHUNKING", None, None)?;
        let drafts = chunker::chunk_document(&doc);
        if drafts.is_empty() {
            let e = LnaError::new(
                ErrorCode::SourceUnsupported,
                "No extractable text in this source",
            );
            let _ =
                self.update_status(source_id, "FAILED", Some(e.code.as_str()), Some(&e.message));
            return Err(e);
        }
        let total_tokens: i64 = drafts.iter().map(|c| c.token_count as i64).sum();

        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute("DELETE FROM source_chunks WHERE source_id = ?1", [source_id])
                .map_err(map_sql_err)?;
            for c in &drafts {
                tx.execute(
                    "INSERT INTO source_chunks
                     (source_id, notebook_id, chunk_index, section_path, page_start, page_end, char_start, char_end, content, token_count)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
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
                        c.token_count
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
