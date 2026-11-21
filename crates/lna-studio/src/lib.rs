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

    pub fn list(&self, notebook_id: &str) -> LnaResult<Vec<StudioArtifact>> {
        let (_, cur_fp) = self.current_sources_fingerprint(notebook_id)?;
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, notebook_id, artifact_type, title, status, stage, progress,
                            config_json, source_ids_json, sources_fingerprint, profile_id,
                            draft_json, payload_json, output_dir, estimated_input_tokens,
                            estimated_output_tokens, error_code, error_detail, feedback,
                            created_at, updated_at, finished_at
                     FROM studio_artifacts WHERE notebook_id = ?1 ORDER BY created_at DESC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([notebook_id], |r| map_artifact_row(r, &cur_fp))
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })
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
        self.db.read(|conn| {
            conn.query_row(
                "SELECT id, notebook_id, artifact_type, title, status, stage, progress,
                        config_json, source_ids_json, sources_fingerprint, profile_id,
                        draft_json, payload_json, output_dir, estimated_input_tokens,
                        estimated_output_tokens, error_code, error_detail, feedback,
                        created_at, updated_at, finished_at
                 FROM studio_artifacts WHERE id = ?1",
                [id],
                |r| map_artifact_row(r, &cur_fp),
            )
            .map_err(map_sql_err)
        })
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
        self.jobs
            .enqueue("studio_generate", Some(&art.notebook_id), id, 90)?;
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
        let topic = format!("studio:{}", art.notebook_id);
        let profile = lna_storage::get_active_profile(&self.db)?.ok_or_else(|| {
            LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured")
        })?;
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

        self.set_stage(artifact_id, "RUNNING", "COLLECT", 0.15, &topic)?;
        let saved_collect = self.get_checkpoint(artifact_id, "collect")?;
        let evidence_txt = if let Some(checkpoint) = saved_collect.filter(|value| {
            !art.sources_stale
                && value.get("sources_fingerprint").and_then(Value::as_str)
                    == Some(art.sources_fingerprint.as_str())
        }) {
            checkpoint
                .get("evidence_text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            let text = self
                .collect_evidence_text(&art, &profile, &resolved, artifact_id, cancel.clone())
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
            self.set_stage(artifact_id, "RUNNING", "PLAN", 0.35, &topic)?;
            let plan_req = LlmRequest {
                system: "你是内容大纲规划助手，请根据资料生成简洁的JSON大纲。".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "配置：{}\n资料：\n{}",
                        art.config, evidence_txt
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::JsonObject,
                max_output_tokens: 1500,
                reasoning: ReasoningOptions::default(),
            };
            let _permits = self.acquire_llm_slots(&profile.id).await?;
            let (draft_raw, _) = self.llm.complete_text(&resolved, &plan_req, cancel).await?;
            self.db.write(|conn| {
                conn.execute(
                    "UPDATE studio_artifacts SET status='DRAFT_REVIEW', stage='DRAFT_REVIEW',
                     progress=0.45, draft_json=?1, updated_at=?2 WHERE id=?3",
                    params![draft_raw, now_ms(), artifact_id],
                )
                .map_err(map_sql_err)?;
                Ok(())
            })?;
            self.emit(
                &topic,
                "studio.status",
                json!({ "artifact_id": artifact_id, "status": "DRAFT_REVIEW", "stage": "DRAFT_REVIEW", "progress": 0.45 }),
            );
            return Ok(());
        }

        self.set_stage(artifact_id, "RUNNING", "GENERATE", 0.55, &topic)?;
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

        let gen_req = LlmRequest {
            system: format!(
                "你是 Local Notebook Studio 生成器。请严格依据给定资料生成 {} 的结构化 JSON，并保留引用编号 [n] 到 citations 数组。",
                art.artifact_type.as_str()
            ),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(format!(
                    "生成参数：{}\n参考大纲：{}\n\n资料：\n{}",
                    art.config,
                    art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "无".into()),
                    evidence_txt
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning: ReasoningOptions::default(),
        };

        let saved_generate = self
            .get_checkpoint(artifact_id, "generate")?
            .filter(|value| {
                !art.sources_stale
                    && value.get("sources_fingerprint").and_then(Value::as_str)
                        == Some(art.sources_fingerprint.as_str())
            });
        let (mut payload, usage) =
            if let Some(saved) = saved_generate.and_then(|v| v.get("payload").cloned()) {
                (saved, Usage::default())
            } else {
                let _permits = self.acquire_llm_slots(&profile.id).await?;
                let (generated, usage) = self
                    .llm
                    .complete_structured(&resolved, &gen_req, schema_name, &schema, cancel.clone())
                    .await?;
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

        // Evidence numbers are only stable on the direct path (all chunks numbered 1..n).
        let max_evidence = validate::max_evidence_no(&evidence_txt);
        let evidence_cap =
            (max_evidence > 0 && evidence_txt.starts_with("[1] ")).then_some(max_evidence);
        if let Some(max) = evidence_cap {
            if art.artifact_type != ArtifactType::Quiz {
                validate::sanitize_citations(&mut payload, max);
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
                validate::validate_quiz_with(&payload, &allowed_types, evidence_cap)?;
            }
            ArtifactType::Flashcards => validate::dedup_flashcards(&mut payload),
            ArtifactType::Mindmap => validate::enforce_mindmap_limits(&mut payload),
            _ => {}
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
                 payload_json=?1, output_dir=?2, updated_at=?3, finished_at=?4 WHERE id=?5",
                params![payload_str, out_dir_str, now, now, artifact_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit(
            &topic,
            "studio.status",
            json!({ "artifact_id": artifact_id, "status": "READY", "stage": "READY", "progress": 1.0 }),
        );
        Ok(())
    }

    pub fn mark_failed(&self, id: &str, err: &LnaError) -> LnaResult<()> {
        let nb_id: Option<String> = self.db.read(|conn| {
            conn.query_row(
                "SELECT notebook_id FROM studio_artifacts WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='FAILED', stage='FAILED', error_code=?1, error_detail=?2, updated_at=?3 WHERE id=?4 AND status != 'CANCELLED'",
                params![err.code.as_str(), err.message, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        if let Some(nid) = nb_id {
            self.emit(&format!("studio:{nid}"), "studio.status", json!({
                "artifact_id": id, "status": "FAILED", "error_code": err.code.as_str(), "error_detail": err.message
            }));
        }
        Ok(())
    }

    pub fn mark_cancelled(&self, id: &str) -> LnaResult<()> {
        let nb_id: Option<String> = self.db.read(|conn| {
            conn.query_row(
                "SELECT notebook_id FROM studio_artifacts WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        let changed = self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='CANCELLED', stage='CANCELLED', updated_at=?1
                 WHERE id=?2 AND status IN ('PENDING','RUNNING','DRAFT_REVIEW')",
                params![now_ms(), id],
            )
            .map_err(map_sql_err)
        })?;
        if changed > 0 {
            if let Some(nid) = nb_id {
                self.emit(
                    &format!("studio:{nid}"),
                    "studio.status",
                    json!({ "artifact_id": id, "status": "CANCELLED", "stage": "CANCELLED" }),
                );
            }
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
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='PENDING', stage='QUEUED', progress=0.0, error_code=NULL, error_detail=NULL, updated_at=?1 WHERE id=?2",
                params![now_ms(), id],
            ).map_err(map_sql_err)?;
            Ok(())
        })?;
        self.jobs
            .enqueue("studio_generate", Some(&art.notebook_id), id, 100)?;
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
        let digests = self
            .get_or_create_source_digests(
                &art.notebook_id,
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
        if let Some(query) = topic_query.as_ref() {
            let related = self
                .retrieval
                .search(
                    &art.notebook_id,
                    std::slice::from_ref(query),
                    &art.source_ids,
                    evidence_budget.saturating_mul(55) / 100,
                    12,
                )
                .await?;
            let related_text = related
                .evidences
                .iter()
                .map(|e| format!("[{}] ({}) {}", e.citation_no, e.source_title, e.content))
                .collect::<Vec<_>>()
                .join("\n\n");
            return Ok(format!(
                "主题相关证据：\n{related_text}\n\n章节摘要：\n{digest_text}"
            ));
        }

        let mut reduced_batches = Vec::new();
        for (batch_index, batch) in digest_entries.chunks(20).enumerate() {
            let batch_text = batch.join("\n\n");
            if digest_entries.len() <= 20 {
                reduced_batches.push(batch_text);
                continue;
            }
            let batch_req = LlmRequest {
                system: "你是资料结构整理助手。合并本批章节摘要，保留章节层级与关键结论，不添加摘要中不存在的事实。".into(),
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
            system: "你是资料结构整理助手。根据章节摘要合并出全局大纲，保留章节层级与关键结论，不添加摘要中不存在的事实。".into(),
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

    async fn get_or_create_source_digests(
        &self,
        notebook_id: &str,
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
        let mut out = Vec::new();
        for ((source_id, section_key), chunks) in groups {
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
                let chunk_text = chunks
                    .iter()
                    .map(|(id, content)| format!("[chunk {id}] {content}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let req = LlmRequest {
                    system: format!("你是章节摘要助手。请用{}概括以下同一章节资料，保留关键概念、公式关系与结论，不添加资料中没有的内容。", language),
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
        topic: &str,
    ) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status=?1, stage=?2, progress=?3, updated_at=?4 WHERE id=?5",
                params![status, stage, progress, now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.emit(
            topic,
            "studio.status",
            json!({ "artifact_id": id, "status": status, "stage": stage, "progress": progress }),
        );
        Ok(())
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

fn map_artifact_row(r: &rusqlite::Row<'_>, cur_fp: &str) -> rusqlite::Result<StudioArtifact> {
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
        sources_stale: fp != cur_fp,
        sources_fingerprint: fp,
        profile_id: r.get(10)?,
        draft: draft_s.and_then(|s| serde_json::from_str(&s).ok()),
        payload: payload_s.and_then(|s| serde_json::from_str(&s).ok()),
        output_dir: r.get(13)?,
        estimated_input_tokens: r.get(14)?,
        estimated_output_tokens: r.get(15)?,
        error_code: r.get(16)?,
        error_detail: r.get(17)?,
        feedback: r.get(18)?,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
        finished_at: r.get(21)?,
    })
}
