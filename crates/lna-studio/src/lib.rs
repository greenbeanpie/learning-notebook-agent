pub mod render;
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
            text_generation_slots: Arc::new(Semaphore::new(2)),
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
        let total_source_tokens: i64 = self.db.read(|conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(token_count),0) FROM sources WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY'",
                [notebook_id],
                |r| r.get(0),
            )
            .map_err(map_sql_err)
        })?;
        let active_profile = lna_storage::get_active_profile(&self.db)?;
        let est_input =
            (((total_source_tokens.max(0) as f64) * 1.1).ceil() as i64).saturating_add(600);
        let est_output = match artifact_type {
            ArtifactType::Quiz | ArtifactType::Flashcards => 2500,
            ArtifactType::Report | ArtifactType::Slides | ArtifactType::Audio => 4500,
            ArtifactType::Mindmap | ArtifactType::Infographic => 1800,
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
            "requires_confirmation": est_input > 200_000
        }))
    }

    pub fn create(
        &self,
        notebook_id: &str,
        artifact_type: ArtifactType,
        title: Option<&str>,
        config: Value,
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

        let est = self.estimate(notebook_id, artifact_type)?;
        let req_conf = est
            .get("requires_confirmation")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        let confirmed = config
            .get("confirmed")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if req_conf && !confirmed {
            return Err(LnaError::invalid(
                "Estimated input exceeds 200k tokens; pass config.confirmed=true to proceed",
            ));
        }
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
        let title = title.unwrap_or(default_title);
        let cfg_json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".into());
        let sids_json = serde_json::to_string(&selected_ids).unwrap_or_else(|_| "[]".into());

        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO studio_artifacts
                 (id, notebook_id, artifact_type, title, status, stage, progress, config_json,
                  source_ids_json, sources_fingerprint, profile_id, estimated_input_tokens,
                  estimated_output_tokens, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,'PENDING','QUEUED',0.0,?5,?6,?7,?8,?9,?10,?11,?12)",
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
             FROM studio_artifacts WHERE notebook_id = ?1 AND in_studio = 1 ORDER BY created_at DESC"
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
        Ok(art)
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
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                "规划演示文稿大纲",
                "正在基于已选来源梳理章节结构与每页要点…",
            );
            let plan_req = LlmRequest {
                system: "You are a presentation and content outline planner. Output a clean, human-readable Markdown outline (using `# Title`, `## Slide N: Slide Title`, `- Key points`, and `> Speaker notes`). Match the language requested in config or the primary language of the sources. Be concise and accurate; never output JSON or fenced code blocks.".into(),
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
                    ArtifactType::Mindmap => validate::enforce_mindmap_limits(&mut generated),
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
                        "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array. Match the language requested in config or the primary language of the sources. Keep every prompt, option, and explanation concise and accurate.",
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

        let is_kg = art.artifact_type == ArtifactType::Mindmap
            && art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let extra_kg_hint = if is_kg {
            r#" Set "kind": "knowledge_graph". Build a concept Knowledge Graph structured as a collapsible tree from foundational prerequisites to core theorems and advanced applications. For each node, set "importance" ("core" | "key" | "detail"), a brief 1-sentence "summary", and "prerequisites" (names of prerequisite concepts when applicable)."#
        } else {
            ""
        };
        let gen_req = LlmRequest {
            system: format!(
                "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array.{} Match the language requested in config or the primary language of the sources. Keep content concise, accurate, and well-structured without filler.",
                art.artifact_type.as_str(),
                extra_kg_hint
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
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
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
        if total_source_tokens.max(0) as usize <= evidence_budget {
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
            let batch_req = LlmRequest {
                system: "You are a document structure synthesis assistant. Merge this batch of section summaries while preserving hierarchy and key conclusions, without adding facts absent from the summaries. Keep the same language as the summaries.".into(),
                messages: vec![LlmMessage { role: Role::User, parts: vec![Part::Text(batch_text)] }],
                tools: vec![], tool_mode: ToolMode::None, output: OutputMode::Text,
                max_output_tokens: 1200, reasoning: ReasoningOptions::default(),
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let (batch_summary, batch_usage) = self
                .llm
                .complete_text(resolved, &batch_req, cancel.clone())
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
            max_output_tokens: 2400, reasoning: ReasoningOptions::default(),
        };
        let _permits = self.acquire_llm_slots(&profile.id).await?;
        let (outline, reduce_usage) = self
            .llm
            .complete_text(resolved, &reduce_req, cancel.clone())
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
                    max_output_tokens: 700, reasoning: ReasoningOptions::default(),
                };
                let _permits = self.acquire_llm_slots(profile_id).await?;
                let (digest, usage) = self
                    .llm
                    .complete_text(profile, &req, cancel.clone())
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
        sources_fingerprint: fp,
        profile_id: r.get(10)?,
        draft: draft_s.and_then(|s| serde_json::from_str(&s).ok()),
        payload: payload_s.and_then(|s| serde_json::from_str(&s).ok()),
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
