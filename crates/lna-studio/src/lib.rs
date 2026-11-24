pub mod mdlite;
pub mod render;
pub mod texfmt;
pub mod validate;

use lna_jobs::JobQueue;
use lna_llm::*;
use lna_retrieval::RetrievalEngine;
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_tts::{concat_wavs, SystemTtsProvider, TtsProvider, Voice};
use lna_types::*;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const SCHEMA_QUIZ: &str = include_str!("../../../schemas/quiz.schema.json");
const SCHEMA_FLASHCARDS: &str = include_str!("../../../schemas/flashcards.schema.json");
const SCHEMA_MINDMAP: &str = include_str!("../../../schemas/mindmap.schema.json");
const SCHEMA_REPORT: &str = include_str!("../../../schemas/report.schema.json");
const SCHEMA_SLIDES: &str = include_str!("../../../schemas/slides.schema.json");
const SCHEMA_INFOGRAPHIC: &str = include_str!("../../../schemas/infographic.schema.json");
const SCHEMA_AUDIO: &str = include_str!("../../../schemas/audio.schema.json");

const MAX_STUDIO_RETRIES: u32 = 3;
const QUIZ_BATCH_SIZE: usize = 5;
const FLASHCARD_BATCH_SIZE: usize = 8;

const ARTIFACT_SELECT_COLS: &str =
    "id, notebook_id, artifact_type, title, status, stage, progress, config_json, \
     source_ids_json, sources_fingerprint, profile_id, draft_json, payload_json, output_dir, \
     estimated_input_tokens, estimated_output_tokens, error_code, error_detail, feedback, \
     created_at, updated_at, finished_at, in_studio, origin_message_id, error_log";

struct AttemptFailureRecord {
    attempt: u32,
    stage: String,
    elapsed_ms: u128,
    code: String,
    retryable: bool,
    message: String,
}

/// (source id, title, content hash, source language, chunk index, chunk id, content, section path)
type ChunkRow = (String, String, String, String, i32, i64, String, String);
/// source id -> (title, content hash, language)
type SourceMeta = HashMap<String, (String, String, String)>;

#[derive(Clone)]
pub struct StudioService {
    db: Database,
    secrets: Arc<dyn SecretStore>,
    jobs: JobQueue,
    llm: LlmClient,
    retrieval: Arc<RetrievalEngine>,
    tts: Arc<dyn TtsProvider>,
    events: broadcast::Sender<ApiEvent>,
    seq: Arc<std::sync::atomic::AtomicU64>,
    text_generation_slots: Arc<Semaphore>,
    profile_generation_slots: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    audio_render_slot: Arc<Semaphore>,
}

impl StudioService {
    pub fn new(
        db: Database,
        secrets: Arc<dyn SecretStore>,
        jobs: JobQueue,
        llm: LlmClient,
        retrieval: Arc<RetrievalEngine>,
        events: broadcast::Sender<ApiEvent>,
        seq: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            db,
            secrets,
            jobs,
            llm,
            retrieval,
            tts: Arc::new(SystemTtsProvider),
            events,
            seq,
            text_generation_slots: Arc::new(Semaphore::new(6)),
            profile_generation_slots: Arc::new(Mutex::new(HashMap::new())),
            audio_render_slot: Arc::new(Semaphore::new(1)),
        }
    }

    fn emit(&self, topic: &str, event: &str, data: Value) {
        let s = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.events.send(ApiEvent {
            seq: s,
            topic: topic.to_string(),
            event: event.to_string(),
            data,
        });
    }

    pub fn estimate(&self, notebook_id: &str, artifact_type: ArtifactType) -> LnaResult<Value> {
        self.estimate_with_config(notebook_id, artifact_type, &json!({}))
    }

    pub fn estimate_with_config(
        &self,
        notebook_id: &str,
        artifact_type: ArtifactType,
        config: &Value,
    ) -> LnaResult<Value> {
        let raw_source_tokens: i64 = self.db.read(|conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(token_count),0) FROM sources WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY'",
                [notebook_id],
                |r| r.get(0),
            )
            .map_err(map_sql_err)
        })?;
        let detail = config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("standard");
        let on_demand_cap: i64 = match artifact_type {
            ArtifactType::Mindmap => match detail {
                "concise" => 8_000,
                "detailed" => 24_000,
                _ => 16_000,
            },
            ArtifactType::Quiz | ArtifactType::Flashcards | ArtifactType::Infographic => 12_000,
            ArtifactType::Report | ArtifactType::Slides | ArtifactType::Audio => 16_000,
        };
        // All Studio artifacts use on-demand reading (compact structural catalog + targeted section reads)
        // for large sources instead of passing an entire book at once.
        let total_source_tokens: i64 = if raw_source_tokens <= 12_000 {
            raw_source_tokens
        } else {
            let index_chars: i64 = self.db.read(|conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(LENGTH(title) + LENGTH(summary) + LENGTH(keywords_json)), 0)
                     FROM source_index_nodes
                     WHERE source_id IN (SELECT id FROM sources WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY')",
                    [notebook_id],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)
            })?;
            let catalog_tokens = ((index_chars / 3).max(1_600)).min(5_500);
            (catalog_tokens + on_demand_cap * 6 / 10).min(on_demand_cap).min(raw_source_tokens)
        };
        let active_profile = lna_storage::get_active_profile(&self.db)?;
        let est_input =
            (((total_source_tokens.max(0) as f64) * 1.1).ceil() as i64).saturating_add(600);
        let est_output = match artifact_type {
            ArtifactType::Quiz | ArtifactType::Flashcards => 2500,
            ArtifactType::Report | ArtifactType::Slides | ArtifactType::Audio => 4500,
            ArtifactType::Mindmap => match detail {
                "concise" => 2000,
                "detailed" => 6000,
                _ => 3800,
            },
            ArtifactType::Infographic => 1800,
        };
        let estimated_cost_usd = active_profile.as_ref().and_then(|profile| {
            match (profile.input_price_per_m, profile.output_price_per_m) {
                (Some(input_price), Some(output_price)) => Some(
                    (est_input as f64 * input_price + est_output as f64 * output_price)
                        / 1_000_000.0,
                ),
                _ => None,
            }
        });
        Ok(json!({
            "estimated_input_tokens": est_input,
            "estimated_output_tokens": est_output,
            "estimated_cost_usd": estimated_cost_usd,
            "input_price_per_m": active_profile.as_ref().and_then(|p| p.input_price_per_m),
            "output_price_per_m": active_profile.as_ref().and_then(|p| p.output_price_per_m),
            "is_estimated": true,
            "requires_confirmation": false
        }))
    }

    pub fn create(
        &self,
        notebook_id: &str,
        artifact_type: ArtifactType,
        title: Option<&str>,
        config: Value,
    ) -> LnaResult<StudioArtifact> {
        let is_kg = artifact_type == ArtifactType::Mindmap
            && config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let explicit_source_id = config
            .get("source_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let (mut selected_ids, mut fp) = self.current_sources_fingerprint(notebook_id)?;
        if is_kg {
            if let Some(sid) = explicit_source_id.as_ref() {
                selected_ids = vec![sid.clone()];
                fp = format!("source:{}", sid);
            }
        }
        if selected_ids.is_empty() {
            return Err(LnaError::new(
                ErrorCode::NoSourceSelected,
                "Please select at least one ready source",
            ));
        }
        let active_profile = lna_storage::get_active_profile(&self.db)?.ok_or_else(|| {
            LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured")
        })?;

        let est = self.estimate_with_config(notebook_id, artifact_type, &config)?;
        let est_in = est.get("estimated_input_tokens").and_then(|x| x.as_i64());
        let est_out = est.get("estimated_output_tokens").and_then(|x| x.as_i64());

        let id = new_ulid();
        let now = now_ms();
        let ui_lang = config
            .get("lang")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                self.db
                    .read(|conn| {
                        conn.query_row(
                            "SELECT value_json FROM app_settings WHERE key = 'ui.preferences'",
                            [],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(map_sql_err)
                    })
                    .ok()
                    .flatten()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v.get("language").and_then(Value::as_str).map(String::from))
            })
            .unwrap_or_default();
        let use_en = ui_lang.to_lowercase().starts_with("en");
        let default_title = if use_en {
            match artifact_type {
                ArtifactType::Audio => "Audio Overview",
                ArtifactType::Slides => "Slides",
                ArtifactType::Mindmap if is_kg => "Knowledge Graph",
                ArtifactType::Mindmap => "Mind Map",
                ArtifactType::Report => "Report",
                ArtifactType::Flashcards => "Flashcards",
                ArtifactType::Quiz => "Quiz",
                ArtifactType::Infographic => "Infographic",
            }
        } else {
            match artifact_type {
                ArtifactType::Audio => "音频概览",
                ArtifactType::Slides => "演示文稿",
                ArtifactType::Mindmap if is_kg => "知识点图谱",
                ArtifactType::Mindmap => "思维导图",
                ArtifactType::Report => "研究报告",
                ArtifactType::Flashcards => "学习闪卡",
                ArtifactType::Quiz => "知识测验",
                ArtifactType::Infographic => "信息图",
            }
        };
        let title = title.unwrap_or(default_title);
        let cfg_json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".into());
        let sids_json = serde_json::to_string(&selected_ids).unwrap_or_else(|_| "[]".into());

        let in_studio: i64 = if is_kg { 0 } else { 1 };
        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO studio_artifacts
                 (id, notebook_id, artifact_type, title, status, stage, progress, config_json,
                  source_ids_json, sources_fingerprint, profile_id, estimated_input_tokens,
                  estimated_output_tokens, in_studio, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,'PENDING','QUEUED',0.0,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    id,
                    notebook_id,
                    artifact_type.as_str(),
                    title,
                    cfg_json,
                    sids_json,
                    fp,
                    active_profile.id,
                    est_in,
                    est_out,
                    in_studio,
                    now,
                    now
                ],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;

        self.jobs
            .enqueue("studio_generate", Some(notebook_id), &id, 100)?;
        self.get(&id)
    }

    /// Create a chat-attached artifact (`in_studio = 0`) and enqueue it asynchronously in the
    /// background studio job queue so the chat agent does not block waiting for completion.
    pub fn create_for_chat(
        &self,
        notebook_id: &str,
        message_id: &str,
        artifact_type: ArtifactType,
        title: Option<&str>,
        mut config: Value,
    ) -> LnaResult<StudioArtifact> {
        let (selected_ids, fp) = self.current_sources_fingerprint(notebook_id)?;
        if selected_ids.is_empty() {
            return Err(LnaError::new(
                ErrorCode::NoSourceSelected,
                "Please select at least one ready source",
            ));
        }
        let active_profile = lna_storage::get_active_profile(&self.db)?.ok_or_else(|| {
            LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured")
        })?;

        if !config.is_object() {
            config = json!({});
        }
        if let Some(obj) = config.as_object_mut() {
            obj.insert("confirmed".into(), json!(true));
            if artifact_type == ArtifactType::Slides && !obj.contains_key("reviewOutline") {
                obj.insert("reviewOutline".into(), json!(false));
            }
        }

        let est = self.estimate(notebook_id, artifact_type)?;
        let est_in = est.get("estimated_input_tokens").and_then(|x| x.as_i64());
        let est_out = est.get("estimated_output_tokens").and_then(|x| x.as_i64());

        let id = new_ulid();
        let now = now_ms();
        let default_title = match artifact_type {
            ArtifactType::Audio => "音频概览",
            ArtifactType::Slides => "演示文稿",
            ArtifactType::Mindmap => "思维导图",
            ArtifactType::Report => "研究报告",
            ArtifactType::Flashcards => "学习闪卡",
            ArtifactType::Quiz => "知识测验",
            ArtifactType::Infographic => "信息图",
        };
        let title = title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_title);
        let cfg_json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".into());
        let sids_json = serde_json::to_string(&selected_ids).unwrap_or_else(|_| "[]".into());

        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO studio_artifacts
                 (id, notebook_id, artifact_type, title, status, stage, progress, config_json,
                  source_ids_json, sources_fingerprint, profile_id, estimated_input_tokens,
                  estimated_output_tokens, in_studio, origin_message_id, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,'PENDING','QUEUED',0.0,?5,?6,?7,?8,?9,?10,0,?11,?12,?13)",
                params![
                    id,
                    notebook_id,
                    artifact_type.as_str(),
                    title,
                    cfg_json,
                    sids_json,
                    fp,
                    active_profile.id,
                    est_in,
                    est_out,
                    message_id,
                    now,
                    now
                ],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;

        self.jobs
            .enqueue("studio_generate", Some(notebook_id), &id, 100)?;
        self.get(&id)
    }

    pub fn list(&self, notebook_id: &str) -> LnaResult<Vec<StudioArtifact>> {
        let (_, cur_fp) = self.current_sources_fingerprint(notebook_id)?;
        let sql = format!(
            "SELECT {ARTIFACT_SELECT_COLS}
             FROM studio_artifacts
             WHERE notebook_id = ?1
               AND in_studio = 1
               AND COALESCE(json_extract(config_json, '$.mode'), '') != 'knowledge_graph'
             ORDER BY created_at DESC"
        );
        let mut out = self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
            let rows = stmt
                .query_map([notebook_id], |r| map_artifact_row(r, &cur_fp))
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        for art in &mut out {
            art.source_refs = self.load_source_refs(&art.source_ids)?;
        }
        Ok(out)
    }

    pub fn list_by_message(&self, message_id: &str) -> LnaResult<Vec<StudioArtifact>> {
        let sql = format!(
            "SELECT {ARTIFACT_SELECT_COLS}
             FROM studio_artifacts WHERE origin_message_id = ?1 ORDER BY created_at ASC"
        );
        let mut out = self.db.read(|conn| {
            let mut stmt = conn.prepare(&sql).map_err(map_sql_err)?;
            let rows = stmt
                .query_map([message_id], |r| map_artifact_row(r, ""))
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        for art in &mut out {
            art.source_refs = self.load_source_refs(&art.source_ids)?;
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> LnaResult<StudioArtifact> {
        let nb_id: String = self.db.read(|conn| {
            conn.query_row(
                "SELECT notebook_id FROM studio_artifacts WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)?
            .ok_or_else(|| LnaError::not_found(format!("Artifact not found: {id}")))
        })?;
        let (_, cur_fp) = self.current_sources_fingerprint(&nb_id)?;
        let sql = format!("SELECT {ARTIFACT_SELECT_COLS} FROM studio_artifacts WHERE id = ?1");
        let mut art = self.db.read(|conn| {
            conn.query_row(&sql, [id], |r| map_artifact_row(r, &cur_fp))
                .map_err(map_sql_err)
        })?;
        art.thoughts = self.list_thoughts(id)?;
        art.source_refs = self.load_source_refs(&art.source_ids)?;
        art.progress_state = self.load_progress_state(id)?;
        Ok(art)
    }

    fn load_progress_state(&self, artifact_id: &str) -> LnaResult<Option<Value>> {
        self.db.read(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT state_json FROM artifact_progress WHERE artifact_id = ?1",
                    [artifact_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sql_err)?;
            Ok(raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()))
        })
    }

    fn load_source_refs(&self, source_ids: &[String]) -> LnaResult<Vec<ArtifactSourceRef>> {
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.db.read(|conn| {
            let mut out = Vec::with_capacity(source_ids.len());
            let mut stmt = conn
                .prepare("SELECT id, title, kind FROM sources WHERE id = ?1")
                .map_err(map_sql_err)?;
            for sid in source_ids {
                if let Some(row) = stmt
                    .query_row([sid], |r| {
                        Ok(ArtifactSourceRef {
                            id: r.get(0)?,
                            title: r.get(1)?,
                            kind: r.get(2)?,
                        })
                    })
                    .optional()
                    .map_err(map_sql_err)?
                {
                    out.push(row);
                }
            }
            Ok(out)
        })
    }

    pub fn list_thoughts(&self, artifact_id: &str) -> LnaResult<Vec<ArtifactThought>> {
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT seq, kind, title, body, created_at
                     FROM artifact_thoughts WHERE artifact_id = ?1 ORDER BY seq ASC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([artifact_id], |r| {
                    Ok(ArtifactThought {
                        seq: r.get(0)?,
                        kind: r.get(1)?,
                        title: r.get(2)?,
                        body: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })
    }

    pub fn import_to_studio(&self, id: &str) -> LnaResult<StudioArtifact> {
        let _ = self.get(id)?;
        let now = now_ms();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET in_studio = 1, updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        let art = self.get(id)?;
        self.emit(
            &format!("studio:{}", art.notebook_id),
            "studio.status",
            json!({
                "artifact_id": id,
                "status": art.status,
                "in_studio": true,
                "origin_message_id": art.origin_message_id
            }),
        );
        if let Some(ref mid) = art.origin_message_id {
            let mut compact = art.clone();
            compact.thoughts.clear();
            self.emit(
                &format!("chat:{}", art.notebook_id),
                "chat.artifact",
                json!({ "message_id": mid, "artifact": compact }),
            );
        }
        Ok(art)
    }

    pub fn delete_unimported_for_messages(&self, message_ids: &[String]) -> LnaResult<()> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let mut dirs_to_remove: Vec<String> = Vec::new();
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            for mid in message_ids {
                let mut stmt = tx
                    .prepare(
                        "SELECT id, output_dir FROM studio_artifacts
                         WHERE origin_message_id = ?1 AND in_studio = 0",
                    )
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([mid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
                    .map_err(map_sql_err)?;
                let mut ids = Vec::new();
                for r in rows {
                    let (aid, dir) = r.map_err(map_sql_err)?;
                    ids.push(aid);
                    if let Some(d) = dir {
                        dirs_to_remove.push(d);
                    }
                }
                drop(stmt);
                for aid in ids {
                    tx.execute("DELETE FROM studio_artifacts WHERE id = ?1", [aid])
                        .map_err(map_sql_err)?;
                }
            }
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })?;
        for dir in dirs_to_remove {
            let p = std::path::Path::new(&dir);
            if p.exists() {
                let _ = std::fs::remove_dir_all(p);
            }
        }
        Ok(())
    }

    pub fn approve_draft(
        &self,
        id: &str,
        edited_draft: Option<Value>,
    ) -> LnaResult<StudioArtifact> {
        let art = self.get(id)?;
        if art.status != ArtifactStatus::DraftReview {
            return Err(LnaError::new(
                ErrorCode::Conflict,
                "Artifact is not in DRAFT_REVIEW state",
            ));
        }
        let draft_str = edited_draft.map(|v| v.to_string());
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='PENDING', stage='GENERATE',
                 draft_json=COALESCE(?1, draft_json), updated_at=?2 WHERE id=?3",
                params![draft_str, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.append_thought(
            id,
            &art.notebook_id,
            "stage",
            "大纲已确认",
            "已提交审核通过的大纲，正在加入后台生成队列…",
        );
        self.jobs
            .enqueue("studio_generate", Some(&art.notebook_id), id, 90)?;
        self.emit_status_event(id, "PENDING", "GENERATE", art.progress, None, None);
        self.get(id)
    }

    pub async fn revise_draft(
        &self,
        id: &str,
        instruction: &str,
        current_draft: Option<&str>,
    ) -> LnaResult<StudioArtifact> {
        let art = self.get(id)?;
        if art.status != ArtifactStatus::DraftReview {
            return Err(LnaError::new(
                ErrorCode::Conflict,
                "Artifact is not in DRAFT_REVIEW state",
            ));
        }
        let instr = instruction.trim();
        if instr.is_empty() {
            return Err(LnaError::invalid("Revision instruction cannot be empty"));
        }
        let profile = lna_storage::get_active_profile(&self.db)?
            .ok_or_else(|| LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured"))?;
        let api_key = match profile.keychain_ref.as_deref() {
            Some(kref) => self.secrets.get(kref)?,
            None => None,
        };
        let resolved = ResolvedProfile {
            id: profile.id.clone(),
            display_name: profile.display_name.clone(),
            protocol: profile.protocol,
            base_url: profile.base_url.clone(),
            model_id: profile.model_id.clone(),
            api_key,
            show_thinking: profile.show_thinking,
            effort: profile.effort.clone(),
            context_window: profile.context_window,
            max_output_tokens: profile.max_output_tokens,
            capabilities: profile.capabilities.clone(),
            extra_headers: profile.extra_headers.clone(),
            anthropic_version: "2023-06-01".into(),
        };

        let existing_md = current_draft
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| match &art.draft {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            });

        self.append_thought(
            id,
            &art.notebook_id,
            "tool",
            "工具调用 · 按提示词迭代调整提纲",
            instr,
        );

        let is_report = art.artifact_type == ArtifactType::Report;
        let sys = if is_report {
            "You are a research report outline editor. Revise the provided Markdown report outline strictly according to the user's instruction while preserving factual grounding and clean Markdown structure (`# Title`, `## Section N: Heading`, `- Key points`). Output ONLY the updated Markdown outline with no JSON or code fences."
        } else {
            "You are a presentation outline editor. Revise the provided Markdown slide outline strictly according to the user's instruction while preserving factual grounding and clean Markdown structure (`# Title`, `## Slide N: Slide Title`, `- Key points`, `> Speaker notes`). Output ONLY the updated Markdown outline with no JSON or code fences."
        };
        let req = LlmRequest {
            system: sys.into(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(format!(
                    "Artifact type: {}
Config: {}

Current Outline:
{}

User Revision Instruction:
{}",
                    art.artifact_type.as_str(),
                    art.config,
                    existing_md,
                    instr
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: 2400,
            reasoning: ReasoningOptions {
                show_summary: profile.show_thinking,
                effort: profile.effort.clone(),
            },
        };

        let _permits = self.acquire_llm_slots(&profile.id).await?;
        let (revised_raw, usage) = self
            .run_text_with_thought(
                id,
                &art.notebook_id,
                "提纲迭代调整思考",
                &resolved,
                &req,
                CancellationToken::new(),
            )
            .await?;
        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&profile.id),
            "studio_revise_draft",
            Some(id),
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
        );

        let revised_md = revised_raw.trim().to_string();
        let draft_md_json =
            serde_json::to_string(&Value::String(revised_md)).unwrap_or_else(|_| String::from("\"\""));
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET draft_json=?1, updated_at=?2 WHERE id=?3 AND status='DRAFT_REVIEW'",
                params![draft_md_json, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit_status_event(id, "DRAFT_REVIEW", "DRAFT_REVIEW", 0.45, None, None);
        self.get(id)
    }

    pub fn save_progress(&self, id: &str, state: Value) -> LnaResult<()> {
        let s = state.to_string();
        self.db.write(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO artifact_progress (artifact_id, state_json, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![id, s, now_ms()],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn get_source_knowledge_graph(&self, source_id: &str) -> LnaResult<Option<StudioArtifact>> {
        let id: Option<String> = self.db.read(|conn| {
            conn.query_row(
                "SELECT id FROM studio_artifacts
                 WHERE artifact_type = 'mindmap'
                   AND COALESCE(json_extract(config_json, '$.mode'), '') = 'knowledge_graph'
                   AND (
                       COALESCE(json_extract(config_json, '$.source_id'), '') = ?1
                       OR EXISTS (SELECT 1 FROM json_each(source_ids_json) WHERE value = ?1)
                   )
                 ORDER BY CASE WHEN COALESCE(json_extract(config_json, '$.source_id'), '') = ?1 THEN 0 ELSE 1 END,
                          created_at DESC
                 LIMIT 1",
                [source_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        match id {
            Some(aid) => Ok(Some(self.get(&aid)?)),
            None => Ok(None),
        }
    }

    pub fn list_notebook_knowledge_graphs(&self, notebook_id: &str) -> LnaResult<Vec<StudioArtifact>> {
        let ids: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM studio_artifacts
                     WHERE notebook_id = ?1
                       AND artifact_type = 'mindmap'
                       AND COALESCE(json_extract(config_json, '$.mode'), '') = 'knowledge_graph'
                     ORDER BY created_at DESC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt.query_map([notebook_id], |r| r.get::<_, String>(0)).map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(art) = self.get(&id) {
                out.push(art);
            }
        }
        Ok(out)
    }

    pub fn compute_kg_progress_summary(&self, notebook_id: &str, source_id: Option<&str>) -> LnaResult<Value> {
        let graphs = if let Some(sid) = source_id.filter(|s| !s.trim().is_empty()) {
            self.get_source_knowledge_graph(sid)?.into_iter().collect()
        } else {
            self.list_notebook_knowledge_graphs(notebook_id)?
        };
        let ready_graphs: Vec<StudioArtifact> = graphs
            .into_iter()
            .filter(|g| g.status == ArtifactStatus::Ready && g.payload.is_some())
            .collect();
        if ready_graphs.is_empty() {
            return Ok(json!({
                "has_knowledge_graph": false,
                "graph_count": 0,
                "total_concepts": 0,
                "mastered": 0,
                "learning": 0,
                "weak": 0,
                "unstarted": 0,
                "mastery_percent": 0,
                "recommended_difficulty": "mixed",
                "weak_concepts": [],
                "unstarted_concepts": [],
                "mastered_concepts": [],
            }));
        }

        let mut total = 0usize;
        let mut mastered = 0usize;
        let mut learning = 0usize;
        let mut weak = 0usize;
        let mut unstarted = 0usize;
        let mut weak_concepts: Vec<String> = Vec::new();
        let mut unstarted_concepts: Vec<String> = Vec::new();
        let mut mastered_concepts: Vec<String> = Vec::new();

        for g in &ready_graphs {
            let Some(payload) = g.payload.as_ref() else { continue };
            let Some(nodes) = payload.get("nodes").and_then(Value::as_array) else { continue };
            let mastery_map = g
                .progress_state
                .as_ref()
                .and_then(|ps| ps.get("mastery"))
                .and_then(Value::as_object);
            let concept_nodes: Vec<&Value> = if nodes.len() > 1 {
                nodes.iter().filter(|n| !n.get("parent_id").map(Value::is_null).unwrap_or(true)).collect()
            } else {
                nodes.iter().collect()
            };
            for n in concept_nodes {
                let nid = n.get("id").and_then(Value::as_str).unwrap_or("");
                let label = n.get("label").and_then(Value::as_str).unwrap_or("").trim();
                if nid.is_empty() || label.is_empty() {
                    continue;
                }
                total += 1;
                let status = mastery_map
                    .and_then(|m| m.get(nid))
                    .and_then(|v| v.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("unstarted");
                match status {
                    "mastered" => {
                        mastered += 1;
                        if mastered_concepts.len() < 10 {
                            mastered_concepts.push(label.to_string());
                        }
                    }
                    "learning" => learning += 1,
                    "weak" => {
                        weak += 1;
                        if weak_concepts.len() < 10 {
                            weak_concepts.push(label.to_string());
                        }
                    }
                    _ => {
                        unstarted += 1;
                        if unstarted_concepts.len() < 10 {
                            unstarted_concepts.push(label.to_string());
                        }
                    }
                }
            }
        }

        let mastery_percent = if total == 0 { 0 } else { (mastered * 100) / total };
        let assessed = mastered + learning + weak;
        let recommended_difficulty = if total == 0 || assessed == 0 {
            "mixed"
        } else if mastery_percent < 35 || weak > mastered {
            "easy"
        } else if mastery_percent < 75 {
            "medium"
        } else {
            "hard"
        };

        Ok(json!({
            "has_knowledge_graph": true,
            "graph_count": ready_graphs.len(),
            "total_concepts": total,
            "mastered": mastered,
            "learning": learning,
            "weak": weak,
            "unstarted": unstarted,
            "mastery_percent": mastery_percent,
            "recommended_difficulty": recommended_difficulty,
            "weak_concepts": weak_concepts,
            "unstarted_concepts": unstarted_concepts,
            "mastered_concepts": mastered_concepts,
        }))
    }

    pub fn apply_quiz_to_knowledge_graphs(
        &self,
        notebook_id: &str,
        quiz_artifact_id: &str,
        results: Value,
    ) -> LnaResult<Value> {
        let quiz = self.get(quiz_artifact_id)?;
        let Some(quiz_payload) = quiz.payload.as_ref() else {
            return Ok(json!({ "updated_graphs": 0, "updated_nodes": 0, "details": [] }));
        };
        let questions = quiz_payload
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let result_items = results.as_array().cloned().unwrap_or_default();
        if questions.is_empty() || result_items.is_empty() {
            return Ok(json!({ "updated_graphs": 0, "updated_nodes": 0, "details": [] }));
        }

        let mut graphs: Vec<StudioArtifact> = self
            .list_notebook_knowledge_graphs(notebook_id)?
            .into_iter()
            .filter(|g| g.status == ArtifactStatus::Ready && g.payload.is_some())
            .collect();
        if graphs.is_empty() {
            return Ok(json!({ "updated_graphs": 0, "updated_nodes": 0, "details": [] }));
        }
        // Prioritize knowledge graphs whose source overlaps with quiz sources
        let quiz_sids: std::collections::HashSet<&str> = quiz.source_ids.iter().map(String::as_str).collect();
        graphs.sort_by_key(|g| {
            let overlaps = g.source_ids.iter().any(|s| quiz_sids.contains(s.as_str()));
            if overlaps { 0 } else { 1 }
        });

        let mut result_by_qid: HashMap<String, (bool, f64)> = HashMap::new();
        for item in &result_items {
            if let Some(qid) = item.get("question_id").and_then(Value::as_str) {
                let is_correct = item.get("is_correct").and_then(Value::as_bool).unwrap_or(false);
                let score = item
                    .get("score")
                    .and_then(Value::as_f64)
                    .unwrap_or(if is_correct { 1.0 } else { 0.0 });
                result_by_qid.insert(qid.to_string(), (is_correct, score));
            }
        }

        let topic_hint = quiz
            .config
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or(&quiz.title);
        let now = now_ms();
        let mut updated_graphs = 0usize;
        let mut updated_nodes = 0usize;
        let mut details: Vec<Value> = Vec::new();

        for g in &graphs {
            let Some(payload) = g.payload.as_ref() else { continue };
            let Some(nodes) = payload.get("nodes").and_then(Value::as_array) else { continue };
            let mut progress_obj = g
                .progress_state
                .as_ref()
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut mastery_obj = progress_obj
                .get("mastery")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut graph_changed = false;

            for q in &questions {
                let qid = q.get("id").and_then(Value::as_str).unwrap_or("");
                let Some(&(is_correct, q_score)) = result_by_qid.get(qid) else { continue };
                let q_text = format!(
                    "{} {} {}",
                    quiz.title,
                    q.get("prompt").and_then(Value::as_str).unwrap_or(""),
                    q.get("explanation").and_then(Value::as_str).unwrap_or("")
                );

                let mut scored_nodes: Vec<(f64, &str, &str)> = Vec::new();
                for n in nodes {
                    let is_root = n.get("parent_id").map(Value::is_null).unwrap_or(false) && nodes.len() > 1;
                    if is_root {
                        continue;
                    }
                    let nid = n.get("id").and_then(Value::as_str).unwrap_or("");
                    let label = n.get("label").and_then(Value::as_str).unwrap_or("");
                    let summary = n.get("summary").and_then(Value::as_str).unwrap_or("");
                    if nid.is_empty() || label.is_empty() {
                        continue;
                    }
                    let s = score_kg_node_match(label, summary, &q_text, topic_hint);
                    if s >= 0.35 {
                        scored_nodes.push((s, nid, label));
                    }
                }
                scored_nodes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                scored_nodes.truncate(2);

                for (_, nid, label) in scored_nodes {
                    let mut entry = mastery_obj
                        .get(nid)
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let prev_attempts = entry.get("attempts").and_then(Value::as_u64).unwrap_or(0);
                    let prev_correct = entry.get("correct").and_then(Value::as_u64).unwrap_or(0);
                    let attempts = prev_attempts + 1;
                    let correct = prev_correct + if is_correct { 1 } else { 0 };
                    let acc = correct as f64 / attempts as f64;
                    let new_score = if prev_attempts == 0 { q_score } else { (acc + q_score) / 2.0 };
                    let status = if is_correct {
                        if new_score >= 0.75 { "mastered" } else { "learning" }
                    } else if new_score < 0.5 {
                        "weak"
                    } else {
                        "learning"
                    };
                    entry.insert("status".into(), json!(status));
                    entry.insert("score".into(), json!(new_score));
                    entry.insert("attempts".into(), json!(attempts));
                    entry.insert("correct".into(), json!(correct));
                    entry.insert("last_updated".into(), json!(now));
                    mastery_obj.insert(nid.to_string(), Value::Object(entry));
                    graph_changed = true;
                    updated_nodes += 1;
                    details.push(json!({
                        "graph_id": g.id,
                        "node_id": nid,
                        "label": label,
                        "status": status,
                    }));
                }
            }

            if graph_changed {
                progress_obj.insert("mastery".into(), Value::Object(mastery_obj));
                progress_obj.insert("updated_at".into(), json!(now));
                self.save_progress(&g.id, Value::Object(progress_obj))?;
                updated_graphs += 1;
            }
        }

        Ok(json!({
            "updated_graphs": updated_graphs,
            "updated_nodes": updated_nodes,
            "details": details,
        }))
    }

    pub async fn process_job(&self, artifact_id: &str, cancel: CancellationToken) -> LnaResult<()> {
        let art = self.get(artifact_id)?;
        let profile = match lna_storage::get_active_profile(&self.db)? {
            Some(p) => p,
            None => {
                let err = LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured");
                let log = format_error_log(
                    &art,
                    None,
                    &[AttemptFailureRecord {
                        attempt: 1,
                        stage: "INIT".into(),
                        elapsed_ms: 0,
                        code: err.code.as_str().into(),
                        retryable: false,
                        message: err.message.clone(),
                    }],
                );
                let _ = self.mark_failed_with_log(artifact_id, &err, &log);
                return Err(err);
            }
        };
        let api_key = match profile.keychain_ref {
            Some(ref k) => self.secrets.get(k)?,
            None => None,
        };
        let resolved = ResolvedProfile {
            id: profile.id.clone(),
            display_name: profile.display_name.clone(),
            protocol: profile.protocol,
            base_url: profile.base_url.clone(),
            model_id: profile.model_id.clone(),
            api_key,
            show_thinking: profile.show_thinking,
            effort: profile.effort.clone(),
            context_window: profile.context_window,
            max_output_tokens: profile.max_output_tokens,
            capabilities: profile.capabilities.clone(),
            extra_headers: profile.extra_headers.clone(),
            anthropic_version: "2023-06-01".to_string(),
        };

        let is_local_test = resolved.base_url.contains("127.0.0.1")
            || resolved.base_url.contains("localhost");
        let mut records: Vec<AttemptFailureRecord> = Vec::new();
        let total_attempts = MAX_STUDIO_RETRIES + 1;

        for attempt in 1..=total_attempts {
            if cancel.is_cancelled() {
                let _ = self.mark_cancelled(artifact_id);
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let fresh_art = self.get(artifact_id)?;
            if fresh_art.status == ArtifactStatus::Cancelled {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }

            let mut current_stage = "COLLECT".to_string();
            let started = std::time::Instant::now();
            match self
                .process_job_once(
                    &fresh_art,
                    &profile,
                    &resolved,
                    artifact_id,
                    attempt,
                    &mut current_stage,
                    cancel.clone(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let elapsed_ms = started.elapsed().as_millis();
                    if cancel.is_cancelled() || e.code == ErrorCode::Cancelled {
                        let _ = self.mark_cancelled(artifact_id);
                        return Err(e);
                    }
                    records.push(AttemptFailureRecord {
                        attempt,
                        stage: current_stage.clone(),
                        elapsed_ms,
                        code: e.code.as_str().to_string(),
                        retryable: e.retryable,
                        message: e.message.clone(),
                    });

                    if is_non_retryable_config_error(e.code) {
                        self.append_thought(
                            artifact_id,
                            &fresh_art.notebook_id,
                            "error",
                            "执行失败",
                            "请检查模型配置或权限设置后重试。",
                        );
                        let log = format_error_log(&fresh_art, Some(&profile), &records);
                        let _ = self.mark_failed_with_log(artifact_id, &e, &log);
                        return Err(e);
                    }

                    if attempt <= MAX_STUDIO_RETRIES {
                        let _ = self.rollback_checkpoints(artifact_id, &current_stage);
                        self.append_thought(
                            artifact_id,
                            &fresh_art.notebook_id,
                            "retry",
                            &format!("第 {attempt}/{MAX_STUDIO_RETRIES} 次回退重试"),
                            &format!(
                                "阶段 {current_stage} 执行失败（{}）：{}。已回退该阶段检查点并准备重试。",
                                e.code.as_str(),
                                truncate_str(&e.message, 220)
                            ),
                        );
                        let topic = format!("studio:{}", fresh_art.notebook_id);
                        let rolled_progress = stage_progress(&current_stage);
                        let _ = self.set_stage(
                            artifact_id,
                            "RUNNING",
                            &current_stage,
                            rolled_progress,
                            &topic,
                        );
                        let delay_ms = if is_local_test {
                            40
                        } else {
                            u64::from(attempt) * 2000
                        };
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                let _ = self.mark_cancelled(artifact_id);
                                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled during retry backoff"));
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                        }
                    } else {
                        self.append_thought(
                            artifact_id,
                            &fresh_art.notebook_id,
                            "error",
                            "执行失败",
                            "未能完成生成，请稍后重试。",
                        );
                        let log = format_error_log(&fresh_art, Some(&profile), &records);
                        let _ = self.mark_failed_with_log(artifact_id, &e, &log);
                        return Err(e);
                    }
                }
            }
        }
        unreachable!()
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_job_once(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        attempt: u32,
        current_stage: &mut String,
        cancel: CancellationToken,
    ) -> LnaResult<()> {
        let topic = format!("studio:{}", art.notebook_id);
        let reasoning = ReasoningOptions {
            show_summary: resolved.show_thinking || resolved.capabilities.reasoning_summary,
            effort: resolved.effort.clone(),
        };

        *current_stage = "COLLECT".to_string();
        self.set_stage(artifact_id, "RUNNING", "COLLECT", 0.15, &topic)?;
        let saved_collect = self.get_checkpoint(artifact_id, "collect")?;
        let evidence_txt = if let Some(checkpoint) = saved_collect.filter(|value| {
            !art.sources_stale
                && value.get("sources_fingerprint").and_then(Value::as_str)
                    == Some(art.sources_fingerprint.as_str())
        }) {
            if attempt == 1 {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "复用已缓存来源证据",
                    "已从检查点加载来源资料，跳过重复收集。",
                );
            }
            checkpoint
                .get("evidence_text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            let text = self
                .collect_evidence_text(art, profile, resolved, artifact_id, cancel.clone())
                .await?;
            self.save_checkpoint(
                artifact_id,
                "collect",
                json!({
                    "sources_fingerprint": art.sources_fingerprint,
                    "evidence_text": text
                }),
            )?;
            text
        };

        let review_outline = art
            .config
            .get("reviewOutline")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if review_outline && art.draft.is_none() {
            *current_stage = "PLAN".to_string();
            self.set_stage(artifact_id, "RUNNING", "PLAN", 0.35, &topic)?;
            let is_report = art.artifact_type == ArtifactType::Report;
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                if is_report { "规划报告提纲" } else { "规划演示文稿提纲" },
                if is_report {
                    "正在基于已选来源梳理报告章节结构、论点脉络与核心证据…"
                } else {
                    "正在基于已选来源梳理演示文稿章节结构与每页要点…"
                },
            );
            let outline_sys = if is_report {
                "You are a research report outline planner. Output a clean, human-readable Markdown outline (using `# Report Title`, `## Section N: Section Heading`, `- Key findings, analysis points, and supporting evidence`). Match the language requested in config or the primary language of the sources. Be concise, well-structured, and accurate; never output JSON or fenced code blocks."
            } else {
                "You are a presentation outline planner. Output a clean, human-readable Markdown outline (using `# Title`, `## Slide N: Slide Title`, `- Key points`, and `> Speaker notes`). Match the language requested in config or the primary language of the sources. Be concise and accurate; never output JSON or fenced code blocks."
            };
            let plan_req = LlmRequest {
                system: outline_sys.into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "Artifact type: {}\nConfig: {}\nSources:\n{}",
                        art.artifact_type.as_str(), art.config, evidence_txt
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: 1800,
                reasoning: reasoning.clone(),
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let (draft_raw, _) = self
                .run_text_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    "大纲规划思考过程",
                    resolved,
                    &plan_req,
                    cancel,
                )
                .await?;
            let draft_md_json = serde_json::to_string(&Value::String(draft_raw.trim().to_string()))
                .unwrap_or_else(|_| "\"\"".into());
            self.db.write(|conn| {
                conn.execute(
                    "UPDATE studio_artifacts SET status='DRAFT_REVIEW', stage='DRAFT_REVIEW',
                     progress=0.45, draft_json=?1, updated_at=?2 WHERE id=?3 AND status != 'CANCELLED'",
                    params![draft_md_json, now_ms(), artifact_id],
                )
                .map_err(map_sql_err)?;
                Ok(())
            })?;
            self.emit_status_event(artifact_id, "DRAFT_REVIEW", "DRAFT_REVIEW", 0.45, None, None);
            return Ok(());
        }

        *current_stage = "GENERATE".to_string();
        self.set_stage(artifact_id, "RUNNING", "GENERATE", 0.55, &topic)?;
        let saved_generate = self
            .get_checkpoint(artifact_id, "generate")?
            .filter(|value| {
                !art.sources_stale
                    && value.get("sources_fingerprint").and_then(Value::as_str)
                        == Some(art.sources_fingerprint.as_str())
            });
        let max_evidence = validate::max_evidence_no(&evidence_txt);
        let evidence_cap =
            (max_evidence > 0 && evidence_txt.starts_with("[1] ")).then_some(max_evidence);

        let (mut payload, usage) =
            if let Some(saved) = saved_generate.and_then(|v| v.get("payload").cloned()) {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "复用已验证内容检查点",
                    "已加载上次生成的结构化内容，准备继续渲染。",
                );
                (saved, Usage::default())
            } else {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "生成结构化成品内容",
                    &format!("正在生成「{}」…", art.title),
                );
                let max_repairs = if attempt == 1 { 2 } else { 0 };
                let (mut generated, usage) = self
                    .generate_structured_payload(
                        art,
                        profile,
                        resolved,
                        artifact_id,
                        &evidence_txt,
                        reasoning,
                        max_repairs,
                        cancel.clone(),
                    )
                    .await?;
                if let Some(max) = evidence_cap {
                    if art.artifact_type != ArtifactType::Quiz {
                        validate::sanitize_citations(&mut generated, max);
                    }
                }
                match art.artifact_type {
                    ArtifactType::Quiz => {
                        let allowed_types: Vec<&str> = art
                            .config
                            .get("types")
                            .and_then(|v| v.as_array())
                            .map(|items| items.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        validate::validate_quiz_with(&generated, &allowed_types, evidence_cap)?;
                    }
                    ArtifactType::Flashcards => validate::dedup_flashcards(&mut generated),
                    ArtifactType::Mindmap => {
                        let detail = art
                            .config
                            .get("detail")
                            .and_then(Value::as_str)
                            .unwrap_or("standard");
                        validate::enforce_mindmap_limits_with_detail(&mut generated, detail);
                    }
                    _ => {}
                }
                // Only save the checkpoint after validation succeeds so broken payloads are never reused.
                self.save_checkpoint(
                    artifact_id,
                    "generate",
                    json!({
                        "sources_fingerprint": art.sources_fingerprint,
                        "payload": generated.clone()
                    }),
                )?;
                (generated, usage)
            };

        if let Some(max) = evidence_cap {
            if art.artifact_type != ArtifactType::Quiz {
                validate::sanitize_citations(&mut payload, max);
            }
        }

        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&profile.id),
            "studio",
            Some(artifact_id),
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
        );

        *current_stage = "RENDER".to_string();
        self.set_stage(artifact_id, "RUNNING", "RENDER", 0.85, &topic)?;
        let out_dir = self.db.paths().artifacts_dir.join(artifact_id);
        std::fs::create_dir_all(&out_dir).map_err(|e| LnaError::internal(e.to_string()))?;
        let payload_hash = hex::encode(Sha256::digest(payload.to_string().as_bytes()));
        let render_checkpoint = self.get_checkpoint(artifact_id, "render")?;
        let render_is_complete = render_checkpoint
            .as_ref()
            .filter(|v| {
                v.get("payload_hash").and_then(Value::as_str) == Some(payload_hash.as_str())
            })
            .and_then(|v| v.get("output_dir"))
            .and_then(Value::as_str)
            .map(|path| {
                let dir = std::path::Path::new(path);
                dir.is_dir()
                    && match art.artifact_type {
                        ArtifactType::Slides => dir.join("slides.pptx").is_file(),
                        ArtifactType::Infographic => dir.join("infographic.svg").is_file(),
                        ArtifactType::Audio => dir.join("overview.wav").is_file(),
                        _ => true,
                    }
            })
            .unwrap_or(false);

        if !render_is_complete {
            let _audio_permit = if art.artifact_type == ArtifactType::Audio {
                Some(
                    self.audio_render_slot
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| LnaError::internal("Audio render semaphore closed"))?,
                )
            } else {
                None
            };
            match art.artifact_type {
                ArtifactType::Slides => {
                    render::render_pptx(&payload, &out_dir.join("slides.pptx"))?;
                }
                ArtifactType::Infographic => {
                    render::render_infographic_svg(&payload, &out_dir.join("infographic.svg"))?;
                }
                ArtifactType::Audio => {
                    let voice_a = Voice {
                        id: "host_a".into(),
                        name: "Host A".into(),
                        lang: "auto".into(),
                    };
                    let voice_b = Voice {
                        id: "host_b".into(),
                        name: "Host B".into(),
                        lang: "auto".into(),
                    };
                    let mut seg_paths = Vec::new();
                    if let Some(turns) = payload.get("turns").and_then(|t| t.as_array()) {
                        for (i, t) in turns.iter().enumerate() {
                            let txt = t.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            let spk = t.get("speaker").and_then(|x| x.as_str()).unwrap_or("A");
                            if !txt.is_empty() {
                                let seg_path = out_dir.join(format!("turn_{i:03}.wav"));
                                if !seg_path.exists() {
                                    let v = if spk.contains('B') || spk.contains('2') {
                                        &voice_b
                                    } else {
                                        &voice_a
                                    };
                                    self.tts
                                        .synthesize(txt, v, &seg_path, cancel.clone())
                                        .await?;
                                }
                                seg_paths.push(seg_path);
                            }
                        }
                    }
                    if !seg_paths.is_empty() {
                        concat_wavs(&seg_paths, &out_dir.join("overview.wav"))?;
                    }
                }
                _ => {}
            }
            self.save_checkpoint(
                artifact_id,
                "render",
                json!({
                    "output_dir": out_dir.to_string_lossy(),
                    "payload_hash": payload_hash
                }),
            )?;
        }

        let payload_str = payload.to_string();
        let out_dir_str = out_dir.to_string_lossy().to_string();
        let now = now_ms();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='READY', stage='READY', progress=1.0,
                 payload_json=?1, output_dir=?2, error_code=NULL, error_detail=NULL, updated_at=?3, finished_at=?4
                 WHERE id=?5 AND status != 'CANCELLED'",
                params![payload_str, out_dir_str, now, now, artifact_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            "成品生成完成",
            "内容校验与渲染均已完成，可开始交互预览或导出。",
        );
        self.emit_status_event(artifact_id, "READY", "READY", 1.0, None, None);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn generate_structured_payload(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        evidence_txt: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage)> {
        let (schema_str, schema_name) = match art.artifact_type {
            ArtifactType::Quiz => (SCHEMA_QUIZ, "quiz_payload"),
            ArtifactType::Flashcards => (SCHEMA_FLASHCARDS, "flashcards_payload"),
            ArtifactType::Mindmap => (SCHEMA_MINDMAP, "mindmap_payload"),
            ArtifactType::Report => (SCHEMA_REPORT, "report_payload"),
            ArtifactType::Slides => (SCHEMA_SLIDES, "slides_payload"),
            ArtifactType::Infographic => (SCHEMA_INFOGRAPHIC, "infographic_payload"),
            ArtifactType::Audio => (SCHEMA_AUDIO, "audio_script_payload"),
        };
        let schema: Value =
            serde_json::from_str(schema_str).map_err(|e| LnaError::internal(e.to_string()))?;

        let explicit_count = art
            .config
            .get("count")
            .and_then(|v| v.as_i64())
            .filter(|n| *n > 0);
        if art.artifact_type == ArtifactType::Quiz
            && explicit_count.is_some_and(|n| n as usize > QUIZ_BATCH_SIZE)
        {
            let total = (explicit_count.unwrap() as usize).clamp(1, 30);
            let batches = total.div_ceil(QUIZ_BATCH_SIZE);
            let mut all_questions: Vec<Value> = Vec::new();
            let mut seen_prompts: Vec<String> = Vec::new();
            let mut title = art.title.clone();
            let mut usage_sum = Usage::default();
            let mut remaining = total;
            for bi in 0..batches {
                if cancel.is_cancelled() {
                    return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                }
                let batch_cnt = remaining.min(QUIZ_BATCH_SIZE);
                remaining -= batch_cnt;
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &format!("分批生成测验题目（{}/{batches}）", bi + 1),
                    &format!("为保证稳定性并避免单次输出过长，本批生成 {batch_cnt} 道题（总目标 {total} 道）。"),
                );
                let avoid_txt = if seen_prompts.is_empty() {
                    String::new()
                } else {
                    let recent: Vec<&str> = seen_prompts
                        .iter()
                        .rev()
                        .take(20)
                        .map(String::as_str)
                        .collect();
                    format!("\n\n已生成题干（严禁重复相同考点或题干）：\n- {}", recent.join("\n- "))
                };
                let mut batch_cfg = art.config.clone();
                if let Some(obj) = batch_cfg.as_object_mut() {
                    obj.insert("count".into(), json!(batch_cnt));
                }
                let batch_req = LlmRequest {
                    system: format!(
                        "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array. Match the language requested in config or the primary language of the sources. Keep every prompt, option, and explanation concise and accurate. Format all mathematical expressions and chemical formulas/equations in LaTeX wrapped in $...$ (e.g., $\\\\ce{{H2O}}$, $\\\\ce{{O2}}$, $\\\\ce{{CO2}}$, $\\\\ce{{2H2 + O2 -> 2H2O}}$, $x^2$, $\\\\frac{{a}}{{b}}$); never write bare ASCII formulas without subscripts/LaTeX, and double-escape all LaTeX backslashes in JSON strings so \\\\frac, \\\\times, \\\\text, \\\\ce are not corrupted.",
                        art.artifact_type.as_str()
                    ),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Batch {}/{}: generate exactly {} questions.\nParameters: {}\nReference outline: {}{}\n\nSources:\n{}",
                            bi + 1,
                            batches,
                            batch_cnt,
                            batch_cfg,
                            art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                            avoid_txt,
                            evidence_txt
                        ))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.max_output_tokens,
                    reasoning: reasoning.clone(),
                };
                let _permits = self.acquire_llm_slots(&profile.id).await?;
                let (part, u) = self
                    .run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &format!("第 {}/{} 批推理思考", bi + 1, batches),
                        resolved,
                        &batch_req,
                        schema_name,
                        &schema,
                        max_repairs,
                        cancel.clone(),
                    )
                    .await?;
                usage_sum.input_tokens = usage_sum.input_tokens.saturating_add(u.input_tokens);
                usage_sum.output_tokens = usage_sum.output_tokens.saturating_add(u.output_tokens);
                usage_sum.cached_tokens = usage_sum.cached_tokens.saturating_add(u.cached_tokens);
                if bi == 0 {
                    if let Some(t) = part.get("title").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                        title = t.to_string();
                    }
                }
                if let Some(qs) = part.get("questions").and_then(Value::as_array) {
                    for mut q in qs.iter().cloned() {
                        let prompt = q
                            .get("prompt")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !prompt.is_empty() && !seen_prompts.contains(&prompt) {
                            seen_prompts.push(prompt);
                            if let Some(obj) = q.as_object_mut() {
                                obj.insert("id".into(), json!(format!("q{}", all_questions.len() + 1)));
                            }
                            all_questions.push(q);
                        }
                    }
                }
            }
            if all_questions.is_empty() {
                return Err(LnaError::new(
                    ErrorCode::OutputInvalid,
                    "Batched quiz generation produced 0 valid questions",
                ));
            }
            return Ok((
                json!({ "title": title, "questions": all_questions }),
                usage_sum,
            ));
        }

        if art.artifact_type == ArtifactType::Flashcards
            && explicit_count.is_some_and(|n| n as usize > FLASHCARD_BATCH_SIZE)
        {
            let total = (explicit_count.unwrap() as usize).clamp(1, 50);
            let batches = total.div_ceil(FLASHCARD_BATCH_SIZE);
            let mut all_cards: Vec<Value> = Vec::new();
            let mut seen_fronts: Vec<String> = Vec::new();
            let mut title = art.title.clone();
            let mut usage_sum = Usage::default();
            let mut remaining = total;
            for bi in 0..batches {
                if cancel.is_cancelled() {
                    return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                }
                let batch_cnt = remaining.min(FLASHCARD_BATCH_SIZE);
                remaining -= batch_cnt;
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &format!("分批生成学习闪卡（{}/{batches}）", bi + 1),
                    &format!("为保证稳定性并避免单次输出过长，本批生成 {batch_cnt} 张卡片（总目标 {total} 张）。"),
                );
                let avoid_txt = if seen_fronts.is_empty() {
                    String::new()
                } else {
                    let recent: Vec<&str> = seen_fronts
                        .iter()
                        .rev()
                        .take(24)
                        .map(String::as_str)
                        .collect();
                    format!("\n\n已生成卡片正面（请勿重复）：\n- {}", recent.join("\n- "))
                };
                let mut batch_cfg = art.config.clone();
                if let Some(obj) = batch_cfg.as_object_mut() {
                    obj.insert("count".into(), json!(batch_cnt));
                }
                let batch_req = LlmRequest {
                    system: format!(
                        "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array. Match the language requested in config or the primary language of the sources. Keep each flashcard front and back crisp and focused on core definitions or takeaways.",
                        art.artifact_type.as_str()
                    ),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Batch {}/{}: generate exactly {} flashcards.\nParameters: {}\nReference outline: {}{}\n\nSources:\n{}",
                            bi + 1,
                            batches,
                            batch_cnt,
                            batch_cfg,
                            art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                            avoid_txt,
                            evidence_txt
                        ))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.max_output_tokens,
                    reasoning: reasoning.clone(),
                };
                let _permits = self.acquire_llm_slots(&profile.id).await?;
                let (part, u) = self
                    .run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &format!("第 {}/{} 批推理思考", bi + 1, batches),
                        resolved,
                        &batch_req,
                        schema_name,
                        &schema,
                        max_repairs,
                        cancel.clone(),
                    )
                    .await?;
                usage_sum.input_tokens = usage_sum.input_tokens.saturating_add(u.input_tokens);
                usage_sum.output_tokens = usage_sum.output_tokens.saturating_add(u.output_tokens);
                usage_sum.cached_tokens = usage_sum.cached_tokens.saturating_add(u.cached_tokens);
                if bi == 0 {
                    if let Some(t) = part.get("title").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                        title = t.to_string();
                    }
                }
                if let Some(cards) = part.get("cards").and_then(Value::as_array) {
                    for mut c in cards.iter().cloned() {
                        let front = c
                            .get("front")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !front.is_empty() && !seen_fronts.contains(&front) {
                            seen_fronts.push(front);
                            if let Some(obj) = c.as_object_mut() {
                                obj.insert("id".into(), json!(format!("c{}", all_cards.len() + 1)));
                            }
                            all_cards.push(c);
                        }
                    }
                }
            }
            if all_cards.is_empty() {
                return Err(LnaError::new(
                    ErrorCode::OutputInvalid,
                    "Batched flashcards generation produced 0 valid cards",
                ));
            }
            return Ok((json!({ "title": title, "cards": all_cards }), usage_sum));
        }

        if art.artifact_type == ArtifactType::Slides {
            let target_pages = art
                .config
                .get("pages")
                .and_then(Value::as_u64)
                .map(|p| p.clamp(4, 30))
                .unwrap_or_else(|| {
                    match art.config.get("length").and_then(Value::as_str).unwrap_or("standard") {
                        "concise" => 6,
                        "detailed" => 14,
                        _ => 10,
                    }
                });

            // Stage 1: Presentation Architecture & Visual Storyboard Planning
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "规划演示文稿视觉主题与逐页版式",
                &format!("正在围绕来源内容设计约 {target_pages} 页演示文稿的叙事节奏、主题配色与多样化版式组合…"),
            );
            let plan_req = LlmRequest {
                system: "You are an executive presentation architect and visual designer. Given the source material and user parameters, design a concise slide-by-slide visual storyboard (NOT raw JSON yet). Choose the best theme from [executive_navy, editorial_slate, academic_sage, warm_terracotta, midnight_aurora]. Plan an engaging narrative arc starting with `title` and `agenda`, ending with `summary`, and using DIVERSE visual layouts across content slides (`cards`, `two_column`, `metrics`, `timeline`, `table`, `formula`, `quote`, `section`, `bullets`) — NEVER use plain `bullets` on more than 25% of slides and never repeat the same content layout twice in a row. For each slide, specify: page number, layout, crisp headline, key facts/numbers/equations/comparisons from the sources, and visual structure. Match the requested language or primary source language.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "Target slide count: ~{target_pages}\nParameters: {}\nApproved outline (if any): {}\n\nResearched Sources:\n{}",
                        art.config,
                        art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                        evidence_txt
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: 2400,
                reasoning: ReasoningOptions {
                    show_summary: profile.show_thinking,
                    effort: profile.effort.clone(),
                },
            };
            let mut usage_sum = Usage::default();
            let storyboard = {
                let _permits = self.acquire_llm_slots(&profile.id).await?;
                match self
                    .run_text_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        "演示文稿视觉架构与版式思考",
                        resolved,
                        &plan_req,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok((txt, u)) => {
                        usage_sum.input_tokens = usage_sum.input_tokens.saturating_add(u.input_tokens);
                        usage_sum.output_tokens = usage_sum.output_tokens.saturating_add(u.output_tokens);
                        usage_sum.cached_tokens = usage_sum.cached_tokens.saturating_add(u.cached_tokens);
                        txt
                    }
                    Err(_) => String::new(),
                }
            };

            // Stage 2: Rich Structured Slide Deck Authoring
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "撰写高颜值演示文稿内容与版式数据",
                "正在基于版式蓝图生成完整结构化幻灯片（含卡片、对比表、流程图、关键指标、核心公式与演讲备注）…",
            );
            let author_req = LlmRequest {
                system: "You are a world-class presentation designer for Local Notebook Studio. Produce a publication-grade SlidesPayload JSON strictly grounded in the provided sources.\n\
Design rules:\n\
1. Set top-level `theme` (one of: executive_navy, editorial_slate, academic_sage, warm_terracotta, midnight_aurora) and `subtitle`.\n\
2. Use rich, varied layouts across slides: `title` (slide 1), `agenda`, `section`, `cards` (3-6 insight cards with tag/title/body), `two_column` (2 titled comparison columns with items), `metrics` (2-4 KPI highlight cards with value/label/detail), `timeline` (3-5 sequential steps with step/title/detail), `table` (structured comparison table with headers and rows), `formula` (highlighted equation/reaction in `formula` plus explanation in `bullets`), `quote`, `bullets` (sparingly), and `summary` (final slide).\n\
3. Populate the layout-specific fields (`cards`, `columns`, `metrics`, `steps`, `table`, `formula`, `subtitle`, `callout`) whenever using those layouts, AND always include concise `bullets` (can be empty [] when structured fields carry the slide), thorough `speaker_notes`, and valid `citations`.\n\
4. Keep on-slide text crisp and scannable (short headlines, concise card bodies, zero walls of text) while putting deeper explanation in `speaker_notes`.\n\
5. Match the language requested in config or the primary language of the sources.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "Target slide count: ~{target_pages}\nParameters: {}\nVisual Storyboard:\n{}\n\nReference outline: {}\n\nResearched Sources:\n{}",
                        art.config,
                        if storyboard.is_empty() { "Design a rich, visually varied deck." } else { &storyboard },
                        art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                        evidence_txt
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::Text,
                max_output_tokens: resolved.max_output_tokens,
                reasoning: reasoning.clone(),
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let (mut deck, u2) = self
                .run_structured_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    "演示文稿内容与排版生成",
                    resolved,
                    &author_req,
                    schema_name,
                    &schema,
                    max_repairs,
                    cancel.clone(),
                )
                .await?;
            drop(_permits);
            usage_sum.input_tokens = usage_sum.input_tokens.saturating_add(u2.input_tokens);
            usage_sum.output_tokens = usage_sum.output_tokens.saturating_add(u2.output_tokens);
            usage_sum.cached_tokens = usage_sum.cached_tokens.saturating_add(u2.cached_tokens);

            // Stage 3: Visual Diversity & Quality Self-Review Pass (only if too many plain bullets slides)
            let needs_polish = deck
                .get("slides")
                .and_then(Value::as_array)
                .map(|arr| {
                    let content_count = arr.len().saturating_sub(2);
                    let plain_bullets = arr
                        .iter()
                        .filter(|s| s.get("layout").and_then(Value::as_str) == Some("bullets"))
                        .count();
                    content_count >= 3 && plain_bullets * 2 > content_count
                })
                .unwrap_or(false);

            if needs_polish && !cancel.is_cancelled() {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "排版美学与信息密度自检优化",
                    "检测到部分页面仍为纯列表版式，正在将其重构为卡片、双栏对比、流程图或表格以提升视觉层次…",
                );
                let polish_req = LlmRequest {
                    system: "You are a senior presentation visual editor. Upgrade the provided SlidesPayload JSON so it looks like a professionally designed deck: convert repetitive `bullets` slides into rich `cards`, `two_column`, `timeline`, `table`, `metrics`, or `formula` layouts with crisp `subtitle` and bottom `callout` takeaways, while preserving 100% factual accuracy and citations.".into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!("Current SlidesPayload JSON:\n{}", deck))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.max_output_tokens,
                    reasoning: ReasoningOptions::default(),
                };
                let _permits = self.acquire_llm_slots(&profile.id).await?;
                if let Ok((polished, u3)) = self
                    .run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        "演示文稿视觉排版精修",
                        resolved,
                        &polish_req,
                        schema_name,
                        &schema,
                        1,
                        cancel.clone(),
                    )
                    .await
                {
                    deck = polished;
                    usage_sum.input_tokens = usage_sum.input_tokens.saturating_add(u3.input_tokens);
                    usage_sum.output_tokens = usage_sum.output_tokens.saturating_add(u3.output_tokens);
                    usage_sum.cached_tokens = usage_sum.cached_tokens.saturating_add(u3.cached_tokens);
                }
            }

            return Ok((deck, usage_sum));
        }

        let is_kg = art.artifact_type == ArtifactType::Mindmap
            && art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let detail_level = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("standard");
        let extra_hint = match art.artifact_type {
            ArtifactType::Mindmap => {
                let detail_rule = match detail_level {
                    "concise" => " Detail level: CONCISE (high-level overview, 2-3 hierarchy levels, ~12-20 nodes, focus strictly on major themes and core concepts).",
                    "detailed" => " Detail level: DETAILED (comprehensive breakdown, 4-5 hierarchy levels, ~35-60 nodes, include key sub-concepts, mechanisms, formulas, and concrete examples within the requested scope).",
                    _ => " Detail level: STANDARD (balanced 3-4 hierarchy levels, ~20-35 nodes covering main themes, sub-topics, and key takeaways).",
                };
                let structure_rule = " STRUCTURE ADAPTATION: Organize the hierarchy according to the actual knowledge structure of the material rather than copying chapter headings mechanically. You may merge closely related or thin sections under a cohesive parent theme, and split dense or foundational topics into finer sub-branches so that each branch goes beyond surface table-of-contents titles to capture core definitions, theorems, formulas, and mechanisms.";
                let kg_rule = if is_kg {
                    r#" Set "kind": "knowledge_graph". Build a concept Knowledge Graph structured as a collapsible tree from foundational prerequisites to core theorems and advanced applications. For each node, set "importance" ("core" | "key" | "detail"), a brief 1-sentence "summary", and "prerequisites" (names of prerequisite concepts when applicable)."#
                } else {
                    ""
                };
                format!(
                    " IMPORTANT: Do NOT include any bracketed citation numbers like [1], [2], or [1][17] inside node labels; keep node labels clean and readable, and leave every node's citations array empty []. Node labels are shown as plain text, so write any math in labels as compact Unicode (e.g. A=SΛS⁻¹, Aᵀ, x₁, ℝⁿ, √2) instead of LaTeX.{}{}{}",
                    detail_rule, structure_rule, kg_rule
                )
            }
            ArtifactType::Report => {
                match art.config.get("depth").and_then(Value::as_str).unwrap_or("standard") {
                    "brief" => " Report depth: BRIEF executive overview with concise bulleted takeaways.".into(),
                    "deep" => " Report depth: DEEP analytical synthesis with thorough explanations, derivations/examples, and structured comparisons.".into(),
                    _ => String::new(),
                }
            }
            ArtifactType::Slides => {
                match art.config.get("length").and_then(Value::as_str).unwrap_or("standard") {
                    "concise" => " Deck length: CONCISE overview (~5-6 slides focusing on core takeaways).".into(),
                    "detailed" => " Deck length: DETAILED coverage (~12-16 slides with thorough explanations and examples).".into(),
                    _ => " Deck length: BALANCED (~8-10 slides covering main themes cleanly).".into(),
                }
            }
            ArtifactType::Quiz => {
                let mut quiz_hint = " Format all mathematical expressions and chemical formulas/equations in prompt, options, accepted_answers, rubric, and explanation using LaTeX wrapped in $...$ (e.g., $\\\\ce{H2O}$, $\\\\ce{O2}$, $\\\\ce{CO2}$, $\\\\ce{2H2 + O2 -> 2H2O}$, $x^2$, $\\\\frac{a}{b}$); never write bare ASCII formulas without subscripts/LaTeX, and double-escape all LaTeX backslashes in JSON strings so \\\\frac, \\\\times, \\\\text, \\\\ce are not corrupted.".to_string();
                let explicit_diff = art
                    .config
                    .get("difficulty")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "auto");
                if let Ok(kg_summary) = self.compute_kg_progress_summary(&art.notebook_id, None) {
                    if kg_summary.get("has_knowledge_graph").and_then(Value::as_bool) == Some(true) {
                        let rec_diff = kg_summary
                            .get("recommended_difficulty")
                            .and_then(Value::as_str)
                            .unwrap_or("mixed");
                        let target_diff = explicit_diff.unwrap_or(rec_diff);
                        let mastery_pct = kg_summary.get("mastery_percent").and_then(Value::as_u64).unwrap_or(0);
                        let weak: Vec<String> = kg_summary
                            .get("weak_concepts")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                            .unwrap_or_default();
                        let unstarted: Vec<String> = kg_summary
                            .get("unstarted_concepts")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                            .unwrap_or_default();
                        quiz_hint.push_str(&format!(
                            " Target difficulty: {} (current Knowledge Graph mastery: {}%).",
                            target_diff, mastery_pct
                        ));
                        if !weak.is_empty() {
                            quiz_hint.push_str(&format!(" Prioritize reinforcing weak concepts: {}.", weak.join(", ")));
                        }
                        if !unstarted.is_empty() {
                            quiz_hint.push_str(&format!(" Also assess unstarted concepts: {}.", unstarted.join(", ")));
                        }
                    } else if let Some(d) = explicit_diff {
                        quiz_hint.push_str(&format!(" Target difficulty: {}.", d));
                    }
                }
                quiz_hint
            }
            ArtifactType::Audio => {
                match art.config.get("length").and_then(Value::as_str).unwrap_or("standard") {
                    "short" | "concise" => " Audio duration: SHORT concise overview (~2-3 minutes of dialogue).".into(),
                    "long" | "detailed" => " Audio duration: DETAILED in-depth discussion (~6-10 minutes of dialogue).".into(),
                    _ => String::new(),
                }
            }
            _ => String::new(),
        };
        let citation_rule = if art.artifact_type == ArtifactType::Mindmap {
            ""
        } else {
            ", preserving source citation indices [n] in the citations array"
        };
        let gen_req = LlmRequest {
            system: format!(
                "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}{}.{} Match the language requested in config or the primary language of the sources. Keep content concise, accurate, and well-structured without filler.",
                art.artifact_type.as_str(),
                citation_rule,
                extra_hint
            ),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(format!(
                    "Parameters: {}\nReference outline: {}\n\nSources:\n{}",
                    art.config,
                    art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                    evidence_txt
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning,
        };
        let _permits = self.acquire_llm_slots(&profile.id).await?;
        self.run_structured_with_thought(
            artifact_id,
            &art.notebook_id,
            "模型推理思考过程",
            resolved,
            &gen_req,
            schema_name,
            &schema,
            max_repairs,
            cancel,
        )
        .await
    }

    pub fn mark_failed(&self, id: &str, err: &LnaError) -> LnaResult<()> {
        let now = now_ms();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts
                 SET status='FAILED', stage='FAILED', error_code=?1, error_detail=?2, updated_at=?3
                 WHERE id=?4 AND status != 'CANCELLED'",
                params![err.code.as_str(), err.message, now, id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit_status_event(id, "FAILED", "FAILED", 0.0, Some(err.code.as_str()), Some(&err.message));
        Ok(())
    }

    pub fn mark_failed_with_log(&self, id: &str, err: &LnaError, error_log: &str) -> LnaResult<()> {
        let now = now_ms();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts
                 SET status='FAILED', stage='FAILED', error_code=?1, error_detail=?2, error_log=?3, updated_at=?4
                 WHERE id=?5 AND status != 'CANCELLED'",
                params![err.code.as_str(), err.message, error_log, now, id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit_status_event(id, "FAILED", "FAILED", 0.0, Some(err.code.as_str()), Some(&err.message));
        Ok(())
    }

    pub fn mark_cancelled(&self, id: &str) -> LnaResult<()> {
        let changed = self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='CANCELLED', stage='CANCELLED', updated_at=?1
                 WHERE id=?2 AND status IN ('PENDING','RUNNING','DRAFT_REVIEW')",
                params![now_ms(), id],
            )
            .map_err(map_sql_err)
        })?;
        if changed > 0 {
            self.emit_status_event(id, "CANCELLED", "CANCELLED", 0.0, None, None);
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> LnaResult<()> {
        let art = self.get(id)?;
        self.db.write(|conn| {
            conn.execute("DELETE FROM studio_artifacts WHERE id = ?1", [id])
                .map_err(map_sql_err)?;
            Ok(())
        })?;
        if let Some(dir) = art.output_dir {
            let p = std::path::Path::new(&dir);
            if p.exists() {
                let _ = std::fs::remove_dir_all(p);
            }
        }
        Ok(())
    }

    pub fn rename(&self, id: &str, title: &str) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET title=?1, updated_at=?2 WHERE id=?3",
                params![title, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn retry(&self, id: &str) -> LnaResult<StudioArtifact> {
        let art = self.get(id)?;
        self.clear_thoughts(id)?;
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='PENDING', stage='QUEUED', progress=0.0, error_code=NULL, error_detail=NULL, error_log=NULL, updated_at=?1 WHERE id=?2",
                params![now_ms(), id],
            ).map_err(map_sql_err)?;
            Ok(())
        })?;
        self.jobs
            .enqueue("studio_generate", Some(&art.notebook_id), id, 100)?;
        self.emit_status_event(id, "PENDING", "QUEUED", 0.0, None, None);
        self.get(id)
    }

    /// Regenerate an existing artifact on user demand using updated requirements (`config`) and/or
    /// the notebook's currently selected ready sources.
    pub fn regenerate(
        &self,
        id: &str,
        config: Option<Value>,
        use_selected_sources: bool,
    ) -> LnaResult<StudioArtifact> {
        let art = self.get(id)?;
        let (source_ids, fp) = if use_selected_sources {
            let (selected_ids, cur_fp) = self.current_sources_fingerprint(&art.notebook_id)?;
            if selected_ids.is_empty() {
                return Err(LnaError::new(
                    ErrorCode::NoSourceSelected,
                    "Please select at least one ready source",
                ));
            }
            (selected_ids, cur_fp)
        } else {
            (art.source_ids.clone(), art.sources_fingerprint.clone())
        };
        let cfg_val = config.unwrap_or(art.config);
        let cfg_json = serde_json::to_string(&cfg_val).unwrap_or_else(|_| "{}".into());
        let sids_json = serde_json::to_string(&source_ids).unwrap_or_else(|_| "[]".into());
        let now = now_ms();

        self.clear_thoughts(id)?;
        self.db.write(|conn| {
            conn.execute(
                "DELETE FROM artifact_checkpoints WHERE artifact_id = ?1",
                params![id],
            )
            .map_err(map_sql_err)?;
            conn.execute(
                "UPDATE studio_artifacts
                 SET status='PENDING', stage='QUEUED', progress=0.0, config_json=?1,
                     source_ids_json=?2, sources_fingerprint=?3, draft_json=NULL,
                     error_code=NULL, error_detail=NULL, error_log=NULL, updated_at=?4
                 WHERE id=?5",
                params![cfg_json, sids_json, fp, now, id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.jobs
            .enqueue("studio_generate", Some(&art.notebook_id), id, 100)?;
        self.emit_status_event(id, "PENDING", "QUEUED", 0.0, None, None);
        self.get(id)
    }

    pub fn set_feedback(&self, id: &str, feedback: i32) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET feedback=?1, updated_at=?2 WHERE id=?3",
                params![feedback, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn grade_answer(
        &self,
        artifact_id: &str,
        question_id: &str,
        answer: &Value,
    ) -> LnaResult<Value> {
        let art = self.get(artifact_id)?;
        let payload = art
            .payload
            .ok_or_else(|| LnaError::not_found("Artifact payload not ready"))?;
        let qs = payload
            .get("questions")
            .and_then(|x| x.as_array())
            .ok_or_else(|| LnaError::invalid("Not a quiz"))?;
        let q = qs
            .iter()
            .find(|x| x.get("id").and_then(|i| i.as_str()) == Some(question_id))
            .ok_or_else(|| LnaError::not_found("Question not found"))?;
        Ok(validate::grade_quiz_answer(q, answer))
    }

    pub fn export_artifact(
        &self,
        artifact_id: &str,
        format: &str,
        out_path: &std::path::Path,
    ) -> LnaResult<()> {
        let art = self.get(artifact_id)?;
        let payload = art.payload.unwrap_or_else(|| json!({}));
        if let Some(p) = out_path.parent() {
            std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
        }
        match format {
            "json" => std::fs::write(out_path, serde_json::to_string_pretty(&payload).unwrap())
                .map_err(|e| LnaError::internal(e.to_string())),
            "csv" => {
                let mut csv = String::from("front,back,hint\n");
                if let Some(cards) = payload.get("cards").and_then(|x| x.as_array()) {
                    for c in cards {
                        let f = c
                            .get("front")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .replace('"', "\"\"");
                        let b = c
                            .get("back")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .replace('"', "\"\"");
                        let h = c
                            .get("hint")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .replace('"', "\"\"");
                        csv.push_str(&format!("\"{f}\",\"{b}\",\"{h}\"\n"));
                    }
                }
                std::fs::write(out_path, csv).map_err(|e| LnaError::internal(e.to_string()))
            }
            "wav" => {
                let src = art
                    .output_dir
                    .as_deref()
                    .map(|d| std::path::Path::new(d).join("overview.wav"))
                    .filter(|p| p.exists())
                    .ok_or_else(|| LnaError::not_found("No audio file for this artifact"))?;
                std::fs::copy(src, out_path)
                    .map(|_| ())
                    .map_err(|e| LnaError::internal(e.to_string()))
            }
            "svg" => {
                if art.artifact_type == ArtifactType::Mindmap {
                    render::render_mindmap_svg(&payload, out_path)
                } else {
                    render::render_infographic_svg(&payload, out_path)
                }
            }
            "png" => render::render_artifact_png(
                art.artifact_type.as_str(),
                &art.title,
                &payload,
                out_path,
            ),
            "html" => render::render_report_html(
                art.artifact_type.as_str(),
                &art.title,
                &payload,
                out_path,
            ),
            "pptx" => render::render_pptx(&payload, out_path),
            _ => {
                let md =
                    render::artifact_markdown(art.artifact_type.as_str(), &art.title, &payload);
                std::fs::write(out_path, md).map_err(|e| LnaError::internal(e.to_string()))
            }
        }
    }

    async fn acquire_llm_slots(
        &self,
        profile_id: &str,
    ) -> LnaResult<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let profile_slot = {
            let mut slots = self.profile_generation_slots.lock().await;
            slots
                .entry(profile_id.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(4)))
                .clone()
        };
        let profile_permit = profile_slot
            .acquire_owned()
            .await
            .map_err(|_| LnaError::internal("Profile generation semaphore closed"))?;
        let global_permit = self
            .text_generation_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| LnaError::internal("Text generation semaphore closed"))?;
        Ok((profile_permit, global_permit))
    }

    fn get_checkpoint(&self, artifact_id: &str, step_key: &str) -> LnaResult<Option<Value>> {
        self.db.read(|conn| {
            let raw: Option<String> = conn.query_row(
                "SELECT output_json FROM artifact_checkpoints WHERE artifact_id=?1 AND step_key=?2",
                params![artifact_id, step_key],
                |r| r.get(0),
            ).optional().map_err(map_sql_err)?;
            Ok(raw.and_then(|value| serde_json::from_str(&value).ok()))
        })
    }

    fn save_checkpoint(&self, artifact_id: &str, step_key: &str, output: Value) -> LnaResult<()> {
        let output_json = output.to_string();
        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO artifact_checkpoints (artifact_id, step_key, output_json, created_at)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(artifact_id,step_key) DO UPDATE SET output_json=excluded.output_json, created_at=excluded.created_at",
                params![artifact_id, step_key, output_json, now_ms()],
            ).map_err(map_sql_err)?;
            Ok(())
        })
    }

    fn rollback_checkpoints(&self, artifact_id: &str, from_stage: &str) -> LnaResult<()> {
        let keys: &[&str] = match from_stage {
            "COLLECT" => &["collect", "generate", "render"],
            "RENDER" => &["render"],
            _ => &["generate", "render"],
        };
        let keys_json = serde_json::to_string(keys).unwrap_or_else(|_| "[]".into());
        self.db.write(|conn| {
            conn.execute(
                "DELETE FROM artifact_checkpoints
                 WHERE artifact_id = ?1 AND step_key IN (SELECT value FROM json_each(?2))",
                params![artifact_id, keys_json],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    fn clear_thoughts(&self, artifact_id: &str) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "DELETE FROM artifact_thoughts WHERE artifact_id = ?1",
                [artifact_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    fn append_thought(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        kind: &str,
        title: &str,
        body: &str,
    ) -> i32 {
        let now = now_ms();
        let inserted = self.db.write(|conn| {
            let next_seq: i32 = conn
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM artifact_thoughts WHERE artifact_id = ?1",
                    [artifact_id],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)?;
            conn.execute(
                "INSERT INTO artifact_thoughts (artifact_id, seq, kind, title, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![artifact_id, next_seq, kind, title, body, now],
            )
            .map_err(map_sql_err)?;
            Ok(next_seq)
        });
        let Ok(seq) = inserted else {
            return 0;
        };
        self.emit(
            &format!("studio:{notebook_id}"),
            "studio.thought",
            json!({
                "artifact_id": artifact_id,
                "seq": seq,
                "kind": kind,
                "title": title,
                "body": body,
                "created_at": now,
            }),
        );
        seq
    }

    fn update_thought_body(&self, artifact_id: &str, seq: i32, body: &str) {
        if seq <= 0 {
            return;
        }
        let _ = self.db.write(|conn| {
            conn.execute(
                "UPDATE artifact_thoughts SET body = ?1 WHERE artifact_id = ?2 AND seq = ?3",
                params![body, artifact_id, seq],
            )
            .map_err(map_sql_err)?;
            Ok(())
        });
    }

    fn emit_thought_delta(&self, artifact_id: &str, notebook_id: &str, seq: i32, delta: &str) {
        if seq <= 0 || delta.is_empty() {
            return;
        }
        self.emit(
            &format!("studio:{notebook_id}"),
            "studio.thought_delta",
            json!({
                "artifact_id": artifact_id,
                "seq": seq,
                "delta": delta,
            }),
        );
    }

    async fn run_text_with_thought(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        thought_title: &str,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        cancel: CancellationToken,
    ) -> LnaResult<(String, Usage)> {
        let mut seq: Option<i32> = None;
        let mut full_thought = String::new();
        let mut pending = String::new();
        let mut last_flush = std::time::Instant::now();
        let res = self
            .llm
            .complete_text_with_thinking(profile, req, cancel, |delta| {
                if delta.is_empty() {
                    return;
                }
                if seq.is_none() {
                    let s = self.append_thought(
                        artifact_id,
                        notebook_id,
                        "reasoning",
                        thought_title,
                        "",
                    );
                    if s > 0 {
                        seq = Some(s);
                    }
                }
                full_thought.push_str(delta);
                pending.push_str(delta);
                if last_flush.elapsed() >= std::time::Duration::from_millis(150)
                    || pending.len() >= 80
                {
                    if let Some(s) = seq {
                        self.emit_thought_delta(artifact_id, notebook_id, s, &pending);
                    }
                    pending.clear();
                    last_flush = std::time::Instant::now();
                }
            })
            .await;
        if let Some(s) = seq {
            if !pending.is_empty() {
                self.emit_thought_delta(artifact_id, notebook_id, s, &pending);
            }
            self.update_thought_body(artifact_id, s, &full_thought);
        }
        res
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_structured_with_thought(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        thought_title: &str,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        schema_name: &str,
        schema: &Value,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage)> {
        let mut seq: Option<i32> = None;
        let mut full_thought = String::new();
        let mut pending = String::new();
        let mut last_flush = std::time::Instant::now();
        let res = self
            .llm
            .complete_structured_with_thinking(
                profile,
                req,
                schema_name,
                schema,
                cancel,
                max_repairs,
                |delta| {
                    if delta.is_empty() {
                        return;
                    }
                    if seq.is_none() {
                        let s = self.append_thought(
                            artifact_id,
                            notebook_id,
                            "reasoning",
                            thought_title,
                            "",
                        );
                        if s > 0 {
                            seq = Some(s);
                        }
                    }
                    full_thought.push_str(delta);
                    pending.push_str(delta);
                    if last_flush.elapsed() >= std::time::Duration::from_millis(150)
                        || pending.len() >= 80
                    {
                        if let Some(s) = seq {
                            self.emit_thought_delta(artifact_id, notebook_id, s, &pending);
                        }
                        pending.clear();
                        last_flush = std::time::Instant::now();
                    }
                },
                |status_msg| {
                    self.append_thought(
                        artifact_id,
                        notebook_id,
                        "stage",
                        "结构化输出自动修复",
                        status_msg,
                    );
                },
            )
            .await;
        if let Some(s) = seq {
            if !pending.is_empty() {
                self.emit_thought_delta(artifact_id, notebook_id, s, &pending);
            }
            self.update_thought_body(artifact_id, s, &full_thought);
        }
        res
    }

    fn load_cached_source_digests(
        &self,
        source_ids: &[String],
        requested_language: &str,
        limit: usize,
    ) -> LnaResult<Vec<(String, String, String)>> {
        let ids_json = serde_json::to_string(source_ids).unwrap_or_else(|_| "[]".into());
        let preferred_lang = if requested_language.to_lowercase().starts_with("en") {
            "en"
        } else {
            "zh"
        };
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.title, d.section_key, d.digest_md
                     FROM source_digests d
                     JOIN sources s ON s.id = d.source_id
                     WHERE d.source_id IN (SELECT value FROM json_each(?1))
                     ORDER BY CASE WHEN d.language = ?2 THEN 0 ELSE 1 END, d.rowid ASC
                     LIMIT ?3",
                )
                .map_err(map_sql_err)?;
            let mapped = stmt
                .query_map(params![ids_json, preferred_lang, limit as i64], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row.map_err(map_sql_err)?);
            }
            Ok(out)
        })
    }

    /// On-demand reading loop for large sources: gives the AI a compact structural catalog
    /// (source guides + index outline) and 3 read tools (`search_sources`, `read_index_node`,
    /// `read_source_pages`) so it fetches only the specific chapters/passages needed instead of
    /// stuffing an entire book into one prompt.
    async fn gather_evidence_on_demand(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        topic_query: Option<&str>,
        token_budget: usize,
        source_ids_override: Option<&[String]>,
        cancel: CancellationToken,
    ) -> LnaResult<Option<String>> {
        let is_mindmap = art.artifact_type == ArtifactType::Mindmap;
        let effective_source_ids = source_ids_override.unwrap_or(&art.source_ids);
        let selected_json = serde_json::to_string(effective_source_ids).unwrap_or_else(|_| "[]".into());
        let sources_meta: Vec<(String, String, Option<String>, String, i64)> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, title, guide_summary_md, COALESCE(guide_topics_json, '[]'), COALESCE(token_count, 0)
                     FROM sources WHERE id IN (SELECT value FROM json_each(?1)) AND status='READY'
                     ORDER BY title",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([&selected_json], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        if sources_meta.is_empty() {
            return Ok(None);
        }

        let mut catalog_lines: Vec<String> = Vec::new();
        let mut collected: Vec<String> = Vec::new();
        let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut used_chars: usize = 0;
        let char_budget = token_budget.saturating_mul(3).clamp(16_000, 64_000);

        for (sid, stitle, guide_md, topics_json, tok) in &sources_meta {
            let topics: Vec<String> = serde_json::from_str(topics_json).unwrap_or_default();
            catalog_lines.push(format!(
                "SOURCE id={sid} | title=\"{stitle}\" | ~{tok} tokens{}",
                if topics.is_empty() {
                    String::new()
                } else {
                    format!(" | topics: {}", topics.join(", "))
                }
            ));
            if let Some(gmd) = guide_md.as_deref().filter(|s| !s.trim().is_empty()) {
                catalog_lines.push(format!("  Overview: {}", truncate_str(gmd, 420)));
            }
            let idx_limit: i64 = if is_mindmap { 160 } else { 60 };
            let idx_nodes: Vec<(String, i32, String, String, Option<i32>, Option<i32>)> =
                self.db.read(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT id, level, title, summary, page_start, page_end
                             FROM source_index_nodes WHERE source_id = ?1
                             ORDER BY ordinal ASC LIMIT ?2",
                        )
                        .map_err(map_sql_err)?;
                    let rows = stmt
                        .query_map(params![sid, idx_limit], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i32>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?,
                                r.get::<_, Option<i32>>(4)?,
                                r.get::<_, Option<i32>>(5)?,
                            ))
                        })
                        .map_err(map_sql_err)?;
                    let mut out = Vec::new();
                    for r in rows {
                        out.push(r.map_err(map_sql_err)?);
                    }
                    Ok(out)
                })?;
            for (nid, lvl, ntitle, nsum, ps, pe) in idx_nodes {
                let pg = match (ps, pe) {
                    (Some(a), Some(b)) if b > a => format!(" p.{a}-{b}"),
                    (Some(a), _) => format!(" p.{a}"),
                    _ => String::new(),
                };
                catalog_lines.push(format!(
                    "  - node_id={nid} [L{lvl}] {ntitle}{pg}: {}",
                    truncate_str(&nsum, 160)
                ));
            }
        }
        let catalog_cap = if is_mindmap { 28_000 } else { 18_000 };
        let catalog_text = truncate_str(&catalog_lines.join("\n"), catalog_cap).to_string();

        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            "按需查阅目录与章节",
            "正在浏览来源目录与结构大纲，按需选择要精读的章节与片段（避免一次性传入整本书）…",
        );

        let tools = vec![
            ToolDef {
                name: "search_sources".into(),
                description: "Search selected sources for specific concepts, chapters, formulas, or keywords and return top matching excerpts.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "queries": { "type": "array", "items": { "type": "string" }, "description": "1 to 3 focused search queries" },
                        "limit": { "type": "integer", "description": "Max excerpts to return (default 8, max 12)" }
                    },
                    "required": ["queries"]
                }),
            },
            ToolDef {
                name: "read_index_node".into(),
                description: "Read excerpts from a specific outline node_id listed in the source catalog.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "The node_id from the catalog" },
                        "max_chunks": { "type": "integer", "description": "Max chunks to read (default 6, max 10)" }
                    },
                    "required": ["node_id"]
                }),
            },
            ToolDef {
                name: "read_source_pages".into(),
                description: "Read excerpts from a specific page range of a source_id.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "source_id": { "type": "string" },
                        "page_start": { "type": "integer" },
                        "page_end": { "type": "integer" }
                    },
                    "required": ["source_id", "page_start", "page_end"]
                }),
            },
        ];

        let planner_instruction = if is_mindmap {
            "Use the reading tools to inspect the table of contents (e.g., early pages via read_source_pages) if the outline is coarse, and read key sections across ALL major chapters/themes evenly (call multiple tools in parallel) so every branch has concrete sub-topics, definitions, theorems, formulas, and details. Do NOT reply \"DONE\" until all major chapters/themes in scope are covered at uniform depth."
        } else {
            "Use the reading tools to fetch ONLY the specific sections or passages needed for this artifact. Call multiple tools in parallel if helpful. Once you have enough material (or if the catalog outline alone is sufficient), reply with \"DONE\" without further tool calls."
        };
        let mut messages = vec![LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(format!(
                "Artifact type: {}\nUser config: {}\nUser topic/focus: {}\n\nSource Catalog & Outline:\n{}\n\n{}",
                art.artifact_type.as_str(),
                art.config,
                topic_query.unwrap_or("Full selected scope (balanced coverage)"),
                catalog_text,
                planner_instruction
            ))],
        }];

        fn push_evidence(
            seen_keys: &mut std::collections::HashSet<String>,
            collected: &mut Vec<String>,
            used_chars: &mut usize,
            char_budget: usize,
            key: String,
            header: String,
            body: String,
        ) -> bool {
            if !seen_keys.insert(key) {
                return false;
            }
            let snippet = truncate_str(&body, 1_400);
            if *used_chars + snippet.len() > char_budget {
                return false;
            }
            *used_chars += snippet.len() + header.len() + 16;
            collected.push(format!("[{}] ({}) {}", collected.len() + 1, header, snippet));
            true
        }

        let max_rounds: usize = if art.artifact_type == ArtifactType::Slides {
            6
        } else if is_mindmap {
            5
        } else {
            3
        };
        let planner_system = if is_mindmap {
            "You are a thorough curriculum and knowledge-structure planner for Local Notebook Studio. Your job is to inspect the source structure and gather balanced, concrete details (sub-topics, definitions, theorems, formulas, mechanisms, and examples) across EVERY major chapter or section in the requested scope using the reading tools. Call multiple tools in parallel across different chapters so no chapter is left shallow or skipped. Only reply DONE once every major branch has sufficient concrete detail to build a deep, evenly balanced hierarchy."
        } else {
            "You are a frugal research planner for Local Notebook Studio. Your job is to read ONLY the relevant sections on demand using tools so token usage stays low. Never request the entire book; target the requested topic/scope or sample key sections evenly. Stop as soon as you have enough evidence by replying DONE."
        };
        for round in 0..max_rounds {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let req = LlmRequest {
                system: planner_system.into(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_mode: ToolMode::Auto,
                output: OutputMode::Text,
                max_output_tokens: 900,
                reasoning: ReasoningOptions {
                    show_summary: profile.show_thinking,
                    effort: profile.effort.clone(),
                },
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let mut calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            let mut round_usage = Usage::default();
            let mut thought_seq: Option<i32> = None;
            let mut full_thought = String::new();
            let mut pending_thought = String::new();
            let mut last_flush = std::time::Instant::now();
            let thought_title = format!("按需资料规划与思考（第 {} 轮）", round + 1);
            let stream_res = self
                .llm
                .stream(resolved, &req, cancel.clone(), |ev| match ev {
                    LlmEvent::ThoughtDelta(delta) => {
                        if !delta.is_empty() {
                            if thought_seq.is_none() {
                                let s = self.append_thought(
                                    artifact_id,
                                    &art.notebook_id,
                                    "reasoning",
                                    &thought_title,
                                    "",
                                );
                                if s > 0 {
                                    thought_seq = Some(s);
                                }
                            }
                            full_thought.push_str(&delta);
                            pending_thought.push_str(&delta);
                            if last_flush.elapsed() >= std::time::Duration::from_millis(150)
                                || pending_thought.len() >= 80
                            {
                                if let Some(s) = thought_seq {
                                    self.emit_thought_delta(artifact_id, &art.notebook_id, s, &pending_thought);
                                }
                                pending_thought.clear();
                                last_flush = std::time::Instant::now();
                            }
                        }
                    }
                    LlmEvent::ToolCallStart { index, id, name } => {
                        calls.insert(index, (id, name, String::new()));
                    }
                    LlmEvent::ToolCallDelta { index, json } => {
                        if let Some(entry) = calls.get_mut(&index) {
                            entry.2.push_str(&json);
                        }
                    }
                    LlmEvent::Usage(u) => {
                        round_usage = u;
                    }
                    _ => {}
                })
                .await;
            if let Some(s) = thought_seq {
                if !pending_thought.is_empty() {
                    self.emit_thought_delta(artifact_id, &art.notebook_id, s, &pending_thought);
                }
                self.update_thought_body(artifact_id, s, &full_thought);
            }
            drop(_permits);

            if stream_res.is_err() {
                break;
            }
            let _ = lna_storage::record_usage(
                &self.db,
                Some(&art.notebook_id),
                Some(&profile.id),
                "studio_on_demand_read",
                Some(artifact_id),
                round_usage.input_tokens as i64,
                round_usage.output_tokens as i64,
                round_usage.cached_tokens as i64,
                false,
            );
            if calls.is_empty() {
                break;
            }

            let mut assistant_parts = Vec::new();
            let mut tool_results = Vec::new();
            for (_idx, (call_id, name, raw_args)) in calls {
                let args: Value = serde_json::from_str(&raw_args).unwrap_or_else(|_| json!({}));
                assistant_parts.push(Part::ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: args.clone(),
                });

                let mut added_here = 0usize;
                let mut summary_note = String::new();
                match name.as_str() {
                    "search_sources" => {
                        let queries: Vec<String> = args
                            .get("queries")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .take(3)
                                    .map(String::from)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let limit = args
                            .get("limit")
                            .and_then(Value::as_u64)
                            .unwrap_or(8)
                            .clamp(2, 12) as usize;
                        if !queries.is_empty() {
                            if let Ok(res) = self
                                .retrieval
                                .search(&art.notebook_id, &queries, &art.source_ids, 6_000, limit)
                                .await
                            {
                                for ev in res.evidences {
                                    let pg = ev
                                        .page_start
                                        .map(|p| format!(", p.{p}"))
                                        .unwrap_or_default();
                                    let key = format!("chunk:{}", ev.chunk_id);
                                    if push_evidence(
                                        &mut seen_keys, &mut collected, &mut used_chars, char_budget,
                                        key,
                                        format!("{}{}", ev.source_title, pg),
                                        ev.content,
                                    ) {
                                        added_here += 1;
                                    }
                                }
                            }
                            summary_note = format!(
                                "Loaded {} excerpts for queries {:?}.",
                                added_here, queries
                            );
                        }
                    }
                    "read_index_node" => {
                        let node_id = args.get("node_id").and_then(Value::as_str).unwrap_or("");
                        let max_chunks = args
                            .get("max_chunks")
                            .and_then(Value::as_i64)
                            .unwrap_or(6)
                            .clamp(1, 10);
                        if !node_id.is_empty() {
                            let rows: Vec<(i64, String, Option<i32>, String)> = self
                                .db
                                .read(|conn| {
                                    let node_meta: Option<(String, i64, i64)> = conn
                                        .query_row(
                                            "SELECT source_id, COALESCE(chunk_start, 0), COALESCE(chunk_end, 0)
                                             FROM source_index_nodes WHERE id = ?1",
                                            [node_id],
                                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                                        )
                                        .optional()
                                        .map_err(map_sql_err)?;
                                    let Some((sid, cstart, cend)) = node_meta else {
                                        return Ok(Vec::new());
                                    };
                                    let mut stmt = conn
                                        .prepare(
                                            "SELECT c.id, s.title, c.page_start, c.content
                                             FROM source_chunks c JOIN sources s ON s.id = c.source_id
                                             WHERE c.source_id = ?1 AND c.chunk_index BETWEEN ?2 AND ?3
                                             ORDER BY c.chunk_index ASC LIMIT ?4",
                                        )
                                        .map_err(map_sql_err)?;
                                    let mapped = stmt
                                        .query_map(params![sid, cstart, cend.max(cstart), max_chunks], |r| {
                                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                                        })
                                        .map_err(map_sql_err)?;
                                    let mut out = Vec::new();
                                    for r in mapped {
                                        out.push(r.map_err(map_sql_err)?);
                                    }
                                    Ok(out)
                                })
                                .unwrap_or_default();
                            for (cid, stitle, pg, content) in rows {
                                let pstr = pg.map(|p| format!(", p.{p}")).unwrap_or_default();
                                if push_evidence(
                                        &mut seen_keys, &mut collected, &mut used_chars, char_budget,
                                    format!("chunk:{cid}"),
                                    format!("{stitle}{pstr}"),
                                    content,
                                ) {
                                    added_here += 1;
                                }
                            }
                            summary_note = format!("Loaded {added_here} excerpts for node {node_id}.");
                        }
                    }
                    "read_source_pages" => {
                        let sid = args.get("source_id").and_then(Value::as_str).unwrap_or("");
                        let pstart = args.get("page_start").and_then(Value::as_i64).unwrap_or(1);
                        let pend = args
                            .get("page_end")
                            .and_then(Value::as_i64)
                            .unwrap_or(pstart)
                            .min(pstart + 12);
                        if !sid.is_empty() {
                            let rows: Vec<(i64, String, Option<i32>, String)> = self
                                .db
                                .read(|conn| {
                                    let mut stmt = conn
                                        .prepare(
                                            "SELECT c.id, s.title, c.page_start, c.content
                                             FROM source_chunks c JOIN sources s ON s.id = c.source_id
                                             WHERE c.source_id = ?1 AND c.page_start BETWEEN ?2 AND ?3
                                             ORDER BY c.chunk_index ASC LIMIT 10",
                                        )
                                        .map_err(map_sql_err)?;
                                    let mapped = stmt
                                        .query_map(params![sid, pstart, pend], |r| {
                                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                                        })
                                        .map_err(map_sql_err)?;
                                    let mut out = Vec::new();
                                    for r in mapped {
                                        out.push(r.map_err(map_sql_err)?);
                                    }
                                    Ok(out)
                                })
                                .unwrap_or_default();
                            for (cid, stitle, pg, content) in rows {
                                let pstr = pg.map(|p| format!(", p.{p}")).unwrap_or_default();
                                if push_evidence(
                                        &mut seen_keys, &mut collected, &mut used_chars, char_budget,
                                    format!("chunk:{cid}"),
                                    format!("{stitle}{pstr}"),
                                    content,
                                ) {
                                    added_here += 1;
                                }
                            }
                            summary_note = format!(
                                "Loaded {added_here} excerpts from pages {pstart}-{pend}."
                            );
                        }
                    }
                    _ => {
                        summary_note = "Unsupported tool.".into();
                    }
                }
                let tool_label = match name.as_str() {
                    "search_sources" => {
                        let qs: Vec<&str> = args
                            .get("queries")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        format!("工具调用 · 检索来源：{}", qs.join(" / "))
                    }
                    "read_index_node" => {
                        let nid = args.get("node_id").and_then(Value::as_str).unwrap_or("");
                        format!("工具调用 · 读取目录节点：{}", nid)
                    }
                    "read_source_pages" => {
                        let ps = args.get("page_start").and_then(Value::as_i64).unwrap_or(1);
                        let pe = args.get("page_end").and_then(Value::as_i64).unwrap_or(ps);
                        format!("工具调用 · 精读页码：第 {}-{} 页", ps, pe)
                    }
                    other => format!("工具调用 · {}", other),
                };
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "tool",
                    &tool_label,
                    &format!("{}（累计已读取 {} 条片段，约 {} tokens）", summary_note, collected.len(), used_chars / 3),
                );
                tool_results.push(Part::ToolResult {
                    call_id,
                    content: format!(
                        "{summary_note} Total collected: {} excerpts (~{} tokens). Reply DONE if sufficient.",
                        collected.len(),
                        used_chars / 3
                    ),
                    is_error: false,
                });
            }

            messages.push(LlmMessage {
                role: Role::Assistant,
                parts: assistant_parts,
            });
            messages.push(LlmMessage {
                role: Role::User,
                parts: tool_results,
            });
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &format!("按需精读第 {} 轮", round + 1),
                &format!("已按需读取 {} 条关键片段（约 {} tokens）。", collected.len(), used_chars / 3),
            );
            if used_chars >= char_budget * 85 / 100 {
                break;
            }
        }

        // If the AI didn't call tools (e.g. model lacks tool-calling or catalog was enough),
        // supplement with focused retrieval or evenly sampled key chunks so evidence is never empty.
        if collected.is_empty() {
            if let Some(q) = topic_query {
                if let Ok(res) = self
                    .retrieval
                    .search(&art.notebook_id, &[q.to_string()], &art.source_ids, 10_000, 14)
                    .await
                {
                    for ev in res.evidences {
                        let pg = ev.page_start.map(|p| format!(", p.{p}")).unwrap_or_default();
                        push_evidence(
                                        &mut seen_keys, &mut collected, &mut used_chars, char_budget,
                            format!("chunk:{}", ev.chunk_id),
                            format!("{}{}", ev.source_title, pg),
                            ev.content,
                        );
                    }
                }
            }
            if collected.is_empty() {
                return Ok(None);
            }
        }

        let outline_return_cap = if is_mindmap { 18_000 } else { 8_000 };
        Ok(Some(format!(
            "Source Outline:\n{}\n\nOn-Demand Retrieved Excerpts:\n{}",
            truncate_str(&catalog_text, outline_return_cap),
            collected.join("\n\n")
        )))
    }

    async fn collect_evidence_text(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        cancel: CancellationToken,
    ) -> LnaResult<String> {
        let topic_query = art
            .config
            .get("topic")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let is_kg = art.artifact_type == ArtifactType::Mindmap
            && art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        if is_kg {
            let target_ids: Vec<String> = if let Some(sid) = art
                .config
                .get("source_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                vec![sid.to_string()]
            } else {
                art.source_ids.clone()
            };
            let target_json = serde_json::to_string(&target_ids).unwrap_or_else(|_| "[]".into());
            let target_tokens: i64 = self.db.read(|conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(token_count),0) FROM sources WHERE id IN (SELECT value FROM json_each(?1)) AND status='READY'",
                    [&target_json],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)
            })?;
            let window = resolved.context_window as usize;
            let reserved_output = (resolved.max_output_tokens as usize).min(window / 4);
            let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
            let kg_budget = available_input.saturating_mul(70) / 100;
            if target_tokens.max(0) as usize > kg_budget.min(12_000) {
                if let Ok(Some(on_demand_text)) = self
                    .gather_evidence_on_demand(
                        art,
                        profile,
                        resolved,
                        artifact_id,
                        topic_query.as_deref(),
                        kg_budget.min(28_000),
                        Some(&target_ids),
                        cancel.clone(),
                    )
                    .await
                {
                    return Ok(on_demand_text);
                }
            }
            let mut evidence_blocks: Vec<String> = Vec::new();
            let sources_meta: Vec<(String, String, Option<String>, String)> = self.db.read(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, title, guide_summary_md, COALESCE(guide_topics_json, '[]')
                         FROM sources WHERE id IN (SELECT value FROM json_each(?1)) AND status='READY'
                         ORDER BY title",
                    )
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([&target_json], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sql_err)?);
                }
                Ok(out)
            })?;

            for (sid, stitle, guide_md, topics_json) in &sources_meta {
                if let Some(gmd) = guide_md.as_deref().filter(|s| !s.trim().is_empty()) {
                    let topics: Vec<String> = serde_json::from_str(topics_json).unwrap_or_default();
                    let topic_str = if topics.is_empty() {
                        String::new()
                    } else {
                        format!("
Core Topics: {}", topics.join(", "))
                    };
                    evidence_blocks.push(format!(
                        "[{}] {} (Overview)
{}{}",
                        evidence_blocks.len() + 1,
                        stitle,
                        truncate_str(gmd, 900),
                        topic_str
                    ));
                }

                let nodes: Vec<(i32, String, String, String, Option<i32>, Option<i32>)> =
                    self.db.read(|conn| {
                        let mut stmt = conn
                            .prepare(
                                "SELECT level, title, summary, COALESCE(keywords_json, '[]'), page_start, page_end
                                 FROM source_index_nodes WHERE source_id = ?1
                                 ORDER BY ordinal ASC LIMIT 160",
                            )
                            .map_err(map_sql_err)?;
                        let rows = stmt
                            .query_map([sid], |r| {
                                Ok((
                                    r.get::<_, i32>(0)?,
                                    r.get::<_, String>(1)?,
                                    r.get::<_, String>(2)?,
                                    r.get::<_, String>(3)?,
                                    r.get::<_, Option<i32>>(4)?,
                                    r.get::<_, Option<i32>>(5)?,
                                ))
                            })
                            .map_err(map_sql_err)?;
                        let mut out = Vec::new();
                        for row in rows {
                            out.push(row.map_err(map_sql_err)?);
                        }
                        Ok(out)
                    })?;

                let few_nodes = nodes.len() < 25;
                if !nodes.is_empty() {
                    for (lvl, ntitle, summary, kw_json, pstart, pend) in nodes {
                        let kws: Vec<String> = serde_json::from_str(&kw_json).unwrap_or_default();
                        let pg = match (pstart, pend) {
                            (Some(a), Some(b)) if b > a => format!(" (p.{a}-{b})"),
                            (Some(a), _) => format!(" (p.{a})"),
                            _ => String::new(),
                        };
                        let kw_part = if kws.is_empty() {
                            String::new()
                        } else {
                            format!(" | Keywords: {}", kws.join(", "))
                        };
                        evidence_blocks.push(format!(
                            "[{}] {} · L{} {}{}: {}{}",
                            evidence_blocks.len() + 1,
                            stitle,
                            lvl,
                            ntitle,
                            pg,
                            truncate_str(&summary, 280),
                            kw_part
                        ));
                    }
                }
                if few_nodes {
                    // Fallback when no AI index exists: sample up to 16 evenly spaced chunks (never full book)
                    let chunks: Vec<(Option<i32>, Option<String>, String)> = self.db.read(|conn| {
                        let mut stmt = conn
                            .prepare(
                                "SELECT page_start, section_path, content FROM source_chunks
                                 WHERE source_id = ?1 ORDER BY chunk_index ASC",
                            )
                            .map_err(map_sql_err)?;
                        let rows = stmt
                            .query_map([sid], |r| {
                                Ok((
                                    r.get::<_, Option<i32>>(0)?,
                                    r.get::<_, Option<String>>(1)?,
                                    r.get::<_, String>(2)?,
                                ))
                            })
                            .map_err(map_sql_err)?;
                        let mut out = Vec::new();
                        for row in rows {
                            out.push(row.map_err(map_sql_err)?);
                        }
                        Ok(out)
                    })?;
                    let step = (chunks.len() / 16).max(1);
                    for (ps, sec, content) in chunks.into_iter().step_by(step).take(16) {
                        let pg = ps.map(|p| format!(" (p.{p})")).unwrap_or_default();
                        let sec_s = sec.filter(|s| !s.is_empty()).map(|s| format!(" · {s}")).unwrap_or_default();
                        evidence_blocks.push(format!(
                            "[{}] {}{}{}
{}",
                            evidence_blocks.len() + 1,
                            stitle,
                            sec_s,
                            pg,
                            truncate_str(&content, 480)
                        ));
                    }
                }
            }
            if !evidence_blocks.is_empty() {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    "Collecting structural outline for Knowledge Graph",
                    &format!("Prepared {} compact structural items without sending full book text.", evidence_blocks.len()),
                );
                return Ok(evidence_blocks.join("

"));
            }
        }

        let selected_json = serde_json::to_string(&art.source_ids).unwrap_or_else(|_| "[]".into());
        let total_source_tokens: i64 = self.db.read(|conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(token_count),0) FROM sources WHERE id IN (SELECT value FROM json_each(?1)) AND status='READY'",
                [&selected_json],
                |r| r.get(0),
            ).map_err(map_sql_err)
        })?;
        let window = resolved.context_window as usize;
        let reserved_output = (resolved.max_output_tokens as usize).min(window / 4);
        let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
        let evidence_budget = available_input.saturating_mul(70) / 100;
        // Only pass full source text directly for compact sources (<= 12,000 tokens).
        // Larger sources use on-demand AI reading so we never dump an entire book at once.
        let direct_full_cap = evidence_budget.min(12_000);
        if total_source_tokens.max(0) as usize <= direct_full_cap {
            let rows: Vec<(String, Option<i32>, Option<String>, String)> =
                self.db.read(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT s.title, c.page_start, c.section_path, c.content
                     FROM source_chunks c JOIN sources s ON s.id=c.source_id
                     WHERE c.source_id IN (SELECT value FROM json_each(?1))
                     ORDER BY s.title, c.chunk_index",
                        )
                        .map_err(map_sql_err)?;
                    let mapped = stmt
                        .query_map([&selected_json], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, Option<i32>>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, String>(3)?,
                            ))
                        })
                        .map_err(map_sql_err)?;
                    let mut out = Vec::new();
                    for row in mapped {
                        out.push(row.map_err(map_sql_err)?);
                    }
                    Ok(out)
                })?;
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "整理来源正文",
                &format!("共加载 {} 个来源正文片段（可直接放入模型上下文）。", rows.len()),
            );
            return Ok(rows
                .iter()
                .enumerate()
                .map(|(idx, (title, page, section, content))| {
                    let page_label = page.map(|p| format!(", 第 {p} 页")).unwrap_or_default();
                    let section_label = section
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|s| format!(", {s}"))
                        .unwrap_or_default();
                    format!(
                        "[{}] ({}{}{}) {}",
                        idx + 1,
                        title,
                        page_label,
                        section_label,
                        content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"));
        }

        // On-demand AI reading: present compact source catalog & outline, let the AI call
        // search_sources / read_index_node / read_source_pages only for sections it needs.
        if let Ok(Some(on_demand_text)) = self
            .gather_evidence_on_demand(
                art,
                profile,
                resolved,
                artifact_id,
                topic_query.as_deref(),
                evidence_budget.min(if matches!(art.artifact_type, ArtifactType::Slides | ArtifactType::Mindmap) { 28_000 } else { 18_000 }),
                None,
                cancel.clone(),
            )
            .await
        {
            return Ok(on_demand_text);
        }

        let language = art
            .config
            .get("lang")
            .and_then(|v| v.as_str())
            .unwrap_or("zh-CN");
        if let Some(query) = topic_query.as_ref() {
            let related = self
                .retrieval
                .search(
                    &art.notebook_id,
                    std::slice::from_ref(query),
                    &art.source_ids,
                    evidence_budget.saturating_mul(65) / 100,
                    20,
                )
                .await?;
            let related_text = if !related.evidences.is_empty() {
                related
                    .evidences
                    .iter()
                    .map(|e| format!("[{}] ({}) {}", e.citation_no, e.source_title, e.content))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            } else {
                let fallback_rows: Vec<(String, String)> = self.db.read(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT s.title, c.content
                             FROM source_chunks c JOIN sources s ON s.id=c.source_id
                             WHERE c.source_id IN (SELECT value FROM json_each(?1))
                             ORDER BY s.title, c.chunk_index
                             LIMIT 12",
                        )
                        .map_err(map_sql_err)?;
                    let mapped = stmt
                        .query_map([&selected_json], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(map_sql_err)?;
                    let mut out = Vec::new();
                    for r in mapped {
                        out.push(r.map_err(map_sql_err)?);
                    }
                    Ok(out)
                })?;
                fallback_rows
                    .iter()
                    .enumerate()
                    .map(|(i, (title, content))| format!("[{}] ({}) {}", i + 1, title, content))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let cached_digests =
                self.load_cached_source_digests(&art.source_ids, language, 16)?;
            let digest_text = cached_digests
                .iter()
                .map(|(title, section, digest)| format!("### {title} / {section}\n{digest}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "按主题快速检索资料",
                &format!(
                    "已针对主题「{}」直接检索 {} 条相关片段（快速检索模式，避免对全书逐章重做摘要卡顿）。",
                    query,
                    related.evidences.len()
                ),
            );
            if digest_text.is_empty() {
                return Ok(format!("主题相关证据：\n{related_text}"));
            }
            return Ok(format!(
                "主题相关证据：\n{related_text}\n\n章节摘要：\n{digest_text}"
            ));
        }

        let digests = self
            .get_or_create_source_digests(
                &art.notebook_id,
                artifact_id,
                &art.source_ids,
                language,
                &profile.id,
                resolved,
                cancel.clone(),
            )
            .await?;
        let digest_entries: Vec<String> = digests
            .iter()
            .map(|(title, section, digest)| format!("### {title} / {section}\n{digest}"))
            .collect();
        let digest_text = digest_entries
            .iter()
            .take(24)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");

        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            "合并全局章节大纲",
            &format!("已整理 {} 组章节摘要，正在提炼全局结构与关键细节…", digest_entries.len()),
        );

        let mut reduced_batches = Vec::new();
        for (batch_index, batch) in digest_entries.chunks(20).enumerate() {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let batch_text = batch.join("\n\n");
            if digest_entries.len() <= 20 {
                reduced_batches.push(batch_text);
                continue;
            }
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "tool",
                &format!("工具调用 · 归约章节摘要（第 {} 批）", batch_index + 1),
                &format!("正在合并本批次 {} 组章节摘要…", batch.len()),
            );
            let batch_req = LlmRequest {
                system: "You are a document structure synthesis assistant. Merge this batch of section summaries while preserving hierarchy and key conclusions, without adding facts absent from the summaries. Keep the same language as the summaries.".into(),
                messages: vec![LlmMessage { role: Role::User, parts: vec![Part::Text(batch_text)] }],
                tools: vec![], tool_mode: ToolMode::None, output: OutputMode::Text,
                max_output_tokens: 1200,
                reasoning: ReasoningOptions {
                    show_summary: profile.show_thinking,
                    effort: profile.effort.clone(),
                },
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let (batch_summary, batch_usage) = self
                .run_text_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &format!("章节归约思考（第 {} 批）", batch_index + 1),
                    resolved,
                    &batch_req,
                    cancel.clone(),
                )
                .await?;
            let _ = lna_storage::record_usage(
                &self.db,
                Some(&art.notebook_id),
                Some(&profile.id),
                "studio_digest_reduce_batch",
                Some(artifact_id),
                batch_usage.input_tokens as i64,
                batch_usage.output_tokens as i64,
                batch_usage.cached_tokens as i64,
                false,
            );
            reduced_batches.push(format!(
                "## 摘要批次 {}\n{}",
                batch_index + 1,
                batch_summary
            ));
        }
        let reduced_text = reduced_batches.join("\n\n");
        let reduce_req = LlmRequest {
            system: "You are a document structure synthesis assistant. Merge the section summaries into a cohesive global outline preserving hierarchy and key conclusions, without adding facts absent from the summaries. Keep the same language as the summaries.".into(),
            messages: vec![LlmMessage { role: Role::User, parts: vec![Part::Text(reduced_text)] }],
            tools: vec![], tool_mode: ToolMode::None, output: OutputMode::Text,
            max_output_tokens: 2400,
            reasoning: ReasoningOptions {
                show_summary: profile.show_thinking,
                effort: profile.effort.clone(),
            },
        };
        let _permits = self.acquire_llm_slots(&profile.id).await?;
        let (outline, reduce_usage) = self
            .run_text_with_thought(
                artifact_id,
                &art.notebook_id,
                "全局章节结构提炼思考",
                resolved,
                &reduce_req,
                cancel.clone(),
            )
            .await?;
        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&profile.id),
            "studio_digest_reduce",
            Some(artifact_id),
            reduce_usage.input_tokens as i64,
            reduce_usage.output_tokens as i64,
            reduce_usage.cached_tokens as i64,
            false,
        );
        if !digests.is_empty() {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "tool",
                "工具调用 · 按章节检索关键原文片段",
                &format!("正在为前 {} 个核心章节检索高相关证据片段…", digests.len().min(12)),
            );
        }
        let mut details = Vec::new();
        let mut next_citation = 1usize;
        for (title, section, _) in digests.iter().take(12) {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let section_query = format!("{title} {section}");
            if let Ok(found) = self
                .retrieval
                .search(&art.notebook_id, &[section_query], &art.source_ids, 900, 2)
                .await
            {
                for ev in found.evidences {
                    details.push(format!(
                        "[{}] ({}) {}",
                        next_citation, ev.source_title, ev.content
                    ));
                    next_citation += 1;
                }
            }
        }
        Ok(format!(
            "全局章节大纲：\n{outline}\n\n章节摘要：\n{digest_text}\n\n按章节检索的细节：\n{}",
            details.join("\n\n")
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_or_create_source_digests(
        &self,
        notebook_id: &str,
        artifact_id: &str,
        source_ids: &[String],
        requested_language: &str,
        profile_id: &str,
        profile: &ResolvedProfile,
        cancel: CancellationToken,
    ) -> LnaResult<Vec<(String, String, String)>> {
        let ids_json = serde_json::to_string(source_ids).unwrap_or_else(|_| "[]".into());
        let rows: Vec<ChunkRow> =
            self.db.read(|conn| {
                let mut stmt = conn.prepare(
                "SELECT s.id, s.title, s.content_hash, COALESCE(s.language,''), c.chunk_index,
                        c.id, c.content, COALESCE(c.section_path,'')
                 FROM source_chunks c JOIN sources s ON s.id=c.source_id
                 WHERE c.source_id IN (SELECT value FROM json_each(?1))
                 ORDER BY s.id, c.chunk_index"
            ).map_err(map_sql_err)?;
                let mapped = stmt
                    .query_map([&ids_json], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i32>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, String>(7)?,
                        ))
                    })
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for row in mapped {
                    out.push(row.map_err(map_sql_err)?);
                }
                Ok(out)
            })?;
        let mut source_meta: SourceMeta = HashMap::new();
        let mut groups: BTreeMap<(String, String), Vec<(i64, String)>> = BTreeMap::new();
        for (
            source_id,
            title,
            content_hash,
            source_language,
            chunk_index,
            chunk_id,
            content,
            section_path,
        ) in rows
        {
            let language = if requested_language.to_lowercase().starts_with("en") {
                "en".to_string()
            } else if requested_language.to_lowercase().starts_with("zh")
                || source_language.is_empty()
            {
                "zh".to_string()
            } else {
                source_language
            };
            source_meta.insert(source_id.clone(), (title, content_hash, language));
            let section = if section_path.trim().is_empty() {
                "未分节".to_string()
            } else {
                section_path
            };
            let bucket = chunk_index / 8;
            groups
                .entry((source_id, format!("{section}::{bucket}")))
                .or_default()
                .push((chunk_id, content));
        }
        let total_groups = groups.len();
        let mut out = Vec::new();
        let mut uncached_done = 0usize;
        for (idx, ((source_id, section_key), chunks)) in groups.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let Some((title, content_hash, language)) = source_meta.get(&source_id).cloned() else {
                continue;
            };
            let cached: Option<(String, String)> = self.db.read(|conn| {
                conn.query_row(
                    "SELECT content_hash, digest_md FROM source_digests WHERE source_id=?1 AND section_key=?2 AND language=?3",
                    params![source_id, section_key, language],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional().map_err(map_sql_err)
            })?;
            let digest = if let Some((_, cached_digest)) =
                cached.filter(|(hash, _)| hash == &content_hash)
            {
                cached_digest
            } else {
                uncached_done += 1;
                if uncached_done == 1 || uncached_done % 3 == 0 || idx + 1 == total_groups {
                    self.append_thought(
                        artifact_id,
                        notebook_id,
                        "stage",
                        "提取来源章节摘要",
                        &format!(
                            "来源较长，正在按章节分段提取摘要（第 {}/{} 段：{} / {}）…",
                            idx + 1,
                            total_groups,
                            title,
                            section_key
                        ),
                    );
                }
                let chunk_text = chunks
                    .iter()
                    .map(|(id, content)| format!("[chunk {id}] {content}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let req = LlmRequest {
                    system: format!("You are a section summarization assistant. Summarize the following content from the same section in {}, preserving key concepts, equations, relationships, and conclusions without adding any facts not present in the source.", language),
                    messages: vec![LlmMessage { role: Role::User, parts: vec![Part::Text(chunk_text)] }],
                    tools: vec![], tool_mode: ToolMode::None, output: OutputMode::Text,
                    max_output_tokens: 700,
                    reasoning: ReasoningOptions {
                        show_summary: profile.show_thinking,
                        effort: profile.effort.clone(),
                    },
                };
                let _permits = self.acquire_llm_slots(profile_id).await?;
                let (digest, usage) = self
                    .run_text_with_thought(
                        artifact_id,
                        notebook_id,
                        &format!("章节摘要思考：{title} / {section_key}"),
                        profile,
                        &req,
                        cancel.clone(),
                    )
                    .await?;
                let digest = digest.trim().to_string();
                let chunk_ids: Vec<i64> = chunks.iter().map(|(id, _)| *id).collect();
                let chunk_ids_json =
                    serde_json::to_string(&chunk_ids).unwrap_or_else(|_| "[]".into());
                let token_count: i64 = chunks
                    .iter()
                    .map(|(_, text)| (text.chars().count() as i64 + 3) / 4)
                    .sum();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT INTO source_digests (source_id, section_key, content_hash, language, digest_md, chunk_ids_json, token_count, created_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                         ON CONFLICT(source_id,section_key,language) DO UPDATE SET content_hash=excluded.content_hash,
                         digest_md=excluded.digest_md, chunk_ids_json=excluded.chunk_ids_json, token_count=excluded.token_count, created_at=excluded.created_at",
                        params![source_id, section_key, content_hash, language, digest, chunk_ids_json, token_count, now_ms()],
                    ).map_err(map_sql_err)?;
                    Ok(())
                })?;
                let _ = lna_storage::record_usage(
                    &self.db,
                    Some(notebook_id),
                    Some(&profile.id),
                    "studio_digest_map",
                    Some(&source_id),
                    usage.input_tokens as i64,
                    usage.output_tokens as i64,
                    usage.cached_tokens as i64,
                    false,
                );
                digest
            };
            out.push((title, section_key, digest));
        }
        Ok(out)
    }

    fn set_stage(
        &self,
        id: &str,
        status: &str,
        stage: &str,
        progress: f64,
        _topic: &str,
    ) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status=?1, stage=?2, progress=?3, updated_at=?4
                 WHERE id=?5 AND status != 'CANCELLED'",
                params![status, stage, progress, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit_status_event(id, status, stage, progress, None, None);
        Ok(())
    }

    fn emit_status_event(
        &self,
        id: &str,
        status: &str,
        stage: &str,
        progress: f64,
        error_code: Option<&str>,
        error_detail: Option<&str>,
    ) {
        let meta: Option<(String, bool, Option<String>)> = self
            .db
            .read(|conn| {
                conn.query_row(
                    "SELECT notebook_id, in_studio, origin_message_id FROM studio_artifacts WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
                )
                .optional()
                .map_err(map_sql_err)
            })
            .ok()
            .flatten();

        let Some((nid, in_studio, origin_message_id)) = meta else {
            return;
        };
        let mut payload = json!({
            "artifact_id": id,
            "status": status,
            "stage": stage,
            "progress": progress,
            "in_studio": in_studio,
            "origin_message_id": origin_message_id,
        });
        if let Some(ec) = error_code {
            payload["error_code"] = json!(ec);
        }
        if let Some(ed) = error_detail {
            payload["error_detail"] = json!(ed);
        }
        self.emit(&format!("studio:{nid}"), "studio.status", payload);

        if let Some(ref mid) = origin_message_id {
            if let Ok(mut art) = self.get(id) {
                art.thoughts.clear();
                self.emit(
                    &format!("chat:{nid}"),
                    "chat.artifact",
                    json!({ "message_id": mid, "artifact": art }),
                );
            }
        }
    }

    fn current_sources_fingerprint(&self, notebook_id: &str) -> LnaResult<(Vec<String>, String)> {
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content_hash FROM sources
                     WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY'
                     ORDER BY content_hash ASC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([notebook_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(map_sql_err)?;
            let mut ids = Vec::new();
            let mut hasher = Sha256::new();
            for r in rows {
                let (id, h) = r.map_err(map_sql_err)?;
                ids.push(id);
                hasher.update(h.as_bytes());
            }
            Ok((ids, hex::encode(hasher.finalize())))
        })
    }
}

fn is_non_retryable_config_error(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::NoSourceSelected
            | ErrorCode::ModelNotConfigured
            | ErrorCode::ModelAuthFailed
            | ErrorCode::ModelNotFound
            | ErrorCode::StorageFull
            | ErrorCode::KeychainUnavailable
    )
}

fn stage_progress(stage: &str) -> f64 {
    match stage {
        "COLLECT" => 0.15,
        "PLAN" => 0.35,
        "GENERATE" => 0.55,
        "RENDER" => 0.85,
        _ => 0.1,
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn format_error_log(
    art: &StudioArtifact,
    profile: Option<&ModelProfile>,
    records: &[AttemptFailureRecord],
) -> String {
    let mut lines = Vec::new();
    lines.push("=== Local Notebook Studio Diagnostic Log ===".to_string());
    lines.push(format!("TimestampMs: {}", now_ms()));
    lines.push(format!("ArtifactId: {}", art.id));
    lines.push(format!("NotebookId: {}", art.notebook_id));
    lines.push(format!("Type: {}", art.artifact_type.as_str()));
    lines.push(format!("Title: {}", art.title));
    lines.push(format!("InStudio: {}", art.in_studio));
    if let Some(ref mid) = art.origin_message_id {
        lines.push(format!("OriginMessageId: {mid}"));
    }
    lines.push(format!("SourceCount: {}", art.source_ids.len()));
    lines.push(format!("Config: {}", art.config));
    if let Some(p) = profile {
        lines.push(format!(
            "Profile: id={} name={} protocol={:?} model={} base_url={} window={} max_output={} show_thinking={} effort={:?} cap_json_schema={} cap_reasoning={}",
            p.id,
            p.display_name,
            p.protocol,
            p.model_id,
            p.base_url,
            p.context_window,
            p.max_output_tokens,
            p.show_thinking,
            p.effort,
            p.capabilities.json_schema,
            p.capabilities.reasoning_summary
        ));
    } else {
        lines.push("Profile: <none configured>".to_string());
    }
    lines.push(String::new());
    lines.push(format!("Attempts ({} recorded):", records.len()));
    for rec in records {
        lines.push(format!(
            "  - Attempt #{} | stage={} | elapsed={}ms | code={} | retryable={} | error={}",
            rec.attempt, rec.stage, rec.elapsed_ms, rec.code, rec.retryable, rec.message
        ));
    }
    lines.join("\n")
}

fn map_artifact_row(r: &rusqlite::Row<'_>, _cur_fp: &str) -> rusqlite::Result<StudioArtifact> {
    let t_s: String = r.get(2)?;
    let artifact_type = match t_s.as_str() {
        "audio" => ArtifactType::Audio,
        "slides" => ArtifactType::Slides,
        "mindmap" => ArtifactType::Mindmap,
        "report" => ArtifactType::Report,
        "flashcards" => ArtifactType::Flashcards,
        "quiz" => ArtifactType::Quiz,
        _ => ArtifactType::Infographic,
    };
    let st_s: String = r.get(4)?;
    let status = match st_s.as_str() {
        "PENDING" => ArtifactStatus::Pending,
        "RUNNING" => ArtifactStatus::Running,
        "DRAFT_REVIEW" => ArtifactStatus::DraftReview,
        "READY" => ArtifactStatus::Ready,
        "CANCELLED" => ArtifactStatus::Cancelled,
        _ => ArtifactStatus::Failed,
    };
    let cfg_s: String = r.get(7)?;
    let sids_s: String = r.get(8)?;
    let fp: String = r.get(9)?;
    let draft_s: Option<String> = r.get(11)?;
    let payload_s: Option<String> = r.get(12)?;
    let in_studio: i64 = r.get(22)?;
    let origin_message_id: Option<String> = r.get(23)?;
    let error_log: Option<String> = r.get(24)?;
    let mut payload: Option<Value> = payload_s.and_then(|s| serde_json::from_str(&s).ok());
    if artifact_type == ArtifactType::Mindmap {
        // Node labels are drawn natively on every client; turn TeX into Unicode math here.
        if let Some(p) = payload.as_mut() {
            texfmt::convert_mindmap_payload(p);
        }
    }

    Ok(StudioArtifact {
        id: r.get(0)?,
        notebook_id: r.get(1)?,
        artifact_type,
        title: r.get(3)?,
        status,
        stage: r.get(5)?,
        progress: r.get(6)?,
        config: serde_json::from_str(&cfg_s).unwrap_or_else(|_| json!({})),
        source_ids: serde_json::from_str(&sids_s).unwrap_or_default(),
        source_refs: Vec::new(),
        sources_stale: false,
        in_studio: in_studio != 0,
        origin_message_id,
        progress_state: None,
        sources_fingerprint: fp,
        profile_id: r.get(10)?,
        draft: draft_s.and_then(|s| serde_json::from_str(&s).ok()),
        payload,
        output_dir: r.get(13)?,
        estimated_input_tokens: r.get(14)?,
        estimated_output_tokens: r.get(15)?,
        error_code: r.get(16)?,
        error_detail: r.get(17)?,
        error_log,
        thoughts: Vec::new(),
        feedback: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
        finished_at: r.get(21)?,
    })
}

fn normalize_match_text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c) {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
}

fn extract_match_tokens(s: &str) -> std::collections::HashSet<String> {
    let mut tokens = std::collections::HashSet::new();
    let norm = normalize_match_text(s);
    for part in norm.split_whitespace() {
        if part.is_empty() {
            continue;
        }
        let has_cjk = part.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        if has_cjk {
            let chars: Vec<char> = part.chars().collect();
            if chars.len() >= 2 {
                for w in chars.windows(2) {
                    tokens.insert(w.iter().collect());
                }
            } else {
                tokens.insert(part.to_string());
            }
        } else if part.len() >= 2 {
            tokens.insert(part.to_string());
        }
    }
    tokens
}

fn score_kg_node_match(label: &str, summary: &str, quiz_text: &str, topic_hint: &str) -> f64 {
    let clean_label = label.split('[').next().unwrap_or(label).trim();
    if clean_label.is_empty() {
        return 0.0;
    }
    let norm_label = normalize_match_text(clean_label).replace(' ', "");
    let norm_quiz = normalize_match_text(quiz_text).replace(' ', "");
    let norm_topic = normalize_match_text(topic_hint).replace(' ', "");
    if norm_label.chars().count() >= 2 && norm_quiz.contains(&norm_label) {
        return 1.0;
    }
    if norm_label.chars().count() >= 2 && norm_topic.contains(&norm_label) {
        return 0.85;
    }
    let label_tokens = extract_match_tokens(clean_label);
    let summary_tokens = extract_match_tokens(summary);
    let quiz_tokens = extract_match_tokens(quiz_text);
    if label_tokens.is_empty() || quiz_tokens.is_empty() {
        return 0.0;
    }
    let label_hits = label_tokens.iter().filter(|t| quiz_tokens.contains(*t)).count();
    let label_ratio = label_hits as f64 / label_tokens.len() as f64;
    let summary_ratio = if summary_tokens.is_empty() {
        0.0
    } else {
        let hits = summary_tokens.iter().filter(|t| quiz_tokens.contains(*t)).count();
        hits as f64 / summary_tokens.len().min(12) as f64
    };
    (label_ratio * 0.75 + summary_ratio * 0.25).min(0.8)
}
