use jieba_rs::Jieba;
use lna_embedding::{cosine_similarity, EmbeddingService};
use lna_storage::{db::map_sql_err, Database};
use lna_types::LnaResult;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

static JIEBA: OnceLock<Jieba> = OnceLock::new();

pub fn jieba() -> &'static Jieba {
    JIEBA.get_or_init(Jieba::new)
}

/// Normalize (NFKC + lowercase) and tokenize with jieba search mode for FTS5.
pub fn tokenize_for_fts(text: &str) -> String {
    let norm: String = text.nfkc().collect::<String>().to_lowercase();
    jieba()
        .cut_for_search(&norm, false)
        .into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build safe FTS5 MATCH query: each token quoted and joined with OR.
const STOP_WORDS: &[&str] = &[
    "的",
    "了",
    "是",
    "在",
    "和",
    "与",
    "或",
    "什么",
    "如何",
    "为什么",
    "吗",
    "呢",
    "the",
    "is",
    "are",
    "of",
    "and",
    "or",
    "to",
    "in",
    "a",
    "an",
];

pub fn build_fts_query(query: &str) -> String {
    let norm: String = query.nfkc().collect::<String>().to_lowercase();
    let tokens: Vec<String> = jieba()
        .cut_for_search(&norm, false)
        .into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !STOP_WORDS.contains(s))
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    tokens.join(" OR ")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedEvidence {
    pub citation_no: i32,
    pub chunk_id: i64,
    pub source_id: String,
    pub source_title: String,
    pub chunk_index: i32,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub section_path: Option<String>,
    pub content: String,
    pub token_count: i32,
    pub rrf_score: f64,
}

#[derive(Debug, Clone)]
pub struct RetrievalOutcome {
    pub evidences: Vec<RetrievedEvidence>,
    pub grounded: bool,
    pub best_cosine: f32,
    pub bm25_hits: usize,
    pub vector_fallback_reason: Option<String>,
}

pub struct RetrievalEngine {
    db: Database,
    embedding: EmbeddingService,
}

impl RetrievalEngine {
    pub fn new(db: Database, embedding: EmbeddingService) -> Self {
        Self { db, embedding }
    }

    pub async fn search(
        &self,
        notebook_id: &str,
        queries: &[String],
        selected_source_ids: &[String],
        token_budget: usize,
        max_groups: usize,
    ) -> LnaResult<RetrievalOutcome> {
        if selected_source_ids.is_empty() || queries.is_empty() {
            return Ok(RetrievalOutcome {
                evidences: Vec::new(),
                grounded: false,
                best_cosine: 0.0,
                bm25_hits: 0,
                vector_fallback_reason: None,
            });
        }

        let selected_json =
            serde_json::to_string(selected_source_ids).unwrap_or_else(|_| "[]".into());
        let mut rrf_scores: HashMap<i64, f64> = HashMap::new();
        let mut total_bm25_hits = 0usize;

        // 1. Lexical recall (BM25 Top-20 per query)
        for q in queries {
            let fts_q = build_fts_query(q);
            if fts_q.is_empty() {
                continue;
            }
            let hits: Vec<i64> = self.db.read(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT c.id
                         FROM chunks_fts
                         JOIN source_chunks c ON c.id = chunks_fts.rowid
                         WHERE chunks_fts MATCH ?1
                           AND c.source_id IN (SELECT value FROM json_each(?2))
                         ORDER BY bm25(chunks_fts)
                         LIMIT 20",
                    )
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map(params![fts_q, selected_json], |r| r.get::<_, i64>(0))
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(map_sql_err)?);
                }
                Ok(out)
            })?;
            total_bm25_hits += hits.len();
            for (rank, cid) in hits.into_iter().enumerate() {
                *rrf_scores.entry(cid).or_insert(0.0) += 1.0 / (60.0 + (rank as f64) + 1.0);
            }
        }

        // 2. Semantic recall (Top-20 per query via vec0 or exact cosine fallback)
        let mut best_cosine = 0.0f32;
        let mut vector_err: Option<String> = None;
        if let Ok(Some(model)) = self.embedding.active_model() {
            let total_nb_sources: i64 = self
                .db
                .read(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM sources WHERE notebook_id = ?1",
                        [notebook_id],
                        |r| r.get(0),
                    )
                    .map_err(map_sql_err)
                })
                .unwrap_or(1)
                .max(1);
            let k = ((20 * total_nb_sources) / (selected_source_ids.len() as i64).max(1))
                .clamp(100, 1000);
            let selected_set: HashSet<String> = selected_source_ids.iter().cloned().collect();

            for q in queries {
                if let Ok(qvec) = self.embedding.embed_query(q).await {
                    let qbytes: Vec<u8> = qvec.iter().flat_map(|f| f.to_le_bytes()).collect();
                    let tbl = &model.vec_table;
                    let vec_hits: Vec<(i64, String, f32)> = self
                        .db
                        .read(|conn| {
                            let sql = format!(
                                "SELECT v.chunk_id, c.source_id, v.distance
                                 FROM {tbl} v
                                 JOIN source_chunks c ON c.id = v.chunk_id
                                 WHERE v.notebook_id = ?1 AND v.embedding MATCH ?2 AND k = ?3
                                 ORDER BY v.distance"
                            );
                            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
                            let rows = stmt
                                .query_map(params![notebook_id, qbytes, k], |r| {
                                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f32>(2)?))
                                })
                                .map_err(map_sql_err)?;
                            let mut out = Vec::new();
                            for r in rows {
                                out.push(r.map_err(map_sql_err)?);
                            }
                            Ok(out)
                        })
                        .unwrap_or_else(|e| {
                            tracing::warn!(error = %e.message, "Vector query failed; degrading to BM25");
                            vector_err = Some(e.message);
                            Vec::new()
                        });

                    let mut filtered: Vec<(i64, f32)> = vec_hits
                        .into_iter()
                        .filter(|(_, sid, _)| selected_set.contains(sid))
                        .map(|(cid, _, dist)| (cid, 1.0 - dist))
                        .take(20)
                        .collect();

                    // Exact cosine fallback when < 20 hits and <= 5000 selected chunks
                    if filtered.len() < 20 {
                        let exact_hits = self
                            .exact_cosine_fallback(&model.vec_table, &qvec, &selected_json)
                            .unwrap_or_else(|e| {
                            tracing::warn!(error = %e.message, "Vector query failed; degrading to BM25");
                            vector_err = Some(e.message);
                            Vec::new()
                        });
                        if !exact_hits.is_empty() {
                            filtered = exact_hits;
                        }
                    }

                    for (rank, (cid, sim)) in filtered.into_iter().enumerate() {
                        if sim > best_cosine {
                            best_cosine = sim;
                        }
                        *rrf_scores.entry(cid).or_insert(0.0) += 1.0 / (60.0 + (rank as f64) + 1.0);
                    }
                }
            }
        }

        // Section 8.5: Groundedness check
        let grounded = total_bm25_hits > 0 || best_cosine >= 0.35;
        if !grounded {
            return Ok(RetrievalOutcome {
                evidences: Vec::new(),
                grounded: false,
                best_cosine,
                bm25_hits: total_bm25_hits,
                vector_fallback_reason: vector_err,
            });
        }

        // Sort candidates by RRF score descending
        let mut ranked: Vec<(i64, f64)> = rrf_scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Fetch chunk details
        let mut candidates = Vec::new();
        for (cid, score) in ranked.into_iter().take(40) {
            if let Ok(Some(ev)) = self.fetch_chunk_detail(cid, score) {
                candidates.push(ev);
            }
        }

        // 4. Diversity: max 60% from a single source (unless only 1 source selected)
        let max_per_source = if selected_source_ids.len() <= 1 {
            max_groups
        } else {
            ((max_groups as f64) * 0.6).ceil().max(1.0) as usize
        };

        let mut source_counts: HashMap<String, usize> = HashMap::new();
        let mut selected = Vec::new();
        let mut used_tokens = 0usize;

        for mut ev in candidates {
            if selected.len() >= max_groups {
                break;
            }
            let cnt = source_counts.get(&ev.source_id).copied().unwrap_or(0);
            if cnt >= max_per_source {
                continue;
            }
            let t = (ev.token_count as usize).max(1);
            if !selected.is_empty() && used_tokens + t > token_budget {
                continue;
            }
            used_tokens += t;
            *source_counts.entry(ev.source_id.clone()).or_insert(0) += 1;
            ev.citation_no = (selected.len() + 1) as i32;
            selected.push(ev);
        }

        Ok(RetrievalOutcome {
            evidences: selected,
            grounded: true,
            best_cosine,
            bm25_hits: total_bm25_hits,
            vector_fallback_reason: vector_err,
        })
    }

    fn exact_cosine_fallback(
        &self,
        vec_table: &str,
        qvec: &[f32],
        selected_json: &str,
    ) -> LnaResult<Vec<(i64, f32)>> {
        self.db.read(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM source_chunks WHERE source_id IN (SELECT value FROM json_each(?1))",
                    [selected_json],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)?;
            if count > 5000 {
                return Ok(Vec::new());
            }
            let sql = format!(
                "SELECT v.chunk_id, v.embedding
                 FROM {vec_table} v
                 JOIN source_chunks c ON c.id = v.chunk_id
                 WHERE c.source_id IN (SELECT value FROM json_each(?1))"
            );
            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
            let rows = stmt
                .query_map([selected_json], |r| {
                    let cid: i64 = r.get(0)?;
                    let bytes: Vec<u8> = r.get(1)?;
                    Ok((cid, bytes))
                })
                .map_err(map_sql_err)?;
            let mut scored = Vec::new();
            for row in rows {
                let (cid, bytes) = row.map_err(map_sql_err)?;
                let vec: Vec<f32> = bytes
                    .as_chunks::<4>().0.iter()
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                scored.push((cid, cosine_similarity(qvec, &vec)));
            }
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(20);
            Ok(scored)
        })
    }

    fn fetch_chunk_detail(
        &self,
        chunk_id: i64,
        rrf_score: f64,
    ) -> LnaResult<Option<RetrievedEvidence>> {
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.source_id, s.title, c.chunk_index, c.page_start, c.page_end,
                            c.section_path, c.content, c.token_count
                     FROM source_chunks c
                     JOIN sources s ON s.id = c.source_id
                     WHERE c.id = ?1",
                )
                .map_err(map_sql_err)?;
            let mut rows = stmt
                .query_map([chunk_id], |r| {
                    Ok(RetrievedEvidence {
                        citation_no: 0,
                        chunk_id: r.get(0)?,
                        source_id: r.get(1)?,
                        source_title: r.get(2)?,
                        chunk_index: r.get(3)?,
                        page_start: r.get(4)?,
                        page_end: r.get(5)?,
                        section_path: r.get(6)?,
                        content: r.get(7)?,
                        token_count: r.get(8)?,
                        rrf_score,
                    })
                })
                .map_err(map_sql_err)?;
            if let Some(res) = rows.next() {
                Ok(Some(res.map_err(map_sql_err)?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn read_adjacent_chunks(
        &self,
        source_id: &str,
        chunk_index: i32,
        direction: &str,
        count: i32,
    ) -> LnaResult<Vec<(i64, i32, String)>> {
        let count = count.clamp(1, 3);
        let (min_idx, max_idx) = match direction {
            "prev" | "before" => (chunk_index - count, chunk_index - 1),
            "next" | "after" => (chunk_index + 1, chunk_index + count),
            _ => (chunk_index - count, chunk_index + count),
        };
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, chunk_index, content FROM source_chunks
                     WHERE source_id = ?1 AND chunk_index BETWEEN ?2 AND ?3
                     ORDER BY chunk_index ASC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map(params![source_id, min_idx, max_idx], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })
    }
}
