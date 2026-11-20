pub mod render;

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
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const SCHEMA_QUIZ: &str = include_str!("../../../schemas/quiz.schema.json");
const SCHEMA_FLASHCARDS: &str = include_str!("../../../schemas/flashcards.schema.json");
const SCHEMA_MINDMAP: &str = include_str!("../../../schemas/mindmap.schema.json");
const SCHEMA_REPORT: &str = include_str!("../../../schemas/report.schema.json");
const SCHEMA_SLIDES: &str = include_str!("../../../schemas/slides.schema.json");
const SCHEMA_INFOGRAPHIC: &str = include_str!("../../../schemas/infographic.schema.json");
const SCHEMA_AUDIO: &str = include_str!("../../../schemas/audio.schema.json");

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
        let est_input = ((total_source_tokens.min(40_000) as f64) * 1.1) as i64 + 600;
        let est_output = match artifact_type {
            ArtifactType::Quiz | ArtifactType::Flashcards => 2500,
            ArtifactType::Report | ArtifactType::Slides | ArtifactType::Audio => 4500,
            ArtifactType::Mindmap | ArtifactType::Infographic => 1800,
        };
        Ok(json!({
            "estimated_input_tokens": est_input,
            "estimated_output_tokens": est_output,
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
        let query = art
            .config
            .get("topic")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&art.title)
            .to_string();
        let evidence = self
            .retrieval
            .search(&art.notebook_id, &[query], &art.source_ids, 12_000, 12)
            .await?;
        let evidence_txt: String = evidence
            .evidences
            .iter()
            .map(|e| format!("[{}] ({}) {}", e.citation_no, e.source_title, e.content))
            .collect::<Vec<_>>()
            .join("\n\n");

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

        let (payload, usage) = self
            .llm
            .complete_structured(&resolved, &gen_req, schema_name, &schema, cancel.clone())
            .await?;

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
                    lang: "zh-CN".into(),
                };
                let voice_b = Voice {
                    id: "host_b".into(),
                    name: "Host B".into(),
                    lang: "zh-CN".into(),
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
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='CANCELLED', stage='CANCELLED', updated_at=?1 WHERE id=?2",
                params![now_ms(), id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
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
