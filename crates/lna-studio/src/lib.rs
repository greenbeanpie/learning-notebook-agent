pub mod expr;
pub mod figure;
pub mod lecture;
pub(crate) mod material;
pub mod mdlite;
pub mod render;
pub mod texfmt;
pub mod tts;
pub mod validate;

use futures_util::StreamExt as _;
use lna_ingest::{attachments, context_files};
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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, Semaphore};
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

/// Appended to prompts whose output is rendered as Markdown with formulas.
const MATH_LATEX_RULE: &str = " MATH: write EVERY mathematical symbol or expression in LaTeX wrapped in $...$, even a single variable or short term (e.g. $\\mathbb{R}^n$, $A^T$, $x_1$, $N(A)$, $\\dim C(A)=r$, $A\\mathbf{x}=\\mathbf{0}$). Bare ASCII such as R^n, A^T, x1 or Ax=0 outside $...$ is wrong. Double-escape backslashes in JSON strings.";

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
    audio_render_slot: Arc<Semaphore>,
    /// Latest unvalidated preview per artifact while it is generating: `(stage, payload)`.
    previews: Arc<std::sync::Mutex<HashMap<String, (String, Value)>>>,
}

/// Whether a UI language setting means English. "auto"/empty/unknown follows the operating system.
/// Orchestrator side of the executor pattern: waits for the next finished executor, but stays
/// awake for cancellation (the orchestrator is never passively blocked on its executors).
/// `Ok(None)` means every executor has finished.
pub(crate) async fn next_or_cancel<S>(
    running: &mut S,
    cancel: &CancellationToken,
) -> LnaResult<Option<S::Item>>
where
    S: futures_util::Stream + Unpin,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(LnaError::new(ErrorCode::Cancelled, "Cancelled")),
        item = futures_util::StreamExt::next(running) => Ok(item),
    }
}

/// Copy of `req` whose user message is followed by `note` (the orchestrator's instruction to one
/// executor: which part of the artifact it writes).
pub(crate) fn with_part_note(req: &LlmRequest, note: &str) -> LlmRequest {
    let mut r = req.clone();
    if let Some(m) = r.messages.last_mut() {
        m.parts.push(Part::Text(format!("\n\n{note}")));
    }
    r
}

/// Splits `total` items into `parts` contiguous 1-based inclusive ranges of near-equal size.
pub(crate) fn split_ranges(total: usize, parts: usize) -> Vec<(usize, usize)> {
    let parts = parts.clamp(1, total.max(1));
    let (base, extra) = (total / parts, total % parts);
    let mut out = Vec::new();
    let mut next = 1;
    for i in 0..parts {
        let n = base + usize::from(i < extra);
        out.push((next, next + n.saturating_sub(1)));
        next += n;
    }
    out
}

fn ui_language_is_english(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    if l.starts_with("en") {
        true
    } else if l.starts_with("zh") {
        false
    } else {
        system_prefers_english()
    }
}

/// The OS display language (looked up once). Defaults to English when it cannot be determined.
fn system_prefers_english() -> bool {
    use std::sync::OnceLock;
    static SYSTEM_EN: OnceLock<bool> = OnceLock::new();
    *SYSTEM_EN.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            // `defaults read -g AppleLanguages` prints ( "en-US", "zh-Hans-US" ); the first entry is the UI language.
            if let Ok(out) = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleLanguages"])
                .output()
            {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                if let Some(first) = text.split('"').nth(1) {
                    return !first.to_lowercase().starts_with("zh");
                }
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if let Ok(out) = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", "(Get-UICulture).Name"])
                .creation_flags(0x0800_0000)
                .output()
            {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
                if !name.is_empty() {
                    return !name.starts_with("zh");
                }
            }
        }
        for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                let v = v.trim().to_lowercase();
                if !v.is_empty() && v != "c" && v != "posix" {
                    return !v.starts_with("zh");
                }
            }
        }
        true
    })
}

impl StudioService {
    /// The UI language chosen in the client's preferences (`ui.preferences.language`).
    fn ui_is_en(&self) -> bool {
        let lang = self
            .db
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
            .unwrap_or_default();
        ui_language_is_english(&lang)
    }

    /// Picks the text for the UI language (used for the step titles/bodies shown in the execution process).
    fn l(&self, zh: &str, en: &str) -> String {
        if self.ui_is_en() {
            en.to_string()
        } else {
            zh.to_string()
        }
    }

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
            audio_render_slot: Arc::new(Semaphore::new(1)),
            previews: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Pushes an unvalidated preview of a generating artifact to the clients and keeps it for
    /// `preview_get`. Previews are never persisted (checkpoints aside) and are dropped when
    /// the artifact leaves the generating state.
    fn emit_preview(&self, artifact_id: &str, notebook_id: &str, stage: &str, payload: &Value) {
        if let Ok(mut m) = self.previews.lock() {
            m.insert(
                artifact_id.to_string(),
                (stage.to_string(), payload.clone()),
            );
        }
        self.emit(
            &format!("studio:{notebook_id}"),
            "studio.preview",
            json!({
                "artifact_id": artifact_id,
                "stage": stage,
                "validated": false,
                "payload": payload,
            }),
        );
    }

    /// Preview of an artifact that is still generating: the live buffer first, else the
    /// latest checkpoint (mind map passes, lecture sections), else `null`.
    pub fn preview_get(&self, artifact_id: &str) -> LnaResult<Value> {
        if let Some((stage, payload)) = self
            .previews
            .lock()
            .ok()
            .and_then(|m| m.get(artifact_id).cloned())
        {
            return Ok(json!({ "stage": stage, "validated": false, "payload": payload }));
        }
        if let Some(v) = self.get_checkpoint(artifact_id, "mindmap_passes")? {
            if let Some(p) = v.get("payload") {
                return Ok(json!({ "stage": "mindmap_passes", "validated": false, "payload": p }));
            }
        }
        if let Some(v) = self.get_checkpoint(artifact_id, "lecture_passes")? {
            return Ok(json!({ "stage": "lecture_passes", "validated": false, "payload": v }));
        }
        Ok(Value::Null)
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
            ArtifactType::Lecture => 24_000,
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
            let catalog_tokens = (index_chars / 3).clamp(1_600, 5_500);
            (catalog_tokens + on_demand_cap * 6 / 10)
                .min(on_demand_cap)
                .min(raw_source_tokens)
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
            ArtifactType::Lecture => 12_000,
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
        if selected_ids.is_empty() && !has_user_material(&config) {
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
        let use_en = if ui_lang.trim().is_empty() {
            self.ui_is_en()
        } else {
            ui_language_is_english(&ui_lang)
        };
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
                ArtifactType::Lecture => "Lecture",
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
                ArtifactType::Lecture => "讲解",
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
            ArtifactType::Lecture => "讲解",
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
            self.fill_refs(art)?;
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
            self.fill_refs(art)?;
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
        self.fill_refs(&mut art)?;
        art.progress_state = self.load_progress_state(id)?;
        // infographics made before the PNG existed get it the first time they are opened
        if art.artifact_type == ArtifactType::Infographic && art.status == ArtifactStatus::Ready {
            if let (Some(dir), Some(p)) = (art.output_dir.as_deref(), art.payload.as_ref()) {
                let png = std::path::Path::new(dir).join("infographic.png");
                if !png.exists() && std::path::Path::new(dir).join("infographic.svg").exists() {
                    let _ = render::render_infographic_png(p, &png);
                }
            }
        }
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

    /// Sources shown with an artifact: the ones its generation actually read once that is known
    /// (older artifacts: worked out once from their saved evidence), otherwise all selected ones.
    /// Also resolves the user's attached pictures and files.
    fn fill_refs(&self, art: &mut StudioArtifact) -> LnaResult<()> {
        let all = self.load_source_refs(&art.source_ids)?;
        let used: Option<Vec<String>> = match self.get_checkpoint(&art.id, "used_sources")? {
            Some(v) => serde_json::from_value(v).ok(),
            None => match self.get_checkpoint(&art.id, "collect")? {
                Some(c) => {
                    let ids = sources_used_in(
                        c.get("evidence_text").and_then(Value::as_str).unwrap_or(""),
                        &all,
                    );
                    self.save_checkpoint(&art.id, "used_sources", json!(ids))?;
                    Some(ids)
                }
                None => None,
            },
        };
        art.source_refs = match used {
            Some(ids) => all.into_iter().filter(|r| ids.contains(&r.id)).collect(),
            None => all,
        };
        let root = &self.db.paths().root;
        art.attachments = ["images", "files"]
            .iter()
            .filter_map(|k| art.config.get(*k))
            .flat_map(|ids| attachments::resolve(root, ids))
            .collect();
        Ok(())
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
                    .query_map([mid], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                    })
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
            &self.l("大纲已确认", "Outline approved"),
            &self.l("已提交审核通过的大纲，正在加入后台生成队列…", "The approved outline has been submitted and is being queued for background generation…"),
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
        images: &[Attachment],
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
        let profile = lna_storage::get_active_profile(&self.db)?.ok_or_else(|| {
            LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured")
        })?;
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
            preset: profile.preset.clone(),
            shrink_media: profile.shrink_media,
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
            &self.l(
                "工具调用 · 按提示词迭代调整提纲",
                "Tool call · Revising the outline from your prompt",
            ),
            instr,
        );
        let files = context_files::context_items(images);
        let files_text = if files.is_empty() {
            String::new()
        } else {
            let ctx = context_files::ToolCtx {
                db: &self.db,
                secrets: &self.secrets,
                llm: &self.llm,
                notebook_id: &art.notebook_id,
                profile: &resolved,
                atts: &files,
            };
            let task = format!(
                "按以下修改指令调整「{}」的提纲：{instr}",
                art.artifact_type.as_str()
            );
            let b = context_files::brief(&ctx, &task, 4, CancellationToken::new()).await?;
            format!("\n\nRelevant content from the user's attached files:\n{b}")
        };

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
                parts: attachments::user_parts(
                    &self.db,
                    &self.secrets,
                    &self.llm,
                    &art.notebook_id,
                    &resolved,
                    format!(
                        "Artifact type: {}
Config: {}

Current Outline:
{}

User Revision Instruction:
{}{}",
                        art.artifact_type.as_str(),
                        art.config,
                        existing_md,
                        instr,
                        files_text
                    ),
                    images,
                )
                .await,
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

        let (revised_raw, usage) = self
            .run_text_with_thought(
                id,
                &art.notebook_id,
                &self.l("提纲迭代调整思考", "Outline revision reasoning"),
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
        let draft_md_json = serde_json::to_string(&Value::String(revised_md))
            .unwrap_or_else(|_| String::from("\"\""));
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
                 ORDER BY CASE WHEN status = 'READY' THEN 0 ELSE 1 END,
                          CASE WHEN COALESCE(json_extract(config_json, '$.source_id'), '') = ?1 THEN 0 ELSE 1 END,
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

    pub fn list_notebook_knowledge_graphs(
        &self,
        notebook_id: &str,
    ) -> LnaResult<Vec<StudioArtifact>> {
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
            let rows = stmt
                .query_map([notebook_id], |r| r.get::<_, String>(0))
                .map_err(map_sql_err)?;
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

    pub fn compute_kg_progress_summary(
        &self,
        notebook_id: &str,
        source_id: Option<&str>,
    ) -> LnaResult<Value> {
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
            let Some(payload) = g.payload.as_ref() else {
                continue;
            };
            let Some(nodes) = payload.get("nodes").and_then(Value::as_array) else {
                continue;
            };
            let mastery_map = g
                .progress_state
                .as_ref()
                .and_then(|ps| ps.get("mastery"))
                .and_then(Value::as_object);
            let concept_nodes: Vec<&Value> = if nodes.len() > 1 {
                nodes
                    .iter()
                    .filter(|n| !n.get("parent_id").map(Value::is_null).unwrap_or(true))
                    .collect()
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

        let mastery_percent = (mastered * 100).checked_div(total).unwrap_or(0);
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
        let quiz_sids: std::collections::HashSet<&str> =
            quiz.source_ids.iter().map(String::as_str).collect();
        graphs.sort_by_key(|g| {
            let overlaps = g.source_ids.iter().any(|s| quiz_sids.contains(s.as_str()));
            if overlaps {
                0
            } else {
                1
            }
        });

        let mut result_by_qid: HashMap<String, (bool, f64)> = HashMap::new();
        for item in &result_items {
            if let Some(qid) = item.get("question_id").and_then(Value::as_str) {
                let is_correct = item
                    .get("is_correct")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
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
            let Some(payload) = g.payload.as_ref() else {
                continue;
            };
            let Some(nodes) = payload.get("nodes").and_then(Value::as_array) else {
                continue;
            };
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
                let Some(&(is_correct, q_score)) = result_by_qid.get(qid) else {
                    continue;
                };
                let q_text = format!(
                    "{} {} {}",
                    quiz.title,
                    q.get("prompt").and_then(Value::as_str).unwrap_or(""),
                    q.get("explanation").and_then(Value::as_str).unwrap_or("")
                );

                let mut scored_nodes: Vec<(f64, &str, &str)> = Vec::new();
                for n in nodes {
                    let is_root =
                        n.get("parent_id").map(Value::is_null).unwrap_or(false) && nodes.len() > 1;
                    if is_root {
                        continue;
                    }
                    let nid = n.get("id").and_then(Value::as_str).unwrap_or("");
                    let label = n.get("label").and_then(Value::as_str).unwrap_or("");
                    let summary = n.get("summary").and_then(Value::as_str).unwrap_or("");
                    if nid.is_empty() || label.is_empty() {
                        continue;
                    }
                    let facets: Vec<&str> = n
                        .get("facets")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let match_text = if facets.is_empty() {
                        summary.to_string()
                    } else {
                        format!("{summary} {}", facets.join(" "))
                    };
                    let s = score_kg_node_match(label, &match_text, &q_text, topic_hint);
                    if s >= 0.35 {
                        scored_nodes.push((s, nid, label));
                    }
                }
                scored_nodes
                    .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
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
                    let new_score = if prev_attempts == 0 {
                        q_score
                    } else {
                        (acc + q_score) / 2.0
                    };
                    let status = if is_correct {
                        if new_score >= 0.75 {
                            "mastered"
                        } else {
                            "learning"
                        }
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
                let err =
                    LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured");
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
            preset: profile.preset.clone(),
            shrink_media: profile.shrink_media,
        };

        let is_local_test =
            resolved.base_url.contains("127.0.0.1") || resolved.base_url.contains("localhost");
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

                    // the model stopped the task on purpose: no retry, the reason is the error
                    if e.code == ErrorCode::ModelRefused {
                        self.append_thought(
                            artifact_id,
                            &fresh_art.notebook_id,
                            "error",
                            &self.l("任务已被模型终止", "The model stopped the task"),
                            &e.message,
                        );
                        let log = format_error_log(&fresh_art, Some(&profile), &records);
                        let _ = self.mark_failed_with_log(artifact_id, &e, &log);
                        return Err(e);
                    }
                    if is_non_retryable_config_error(e.code) {
                        self.append_thought(
                            artifact_id,
                            &fresh_art.notebook_id,
                            "error",
                            &self.l("执行失败", "Run failed"),
                            &self.l(
                                "请检查模型配置或权限设置后重试。",
                                "Check the model configuration or permissions, then try again.",
                            ),
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
                            &self.l(&format!("第 {attempt}/{MAX_STUDIO_RETRIES} 次回退重试"), &format!("Rollback retry {attempt}/{MAX_STUDIO_RETRIES}")),
                            &self.l(&format!(
                                "阶段 {current_stage} 执行失败（{}）：{}。已回退该阶段检查点并准备重试。",
                                e.code.as_str(),
                                truncate_str(&e.message, 220)
                            ), &format!(
                                "Stage {current_stage} failed ({}): {}. Rolled back the stage checkpoint and preparing to retry.",
                                e.code.as_str(),
                                truncate_str(&e.message, 220)
                            )),
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
                            &self.l("执行失败", "Run failed"),
                            &self.l(
                                "未能完成生成，请稍后重试。",
                                "Generation could not be completed. Please try again later.",
                            ),
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
        // A model that sees images gets the pictures themselves in its requests; then no
        // separate description is made. Otherwise (or if the pictures cannot be loaded) the
        // text description carries them.
        let pictures = material::image_parts(&self.db.paths().root, art, resolved);
        let direct_pictures = !pictures.is_empty();

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
                    &self.l("复用已缓存来源证据", "Reusing cached source evidence"),
                    &self.l(
                        "已从检查点加载来源资料，跳过重复收集。",
                        "Loaded the sources from the checkpoint; skipping repeated collection.",
                    ),
                );
            }
            checkpoint
                .get("evidence_text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            // What the user attached is read first (both briefs are checkpointed): it fixes the
            // scope, so the source reading below fetches only what that material needs instead of
            // drifting to the whole chapter that happens to contain it.
            let material_hint = if has_user_material(&art.config) {
                let files = self
                    .file_brief(art, resolved, artifact_id, cancel.clone())
                    .await?;
                let images = if direct_pictures {
                    None
                } else {
                    self.image_brief(art, resolved, artifact_id, cancel.clone())
                        .await?
                };
                let joined = [files, images]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(truncate_str(&joined, 1_500).to_string()).filter(|s| !s.trim().is_empty())
            } else {
                None
            };
            // A lecture without an outline to review is planned by the reader itself once it
            // has read enough (one request instead of a reading pass plus a planning pass that
            // sends all the evidence again).
            let review_requested = art
                .config
                .get("reviewOutline")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let finish = (art.artifact_type == ArtifactType::Lecture
                && !review_requested
                && art.draft.is_none())
            .then(|| lecture::lecture_reader_finish(art, &self.lecture_kg_hint(art)));
            let mut read_plan: Option<Value> = None;
            let text = self
                .collect_evidence_text(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    material_hint.as_deref(),
                    finish.as_ref(),
                    &mut read_plan,
                    cancel.clone(),
                )
                .await?;
            self.save_checkpoint(
                artifact_id,
                "collect",
                json!({
                    "sources_fingerprint": art.sources_fingerprint,
                    "evidence_text": text
                }),
            )?;
            if let Some(outline) = read_plan.filter(|o| {
                o.get("sections")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
            }) {
                // the lecture pass finds this checkpoint and skips its own planner
                let passes = json!({ "sources_fingerprint": art.sources_fingerprint, "outline": outline, "done": {} });
                self.save_checkpoint(artifact_id, "lecture_passes", passes)?;
                self.emit_preview(
                    artifact_id,
                    &art.notebook_id,
                    "lecture_passes",
                    &json!({ "outline": outline, "done": {} }),
                );
            }
            text
        };
        // which notebook sources the evidence actually came from (shown with the artifact)
        let used = sources_used_in(&evidence_txt, &self.load_source_refs(&art.source_ids)?);
        self.save_checkpoint(artifact_id, "used_sources", json!(used))?;

        let evidence_txt = match self.file_brief(art, resolved, artifact_id, cancel.clone()).await? {
            Some(brief) => format!(
                "<user_files>\n用户附带的一次性参考资料中与本次任务相关的内容（视为用户直接提供的材料，与来源同等重要；引用时注明附件名和页码，不要编造）：\n{brief}\n</user_files>\n\n{evidence_txt}"
            ),
            None => evidence_txt,
        };
        let image_brief = if direct_pictures {
            None
        } else {
            self.image_brief(art, resolved, artifact_id, cancel.clone())
                .await?
        };
        let evidence_txt = match image_brief {
            Some(brief) => format!(
                "<user_images>\n用户附带的图片内容（视为用户直接提供的材料，与来源同等重要；可以引用，但不要编造图片中没有的内容）：\n{brief}\n</user_images>\n\n{evidence_txt}"
            ),
            None => evidence_txt,
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
            let is_lecture = art.artifact_type == ArtifactType::Lecture;
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &if is_lecture {
                    self.l("确定讲解范围与大纲", "Defining the lecture scope and outline")
                } else if is_report {
                    self.l("规划报告提纲", "Planning the report outline")
                } else {
                    self.l("规划演示文稿提纲", "Planning the slide outline")
                },
                &if is_lecture {
                    self.l(
                        "正在根据你给出的题目或范围，在来源中定位对应章节，并规划学习目标与讲解小节…",
                        "Locating the matching chapters in the sources for your topic or scope and planning learning goals and lecture sections…",
                    )
                } else if is_report {
                    self.l(
                        "正在基于已选来源梳理报告章节结构、论点脉络与核心证据…",
                        "Organizing the report's chapter structure, argument flow and key evidence from the selected sources…",
                    )
                } else {
                    self.l(
                        "正在基于已选来源梳理演示文稿章节结构与每页要点…",
                        "Organizing the deck's chapter structure and per-slide key points from the selected sources…",
                    )
                },
            );
            let lecture_sys = format!(
                "You are a lecture planner. The learner wants a narrated lesson on the topic, problem or unit given in config.topic (the whole selection if empty). Output a clean, human-readable Markdown outline for the learner to confirm: `# Lecture title`, a line `范围 / Scope:` naming exactly which chapters, sections or pages of the sources it covers, `## 学习目标 / Objectives` with bullets, then `## Section N: Title` with bullets for the concepts, examples or problems each section covers. {} Match the language requested in config or the primary language of the sources. Never output JSON or fenced code blocks.",
                lecture::lecture_rules(
                    art.config.get("mode").and_then(Value::as_str).unwrap_or("general"),
                    art.config.get("detail").and_then(Value::as_str).unwrap_or("auto"),
                )
            );
            let outline_sys = if is_lecture {
                lecture_sys.as_str()
            } else if is_report {
                "You are a research report outline planner. Output a clean, human-readable Markdown outline (using `# Report Title`, `## Section N: Section Heading`, `- Key findings, analysis points, and supporting evidence`). Match the language requested in config or the primary language of the sources. Be concise, well-structured, and accurate; never output JSON or fenced code blocks."
            } else {
                "You are a presentation outline planner. Output a clean, human-readable Markdown outline (using `# Title`, `## Slide N: Slide Title`, `- Key points`, and `> Speaker notes`). Match the language requested in config or the primary language of the sources. Be concise and accurate; never output JSON or fenced code blocks."
            };
            let plan_req = material::with_images(
                LlmRequest {
                    system: outline_sys.into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Artifact type: {}\nConfig: {}\nSources:\n{}",
                            art.artifact_type.as_str(),
                            material::config_for_prompt(&art.config, direct_pictures),
                            evidence_txt
                        ))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: 1800,
                    reasoning: reasoning.clone(),
                },
                &pictures,
            );
            let (draft_raw, _) = self
                .run_text_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &self.l("大纲规划思考过程", "Outline planning reasoning"),
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
            self.emit_status_event(
                artifact_id,
                "DRAFT_REVIEW",
                "DRAFT_REVIEW",
                0.45,
                None,
                None,
            );
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

        let (mut payload, usage) = if let Some(saved) =
            saved_generate.and_then(|v| v.get("payload").cloned())
        {
            self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l("复用已验证内容检查点", "Reusing the validated content checkpoint"),
                    &self.l("已加载上次生成的结构化内容，准备继续渲染。", "Loaded the previously generated structured content; ready to continue rendering."),
                );
            (saved, Usage::default())
        } else {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l("生成结构化成品内容", "Generating structured content"),
                &self.l(
                    &format!("正在生成「{}」…", art.title),
                    &format!("Generating \"{}\"…", art.title),
                ),
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
                ArtifactType::Mindmap => validate::normalize_mindmap(&mut generated),
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
                        ArtifactType::Lecture => dir.join("lecture.pptx").is_file(),
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
                    lecture::render_figure_assets(&payload, &out_dir)?;
                    render::render_pptx_in(&payload, &out_dir.join("slides.pptx"), Some(&out_dir))?;
                }
                ArtifactType::Lecture => {
                    lecture::render_figure_assets(&payload, &out_dir)?;
                    render::render_pptx_in(
                        &payload,
                        &out_dir.join("lecture.pptx"),
                        Some(&out_dir),
                    )?;
                }
                ArtifactType::Infographic => {
                    render::render_infographic_svg(&payload, &out_dir.join("infographic.svg"))?;
                    // best effort: clients that cannot draw SVG text show the picture instead
                    let _ =
                        render::render_infographic_png(&payload, &out_dir.join("infographic.png"));
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
                    // the first distinct speaker gets voice A, the second voice B, and so on
                    let mut speakers: Vec<String> = Vec::new();
                    if let Some(turns) = payload.get("turns").and_then(|t| t.as_array()) {
                        for (i, t) in turns.iter().enumerate() {
                            let txt = t.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            let spk = t.get("speaker").and_then(|x| x.as_str()).unwrap_or("A");
                            let slot = match speakers.iter().position(|s| s == spk) {
                                Some(p) => p,
                                None => {
                                    speakers.push(spk.to_string());
                                    speakers.len() - 1
                                }
                            };
                            if !txt.is_empty() {
                                let seg_path = out_dir.join(format!("turn_{i:03}.wav"));
                                if !seg_path.exists() {
                                    let v = if slot % 2 == 1 { &voice_b } else { &voice_a };
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
            &self.l("成品生成完成", "Generation complete"),
            &self.l(
                "内容校验与渲染均已完成，可开始交互预览或导出。",
                "Validation and rendering are done. You can now preview interactively or export.",
            ),
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
        if art.artifact_type == ArtifactType::Lecture {
            return self
                .generate_lecture_multipass(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    evidence_txt,
                    reasoning,
                    max_repairs,
                    cancel,
                )
                .await;
        }
        let pictures = material::image_parts(&self.db.paths().root, art, resolved);
        let pictures_ref = &pictures;
        let pictures_shown = !pictures.is_empty();
        let (schema_str, schema_name) = match art.artifact_type {
            ArtifactType::Lecture => (SCHEMA_SLIDES, "lecture_payload"),
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
            // Orchestrator: split the target into batch sizes and give every executor its own
            // portion of the material so the parallel batches do not overlap. Executors run at
            // once (up to the profile's parallel-request limit); results merge in batch order.
            let batch_sizes: Vec<usize> = (0..batches)
                .map(|bi| (total - bi * QUIZ_BATCH_SIZE).min(QUIZ_BATCH_SIZE))
                .collect();
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l(&format!("并行分批生成测验题目（{batches} 批）"), &format!("Generating quiz questions in {batches} parallel batches")),
                &self.l(&format!("总目标 {total} 道题，拆成 {batches} 批并行撰写，{}。", self.parallel_phrase(profile).0), &format!("Target of {total} quiz questions split into {batches} batches written in parallel, {}.", self.parallel_phrase(profile).1)),
            );
            let limit = self.parallelism(profile).min(batches);
            let schema_ref = &schema;
            let mut running = futures_util::stream::iter(batch_sizes.iter().copied().enumerate().map(|(bi, batch_cnt)| {
                let (reasoning_c, cancel_c) = (reasoning.clone(), cancel.clone());
                async move {
                    let batch_req = material::with_images(LlmRequest {
                        system: format!(
                            "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array. Match the language requested in config or the primary language of the sources. Keep every prompt, option, and explanation concise and accurate. Format all mathematical expressions and chemical formulas/equations in LaTeX wrapped in $...$ (e.g., $\\\\ce{{H2O}}$, $\\\\ce{{O2}}$, $\\\\ce{{CO2}}$, $\\\\ce{{2H2 + O2 -> 2H2O}}$, $x^2$, $\\\\frac{{a}}{{b}}$); never write bare ASCII formulas without subscripts/LaTeX, and double-escape all LaTeX backslashes in JSON strings so \\\\frac, \\\\times, \\\\text, \\\\ce are not corrupted.",
                            art.artifact_type.as_str()
                        ),
                        messages: vec![LlmMessage {
                            role: Role::User,
                            parts: vec![Part::Text(format!(
                                "Parameters: {}\nReference outline: {}\n\nSources:\n{}\n\n=== THIS BATCH (batch {}/{}; everything above is identical for every batch) ===\nGenerate exactly {} quiz questions (this overrides any count in the parameters).\nParallel generation: the other batches are written at the same time by other authors. Draw your quiz questions mainly from portion {} of {} (equal portions of the material, in the order given) and prefer topics the other portions would not cover, so the batches do not overlap.",
                                material::config_for_prompt(&art.config, pictures_shown),
                                art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                                evidence_txt,
                                bi + 1,
                                batches,
                                batch_cnt,
                                bi + 1,
                                batches
                            ))],
                        }],
                        tools: vec![],
                        tool_mode: ToolMode::None,
                        output: OutputMode::Text,
                        max_output_tokens: resolved.max_output_tokens,
                        reasoning: reasoning_c,
                    }, pictures_ref);
                    let res = self
                        .run_structured_with_thought(
                            artifact_id,
                            &art.notebook_id,
                            &self.l(
                                &format!("第 {}/{} 批推理思考", bi + 1, batches),
                                &format!("Batch {}/{} reasoning", bi + 1, batches),
                            ),
                            resolved,
                            &batch_req,
                            schema_name,
                            schema_ref,
                            max_repairs,
                            cancel_c,
                        )
                        .await;
                    (bi, res)
                }
            }))
            .buffer_unordered(limit);
            let mut parts: Vec<Option<Value>> = vec![None; batches];
            while let Some((bi, res)) = next_or_cancel(&mut running, &cancel).await? {
                let (part, u) = res?;
                add_usage(&mut usage_sum, &u);
                parts[bi] = Some(part);
            }
            drop(running);
            for (bi, part) in parts.into_iter().enumerate() {
                let Some(part) = part else { continue };
                if bi == 0 {
                    if let Some(t) = part
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                    {
                        title = t.to_string();
                    }
                }
                if let Some(list) = part.get("questions").and_then(Value::as_array) {
                    for mut it in list.iter().cloned() {
                        let key = it
                            .get("prompt")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !key.is_empty() && !seen_prompts.contains(&key) {
                            seen_prompts.push(key);
                            if let Some(obj) = it.as_object_mut() {
                                obj.insert(
                                    "id".into(),
                                    json!(format!("q{}", all_questions.len() + 1)),
                                );
                            }
                            all_questions.push(it);
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
            // Orchestrator: split the target into batch sizes and give every executor its own
            // portion of the material so the parallel batches do not overlap. Executors run at
            // once (up to the profile's parallel-request limit); results merge in batch order.
            let batch_sizes: Vec<usize> = (0..batches)
                .map(|bi| (total - bi * FLASHCARD_BATCH_SIZE).min(FLASHCARD_BATCH_SIZE))
                .collect();
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l(&format!("并行分批生成学习闪卡（{batches} 批）"), &format!("Generating flashcards in {batches} parallel batches")),
                &self.l(&format!("总目标 {total} 张卡片，拆成 {batches} 批并行撰写，{}。", self.parallel_phrase(profile).0), &format!("Target of {total} flashcards split into {batches} batches written in parallel, {}.", self.parallel_phrase(profile).1)),
            );
            let limit = self.parallelism(profile).min(batches);
            let schema_ref = &schema;
            let mut running = futures_util::stream::iter(batch_sizes.iter().copied().enumerate().map(|(bi, batch_cnt)| {
                let (reasoning_c, cancel_c) = (reasoning.clone(), cancel.clone());
                async move {
                    let batch_req = material::with_images(LlmRequest {
                        system: format!(
                            "You are the Local Notebook Studio generator. Strictly ground your output in the provided sources to generate structured JSON for {}, preserving source citation indices [n] in the citations array. Match the language requested in config or the primary language of the sources. Keep each flashcard front and back crisp and focused on core definitions or takeaways. Flashcards are drawn as plain text: write mathematics as compact Unicode (e.g. ℝⁿ, Aᵀ, x₁, N(A), dim C(A)=r, n−r, Ax=0), never LaTeX or `$` delimiters.",
                            art.artifact_type.as_str()
                        ),
                        messages: vec![LlmMessage {
                            role: Role::User,
                            parts: vec![Part::Text(format!(
                                "Parameters: {}\nReference outline: {}\n\nSources:\n{}\n\n=== THIS BATCH (batch {}/{}; everything above is identical for every batch) ===\nGenerate exactly {} flashcards (this overrides any count in the parameters).\nParallel generation: the other batches are written at the same time by other authors. Draw your flashcards mainly from portion {} of {} (equal portions of the material, in the order given) and prefer topics the other portions would not cover, so the batches do not overlap.",
                                material::config_for_prompt(&art.config, pictures_shown),
                                art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                                evidence_txt,
                                bi + 1,
                                batches,
                                batch_cnt,
                                bi + 1,
                                batches
                            ))],
                        }],
                        tools: vec![],
                        tool_mode: ToolMode::None,
                        output: OutputMode::Text,
                        max_output_tokens: resolved.max_output_tokens,
                        reasoning: reasoning_c,
                    }, pictures_ref);
                    let res = self
                        .run_structured_with_thought(
                            artifact_id,
                            &art.notebook_id,
                            &self.l(
                                &format!("第 {}/{} 批推理思考", bi + 1, batches),
                                &format!("Batch {}/{} reasoning", bi + 1, batches),
                            ),
                            resolved,
                            &batch_req,
                            schema_name,
                            schema_ref,
                            max_repairs,
                            cancel_c,
                        )
                        .await;
                    (bi, res)
                }
            }))
            .buffer_unordered(limit);
            let mut parts: Vec<Option<Value>> = vec![None; batches];
            while let Some((bi, res)) = next_or_cancel(&mut running, &cancel).await? {
                let (part, u) = res?;
                add_usage(&mut usage_sum, &u);
                parts[bi] = Some(part);
            }
            drop(running);
            for (bi, part) in parts.into_iter().enumerate() {
                let Some(part) = part else { continue };
                if bi == 0 {
                    if let Some(t) = part
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                    {
                        title = t.to_string();
                    }
                }
                if let Some(list) = part.get("cards").and_then(Value::as_array) {
                    for mut it in list.iter().cloned() {
                        let key = it
                            .get("front")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !key.is_empty() && !seen_fronts.contains(&key) {
                            seen_fronts.push(key);
                            if let Some(obj) = it.as_object_mut() {
                                obj.insert("id".into(), json!(format!("c{}", all_cards.len() + 1)));
                            }
                            all_cards.push(it);
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
                    match art
                        .config
                        .get("length")
                        .and_then(Value::as_str)
                        .unwrap_or("standard")
                    {
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
                &self.l("规划演示文稿视觉主题与逐页版式", "Planning the visual theme and per-slide layouts"),
                &self.l(&format!("正在围绕来源内容设计约 {target_pages} 页演示文稿的叙事节奏、主题配色与多样化版式组合…"), &format!("Designing the narrative pacing, color theme and varied layout mix for a deck of about {target_pages} slides…")),
            );
            let plan_req = material::with_images(LlmRequest {
                system: "You are an executive presentation architect and visual designer. Given the source material and user parameters, design a concise slide-by-slide visual storyboard (NOT raw JSON yet). Choose the best theme from [executive_navy, editorial_slate, academic_sage, warm_terracotta, midnight_aurora]. Plan an engaging narrative arc starting with `title` and `agenda`, ending with `summary`, and using DIVERSE visual layouts across content slides (`cards`, `two_column`, `metrics`, `timeline`, `table`, `formula`, `quote`, `section`, `bullets`) — NEVER use plain `bullets` on more than 25% of slides and never repeat the same content layout twice in a row. For each slide, specify: page number, layout, crisp headline, key facts/numbers/equations/comparisons from the sources, and visual structure. Match the requested language or primary source language.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "Target slide count: ~{target_pages}\nParameters: {}\nApproved outline (if any): {}\n\nResearched Sources:\n{}",
                                material::config_for_prompt(&art.config, pictures_shown),
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
            }, pictures_ref);
            let mut usage_sum = Usage::default();
            let storyboard = {
                match self
                    .run_text_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &self.l(
                            "演示文稿视觉架构与版式思考",
                            "Slide visual architecture and layout reasoning",
                        ),
                        resolved,
                        &plan_req,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok((txt, u)) => {
                        usage_sum.input_tokens =
                            usage_sum.input_tokens.saturating_add(u.input_tokens);
                        usage_sum.output_tokens =
                            usage_sum.output_tokens.saturating_add(u.output_tokens);
                        usage_sum.cached_tokens =
                            usage_sum.cached_tokens.saturating_add(u.cached_tokens);
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
                &self.l("撰写高颜值演示文稿内容与版式数据", "Writing slide content and layout data"),
                &self.l("正在基于版式蓝图生成完整结构化幻灯片（含卡片、对比表、流程图、关键指标、核心公式与演讲备注）…", "Generating the full structured slides from the layout blueprint (cards, comparison tables, flow charts, key metrics, core formulas and speaker notes)…"),
            );
            let author_base = material::with_images(LlmRequest {
                system: "You are a world-class presentation designer for Local Notebook Studio. Produce a publication-grade SlidesPayload JSON strictly grounded in the provided sources.\n\
Design rules:\n\
1. Set top-level `theme` (one of: executive_navy, editorial_slate, academic_sage, warm_terracotta, midnight_aurora) and `subtitle`.\n\
2. Use rich, varied layouts across slides: `title` (slide 1), `agenda`, `section`, `cards` (3-6 insight cards with tag/title/body), `two_column` (2 titled comparison columns with items), `metrics` (2-4 KPI highlight cards with value/label/detail), `timeline` (3-5 sequential steps with step/title/detail), `table` (structured comparison table with headers and rows), `formula` (highlighted equation/reaction in `formula` plus explanation in `bullets`), `quote`, `bullets` (sparingly), and `summary` (final slide).\n\
3. Populate the layout-specific fields (`cards`, `columns`, `metrics`, `steps`, `table`, `formula`, `subtitle`, `callout`) whenever using those layouts, AND always include concise `bullets` (can be empty [] when structured fields carry the slide), thorough `speaker_notes`, and valid `citations`.\n\
4. Keep on-slide text crisp and scannable (short headlines, concise card bodies, zero walls of text) while putting deeper explanation in `speaker_notes`.\n\
5. Step-by-step builds (optional): for derivations, procedures, comparisons or lists that are presented one item at a time, add `builds`: [{\"step\":1,\"targets\":[ids],\"effect\":\"fade|appear|fly_up|draw|emphasis|dim_others\"}] with steps numbered 1..K; ids: title, subtitle, b1..bN, card1.., metric1.., step1.., col1, col1.i2, table, table.r1, formula, callout, figure, figure:<part id>. Elements not listed are visible from the start. Keep simple slides static.\n\
6. Figures (optional): when a function graph, 3-D surface, geometric figure or physics diagram genuinely helps, use layout `figure` with `figure_request`: {\"kind\":\"plot|plot3d|diagram\",\"purpose\":...,\"details\": exact functions/ranges/points/forces/labels and the part ids you reference}. A separate step draws it precisely; never describe SVG.\n\
7. Match the language requested in config or the primary language of the sources.".into(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "Target slide count: ~{target_pages}\nParameters: {}\nVisual Storyboard:\n{}\n\nReference outline: {}\n\nResearched Sources:\n{}",
                                material::config_for_prompt(&art.config, pictures_shown),
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
            }, pictures_ref);
            // Orchestrator: the storyboard is the plan; a longer deck is written by several
            // executors at once, each owning a contiguous range of pages, then merged in order.
            let deck_parts = if target_pages >= 8 {
                (target_pages as usize).div_ceil(6)
            } else {
                1
            };
            let ranges = split_ranges(target_pages as usize, deck_parts);
            if deck_parts > 1 {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l(&format!("并行撰写演示文稿（{deck_parts} 段）"), &format!("Writing the deck in {deck_parts} parallel parts")),
                    &self.l(&format!("按版式蓝图把约 {target_pages} 页拆成 {deck_parts} 段并行撰写，{}。", self.parallel_phrase(profile).0), &format!("Splitting about {target_pages} slides into {deck_parts} parts written in parallel, {}.", self.parallel_phrase(profile).1)),
                );
            }
            let results = self
                .run_parts(deck_parts, self.parallelism(profile), &cancel, |k| {
                    let (from, to) = ranges[k];
                    let req = if deck_parts == 1 {
                        author_base.clone()
                    } else {
                        with_part_note(
                            &author_base,
                            &format!(
                                "PART {} OF {}: other parts are written at the same time by other designers. Write ONLY slides {from}-{to} of the ~{target_pages}-slide deck exactly as the storyboard lays them out{}{}. Keep the deck's theme and numbering consistent with the storyboard; do not repeat slides that belong to other ranges.",
                                k + 1,
                                deck_parts,
                                if k == 0 { " (slide 1 is the title slide)" } else { "" },
                                if k + 1 == deck_parts { " (the last slide is the summary)" } else { "" },
                            ),
                        )
                    };
                    let (schema_ref, cancel_c) = (&schema, cancel.clone());
                    async move {
                        self.run_structured_with_thought(
                            artifact_id,
                            &art.notebook_id,
                            &if deck_parts == 1 {
                                self.l("演示文稿内容与排版生成", "Slide content and layout generation")
                            } else {
                                self.l(&format!("演示文稿第 {}/{} 段生成", k + 1, deck_parts), &format!("Slide deck part {}/{}", k + 1, deck_parts))
                            },
                            resolved,
                            &req,
                            schema_name,
                            schema_ref,
                            max_repairs,
                            cancel_c,
                        )
                        .await
                    }
                })
                .await?;
            let mut deck = Value::Null;
            for (k, (part, u2)) in results.into_iter().enumerate() {
                add_usage(&mut usage_sum, &u2);
                if k == 0 {
                    deck = part;
                } else if let (Some(all), Some(more)) = (
                    deck.get_mut("slides").and_then(Value::as_array_mut),
                    part.get("slides").and_then(Value::as_array),
                ) {
                    all.extend(more.iter().cloned());
                }
            }

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
                    &self.l("排版美学与信息密度自检优化", "Self-check of layout aesthetics and information density"),
                    &self.l("检测到部分页面仍为纯列表版式，正在将其重构为卡片、双栏对比、流程图或表格以提升视觉层次…", "Some slides are still plain lists; restructuring them into cards, two-column comparisons, flow charts or tables for a better visual hierarchy…"),
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
                if let Ok((polished, u3)) = self
                    .run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &self.l("演示文稿视觉排版精修", "Slide visual layout polish"),
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
                    usage_sum.output_tokens =
                        usage_sum.output_tokens.saturating_add(u3.output_tokens);
                    usage_sum.cached_tokens =
                        usage_sum.cached_tokens.saturating_add(u3.cached_tokens);
                }
            }

            // Stage 4: figures (each in its own clean-context call) and step builds
            if let Some(slides) = deck.get_mut("slides").and_then(Value::as_array_mut) {
                if slides
                    .iter()
                    .any(|s| s.get("figure_request").is_some_and(|r| r.is_object()))
                {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l("绘制配图", "Drawing figures"),
                        &self.l("正在为需要配图的页面单独绘制函数图 / 示意图，并检查渲染结果…", "Drawing function graphs / diagrams for the slides that need figures and checking the rendered result…"),
                    );
                    let u4 = self
                        .fulfil_figure_requests(
                            art,
                            profile,
                            resolved,
                            artifact_id,
                            slides,
                            evidence_txt,
                            cancel.clone(),
                        )
                        .await?;
                    add_usage(&mut usage_sum, &u4);
                }
                for s in slides.iter_mut() {
                    lecture::texify_slide_text(s);
                }
                let log = lecture::sanitize_deck_builds(slides);
                if !log.is_empty() {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l("分步动画校验", "Step animation check"),
                        &log.join("\n"),
                    );
                }
                for s in slides.iter_mut() {
                    if let Some(o) = s.as_object_mut() {
                        o.remove("figure_request");
                    }
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
        if art.artifact_type == ArtifactType::Mindmap && (is_kg || detail_level != "concise") {
            return self
                .generate_mindmap_multipass(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    evidence_txt,
                    reasoning,
                    max_repairs,
                    cancel,
                )
                .await;
        }
        let extra_hint = match art.artifact_type {
            ArtifactType::Mindmap => mindmap_rules(is_kg, detail_level),
            ArtifactType::Report => {
                let depth = match art.config.get("depth").and_then(Value::as_str).unwrap_or("standard") {
                    "brief" => " Report depth: BRIEF executive overview with concise bulleted takeaways.",
                    "deep" => " Report depth: DEEP analytical synthesis with thorough explanations, derivations/examples, and structured comparisons.",
                    _ => "",
                };
                format!("{depth}{MATH_LATEX_RULE} Exception: headings and the title are plain text, so write maths in them as compact Unicode (e.g. ℝⁿ, N(A), Aᵀ), never with $.")
            }
            ArtifactType::Infographic => " Infographic text is drawn as plain SVG text: write mathematics as compact Unicode (e.g. ℝⁿ, Aᵀ, x₁, N(A), dim C(A)=r, n−r), never LaTeX or ASCII carets. Keep each heading under 20 characters, each metric under 24 characters, and each summary within about 100 characters.".to_string(),
            ArtifactType::Slides => {
                let len = match art.config.get("length").and_then(Value::as_str).unwrap_or("standard") {
                    "concise" => " Deck length: CONCISE overview (~5-6 slides focusing on core takeaways).",
                    "detailed" => " Deck length: DETAILED coverage (~12-16 slides with thorough explanations and examples).",
                    _ => " Deck length: BALANCED (~8-10 slides covering main themes cleanly).",
                };
                format!("{len} Slide text is drawn as plain text: write mathematics as compact Unicode (e.g. ℝⁿ, Aᵀ, x₁, N(A), dim C(A)=r, n−r, Ax=0), never LaTeX or `$` delimiters.")
            }
            ArtifactType::Quiz => {
                let mut quiz_hint = " Format all mathematical expressions and chemical formulas/equations in prompt, options, accepted_answers, rubric, and explanation using LaTeX wrapped in $...$ (e.g., $\\\\ce{H2O}$, $\\\\ce{O2}$, $\\\\ce{CO2}$, $\\\\ce{2H2 + O2 -> 2H2O}$, $x^2$, $\\\\frac{a}{b}$); never write bare ASCII formulas without subscripts/LaTeX, and double-escape all LaTeX backslashes in JSON strings so \\\\frac, \\\\times, \\\\text, \\\\ce are not corrupted.".to_string();
                quiz_hint.push_str(" QUESTION QUALITY: every question must test the knowledge point's condition of use (when it applies and when it does not) or a common misconception, not just recall of a definition. Each wrong option must correspond to a typical mistake that really occurs for this topic according to the sources; never invent arbitrary distractors. The explanation must say where the wrong options go wrong and when the right method applies. If a node summary or facets are given as clues, use them as the starting point.");
                let explicit_diff = art
                    .config
                    .get("difficulty")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "auto");
                if let Ok(kg_summary) = self.compute_kg_progress_summary(&art.notebook_id, None) {
                    if kg_summary
                        .get("has_knowledge_graph")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        let rec_diff = kg_summary
                            .get("recommended_difficulty")
                            .and_then(Value::as_str)
                            .unwrap_or("mixed");
                        let target_diff = explicit_diff.unwrap_or(rec_diff);
                        let mastery_pct = kg_summary
                            .get("mastery_percent")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        let weak: Vec<String> = kg_summary
                            .get("weak_concepts")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(String::from)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let unstarted: Vec<String> = kg_summary
                            .get("unstarted_concepts")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(String::from)
                                    .collect()
                            })
                            .unwrap_or_default();
                        quiz_hint.push_str(&format!(
                            " Target difficulty: {} (current Knowledge Graph mastery: {}%).",
                            target_diff, mastery_pct
                        ));
                        if !weak.is_empty() {
                            quiz_hint.push_str(&format!(
                                " Prioritize reinforcing weak concepts: {}.",
                                weak.join(", ")
                            ));
                        }
                        if !unstarted.is_empty() {
                            quiz_hint.push_str(&format!(
                                " Also assess unstarted concepts: {}.",
                                unstarted.join(", ")
                            ));
                        }
                    } else if let Some(d) = explicit_diff {
                        quiz_hint.push_str(&format!(" Target difficulty: {}.", d));
                    }
                }
                quiz_hint
            }
            ArtifactType::Audio => {
                let dur: &str = match art
                    .config
                    .get("length")
                    .and_then(Value::as_str)
                    .unwrap_or("standard")
                {
                    "short" | "concise" => {
                        " Audio duration: SHORT concise overview (~2-3 minutes of dialogue)."
                    }
                    "long" | "detailed" => {
                        " Audio duration: DETAILED in-depth discussion (~6-10 minutes of dialogue)."
                    }
                    _ => "",
                };
                format!("{dur} The script is read aloud by a speech engine: write mathematics the way it is spoken (for example \"R n\", \"A 转置\", \"n 减 r\", \"A x 等于 0\") instead of symbols, carets or LaTeX.")
            }
            _ => String::new(),
        };
        let citation_rule = if art.artifact_type == ArtifactType::Mindmap {
            ""
        } else {
            ", preserving source citation indices [n] in the citations array"
        };
        let gen_req = material::with_images(LlmRequest {
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
                                material::config_for_prompt(&art.config, pictures_shown),
                    art.draft.as_ref().map(|d| d.to_string()).unwrap_or_else(|| "None".into()),
                    evidence_txt
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning,
        }, pictures_ref);
        // Long reports / long audio scripts: planner + parallel part writers.
        let multi_kind = match art.artifact_type {
            ArtifactType::Report
                if art.config.get("depth").and_then(Value::as_str) == Some("deep") =>
            {
                Some("report")
            }
            ArtifactType::Audio
                if matches!(
                    art.config.get("length").and_then(Value::as_str),
                    Some("long" | "detailed")
                ) =>
            {
                Some("audio")
            }
            _ => None,
        };
        if let Some(kind) = multi_kind {
            if self.parallelism(profile) > 1 {
                return self
                    .generate_in_parts(
                        kind,
                        art,
                        profile,
                        resolved,
                        artifact_id,
                        &gen_req,
                        schema_name,
                        &schema,
                        max_repairs,
                        cancel,
                    )
                    .await;
            }
        }
        self.run_structured_with_thought(
            artifact_id,
            &art.notebook_id,
            &self.l("模型推理思考过程", "Model reasoning"),
            resolved,
            &gen_req,
            schema_name,
            &schema,
            max_repairs,
            cancel,
        )
        .await
    }

    /// Orchestrator/executor generation of a long report or audio script: one planner call splits
    /// the work into independent parts, then every part is written by its own executor at once
    /// (up to the profile's parallel-request limit) and the parts are merged in order.
    #[allow(clippy::too_many_arguments)]
    async fn generate_in_parts(
        &self,
        kind: &str,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        gen_req: &LlmRequest,
        schema_name: &str,
        schema: &Value,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage)> {
        let (noun_zh, noun_en, what) = if kind == "report" {
            ("章节", "sections", "report sections")
        } else {
            (
                "对话段落",
                "dialogue segments",
                "dialogue segments of the audio script",
            )
        };
        let mut usage_sum = Usage::default();
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            &self.l(
                &format!("规划{noun_zh}"),
                &format!("Planning the {noun_en}"),
            ),
            &self.l(
                &format!("先规划彼此独立的{noun_zh}，再并行撰写。"),
                &format!("Planning independent {noun_en} first, then writing them in parallel."),
            ),
        );
        let plan_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["parts"],
            "properties": { "parts": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["heading", "focus"],
                "properties": { "heading": { "type": "string" }, "focus": { "type": "string" } }
            }}}
        });
        // The planner keeps the generator's system prompt and message; its instruction is appended
        // at the end, so the planner call and every part call share one long prefix (the planner
        // call warms the provider's prompt cache for all of them).
        let mut plan_req = with_part_note(
            gen_req,
            &format!(
                "PLANNING TASK (do not write the {kind} itself): split the requested work into 3-6 {what} that can be written independently and in parallel, in reading order, covering the requested scope without overlap. For each give a short `heading` and a `focus` naming exactly which concepts / source parts it covers. Output the structured JSON requested below only."
            ),
        );
        plan_req.max_output_tokens = resolved.bounded_output(2000);
        let (plan, u) = self
            .run_structured_with_thought(
                artifact_id,
                &art.notebook_id,
                &self.l(
                    &format!("{noun_zh}规划思考"),
                    &format!("{noun_en} planning reasoning"),
                ),
                resolved,
                &plan_req,
                "studio_parts_plan",
                &plan_schema,
                max_repairs,
                cancel.clone(),
            )
            .await?;
        add_usage(&mut usage_sum, &u);
        let parts: Vec<(String, String)> = plan
            .get("parts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|p| (mm_str(p, "heading"), mm_str(p, "focus")))
                    .filter(|(h, _)| !h.trim().is_empty())
                    .take(8)
                    .collect()
            })
            .unwrap_or_default();
        if parts.len() < 2 {
            // nothing worth splitting: write it in one piece
            return self
                .run_structured_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &self.l("模型推理思考过程", "Model reasoning"),
                    resolved,
                    gen_req,
                    schema_name,
                    schema,
                    max_repairs,
                    cancel,
                )
                .await
                .map(|(v, u)| {
                    add_usage(&mut usage_sum, &u);
                    (v, usage_sum)
                });
        }
        let total = parts.len();
        let outline = parts
            .iter()
            .enumerate()
            .map(|(i, (h, f))| format!("{}. {h} — {f}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        let results = self
            .run_parts(total, self.parallelism(profile), &cancel, |k| {
                let (heading, focus) = &parts[k];
                let note = if kind == "report" {
                    format!(
                        "PART {} OF {total}: other sections are written at the same time by other authors. Write ONLY the section \"{heading}\" (focus: {focus}) as the single entry of `sections`; do not repeat the other sections of this outline:\n{outline}",
                        k + 1
                    )
                } else {
                    format!(
                        "PART {} OF {total}: other segments are written at the same time by other authors. Write ONLY the dialogue turns of the segment \"{heading}\" (focus: {focus}); {}{}continue the same conversation naturally and do not repeat the other segments of this outline:\n{outline}",
                        k + 1,
                        if k == 0 { "open the conversation; " } else { "do not greet or re-introduce the topic; " },
                        if k + 1 == total { "close the conversation at the end; " } else { "do not wrap up the conversation; " },
                    )
                };
                let req = with_part_note(gen_req, &note);
                let cancel_c = cancel.clone();
                async move {
                    self.run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &self.l(&format!("第 {}/{total} 部分推理思考", k + 1), &format!("Part {}/{total} reasoning", k + 1)),
                        resolved,
                        &req,
                        schema_name,
                        schema,
                        max_repairs,
                        cancel_c,
                    )
                    .await
                }
            })
            .await?;
        let mut merged = Value::Null;
        for (k, (part, u)) in results.into_iter().enumerate() {
            add_usage(&mut usage_sum, &u);
            if k == 0 {
                merged = part;
                continue;
            }
            let list_key = if kind == "report" {
                "sections"
            } else {
                "turns"
            };
            if let Some(more) = part.get(list_key).and_then(Value::as_array).cloned() {
                if let Some(all) = merged.get_mut(list_key).and_then(Value::as_array_mut) {
                    all.extend(more);
                }
            }
            if kind == "report" {
                let extra: Vec<Value> = part
                    .get("citations")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if let Some(all) = merged.get_mut("citations").and_then(Value::as_array_mut) {
                    for c in extra {
                        if !all.contains(&c) {
                            all.push(c);
                        }
                    }
                }
            }
        }
        Ok((merged, usage_sum))
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
        self.emit_status_event(
            id,
            "FAILED",
            "FAILED",
            0.0,
            Some(err.code.as_str()),
            Some(&err.message),
        );
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
        self.emit_status_event(
            id,
            "FAILED",
            "FAILED",
            0.0,
            Some(err.code.as_str()),
            Some(&err.message),
        );
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

    /// Stops generation like `mark_cancelled`, but records that the user meant to come back:
    /// the checkpoints stay and `retry` continues from them. Shown as stage `PAUSED`.
    pub fn mark_paused(&self, id: &str) -> LnaResult<()> {
        let changed = self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET status='CANCELLED', stage='PAUSED', updated_at=?1
                 WHERE id=?2 AND status IN ('PENDING','RUNNING','DRAFT_REVIEW')",
                params![now_ms(), id],
            )
            .map_err(map_sql_err)
        })?;
        if changed > 0 {
            self.emit_status_event(id, "CANCELLED", "PAUSED", 0.0, None, None);
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
        for key in ["images", "files"] {
            if let Some(ids) = art.config.get(key) {
                attachments::delete_files(&attachments::resolve(&self.db.paths().root, ids));
            }
        }
        Ok(())
    }

    /// Pictures attached to the Studio requests of a notebook (one list per artifact).
    pub fn attachments_of_notebook(&self, notebook_id: &str) -> Vec<Vec<Attachment>> {
        let root = self.db.paths().root.clone();
        self.db
            .read(|conn| {
                let mut stmt = conn
                    .prepare("SELECT json_extract(config_json, '$.images'), json_extract(config_json, '$.files') FROM studio_artifacts WHERE notebook_id = ?1")
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([notebook_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for (imgs, files) in rows.flatten() {
                    for raw in [imgs, files].into_iter().flatten() {
                        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                            out.push(attachments::resolve(&root, &v));
                        }
                    }
                }
                Ok(out)
            })
            .unwrap_or_default()
    }

    /// Text description of the pictures the user attached when creating the artifact, produced once
    /// (by the active model if it can see images, else the vision model / local OCR) and kept in a
    /// checkpoint so retries and later stages do not repeat it.
    /// What the user's attached files say about this request, read on demand by the model with
    /// the attachment tools once and kept in a checkpoint.
    async fn file_brief(
        &self,
        art: &StudioArtifact,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        cancel: CancellationToken,
    ) -> LnaResult<Option<String>> {
        let atts = art
            .config
            .get("files")
            .map(|v| attachments::resolve(&self.db.paths().root, v))
            .unwrap_or_default();
        let atts = context_files::context_items(&atts);
        if atts.is_empty() {
            return Ok(None);
        }
        let ids: Vec<&str> = atts.iter().map(|a| a.id.as_str()).collect();
        if let Some(saved) = self.get_checkpoint(artifact_id, "file_brief")? {
            let same = saved
                .get("ids")
                .and_then(Value::as_array)
                .map(|v| v.iter().filter_map(Value::as_str).eq(ids.iter().copied()))
                .unwrap_or(false);
            if same {
                if let Some(t) = saved.get("text").and_then(Value::as_str) {
                    return Ok(Some(t.to_string()));
                }
            }
        }
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "tool",
            &self.l("按需阅读附件", "Reading attachments on demand"),
            &self.l(
                &format!("正在按需读取用户附带的 {} 个参考文件/链接…", atts.len()),
                &format!(
                    "Reading the {} reference file(s)/link(s) you attached…",
                    atts.len()
                ),
            ),
        );
        let task = format!(
            "为生成学习成品「{}」收集材料。用户要求（config）：{}",
            art.artifact_type.as_str(),
            material::config_for_prompt(
                &art.config,
                !material::image_parts(&self.db.paths().root, art, resolved).is_empty()
            )
        );
        let ctx = context_files::ToolCtx {
            db: &self.db,
            secrets: &self.secrets,
            llm: &self.llm,
            notebook_id: &art.notebook_id,
            profile: resolved,
            atts: &atts,
        };
        let text = context_files::brief(&ctx, &task, 6, cancel).await?;
        self.save_checkpoint(
            artifact_id,
            "file_brief",
            json!({ "ids": ids, "text": text }),
        )?;
        Ok(Some(text).filter(|t| !t.trim().is_empty()))
    }

    async fn image_brief(
        &self,
        art: &StudioArtifact,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        cancel: CancellationToken,
    ) -> LnaResult<Option<String>> {
        let atts = art
            .config
            .get("images")
            .map(|v| attachments::resolve(&self.db.paths().root, v))
            .unwrap_or_default();
        if atts.is_empty() {
            return Ok(None);
        }
        let ids: Vec<&str> = atts.iter().map(|a| a.id.as_str()).collect();
        if let Some(saved) = self.get_checkpoint(artifact_id, "image_brief")? {
            let same = saved
                .get("ids")
                .and_then(Value::as_array)
                .map(|v| v.iter().filter_map(Value::as_str).eq(ids.iter().copied()))
                .unwrap_or(false);
            // a brief made by local OCR (no vision model then) is not reused once the active
            // model can read images: its text is far better
            let stale_ocr = !saved
                .get("vision")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && resolved.capabilities.supports_vision;
            if same && !stale_ocr {
                if let Some(t) = saved.get("text").and_then(Value::as_str) {
                    return Ok(Some(t.to_string()));
                }
            }
        }
        if cancel.is_cancelled() {
            return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
        }
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "tool",
            &self.l(
                "工具调用 · 识别附带图片",
                "Tool call · Reading attached images",
            ),
            &self.l(
                &format!("正在读取 {} 张图片", atts.len()),
                &format!("Reading {} image(s)", atts.len()),
            ),
        );
        let topic = art
            .config
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = attachments::describe(
            &self.db,
            &self.secrets,
            &self.llm,
            &art.notebook_id,
            resolved,
            &atts,
            topic,
        )
        .await;
        self.save_checkpoint(
            artifact_id,
            "image_brief",
            json!({ "ids": ids, "text": text, "vision": resolved.capabilities.supports_vision }),
        )?;
        Ok(Some(text))
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
        new_images: Vec<Attachment>,
    ) -> LnaResult<StudioArtifact> {
        let art = self.get(id)?;
        let has_material = !new_images.is_empty()
            || has_user_material(config.as_ref().unwrap_or(&art.config))
            || has_user_material(&art.config);
        let (source_ids, fp) = if use_selected_sources {
            let (selected_ids, cur_fp) = self.current_sources_fingerprint(&art.notebook_id)?;
            if selected_ids.is_empty() && !has_material {
                return Err(LnaError::new(
                    ErrorCode::NoSourceSelected,
                    "Please select at least one ready source",
                ));
            }
            (selected_ids, cur_fp)
        } else {
            (art.source_ids.clone(), art.sources_fingerprint.clone())
        };
        let mut cfg_val = config.unwrap_or_else(|| art.config.clone());
        // Pictures and files: a new set replaces the old one of the same kind; without new ones
        // the earlier attachments stay.
        if let Some(obj) = cfg_val.as_object_mut() {
            let new_pics = attachments::images(&new_images);
            let new_files = context_files::context_items(&new_images);
            for (key, fresh) in [("images", new_pics), ("files", new_files)] {
                if !fresh.is_empty() {
                    if let Some(old) = art.config.get(key) {
                        let old = attachments::resolve(&self.db.paths().root, old);
                        let stale: Vec<Attachment> = old
                            .into_iter()
                            .filter(|o| !fresh.iter().any(|n| n.id == o.id))
                            .collect();
                        attachments::delete_files(&stale);
                    }
                    obj.insert(key.into(), attachments::ids_of(&fresh));
                } else if !obj.contains_key(key) {
                    if let Some(old) = art.config.get(key) {
                        obj.insert(key.into(), old.clone());
                    }
                }
            }
        }
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
            "pptx" => {
                // figure assets live next to the artifact; (re)write them so the deck embeds them
                let asset_dir = art
                    .output_dir
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| self.db.paths().artifacts_dir.join(artifact_id));
                if let Err(e) = lecture::render_figure_assets(&payload, &asset_dir) {
                    tracing::warn!("figure assets for export: {}", e.message);
                }
                render::render_pptx_in(&payload, out_path, Some(&asset_dir))
            }
            _ => {
                let md =
                    render::artifact_markdown(art.artifact_type.as_str(), &art.title, &payload);
                std::fs::write(out_path, md).map_err(|e| LnaError::internal(e.to_string()))
            }
        }
    }

    /// Executors of one artifact: runs `n` part writers with at most `limit` in flight and returns
    /// their results in part order. The orchestrator awaits them as they finish and stays awake
    /// for cancellation; the first failure ends the whole run.
    async fn run_parts<F, Fut>(
        &self,
        n: usize,
        limit: usize,
        cancel: &CancellationToken,
        make: F,
    ) -> LnaResult<Vec<(Value, Usage)>>
    where
        F: Fn(usize) -> Fut,
        Fut: std::future::Future<Output = LnaResult<(Value, Usage)>>,
    {
        let mut running = futures_util::stream::iter((0..n).map(|i| {
            let fut = make(i);
            async move { (i, fut.await) }
        }))
        .buffer_unordered(limit.clamp(1, n.max(1)));
        let mut out: Vec<Option<(Value, Usage)>> = (0..n).map(|_| None).collect();
        while let Some((i, res)) = next_or_cancel(&mut running, cancel).await? {
            out[i] = Some(res?);
        }
        Ok(out.into_iter().flatten().collect())
    }

    /// Executors the orchestrators of the Studio types fan out to for the profile: its request
    /// limit, or `MAX_PARALLEL_REQUESTS` when the profile is unlimited (the client gate enforces
    /// the real limit for every request anyway).
    fn parallelism(&self, profile: &ModelProfile) -> usize {
        if profile.max_parallel == 0 {
            MAX_PARALLEL_REQUESTS as usize
        } else {
            profile.max_parallel.clamp(1, 64) as usize
        }
    }

    /// The request limit of the profile as a (Chinese, English) phrase for progress messages.
    fn parallel_phrase(&self, profile: &ModelProfile) -> (String, String) {
        if profile.max_parallel == 0 {
            (
                "同时请求数不限".into(),
                "with no limit on requests at a time".into(),
            )
        } else {
            let n = self.parallelism(profile);
            (
                format!("同时最多 {n} 个请求"),
                format!("at most {n} requests at a time"),
            )
        }
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
        self.run_structured_with_thought_ex(
            artifact_id,
            notebook_id,
            thought_title,
            profile,
            req,
            schema_name,
            schema,
            max_repairs,
            None,
            cancel,
        )
        .await
    }

    /// `array_key`: the flat item array of the answer; a length cut-off then keeps finished
    /// items and continues instead of rewriting the whole output.
    #[allow(clippy::too_many_arguments)]
    async fn run_structured_with_thought_ex(
        &self,
        artifact_id: &str,
        notebook_id: &str,
        thought_title: &str,
        profile: &ResolvedProfile,
        req: &LlmRequest,
        schema_name: &str,
        schema: &Value,
        max_repairs: usize,
        array_key: Option<&str>,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage)> {
        let mut seq: Option<i32> = None;
        let mut full_thought = String::new();
        let mut pending = String::new();
        let mut last_flush = std::time::Instant::now();
        // Streamed text is previewed only for the artifact's own payload call (schema name
        // `*_payload`, not the mind map, which previews from its checkpoints).
        let preview_type: Option<ArtifactType> = if schema_name.ends_with("_payload")
            && schema_name != "mindmap_payload"
            && schema_name != "lecture_payload"
        {
            self.get(artifact_id).ok().map(|a| a.artifact_type)
        } else {
            None
        };
        let mut last_preview = std::time::Instant::now();
        let res = self
            .llm
            .complete_structured_continuing(
                profile,
                req,
                schema_name,
                schema,
                cancel,
                max_repairs,
                array_key,
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
                        &self.l("结构化输出自动修复", "Structured output auto-repair"),
                        lna_llm::pick_status_lang(status_msg, self.ui_is_en()),
                    );
                },
                |text| {
                    let Some(ty) = preview_type else { return };
                    if last_preview.elapsed() < std::time::Duration::from_millis(500) {
                        return;
                    }
                    last_preview = std::time::Instant::now();
                    if let Some(mut v) = close_partial_json(text) {
                        sanitize_preview(ty, &mut v);
                        self.emit_preview(artifact_id, notebook_id, "stream", &v);
                    }
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

    /// Full text of the given sources as numbered evidence blocks: `(block count, text)`.
    fn load_full_source_text(&self, source_ids: &[String]) -> LnaResult<(usize, String)> {
        let ids_json = serde_json::to_string(source_ids).unwrap_or_else(|_| "[]".into());
        let rows: Vec<(String, Option<i32>, Option<String>, String)> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.title, c.page_start, c.section_path, c.content
                     FROM source_chunks c JOIN sources s ON s.id=c.source_id
                     WHERE c.source_id IN (SELECT value FROM json_each(?1))
                     ORDER BY s.title, c.chunk_index",
                )
                .map_err(map_sql_err)?;
            let mapped = stmt
                .query_map([&ids_json], |r| {
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
        let text = rows
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
            .join("\n\n");
        Ok((rows.len(), text))
    }

    /// Sources a generation reads: a knowledge graph's own source, else the selection.
    fn generation_source_ids(art: &StudioArtifact) -> Vec<String> {
        let is_kg = art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        art.config
            .get("source_id")
            .and_then(Value::as_str)
            .filter(|s| is_kg && !s.is_empty())
            .map(|s| vec![s.to_string()])
            .unwrap_or_else(|| art.source_ids.clone())
    }

    /// Whether the full text of these sources fits directly into a generation prompt.
    fn sources_fit_directly(&self, ids: &[String], resolved: &ResolvedProfile) -> LnaResult<bool> {
        let ids_json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
        let tokens: i64 = self.db.read(|conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(token_count),0) FROM sources WHERE id IN (SELECT value FROM json_each(?1)) AND status='READY'",
                [&ids_json],
                |r| r.get(0),
            )
            .map_err(map_sql_err)
        })?;
        let window = resolved.context_window as usize;
        let reserved_output = (resolved.output_budget() as usize).min(window / 4);
        let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
        let budget = available_input.saturating_mul(70) / 100;
        Ok(tokens.max(0) as usize <= budget.min(12_000))
    }

    fn resolve_active_profile(&self) -> LnaResult<(ModelProfile, ResolvedProfile)> {
        let profile = lna_storage::get_active_profile(&self.db)?.ok_or_else(|| {
            LnaError::new(ErrorCode::ModelNotConfigured, "No active model configured")
        })?;
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
            preset: profile.preset.clone(),
            shrink_media: profile.shrink_media,
        };
        Ok((profile, resolved))
    }

    /// Mind map / knowledge graph in several passes so size is never limited by one output:
    /// pass 1 initialises the skeleton (root + every top-level branch), then each branch is
    /// re-read from the sources (multi-round tool reading for large sources) and its detailed
    /// descendants are appended. Progress is checkpointed per branch so retries resume.
    #[allow(clippy::too_many_arguments)]
    async fn generate_mindmap_multipass(
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
        let is_kg = art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let pictures = material::image_parts(&self.db.paths().root, art, resolved);
        let pictures_shown = !pictures.is_empty();
        let detail = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("standard");
        let kind = if is_kg { "knowledge graph" } else { "mind map" };
        let rules = mindmap_rules(is_kg, detail);
        let schema: Value =
            serde_json::from_str(SCHEMA_MINDMAP).map_err(|e| LnaError::internal(e.to_string()))?;
        let mut usage_sum = Usage::default();
        let target_ids = Self::generation_source_ids(art);
        let full_text = if self.sources_fit_directly(&target_ids, resolved)? {
            Some(evidence_txt)
        } else {
            None
        };

        let saved = self
            .get_checkpoint(artifact_id, "mindmap_passes")?
            .filter(|v| {
                v.get("sources_fingerprint").and_then(Value::as_str)
                    == Some(art.sources_fingerprint.as_str())
            });
        let (mut payload, mut done): (Value, Vec<String>) = match saved {
            Some(v) => {
                let done: Vec<String> = v
                    .get("done")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l("复用已完成的图谱分支", "Reusing completed graph branches"),
                    &self.l(&format!(
                        "已从检查点恢复图谱骨架与 {} 个已追加的分支，继续处理剩余分支。",
                        done.len()
                    ), &format!(
                        "Restored the graph skeleton and {} already-added branches from the checkpoint; continuing with the remaining branches.",
                        done.len()
                    )),
                );
                (v.get("payload").cloned().unwrap_or_else(|| json!({})), done)
            }
            None => {
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l("初始化图谱骨架", "Initializing the graph skeleton"),
                    &self.l("第 1 步：搭建根节点与全部一级分支；之后逐个分支重新查阅原文，追加更细的知识点节点。", "Step 1: build the root node and all first-level branches; then revisit the source for each branch and add finer knowledge-point nodes."),
                );
                let req = material::with_images(LlmRequest {
                    system: format!(
                        "You are the Local Notebook Studio generator producing structured JSON for mindmap ({kind}), built in several passes. THIS IS PASS 1 (initialization): output ONLY the skeleton as structured JSON — the root node and the top-level branches (3-7 major themes of the requested scope that together cover all of it; omit nothing, but merge thin chapters into one theme and split none just because the book has many chapters). Do NOT output any deeper nodes: later passes re-read the sources branch by branch and add the topics and knowledge points. Each top-level branch is a reorganized theme, not a copy of a chapter title; its \"summary\" states in one or two sentences what the theme is about and which chapters / pages it draws from. Use short ids such as \"b1\", \"b2\".{rules} Strictly ground the structure in the provided sources. Match the language requested in config or the primary language of the sources."
                    ),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text(format!(
                            "Parameters: {}\n\nSources:\n{}",
                            material::config_for_prompt(&art.config, pictures_shown), evidence_txt
                        ))],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.max_output_tokens,
                    reasoning: reasoning.clone(),
                }, &pictures);
                let (mut skeleton, u) = self
                    .run_structured_with_thought(
                        artifact_id,
                        &art.notebook_id,
                        &self.l("图谱骨架规划思考", "Graph skeleton planning reasoning"),
                        resolved,
                        &req,
                        "mindmap_payload",
                        &schema,
                        max_repairs,
                        cancel.clone(),
                    )
                    .await?;
                add_usage(&mut usage_sum, &u);
                validate::normalize_mindmap(&mut skeleton);
                // The skeleton is root + top-level themes only; anything deeper the model
                // produced is folded into the theme's facets and re-derived by the branch pass.
                if let Some(root) = mm_root_id(&mm_nodes(&skeleton)) {
                    mm_enforce_shape(&mut skeleton, &root, 1);
                }
                if let Some(obj) = skeleton.as_object_mut() {
                    obj.insert(
                        "kind".into(),
                        json!(if is_kg { "knowledge_graph" } else { "mindmap" }),
                    );
                }
                self.save_checkpoint(
                    artifact_id,
                    "mindmap_passes",
                    json!({ "sources_fingerprint": art.sources_fingerprint, "payload": skeleton, "done": [] }),
                )?;
                (skeleton, Vec::new())
            }
        };

        let nodes = mm_nodes(&payload);
        let Some(root) = mm_root_id(&nodes) else {
            return Ok((payload, usage_sum));
        };
        let branches: Vec<(String, String)> = mm_children(&nodes, &root)
            .iter()
            .map(|n| (mm_str(n, "id"), mm_str(n, "label")))
            .collect();
        let total = branches.len();
        let pending: Vec<(usize, String, String)> = branches
            .iter()
            .enumerate()
            .filter(|(_, (bid, _))| !done.contains(bid))
            .map(|(i, (bid, label))| (i, bid.clone(), label.clone()))
            .collect();
        // Branches are independent (own ids, same skeleton titles), so several are generated
        // at once. Each task works on a local copy; only this task merges and checkpoints.
        let concurrency = self.parallelism(profile).min(pending.len().max(1));
        let base = payload.clone();
        let mut queue = pending.into_iter();
        let mut running = futures_util::stream::FuturesUnordered::new();
        let mut finished = done.len();
        let mut first_error: Option<LnaError> = None;
        loop {
            while running.len() < concurrency && first_error.is_none() {
                let Some((i, bid, blabel)) = queue.next() else {
                    break;
                };
                if cancel.is_cancelled() {
                    first_error = Some(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                    break;
                }
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l(&format!("追加分支「{blabel}」（共 {total} 个分支，并行 {concurrency} 个）"), &format!("Expanding branch \"{blabel}\" ({total} branches, {concurrency} in parallel)")),
                    &self.l("正在针对该分支查阅原文，并追加更细的知识点节点…", "Reading the source for this branch and adding finer knowledge-point nodes…"),
                );
                let base_ref = &base;
                let (rules_ref, reasoning_c, cancel_c) =
                    (&rules, reasoning.clone(), cancel.clone());
                running.push(async move {
                    let res = self
                        .process_mindmap_branch(
                            art,
                            profile,
                            resolved,
                            artifact_id,
                            base_ref,
                            &bid,
                            full_text,
                            rules_ref,
                            reasoning_c,
                            max_repairs,
                            cancel_c,
                        )
                        .await;
                    (i, bid, blabel, res)
                });
            }
            let Some((_i, bid, blabel, res)) = futures_util::StreamExt::next(&mut running).await
            else {
                break;
            };
            match res {
                Ok((local, u, added, regrouped, retried)) => {
                    add_usage(&mut usage_sum, &u);
                    mm_adopt_branch(&mut payload, &local, &bid);
                    let kept = mm_nodes(&payload).len();
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "tool",
                        &self.l(&format!("工具调用 · 追加节点：{blabel}"), &format!("Tool call · Adding nodes: {blabel}")),
                        &self.l(
                            &format!(
                                "已为该分支追加 {added} 个节点{}；新建 {regrouped} 个分组；全图目前 {kept} 个节点。",
                                if retried { "（首次失败，重试后成功）" } else { "" }
                            ),
                            &format!(
                                "Added {added} node(s) to this branch{}; created {regrouped} group(s); the graph now has {kept} nodes.",
                                if retried { " (failed at first, succeeded on retry)" } else { "" }
                            ),
                        ),
                    );
                }
                Err(e) if e.code == ErrorCode::Cancelled || cancel.is_cancelled() => {
                    first_error.get_or_insert(e);
                    continue;
                }
                Err(e) => {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l(&format!("分支追加失败：{blabel}"), &format!("Branch expansion failed: {blabel}")),
                        &self.l(&format!(
                            "{}。已重试仍失败，保留该分支的骨架节点，可稍后对其使用「细化」。",
                            truncate_str(&e.message, 200)
                        ), &format!(
                            "{}. Still failing after a retry; keeping the branch's skeleton nodes — you can use \"Refine\" on it later.",
                            truncate_str(&e.message, 200)
                        )),
                    );
                }
            }
            done.push(bid.clone());
            finished += 1;
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l(
                    &format!("已完成 {finished}/{total} 个分支"),
                    &format!("Finished {finished}/{total} branches"),
                ),
                "",
            );
            self.save_checkpoint(
                artifact_id,
                "mindmap_passes",
                json!({ "sources_fingerprint": art.sources_fingerprint, "payload": payload, "done": done }),
            )?;
            self.emit_preview(artifact_id, &art.notebook_id, "mindmap_passes", &payload);
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        let order: Vec<String> = branches.iter().map(|(b, _)| b.clone()).collect();
        mm_sort_by_branch_order(&mut payload, &order);
        let merged = mm_dedup_labels(&mut payload);
        if merged > 0 {
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l("合并重复节点", "Merging duplicate nodes"),
                &self.l(&format!("按标题归一化后合并了 {merged} 个跨分支重复的节点。"), &format!("Merged {merged} duplicate node(s) across branches after normalizing titles.")),
            );
        }
        match self
            .review_mindmap_structure(
                art,
                resolved,
                artifact_id,
                &mut payload,
                reasoning.clone(),
                max_repairs,
                cancel.clone(),
            )
            .await
        {
            Ok((n, u)) => {
                add_usage(&mut usage_sum, &u);
                if n > 0 {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l("结构审查", "Structure review"),
                        &self.l(
                            &format!("按审查建议调整了 {n} 处主题的位置或合并。"),
                            &format!("Moved or merged {n} topic(s) per the review suggestions."),
                        ),
                    );
                }
            }
            Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                return Err(e)
            }
            Err(_) => {}
        }
        mm_mark_expandable(&mut payload);
        validate::normalize_mindmap(&mut payload);
        Ok((payload, usage_sum))
    }

    /// One branch append pass on a local copy of the skeleton: read sources, append at most
    /// two levels, enforce the shape rules, regroup crowded parents. A failed attempt is
    /// retried once with a smaller scope. Returns `(local payload, usage, added, regrouped, retried)`.
    #[allow(clippy::too_many_arguments)]
    async fn process_mindmap_branch(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        base: &Value,
        bid: &str,
        full_text: Option<&str>,
        rules: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(Value, Usage, usize, usize, bool)> {
        let mut local = base.clone();
        let mut usage = Usage::default();
        let mut retried = false;
        let mut attempt_rules = rules.to_string();
        let exp = loop {
            match self
                .expand_mindmap_node(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    &local,
                    bid,
                    false,
                    full_text,
                    &attempt_rules,
                    reasoning.clone(),
                    max_repairs,
                    cancel.clone(),
                )
                .await
            {
                Ok((exp, u)) => {
                    add_usage(&mut usage, &u);
                    break exp;
                }
                Err(e) if e.code == ErrorCode::Cancelled || cancel.is_cancelled() => return Err(e),
                Err(e) if !retried => {
                    retried = true;
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l(
                            "分支追加失败，缩小范围重试",
                            "Branch expansion failed; retrying with a smaller scope",
                        ),
                        &truncate_str(&e.message, 200),
                    );
                    attempt_rules = format!(
                        "{rules} RETRY: the previous attempt failed. Produce a smaller, simpler answer: at most 25 nodes in total, concise summaries, and no more than 2 facets per node."
                    );
                }
                Err(e) => return Err(e),
            }
        };
        let mut exp = exp;
        // Deviation check: a branch far outside the range expected for the detail level is
        // regenerated once with feedback (merge or split) instead of being truncated.
        let detail = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("standard");
        let (lo, hi) = match detail {
            "concise" => (4usize, 28usize),
            "detailed" => (12, 90),
            _ => (6, 55),
        };
        let n_new = exp.nodes.len();
        if !retried && (n_new > hi || n_new < lo) && !cancel.is_cancelled() {
            let feedback = if n_new > hi {
                format!(
                    "{rules} FEEDBACK: your previous answer had {n_new} nodes, far too fine for this detail level. Merge related knowledge points into fewer, broader ones (about {} nodes at most) and put minor items into facets.",
                    hi * 3 / 4
                )
            } else {
                format!(
                    "{rules} FEEDBACK: your previous answer had only {n_new} nodes, too coarse for the material of this branch. Cover the branch's sub-topics and methods more fully (at least {lo} nodes), reading the sources again if needed."
                )
            };
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l("粒度偏离，带反馈重新生成该分支", "Granularity off; regenerating the branch with feedback"),
                &self.l(&format!("首次生成 {n_new} 个节点，超出该详细程度的合理范围（{lo}–{hi}）。"), &format!("The first pass produced {n_new} nodes, outside the reasonable range for this detail level ({lo}–{hi}).")),
            );
            if let Ok((again, u)) = self
                .expand_mindmap_node(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    &local,
                    bid,
                    false,
                    full_text,
                    &feedback,
                    reasoning.clone(),
                    max_repairs,
                    cancel.clone(),
                )
                .await
            {
                add_usage(&mut usage, &u);
                let m = again.nodes.len();
                let better = if n_new > hi {
                    m < n_new && m > 0
                } else {
                    m > n_new
                };
                if better {
                    exp = again;
                }
            }
        }
        let added = merge_mindmap_subtree(&mut local, bid, exp.nodes);
        // Shape rules: at most topic + knowledge-point levels under the branch, no
        // single-child parents, and crowded parents regrouped into topics.
        let mut crowded = mm_enforce_shape(&mut local, bid, 2);
        let mut regrouped = 0;
        for pid in crowded.drain(..).take(6) {
            match self
                .regroup_mindmap_children(
                    art,
                    profile,
                    resolved,
                    artifact_id,
                    &mut local,
                    &pid,
                    reasoning.clone(),
                    max_repairs,
                    cancel.clone(),
                )
                .await
            {
                Ok((n, u)) => {
                    add_usage(&mut usage, &u);
                    regrouped += n;
                }
                Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                    return Err(e)
                }
                Err(_) => {}
            }
        }
        if regrouped > 0 {
            mm_enforce_shape(&mut local, bid, 3);
        }
        Ok((local, usage, added, regrouped, retried))
    }

    /// Generates new descendants for one node: a branch append pass (`refine == false`) or a
    /// user-requested refinement, where the model may decline because the node is atomic.
    #[allow(clippy::too_many_arguments)]
    async fn expand_mindmap_node(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        payload: &Value,
        node_id: &str,
        refine: bool,
        full_text: Option<&str>,
        rules: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(NodeExpansion, Usage)> {
        let pictures = material::image_parts(&self.db.paths().root, art, resolved);
        let pictures_shown = !pictures.is_empty();
        let nodes = mm_nodes(payload);
        let node = nodes
            .iter()
            .find(|n| mm_str(n, "id") == node_id)
            .ok_or_else(|| LnaError::new(ErrorCode::NotFound, "Node not found"))?;
        let label = mm_str(node, "label");
        let summary = mm_str(node, "summary");
        let facets: Vec<String> = node
            .get("facets")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let path = mm_ancestor_labels(&nodes, node_id);
        let mut outline = String::new();
        mm_outline(&nodes, node_id, 0, &mut outline);
        let root = mm_root_id(&nodes).unwrap_or_default();
        let own_branch = mm_top_branch(&nodes, node_id, &root);
        let others: Vec<String> = mm_children(&nodes, &root)
            .iter()
            .filter(|n| Some(mm_str(n, "id")) != own_branch)
            .map(|n| mm_str(n, "label"))
            .collect();
        let focus = {
            let mut f = path.join(" > ");
            if !f.is_empty() {
                f.push_str(" > ");
            }
            f.push_str(&label);
            if !summary.is_empty() {
                f.push_str(" — ");
                f.push_str(&summary);
            }
            f
        };

        let evidence = match full_text {
            Some(t) => t.to_string(),
            None => {
                let target_ids = Self::generation_source_ids(art);
                let window = resolved.context_window as usize;
                let reserved_output = (resolved.output_budget() as usize).min(window / 4);
                let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
                let budget = (available_input.saturating_mul(70) / 100).min(16_000);
                match self
                    .gather_evidence_on_demand(
                        art,
                        profile,
                        resolved,
                        artifact_id,
                        Some(&focus),
                        budget,
                        Some(&target_ids),
                        true,
                        None,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(Some((t, _))) => t,
                    Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                        return Err(e)
                    }
                    _ => {
                        let mut queries = vec![label.clone()];
                        if let Some(parent) = path.last() {
                            queries.push(format!("{parent} {label}"));
                        }
                        match self
                            .retrieval
                            .search(&art.notebook_id, &queries, &target_ids, 6_000, 12)
                            .await
                        {
                            Ok(res) => res
                                .evidences
                                .iter()
                                .enumerate()
                                .map(|(i, e)| {
                                    format!("[{}] ({}) {}", i + 1, e.source_title, e.content)
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n"),
                            Err(_) => String::new(),
                        }
                    }
                }
            }
        };

        let base: Value =
            serde_json::from_str(SCHEMA_MINDMAP).map_err(|e| LnaError::internal(e.to_string()))?;
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["nodes"],
            "properties": {
                "title": { "type": "string" },
                "atomic": { "type": "boolean" },
                "reason": { "type": "string" },
                "nodes": base["properties"]["nodes"].clone()
            }
        });
        let is_kg = art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let kind = if is_kg { "knowledge graph" } else { "mind map" };
        let task = if refine {
            format!(
                "The learner asked to REFINE the node \"{label}\" into finer knowledge points. First decide whether it is already a minimal, indivisible knowledge point (a single definition, formula, fact or step whose parts cannot be learned or tested separately). If it is, return {{\"atomic\": true, \"reason\": <one short sentence in the learner's language explaining why>, \"nodes\": []}}. Otherwise return \"atomic\": false and the NEW child nodes (normally one level, two only when necessary) that split it into separately learnable knowledge points — distinct methods, cases, theorems, forms or steps — turning its facets into children where they deserve it. Never repeat existing children."
            )
        } else {
            format!(
                "THIS IS AN APPEND PASS for the top-level branch \"{label}\" of a {kind} whose skeleton already exists. Using the source excerpts, output ONLY the NEW descendant nodes of this branch, in AT MOST TWO LEVELS: topic nodes directly under the branch, and knowledge-point nodes under each topic, at the requested detail level. Anything finer goes into facets; the learner can expand a knowledge point later on demand. Do not repeat nodes listed under \"Existing subtree\" and do not cover material that belongs to the other top-level branches."
            )
        };
        let req = material::with_images(LlmRequest {
            // The system prompt is the same for every branch of the map; what is specific to this
            // branch (task, node, subtree) goes at the end of the user message, after the shared
            // parameters and the sources, so parallel branches share the longest possible prefix.
            system: format!(
                "You are the Local Notebook Studio generator producing structured JSON for mindmap ({kind}). Follow the TASK at the end of the user message.{rules} Strictly ground everything in the provided sources. Match the language requested in config or the primary language of the sources."
            ),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(format!(
                    "Parameters: {}\n\nSources:\n{evidence}\n\n=== TASK ===\n{task} Every new node needs a unique new \"id\" and a \"parent_id\" that is either \"{node_id}\", an id from the existing subtree, or the id of another new node you output.\nPath: {}\nNode: [{node_id}] {label}\nSummary: {summary}\nFacets: {}\nExisting subtree:\n{outline}\nOther top-level branches (out of scope): {}",
                                material::config_for_prompt(&art.config, pictures_shown),
                    if path.is_empty() { "(root)".to_string() } else { path.join(" > ") },
                    if facets.is_empty() { "(none)".to_string() } else { facets.join("; ") },
                    if others.is_empty() { "(none)".to_string() } else { others.join("; ") },
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning,
        }, &pictures);
        let (out, usage) = self
            .run_structured_with_thought_ex(
                artifact_id,
                &art.notebook_id,
                &if refine {
                    self.l(
                        &format!("细化「{label}」思考"),
                        &format!("Refining \"{label}\" reasoning"),
                    )
                } else {
                    self.l(
                        &format!("分支「{label}」追加思考"),
                        &format!("Branch \"{label}\" expansion reasoning"),
                    )
                },
                resolved,
                &req,
                "mindmap_branch",
                &schema,
                max_repairs,
                Some("nodes"),
                cancel,
            )
            .await?;
        Ok((
            NodeExpansion {
                atomic: out.get("atomic").and_then(Value::as_bool).unwrap_or(false),
                reason: mm_str(&out, "reason"),
                nodes: out
                    .get("nodes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            },
            usage,
        ))
    }

    /// One cheap review pass over the finished map (titles and summaries of branches and
    /// topics only): moves topics that sit under the wrong branch and merges overlapping
    /// topics. Returns the number of changes applied.
    #[allow(clippy::too_many_arguments)]
    async fn review_mindmap_structure(
        &self,
        art: &StudioArtifact,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        payload: &mut Value,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(usize, Usage)> {
        let nodes = mm_nodes(payload);
        let Some(root) = mm_root_id(&nodes) else {
            return Ok((0, Usage::default()));
        };
        let mut listing = String::new();
        for b in mm_children(&nodes, &root) {
            let bid = mm_str(b, "id");
            listing.push_str(&format!(
                "BRANCH [{bid}] {} — {}\n",
                mm_str(b, "label"),
                mm_str(b, "summary")
            ));
            for t in mm_children(&nodes, &bid) {
                listing.push_str(&format!(
                    "  TOPIC [{}] {} — {}\n",
                    mm_str(t, "id"),
                    mm_str(t, "label"),
                    mm_str(t, "summary")
                ));
            }
        }
        if listing.lines().count() < 6 {
            return Ok((0, Usage::default()));
        }
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["moves", "merges"],
            "properties": {
                "moves": { "type": "array", "items": { "type": "object", "additionalProperties": false,
                    "required": ["topic_id", "new_branch_id"],
                    "properties": { "topic_id": {"type":"string"}, "new_branch_id": {"type":"string"}, "reason": {"type":"string"} } } },
                "merges": { "type": "array", "items": { "type": "object", "additionalProperties": false,
                    "required": ["keep_id", "drop_id"],
                    "properties": { "keep_id": {"type":"string"}, "drop_id": {"type":"string"}, "reason": {"type":"string"} } } }
            }
        });
        let req = LlmRequest {
            system: "You review the structure of a knowledge map. You see its top-level BRANCHES and their TOPICS (title and summary only). Return structured JSON with: \"moves\" — topics that clearly belong to a different branch (e.g. a concept that a later branch builds on sits in the wrong theme), each with the id of the better branch; \"merges\" — two topics that cover the same ground (keep one, drop the other; both must be under the same or different branches). Be conservative: only clear problems, at most 6 changes in total, empty arrays when the structure is fine. Use only ids that appear in the listing.".into(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(listing)],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.bounded_output(2000),
            reasoning,
        };
        let (out, usage) = self
            .run_structured_with_thought(
                artifact_id,
                &art.notebook_id,
                &self.l("图谱结构审查思考", "Graph structure review reasoning"),
                resolved,
                &req,
                "mindmap_review",
                &schema,
                max_repairs.min(1),
                cancel,
            )
            .await?;
        let moves: Vec<(String, String)> = out
            .get("moves")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|m| (mm_str(m, "topic_id"), mm_str(m, "new_branch_id")))
                    .collect()
            })
            .unwrap_or_default();
        let merges: Vec<(String, String)> = out
            .get("merges")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|m| (mm_str(m, "keep_id"), mm_str(m, "drop_id")))
                    .collect()
            })
            .unwrap_or_default();
        Ok((mm_apply_review(payload, &moves, &merges), usage))
    }

    /// Asks the model to group the children of a crowded parent (more than
    /// `MM_MAX_CHILDREN`) under 3-7 intermediate topics, then applies the grouping.
    #[allow(clippy::too_many_arguments)]
    async fn regroup_mindmap_children(
        &self,
        art: &StudioArtifact,
        _profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        payload: &mut Value,
        parent_id: &str,
        reasoning: ReasoningOptions,
        max_repairs: usize,
        cancel: CancellationToken,
    ) -> LnaResult<(usize, Usage)> {
        let nodes = mm_nodes(payload);
        let Some(parent) = nodes.iter().find(|n| mm_str(n, "id") == parent_id) else {
            return Ok((0, Usage::default()));
        };
        let parent_label = mm_str(parent, "label");
        let listing: String = mm_children(&nodes, parent_id)
            .iter()
            .map(|c| {
                let s = mm_str(c, "summary");
                if s.is_empty() {
                    format!("- [{}] {}\n", mm_str(c, "id"), mm_str(c, "label"))
                } else {
                    format!("- [{}] {} — {}\n", mm_str(c, "id"), mm_str(c, "label"), s)
                }
            })
            .collect();
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["groups"],
            "properties": {
                "groups": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label", "summary", "members"],
                        "properties": {
                            "label": { "type": "string" },
                            "summary": { "type": "string" },
                            "members": { "type": "array", "items": { "type": "string" } }
                        }
                    }
                }
            }
        });
        let req = LlmRequest {
            system: "You are the Local Notebook Studio generator. A node of a mind map / knowledge graph has too many children. Group them into 3-7 cohesive intermediate topics by concept (not by order of appearance). Output {\"groups\": [{\"label\", \"summary\", \"members\": [child ids]}]}. Every child id belongs to exactly one group; a child that fits no group may be left out and stays directly under the node. Labels are short; each summary is one or two sentences that synthesize what the members share. Match the language of the labels.".into(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(format!(
                    "Node: [{parent_id}] {parent_label}\nChildren:\n{listing}"
                ))],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: resolved.max_output_tokens,
            reasoning,
        };
        let (out, usage) = self
            .run_structured_with_thought(
                artifact_id,
                &art.notebook_id,
                &self.l(
                    &format!("重新分组「{parent_label}」思考"),
                    &format!("Regrouping \"{parent_label}\" reasoning"),
                ),
                resolved,
                &req,
                "mindmap_groups",
                &schema,
                max_repairs,
                cancel,
            )
            .await?;
        let groups = out
            .get("groups")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok((mm_apply_groups(payload, parent_id, &groups), usage))
    }

    /// Splits one mind-map / knowledge-graph node into finer child nodes from the sources.
    /// The model may decline when the node is already a minimal knowledge point; such nodes
    /// are marked `atomic` so they cannot be refined again (no endless subdivision).
    pub async fn refine_node(&self, artifact_id: &str, node_id: &str) -> LnaResult<Value> {
        let art = self.get(artifact_id)?;
        if art.artifact_type != ArtifactType::Mindmap {
            return Err(LnaError::invalid(
                "Only mind maps and knowledge graphs can be refined",
            ));
        }
        let payload = art
            .payload
            .clone()
            .ok_or_else(|| LnaError::new(ErrorCode::Conflict, "Artifact has no content yet"))?;
        let nodes = mm_nodes(&payload);
        let node = nodes
            .iter()
            .find(|n| mm_str(n, "id") == node_id)
            .ok_or_else(|| LnaError::new(ErrorCode::NotFound, "Node not found"))?;
        let label = mm_str(node, "label");
        if node.get("atomic").and_then(Value::as_bool) == Some(true) {
            return Ok(
                json!({ "refined": false, "atomic": true, "added": 0, "reason": "", "artifact": art }),
            );
        }
        let (profile, resolved) = self.resolve_active_profile()?;
        let is_kg = art.config.get("mode").and_then(Value::as_str) == Some("knowledge_graph");
        let detail = art
            .config
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("standard");
        let rules = mindmap_rules(is_kg, detail);
        let target_ids = Self::generation_source_ids(&art);
        let full = if self.sources_fit_directly(&target_ids, &resolved)? {
            Some(self.load_full_source_text(&target_ids)?.1)
        } else {
            None
        };
        let reasoning = ReasoningOptions {
            show_summary: resolved.show_thinking || resolved.capabilities.reasoning_summary,
            effort: resolved.effort.clone(),
        };
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            &self.l(
                &format!("细化知识点：{label}"),
                &format!("Refining knowledge point: {label}"),
            ),
            &self.l(
                "正在查阅原文，判断该知识点能否继续拆分…",
                "Reading the source to decide whether this point can be split further…",
            ),
        );
        let (exp, usage) = self
            .expand_mindmap_node(
                &art,
                &profile,
                &resolved,
                artifact_id,
                &payload,
                node_id,
                true,
                full.as_deref(),
                &rules,
                reasoning,
                2,
                CancellationToken::new(),
            )
            .await?;
        let _ = lna_storage::record_usage(
            &self.db,
            Some(&art.notebook_id),
            Some(&profile.id),
            "studio_refine_node",
            Some(artifact_id),
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cached_tokens as i64,
            false,
        );

        // Merge into the latest stored payload so a concurrent refine isn't lost.
        let mut latest = self.get(artifact_id)?.payload.unwrap_or(payload);
        let added = if exp.atomic {
            0
        } else {
            merge_mindmap_subtree(&mut latest, node_id, exp.nodes)
        };
        if added == 0 {
            if let Some(arr) = latest.get_mut("nodes").and_then(Value::as_array_mut) {
                for n in arr.iter_mut() {
                    if mm_str(n, "id") == node_id {
                        if let Some(obj) = n.as_object_mut() {
                            obj.insert("atomic".into(), Value::Bool(true));
                        }
                    }
                }
            }
        }
        mm_enforce_shape(&mut latest, node_id, 2);
        validate::normalize_mindmap(&mut latest);
        mm_mark_expandable(&mut latest);
        let payload_str = latest.to_string();
        self.db.write(|conn| {
            conn.execute(
                "UPDATE studio_artifacts SET payload_json=?1, updated_at=?2 WHERE id=?3",
                params![payload_str, now_ms(), artifact_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
        let reason = if added > 0 {
            String::new()
        } else if !exp.reason.trim().is_empty() {
            exp.reason
        } else {
            self.l(
                "未在来源中找到可进一步拆分的内容，已标记为最小知识点。",
                "No further split was found in the sources; marked as a minimal knowledge point.",
            )
        };
        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            &self.l(
                &format!("细化完成：{label}"),
                &format!("Refinement done: {label}"),
            ),
            &if added > 0 {
                self.l(
                    &format!("已追加 {added} 个更细的子知识点。"),
                    &format!("Added {added} finer sub-point(s)."),
                )
            } else {
                self.l(
                    &format!("该知识点已是最小单位：{reason}"),
                    &format!("This point is already minimal: {reason}"),
                )
            },
        );
        self.emit_status_event(artifact_id, "READY", "READY", 1.0, None, None);
        Ok(json!({
            "refined": added > 0,
            "atomic": added == 0,
            "added": added,
            "reason": reason,
            "artifact": self.get(artifact_id)?
        }))
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
    #[allow(clippy::too_many_arguments)]
    async fn gather_evidence_on_demand(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        topic_query: Option<&str>,
        token_budget: usize,
        source_ids_override: Option<&[String]>,
        branch_focus: bool,
        finish: Option<&ReaderFinish>,
        cancel: CancellationToken,
    ) -> LnaResult<Option<(String, Option<Value>)>> {
        let is_mindmap = art.artifact_type == ArtifactType::Mindmap;
        let effective_source_ids = source_ids_override.unwrap_or(&art.source_ids);
        let selected_json =
            serde_json::to_string(effective_source_ids).unwrap_or_else(|_| "[]".into());
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
            let idx_limit: i64 = if is_mindmap { 400 } else { 120 };
            #[allow(clippy::type_complexity)]
            let idx_nodes: Vec<(
                String,
                i32,
                String,
                String,
                Option<i32>,
                Option<i32>,
            )> = self.db.read(|conn| {
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
        let catalog_cap = if is_mindmap { 48_000 } else { 24_000 };
        let catalog_text = truncate_str(&catalog_lines.join("\n"), catalog_cap).to_string();

        self.append_thought(
            artifact_id,
            &art.notebook_id,
            "stage",
            &self.l("按需查阅目录与章节", "Browsing the outline and chapters on demand"),
            &self.l("正在浏览来源目录与结构大纲，按需选择要精读的章节与片段（避免一次性传入整本书）…", "Browsing the source outline and structure, picking chapters and passages to read closely (instead of sending the whole book at once)…"),
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
            ToolDef {
                name: "abort_task".into(),
                description: "Call this only when the task cannot be done faithfully — e.g. the user's attached picture or material is unreadable or missing, or the request contradicts the sources. It stops the whole job and shows the reason to the user. Never guess or invent content instead.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "reason": { "type": "string", "description": "Why the task cannot be done faithfully" }
                    },
                    "required": ["reason"]
                }),
            },
        ];

        let user_material = has_user_material(&art.config);
        // a model that sees images gets the pictures in its requests; the text briefs are for the
        // others (a picture brief from an earlier non-vision run is not shown to a vision model)
        let pictures = material::image_parts(&self.db.paths().root, art, resolved);
        // the readers must see what the user's material says, or they cannot tell what to read
        let mut material_txt = String::new();
        for (key, tag) in [("file_brief", "user_files"), ("image_brief", "user_images")] {
            if key == "image_brief" && !pictures.is_empty() {
                continue;
            }
            if let Ok(Some(saved)) = self.get_checkpoint(artifact_id, key) {
                if let Some(t) = saved
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                {
                    material_txt.push_str(&format!("<{tag}>\n{t}\n</{tag}>\n\n"));
                }
            }
        }
        let planner_instruction = if branch_focus {
            "Focus ONLY on the branch/topic given above (other branches are handled separately). Using the reading tools, read the parts of the sources that cover this branch in depth: read_index_node for its outline nodes, read_source_pages for its page ranges, search_sources for specific methods, theorems, formulas or cases. Make several rounds of calls when needed so that every sub-topic, method, theorem and formula of this branch is backed by actual source text, not just outline titles. Reply \"DONE\" only when the branch is fully covered."
        } else if is_mindmap {
            "Use the reading tools to inspect the table of contents (e.g., early pages via read_source_pages) if the outline is coarse, and read key sections across ALL major chapters/themes evenly (call multiple tools in parallel) so every branch has concrete sub-topics, definitions, theorems, formulas, and details. Do NOT reply \"DONE\" until all major chapters/themes in scope are covered at uniform depth."
        } else if finish.is_some() {
            // how the reader ends is given by `finish` (appended below)
            "Use the reading tools to fetch the specific sections or passages this artifact needs (not unrelated material). Call multiple tools in parallel and continue over several rounds when parts of the requested scope are still unread; every tool result shows you the passages it added."
        } else {
            "Use the reading tools to fetch the specific sections or passages this artifact needs (not unrelated material). Call multiple tools in parallel and continue over several rounds when parts of the requested scope are still unread. Reply with \"DONE\" (without further tool calls) once the collected excerpts actually cover the requested scope."
        };
        let planner_instruction = if user_material {
            format!("{planner_instruction}\n\n{USER_MATERIAL_SCOPE}")
        } else {
            planner_instruction.to_string()
        };
        let planner_instruction = format!(
            "{planner_instruction}\n\nIf the user's attached material or the sources cannot support this task faithfully (for example a picture is unreadable or missing, or the request contradicts the sources), you may call abort_task with the reason instead of guessing.{}",
            finish
                .map(|f| format!("\n\n{}", f.instruction))
                .unwrap_or_default()
        );
        let mut plan: Option<Value> = None;
        let mut messages = vec![LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(format!(
                // the catalog is the same for every reader of these sources: it goes before the
                // topic/focus, which differs per section or branch (prompt-cache prefix)
                "Artifact type: {}\nUser config: {}\n\nSource Catalog & Outline:\n{}\n\n{}User topic/focus: {}\n\n{}",
                art.artifact_type.as_str(),
                material::config_for_prompt(&art.config, !pictures.is_empty()),
                catalog_text,
                material_txt,
                topic_query.unwrap_or("Full selected scope (balanced coverage)"),
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
            collected.push(format!(
                "[{}] ({}) {}",
                collected.len() + 1,
                header,
                snippet
            ));
            true
        }

        let max_rounds: usize = if branch_focus {
            5
        } else if is_mindmap {
            8
        } else {
            6
        };
        let planner_system = if branch_focus {
            "You are a thorough knowledge-structure researcher for Local Notebook Studio. You are deepening ONE branch of a knowledge map: read the source sections behind that branch in detail with the reading tools (several rounds if needed) so every distinct concept, method, theorem, formula and case in it can be grounded in source text. Ignore material outside the branch."
        } else if is_mindmap {
            "You are a thorough curriculum and knowledge-structure planner for Local Notebook Studio. Your job is to inspect the source structure and gather balanced, concrete details (sub-topics, definitions, theorems, formulas, mechanisms, and examples) across EVERY major chapter or section in the requested scope using the reading tools. Call multiple tools in parallel across different chapters so no chapter is left shallow or skipped. Only reply DONE once every major branch has sufficient concrete detail to build a deep, evenly balanced hierarchy."
        } else {
            "You are a research planner for Local Notebook Studio. Read the sources on demand with the tools: target the requested topic/scope (or, for a whole-book request, its key sections evenly across all chapters), never request the entire book at once, and avoid unrelated material. Work in as many rounds as the artifact needs; reply DONE once the evidence covers the requested scope."
        };
        for round in 0..max_rounds {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            // the pictures ride in the first user message of every round (the same prefix)
            let req = material::with_images(
                LlmRequest {
                    system: planner_system.into(),
                    messages: messages.clone(),
                    tools: tools.clone(),
                    tool_mode: ToolMode::Auto,
                    output: OutputMode::Text,
                    // Reasoning models spend output tokens on thinking before emitting tool calls;
                    // a small cap silently ended the reading loop after the first round.
                    max_output_tokens: resolved.output_budget().clamp(2048, 8192),
                    reasoning: ReasoningOptions {
                        show_summary: profile.show_thinking,
                        effort: profile.effort.clone(),
                    },
                },
                &pictures,
            );
            let mut calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            let mut round_text = String::new();
            let mut round_usage = Usage::default();
            let mut thought_seq: Option<i32> = None;
            let mut full_thought = String::new();
            let mut pending_thought = String::new();
            let mut last_flush = std::time::Instant::now();
            let thought_title = self.l(
                &format!("按需资料规划与思考（第 {} 轮）", round + 1),
                &format!(
                    "On-demand reading planning and reasoning (round {})",
                    round + 1
                ),
            );
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
                                    self.emit_thought_delta(
                                        artifact_id,
                                        &art.notebook_id,
                                        s,
                                        &pending_thought,
                                    );
                                }
                                pending_thought.clear();
                                last_flush = std::time::Instant::now();
                            }
                        }
                    }
                    LlmEvent::TextDelta(t) => round_text.push_str(&t),
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

            if let Err(e) = stream_res {
                if e.code == ErrorCode::Cancelled || cancel.is_cancelled() {
                    return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
                }
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "stage",
                    &self.l("按需精读中断", "On-demand reading interrupted"),
                    &self.l(&format!(
                        "第 {} 轮资料读取请求失败（{}），将使用已读取的 {} 条片段继续。",
                        round + 1,
                        truncate_str(&e.message, 160),
                        collected.len()
                    ), &format!(
                        "Reading round {} failed ({}); continuing with the {} passages already read.",
                        round + 1,
                        truncate_str(&e.message, 160),
                        collected.len()
                    )),
                );
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
            // a call without a name can be neither run nor echoed back to the model: dropped
            calls.retain(|_, (_, name, _)| !name.trim().is_empty());
            if calls.is_empty() {
                // the reader's closing reply: with `finish` it should be the requested result
                if let Some(f) = finish {
                    plan = lna_llm::parse_structured(&f.schema, &round_text)?;
                    if plan.is_none() && !round_text.trim().is_empty() {
                        messages.push(LlmMessage {
                            role: Role::Assistant,
                            parts: vec![Part::Text(round_text.clone())],
                        });
                    }
                }
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

                let first_new = collected.len();
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
                                        &mut seen_keys,
                                        &mut collected,
                                        &mut used_chars,
                                        char_budget,
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
                                    let node_meta: Option<String> = conn
                                        .query_row(
                                            "SELECT source_id FROM source_index_nodes WHERE id = ?1",
                                            [node_id],
                                            |r| r.get(0),
                                        )
                                        .optional()
                                        .map_err(map_sql_err)?;
                                    let Some(sid) = node_meta else {
                                        return Ok(Vec::new());
                                    };
                                    let mut stmt = conn
                                        .prepare(
                                            "SELECT c.id, s.title, c.page_start, c.content
                                             FROM source_chunks c JOIN sources s ON s.id = c.source_id
                                             WHERE c.source_id = ?1 AND c.node_id = ?2
                                             ORDER BY c.chunk_index ASC LIMIT ?3",
                                        )
                                        .map_err(map_sql_err)?;
                                    let mapped = stmt
                                        .query_map(params![sid, node_id, max_chunks], |r| {
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
                                    &mut seen_keys,
                                    &mut collected,
                                    &mut used_chars,
                                    char_budget,
                                    format!("chunk:{cid}"),
                                    format!("{stitle}{pstr}"),
                                    content,
                                ) {
                                    added_here += 1;
                                }
                            }
                            summary_note =
                                format!("Loaded {added_here} excerpts for node {node_id}.");
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
                                    &mut seen_keys,
                                    &mut collected,
                                    &mut used_chars,
                                    char_budget,
                                    format!("chunk:{cid}"),
                                    format!("{stitle}{pstr}"),
                                    content,
                                ) {
                                    added_here += 1;
                                }
                            }
                            summary_note =
                                format!("Loaded {added_here} excerpts from pages {pstart}-{pend}.");
                        }
                    }
                    "abort_task" => {
                        // the model found the task cannot be done faithfully: the whole job
                        // stops with its reason (never a guessed result)
                        let reason = args
                            .get("reason")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .unwrap_or("(no reason given)");
                        return Err(LnaError::new(
                            ErrorCode::ModelRefused,
                            format!("模型主动终止任务：{reason}"),
                        ));
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
                        self.l(
                            &format!("工具调用 · 检索来源：{}", qs.join(" / ")),
                            &format!("Tool call · Searching sources: {}", qs.join(" / ")),
                        )
                    }
                    "read_index_node" => {
                        let nid = args.get("node_id").and_then(Value::as_str).unwrap_or("");
                        self.l(
                            &format!("工具调用 · 读取目录节点：{}", nid),
                            &format!("Tool call · Reading outline node: {}", nid),
                        )
                    }
                    "read_source_pages" => {
                        let ps = args.get("page_start").and_then(Value::as_i64).unwrap_or(1);
                        let pe = args.get("page_end").and_then(Value::as_i64).unwrap_or(ps);
                        self.l(
                            &format!("工具调用 · 精读页码：第 {}-{} 页", ps, pe),
                            &format!("Tool call · Reading pages {}-{}", ps, pe),
                        )
                    }
                    other => self.l(
                        &format!("工具调用 · {}", other),
                        &format!("Tool call · {}", other),
                    ),
                };
                self.append_thought(
                    artifact_id,
                    &art.notebook_id,
                    "tool",
                    &tool_label,
                    &self.l(
                        &format!(
                            "{}（累计已读取 {} 条片段，约 {} tokens）",
                            summary_note,
                            collected.len(),
                            used_chars / 3
                        ),
                        &format!(
                            "{} ({} passages read so far, ~{} tokens)",
                            summary_note,
                            collected.len(),
                            used_chars / 3
                        ),
                    ),
                );
                // the reader sees what it read (numbered as in the final evidence), so it can
                // judge coverage from the text and not only from outline titles
                let new_passages = collected[first_new..].join("\n\n");
                let closing = if finish.is_some() {
                    "If parts of the scope are still unread or only known from outline titles, keep reading them; otherwise give the final answer as instructed."
                } else {
                    "If parts of the scope are still unread or only known from outline titles, keep reading them; reply DONE only when the scope is covered."
                };
                tool_results.push(Part::ToolResult {
                    call_id,
                    content: format!(
                        "{}{summary_note} Total collected: {} excerpts (~{} tokens of ~{} budget). {closing}",
                        if new_passages.is_empty() {
                            "(No new passages: already read, nothing found, or the reading budget is full.)\n".to_string()
                        } else {
                            format!("{new_passages}\n\n")
                        },
                        collected.len(),
                        used_chars / 3,
                        char_budget / 3
                    ),
                    is_error: false,
                });
            }

            messages.push(LlmMessage {
                role: Role::Assistant,
                parts: assistant_parts,
            });
            messages.push(LlmMessage {
                role: Role::Tool,
                parts: tool_results,
            });
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l(
                    &format!("按需精读第 {} 轮", round + 1),
                    &format!("On-demand reading, round {}", round + 1),
                ),
                &self.l(
                    &format!(
                        "已按需读取 {} 条关键片段（约 {} tokens）。",
                        collected.len(),
                        used_chars / 3
                    ),
                    &format!(
                        "Read {} key passages on demand (~{} tokens).",
                        collected.len(),
                        used_chars / 3
                    ),
                ),
            );
            if used_chars >= char_budget * 85 / 100 {
                break;
            }
        }

        // The reader stopped without a valid closing answer (rounds or budget used up, or the
        // reply did not fit the schema): asked once more in the same conversation, which
        // still holds every passage it read. Failing that, the caller plans on its own.
        if let Some(f) = finish.filter(|_| plan.is_none() && !collected.is_empty()) {
            if cancel.is_cancelled() {
                return Err(LnaError::new(ErrorCode::Cancelled, "Cancelled"));
            }
            let mut msgs = messages.clone();
            msgs.push(LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(
                    "Stop reading now and give the final answer as instructed above, based on the passages you have read (no more tool calls).".into(),
                )],
            });
            let req = material::with_images(
                LlmRequest {
                    system: planner_system.into(),
                    messages: msgs,
                    // the conversation holds tool calls, so the tools stay declared
                    tools: tools.clone(),
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: resolved.bounded_output(6000),
                    reasoning: ReasoningOptions {
                        show_summary: profile.show_thinking,
                        effort: profile.effort.clone(),
                    },
                },
                &pictures,
            );
            match self
                .run_structured_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &f.thought_title,
                    resolved,
                    &req,
                    f.schema_name,
                    &f.schema,
                    1,
                    cancel.clone(),
                )
                .await
            {
                Ok((v, u)) => {
                    let _ = lna_storage::record_usage(
                        &self.db,
                        Some(&art.notebook_id),
                        Some(&profile.id),
                        "studio_on_demand_read",
                        Some(artifact_id),
                        u.input_tokens as i64,
                        u.output_tokens as i64,
                        u.cached_tokens as i64,
                        false,
                    );
                    plan = Some(v);
                }
                Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                    return Err(e)
                }
                Err(e) => {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l("阅读后未能直接给出结果", "No result from the reading pass"),
                        &self.l(
                            &format!("将单独规划（{}）。", truncate_str(&e.message, 160)),
                            &format!("Planning separately ({}).", truncate_str(&e.message, 160)),
                        ),
                    );
                }
            }
        }

        // If the AI didn't call tools (e.g. model lacks tool-calling or catalog was enough),
        // supplement with focused retrieval or evenly sampled key chunks so evidence is never empty.
        if collected.is_empty() {
            if let Some(q) = topic_query {
                if let Ok(res) = self
                    .retrieval
                    .search(
                        &art.notebook_id,
                        &[q.to_string()],
                        &art.source_ids,
                        10_000,
                        14,
                    )
                    .await
                {
                    for ev in res.evidences {
                        let pg = ev
                            .page_start
                            .map(|p| format!(", p.{p}"))
                            .unwrap_or_default();
                        push_evidence(
                            &mut seen_keys,
                            &mut collected,
                            &mut used_chars,
                            char_budget,
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

        let outline_return_cap = if branch_focus {
            4_000
        } else if is_mindmap {
            32_000
        } else {
            8_000
        };
        Ok(Some((
            format!(
                "Source Outline:\n{}\n\nOn-Demand Retrieved Excerpts:\n{}",
                truncate_str(&catalog_text, outline_return_cap),
                collected.join("\n\n")
            ),
            plan,
        )))
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_evidence_text(
        &self,
        art: &StudioArtifact,
        profile: &ModelProfile,
        resolved: &ResolvedProfile,
        artifact_id: &str,
        material_hint: Option<&str>,
        finish: Option<&ReaderFinish>,
        plan_out: &mut Option<Value>,
        cancel: CancellationToken,
    ) -> LnaResult<String> {
        if art.source_ids.is_empty() {
            // generated from the user's own pictures / files only
            return Ok(String::new());
        }
        let topic_query = art
            .config
            .get("topic")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                // No typed topic, but the user attached their own material (e.g. a photo of one
                // exercise): that material is the scope, not the selection as a whole.
                material_hint.map(|h| {
                    format!(
                        "(no topic typed) exactly what the user's attached material asks for — {h}"
                    )
                })
            });
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
            let reserved_output = (resolved.output_budget() as usize).min(window / 4);
            let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
            let kg_budget = available_input.saturating_mul(70) / 100;
            if target_tokens.max(0) as usize > kg_budget.min(12_000) {
                match self
                    .gather_evidence_on_demand(
                        art,
                        profile,
                        resolved,
                        artifact_id,
                        topic_query.as_deref(),
                        kg_budget.min(28_000),
                        Some(&target_ids),
                        false,
                        None,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(Some((on_demand_text, _))) => return Ok(on_demand_text),
                    Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                        return Err(e)
                    }
                    _ => {}
                }
            } else {
                // Small enough to fit: give the model the actual text, not just index titles.
                let (n, text) = self.load_full_source_text(&target_ids)?;
                if n > 0 {
                    self.append_thought(
                        artifact_id,
                        &art.notebook_id,
                        "stage",
                        &self.l("整理来源正文", "Preparing source text"),
                        &self.l(&format!("共加载 {n} 个来源正文片段（可直接放入模型上下文）。"), &format!("Loaded {n} source text passages (they fit directly into the model context).")),
                    );
                    return Ok(text);
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
                        format!(
                            "
Core Topics: {}",
                            topics.join(", ")
                        )
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

                #[allow(clippy::type_complexity)]
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
                    let chunks: Vec<(Option<i32>, Option<String>, String)> =
                        self.db.read(|conn| {
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
                        let sec_s = sec
                            .filter(|s| !s.is_empty())
                            .map(|s| format!(" · {s}"))
                            .unwrap_or_default();
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
                    &format!(
                        "Prepared {} compact structural items without sending full book text.",
                        evidence_blocks.len()
                    ),
                );
                return Ok(evidence_blocks.join(
                    "

",
                ));
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
        let reserved_output = (resolved.output_budget() as usize).min(window / 4);
        let available_input = window.saturating_sub(reserved_output + 800 + window / 20);
        let evidence_budget = available_input.saturating_mul(70) / 100;
        // Only pass full source text directly for compact sources (<= 12,000 tokens).
        // Larger sources use on-demand AI reading so we never dump an entire book at once.
        let direct_full_cap = evidence_budget.min(12_000);
        if total_source_tokens.max(0) as usize <= direct_full_cap {
            let (n, text) = self.load_full_source_text(&art.source_ids)?;
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l("整理来源正文", "Preparing source text"),
                &self.l(&format!("共加载 {n} 个来源正文片段（可直接放入模型上下文）。"), &format!("Loaded {n} source text passages (they fit directly into the model context).")),
            );
            return Ok(text);
        }

        // On-demand AI reading: present compact source catalog & outline, let the AI call
        // search_sources / read_index_node / read_source_pages only for sections it needs.
        let on_demand = self
            .gather_evidence_on_demand(
                art,
                profile,
                resolved,
                artifact_id,
                topic_query.as_deref(),
                evidence_budget.min(
                    if matches!(
                        art.artifact_type,
                        ArtifactType::Slides | ArtifactType::Mindmap
                    ) {
                        28_000
                    } else {
                        18_000
                    },
                ),
                None,
                false,
                finish,
                cancel.clone(),
            )
            .await;
        match on_demand {
            Ok(Some((on_demand_text, read_plan))) => {
                *plan_out = read_plan;
                return Ok(on_demand_text);
            }
            Err(e) if matches!(e.code, ErrorCode::Cancelled | ErrorCode::ModelRefused) => {
                return Err(e)
            }
            _ => {}
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
            let cached_digests = self.load_cached_source_digests(&art.source_ids, language, 16)?;
            let digest_text = cached_digests
                .iter()
                .map(|(title, section, digest)| format!("### {title} / {section}\n{digest}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            self.append_thought(
                artifact_id,
                &art.notebook_id,
                "stage",
                &self.l("按主题快速检索资料", "Quick topic search"),
                &self.l(&format!(
                    "已针对主题「{}」直接检索 {} 条相关片段（快速检索模式，避免对全书逐章重做摘要卡顿）。",
                    query,
                    related.evidences.len()
                ), &format!(
                    "Searched directly for the topic \"{}\" and found {} related passages (quick-search mode, which avoids re-summarizing the whole book chapter by chapter).",
                    query,
                    related.evidences.len()
                )),
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
            &self.l("合并全局章节大纲", "Merging the global chapter outline"),
            &self.l(&format!(
                "已整理 {} 组章节摘要，正在提炼全局结构与关键细节…",
                digest_entries.len()
            ), &format!(
                "Collected {} groups of chapter summaries; distilling the global structure and key details…",
                digest_entries.len()
            )),
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
                &self.l(
                    &format!("工具调用 · 归约章节摘要（第 {} 批）", batch_index + 1),
                    &format!(
                        "Tool call · Reducing chapter summaries (batch {})",
                        batch_index + 1
                    ),
                ),
                &self.l(
                    &format!("正在合并本批次 {} 组章节摘要…", batch.len()),
                    &format!(
                        "Merging {} groups of chapter summaries in this batch…",
                        batch.len()
                    ),
                ),
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
            let (batch_summary, batch_usage) = self
                .run_text_with_thought(
                    artifact_id,
                    &art.notebook_id,
                    &self.l(
                        &format!("章节归约思考（第 {} 批）", batch_index + 1),
                        &format!("Chapter reduction reasoning (batch {})", batch_index + 1),
                    ),
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
        let (outline, reduce_usage) = self
            .run_text_with_thought(
                artifact_id,
                &art.notebook_id,
                &self.l("全局章节结构提炼思考", "Global chapter structure reasoning"),
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
                &self.l(
                    "工具调用 · 按章节检索关键原文片段",
                    "Tool call · Retrieving key passages by chapter",
                ),
                &self.l(
                    &format!(
                        "正在为前 {} 个核心章节检索高相关证据片段…",
                        digests.len().min(12)
                    ),
                    &format!(
                        "Retrieving highly relevant evidence for the top {} core chapters…",
                        digests.len().min(12)
                    ),
                ),
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
        // Map step of the digest: cached digests are read straight away, the missing ones are
        // summarised by parallel executors (one per section group) and assembled in order.
        struct Group {
            title: String,
            source_id: String,
            section_key: String,
            content_hash: String,
            language: String,
            chunks: Vec<(i64, String)>,
            cached: Option<String>,
        }
        let mut entries: Vec<Group> = Vec::new();
        for ((source_id, section_key), chunks) in groups.into_iter() {
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
            let cached = cached
                .filter(|(hash, _)| hash == &content_hash)
                .map(|(_, d)| d);
            entries.push(Group {
                title,
                source_id,
                section_key,
                content_hash,
                language,
                chunks,
                cached,
            });
        }
        let missing: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, g)| g.cached.is_none())
            .map(|(i, _)| i)
            .collect();
        if !missing.is_empty() {
            self.append_thought(
                artifact_id,
                notebook_id,
                "stage",
                &self.l("提取来源章节摘要", "Extracting chapter summaries"),
                &self.l(&format!(
                    "来源较长，正在把 {} 个章节段并行提取摘要（共 {} 段，其余已有缓存）…",
                    missing.len(),
                    total_groups
                ), &format!(
                    "The source is long; summarising {} section groups in parallel ({} in total, the rest are cached)…",
                    missing.len(),
                    total_groups
                )),
            );
        }
        let limit = self.llm.parallel_limit(profile_id).clamp(1, 64);
        let entries_ref = &entries;
        let mut running = futures_util::stream::iter(missing.iter().copied().map(|idx| {
            let cancel_c = cancel.clone();
            async move {
                let g = &entries_ref[idx];
                let chunk_text = g
                    .chunks
                    .iter()
                    .map(|(id, content)| format!("[chunk {id}] {content}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let req = LlmRequest {
                    system: format!("You are a section summarization assistant. Summarize the following content from the same section in {}, preserving key concepts, equations, relationships, and conclusions without adding any facts not present in the source.", g.language),
                    messages: vec![LlmMessage { role: Role::User, parts: vec![Part::Text(chunk_text)] }],
                    tools: vec![], tool_mode: ToolMode::None, output: OutputMode::Text,
                    max_output_tokens: 700,
                    reasoning: ReasoningOptions {
                        show_summary: profile.show_thinking,
                        effort: profile.effort.clone(),
                    },
                };
                let res = self
                    .run_text_with_thought(
                        artifact_id,
                        notebook_id,
                        &self.l(
                            &format!("章节摘要思考：{} / {}", g.title, g.section_key),
                            &format!("Chapter summary reasoning: {} / {}", g.title, g.section_key),
                        ),
                        profile,
                        &req,
                        cancel_c,
                    )
                    .await;
                (idx, res)
            }
        }))
        .buffer_unordered(limit);
        let mut fresh: HashMap<usize, String> = HashMap::new();
        while let Some((idx, res)) = next_or_cancel(&mut running, &cancel).await? {
            let (digest, usage) = res?;
            let digest = digest.trim().to_string();
            let g = &entries[idx];
            let chunk_ids: Vec<i64> = g.chunks.iter().map(|(id, _)| *id).collect();
            let chunk_ids_json = serde_json::to_string(&chunk_ids).unwrap_or_else(|_| "[]".into());
            let token_count: i64 = g
                .chunks
                .iter()
                .map(|(_, text)| (text.chars().count() as i64 + 3) / 4)
                .sum();
            self.db.write(|conn| {
                conn.execute(
                    "INSERT INTO source_digests (source_id, section_key, content_hash, language, digest_md, chunk_ids_json, token_count, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(source_id,section_key,language) DO UPDATE SET content_hash=excluded.content_hash,
                     digest_md=excluded.digest_md, chunk_ids_json=excluded.chunk_ids_json, token_count=excluded.token_count, created_at=excluded.created_at",
                    params![g.source_id, g.section_key, g.content_hash, g.language, digest, chunk_ids_json, token_count, now_ms()],
                ).map_err(map_sql_err)?;
                Ok(())
            })?;
            let _ = lna_storage::record_usage(
                &self.db,
                Some(notebook_id),
                Some(&profile.id),
                "studio_digest_map",
                Some(&g.source_id),
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cached_tokens as i64,
                false,
            );
            fresh.insert(idx, digest);
        }
        drop(running);
        let mut out = Vec::with_capacity(entries.len());
        for (idx, g) in entries.iter().enumerate() {
            let digest = match &g.cached {
                Some(d) => d.clone(),
                None => fresh.remove(&idx).unwrap_or_default(),
            };
            out.push((g.title.clone(), g.section_key.clone(), digest));
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
        if matches!(status, "READY" | "FAILED" | "CANCELLED") {
            if let Ok(mut m) = self.previews.lock() {
                m.remove(id);
            }
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
            | ErrorCode::ModelRefused
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

/// Generation rules shared by single-pass, multi-pass and refine prompts for mind maps and
/// knowledge graphs: correct granularity, facets, atomic marking and no size cap.
fn mindmap_rules(is_kg: bool, detail: &str) -> String {
    // Granularity is defined per leaf (how much source material one knowledge point stands
    // for), never as a total node count, so a short book and a long textbook both get a map
    // whose size follows the material.
    let detail_rule = match detail {
        "concise" => " DETAIL LEVEL: CONCISE — one knowledge point stands for roughly one textbook section.",
        "detailed" => " DETAIL LEVEL: DETAILED — one knowledge point stands for roughly one page of material, or one important property / corollary.",
        _ => " DETAIL LEVEL: STANDARD — one knowledge point stands for roughly 2-4 pages of material, or one concept / theorem / method that deserves its own explanation or its own quiz question.",
    };
    let size_rule = " SIZE FOLLOWS THE MATERIAL: there is no target node count. The number of nodes must follow from the amount of distinct knowledge in the sources at the requested detail level — a short simple text yields few nodes, a long dense textbook more. STOP SPLITTING a node when the material it covers is smaller than one knowledge point at this detail level, or when its parts could not each be explained or tested on their own; put those parts into facets instead.";
    let granularity_rule = " BRANCHING: every parent has 3-7 children. If you would list more than 7, group them under intermediate topic nodes (e.g. vector addition, scalar multiplication, subtraction, zero vector → one node \"向量的基础运算\"). Never create a parent with a single child. Parallel operations, properties, conditions, notations and examples of the same concept are NOT separate nodes — they are facets of one node.";
    let facet_rule = " FACETS: 0-3 short phrases per node for details that do not deserve a node (conditions, special cases, notations, minor variants, e.g. \"invertible ⇔ det≠0\"); leave the array empty when there is nothing worth adding.";
    let atomic_rule = " Do not set \"atomic\"; the program decides it.";
    let structure_rule = " REORGANIZE, DO NOT COPY THE BOOK: the table of contents is raw material, not the map structure. Organize by concept dependency and learning logic (what it is → why it is needed → how to compute / use it), not by chapter order; do not reuse section titles verbatim as labels. Place a concept that is used in many places (e.g. linear combination) once, where it is first defined, and refer to it from elsewhere via \"prerequisites\". SUMMARIES must synthesize, not describe: say what the grouped ideas have in common and how they relate (bad: \"介绍向量的加法和数乘\"; good: \"加法与数乘都逐分量进行，满足交换律和结合律，是线性组合的基础\"); keep each summary within two sentences.";
    let kg_rule = if is_kg {
        r#" Set "kind": "knowledge_graph". Build a concept Knowledge Graph structured as a collapsible tree from foundational prerequisites to core theorems and advanced applications. For each node, set "importance" ("core" | "key" | "detail"), a brief 1-sentence "summary", and "prerequisites" (names of prerequisite concepts when applicable)."#
    } else {
        ""
    };
    format!(
        " IMPORTANT: Do NOT include any bracketed citation numbers like [1], [2], or [1][17] inside node labels or facets; keep labels clean and readable, and omit the \"citations\" field of every node. Node labels are shown as plain text, so write any math in labels and facets as compact Unicode (e.g. A=SΛS⁻¹, Aᵀ, x₁, ℝⁿ, √2) instead of LaTeX.{detail_rule}{size_rule}{granularity_rule}{facet_rule}{atomic_rule}{structure_rule}{kg_rule}"
    )
}

/// Best-effort parse of a JSON document that is still streaming in: closes an open string
/// and all open containers, and drops a dangling partial element if needed.
fn close_partial_json(text: &str) -> Option<Value> {
    let start = text.find(['{', '['])?;
    let mut cur: &str = text[start..].trim_end();
    for _ in 0..64 {
        let mut stack: Vec<char> = Vec::new();
        let mut in_str = false;
        let mut esc = false;
        let mut last_cut: Option<usize> = None;
        for (i, ch) in cur.char_indices() {
            if in_str {
                if esc {
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    in_str = false;
                }
                continue;
            }
            match ch {
                '"' => in_str = true,
                '{' | '[' => {
                    stack.push(ch);
                    last_cut = Some(i);
                }
                '}' | ']' => {
                    stack.pop();
                }
                ',' => last_cut = Some(i),
                _ => {}
            }
        }
        let mut cand = cur.to_string();
        if in_str {
            if esc {
                cand.pop();
            }
            cand.push('"');
        }
        let trimmed = cand.trim_end().trim_end_matches(',').to_string();
        let mut closed = trimmed;
        for open in stack.iter().rev() {
            closed.push(if *open == '{' { '}' } else { ']' });
        }
        if let Ok(v) = serde_json::from_str::<Value>(&closed) {
            return Some(v);
        }
        // Cut back to the last element boundary and retry.
        match last_cut {
            Some(0) | None => return None,
            Some(i) => {
                let c = &cur[i..i + 1];
                cur = if c == "," { &cur[..i] } else { &cur[..=i] };
                cur = cur.trim_end();
            }
        }
    }
    None
}

/// Removes what must never be shown before a draft is validated: for quizzes only the
/// question stems and options survive (no answers, explanations, rubrics or citations).
fn sanitize_preview(ty: ArtifactType, v: &mut Value) {
    if ty != ArtifactType::Quiz {
        return;
    }
    if let Some(qs) = v.get_mut("questions").and_then(Value::as_array_mut) {
        for q in qs.iter_mut() {
            if let Some(obj) = q.as_object_mut() {
                obj.retain(|k, _| matches!(k.as_str(), "id" | "type" | "prompt" | "options"));
            }
        }
    }
}

fn add_usage(sum: &mut Usage, u: &Usage) {
    sum.input_tokens = sum.input_tokens.saturating_add(u.input_tokens);
    sum.output_tokens = sum.output_tokens.saturating_add(u.output_tokens);
    sum.cached_tokens = sum.cached_tokens.saturating_add(u.cached_tokens);
}

/// Result of a branch append / node refine pass.
struct NodeExpansion {
    atomic: bool,
    reason: String,
    nodes: Vec<Value>,
}

fn mm_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn mm_nodes(payload: &Value) -> Vec<Value> {
    payload
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn mm_parent(n: &Value) -> Option<String> {
    n.get("parent_id").and_then(Value::as_str).map(String::from)
}

fn mm_root_id(nodes: &[Value]) -> Option<String> {
    nodes
        .iter()
        .find(|n| mm_parent(n).is_none())
        .or_else(|| nodes.first())
        .map(|n| mm_str(n, "id"))
}

fn mm_children<'a>(nodes: &'a [Value], id: &str) -> Vec<&'a Value> {
    nodes
        .iter()
        .filter(|n| mm_parent(n).as_deref() == Some(id))
        .collect()
}

/// Labels from the root down to (excluding) `id`.
fn mm_ancestor_labels(nodes: &[Value], id: &str) -> Vec<String> {
    let by_id: HashMap<String, &Value> = nodes.iter().map(|n| (mm_str(n, "id"), n)).collect();
    let mut out = Vec::new();
    let mut cur = by_id.get(id).and_then(|n| mm_parent(n));
    while let Some(p) = cur {
        if out.len() > nodes.len() {
            break;
        }
        let Some(n) = by_id.get(&p) else { break };
        out.push(mm_str(n, "label"));
        cur = mm_parent(n);
    }
    out.reverse();
    out
}

/// The top-level branch (child of the root) containing `id`.
fn mm_top_branch(nodes: &[Value], id: &str, root: &str) -> Option<String> {
    let by_id: HashMap<String, &Value> = nodes.iter().map(|n| (mm_str(n, "id"), n)).collect();
    let mut cur = id.to_string();
    for _ in 0..=nodes.len() {
        let parent = by_id.get(&cur).and_then(|n| mm_parent(n))?;
        if parent == root {
            return Some(cur);
        }
        cur = parent;
    }
    None
}

/// Indented outline of the subtree under `id` (excluding `id`), with ids for parent linking.
fn mm_outline(nodes: &[Value], id: &str, depth: usize, out: &mut String) {
    if depth > 24 {
        return;
    }
    for c in mm_children(nodes, id) {
        let cid = mm_str(c, "id");
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("- [{cid}] {}", mm_str(c, "label")));
        if let Some(f) = c
            .get("facets")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        {
            let fs: Vec<&str> = f.iter().filter_map(Value::as_str).collect();
            out.push_str(&format!(" (facets: {})", fs.join("; ")));
        }
        out.push('\n');
        mm_outline(nodes, &cid, depth + 1, out);
    }
    if depth == 0 && out.is_empty() {
        out.push_str("(none yet)\n");
    }
}

fn mm_label_key(label: &str) -> String {
    label
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Appends model-generated nodes under `anchor_id`: ids are namespaced under the anchor so
/// they never collide, parents outside the anchor's subtree fall back to the anchor, and a
/// node whose label duplicates an existing sibling is merged into it. Returns nodes added.
fn merge_mindmap_subtree(payload: &mut Value, anchor_id: &str, new_nodes: Vec<Value>) -> usize {
    let existing = mm_nodes(payload);
    let mut ids: std::collections::HashSet<String> =
        existing.iter().map(|n| mm_str(n, "id")).collect();
    let mut subtree: std::collections::HashSet<String> = std::collections::HashSet::new();
    subtree.insert(anchor_id.to_string());
    loop {
        let before = subtree.len();
        for n in &existing {
            if mm_parent(n).is_some_and(|p| subtree.contains(&p)) {
                subtree.insert(mm_str(n, "id"));
            }
        }
        if subtree.len() == before {
            break;
        }
    }
    let mut sibling_labels: HashMap<(String, String), String> = existing
        .iter()
        .filter_map(|n| {
            mm_parent(n).map(|p| ((p, mm_label_key(&mm_str(n, "label"))), mm_str(n, "id")))
        })
        .collect();

    // Pass 1: allocate ids for every new node.
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut prepared: Vec<(String, Value)> = Vec::new();
    for (i, n) in new_nodes.into_iter().enumerate() {
        let orig = mm_str(&n, "id");
        if !orig.is_empty() && subtree.contains(&orig) {
            continue; // the model re-emitted an existing node
        }
        let base = if orig.is_empty() {
            format!("{anchor_id}.{}", i + 1)
        } else {
            format!("{anchor_id}.{orig}")
        };
        let mut nid = base.clone();
        let mut k = 2;
        while ids.contains(&nid) {
            nid = format!("{base}_{k}");
            k += 1;
        }
        ids.insert(nid.clone());
        if !orig.is_empty() {
            id_map.insert(orig, nid.clone());
        }
        prepared.push((nid, n));
    }
    // Pass 2: resolve parents, merge duplicates, append.
    let mut alias: HashMap<String, String> = HashMap::new();
    let mut appended: Vec<Value> = Vec::new();
    for (nid, mut n) in prepared {
        let parent = match mm_parent(&n) {
            Some(p) if id_map.contains_key(&p) => id_map[&p].clone(),
            Some(p) if subtree.contains(&p) => p,
            _ => anchor_id.to_string(),
        };
        let parent = alias.get(&parent).cloned().unwrap_or(parent);
        let key = (parent.clone(), mm_label_key(&mm_str(&n, "label")));
        if key.1.is_empty() {
            continue;
        }
        if let Some(existing_id) = sibling_labels.get(&key) {
            alias.insert(nid, existing_id.clone());
            continue;
        }
        sibling_labels.insert(key, nid.clone());
        if let Some(obj) = n.as_object_mut() {
            obj.insert("id".into(), Value::String(nid));
            obj.insert("parent_id".into(), Value::String(parent));
            obj.insert("citations".into(), Value::Array(Vec::new()));
        }
        appended.push(n);
    }
    // Children listed before a parent that was merged away still point at the alias.
    for n in appended.iter_mut() {
        if let Some(p) = mm_parent(n) {
            if let Some(real) = alias.get(&p) {
                if let Some(obj) = n.as_object_mut() {
                    obj.insert("parent_id".into(), Value::String(real.clone()));
                }
            }
        }
    }
    let added = appended.len();
    if let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) {
        arr.extend(appended);
    }
    added
}

/// Replaces the subtree under `bid` in `main` (and the branch node itself) with the one from
/// `local`, which was grown from the same skeleton.
fn mm_adopt_branch(main: &mut Value, local: &Value, bid: &str) {
    let local_nodes = mm_nodes(local);
    let mut subtree: HashSet<String> = HashSet::new();
    subtree.insert(bid.to_string());
    for _ in 0..local_nodes.len() + 1 {
        let before = subtree.len();
        for n in &local_nodes {
            if let Some(p) = mm_parent(n) {
                if subtree.contains(&p) {
                    subtree.insert(mm_str(n, "id"));
                }
            }
        }
        if subtree.len() == before {
            break;
        }
    }
    let Some(arr) = main.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    arr.retain(|n| !subtree.contains(&mm_str(n, "id")));
    for n in local_nodes {
        if subtree.contains(&mm_str(&n, "id")) {
            arr.push(n);
        }
    }
}

/// Stable order: root first, then each top-level branch (in skeleton order) with its subtree.
fn mm_sort_by_branch_order(payload: &mut Value, order: &[String]) {
    let all = mm_nodes(payload);
    let Some(root) = mm_root_id(&all) else {
        return;
    };
    let mut keyed: Vec<(usize, Value)> = all
        .iter()
        .map(|n| {
            let id = mm_str(n, "id");
            let key = if id == root {
                0
            } else {
                mm_top_branch(&all, &id, &root)
                    .and_then(|b| order.iter().position(|o| *o == b))
                    .map(|i| i + 1)
                    .unwrap_or(usize::MAX)
            };
            (key, n.clone())
        })
        .collect();
    keyed.sort_by_key(|(k, _)| *k);
    payload["nodes"] = Value::Array(keyed.into_iter().map(|(_, n)| n).collect());
}

fn mm_norm_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Merges nodes whose normalized labels are identical anywhere in the graph below the
/// top-level branches: children and facets move to the first occurrence. Returns merged count.
fn mm_dedup_labels(payload: &mut Value) -> usize {
    let nodes = mm_nodes(payload);
    let Some(root) = mm_root_id(&nodes) else {
        return 0;
    };
    let by_id: HashMap<String, Value> =
        nodes.iter().map(|n| (mm_str(n, "id"), n.clone())).collect();
    let is_ancestor = |anc: &str, of: &str| -> bool {
        let mut cur = by_id.get(of).and_then(mm_parent);
        for _ in 0..nodes.len() {
            match cur {
                Some(p) if p == anc => return true,
                Some(p) => cur = by_id.get(&p).and_then(mm_parent),
                None => return false,
            }
        }
        false
    };
    let mut first: HashMap<String, String> = HashMap::new();
    let mut remap: Vec<(String, String)> = Vec::new();
    for n in &nodes {
        let id = mm_str(n, "id");
        let parent = mm_parent(n);
        if id == root || parent.as_deref() == Some(root.as_str()) {
            continue;
        }
        let key = mm_norm_label(&mm_str(n, "label"));
        if key.chars().count() < 2 {
            continue;
        }
        match first.get(&key) {
            Some(keep) if !is_ancestor(keep, &id) && !is_ancestor(&id, keep) => {
                remap.push((id, keep.clone()));
            }
            Some(_) => {}
            None => {
                first.insert(key, id);
            }
        }
    }
    if remap.is_empty() {
        return 0;
    }
    let drop_ids: HashSet<String> = remap.iter().map(|(d, _)| d.clone()).collect();
    let target: HashMap<String, String> = remap.iter().cloned().collect();
    let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
        return 0;
    };
    // Facets of dropped duplicates go to the node that is kept.
    let dropped_facets: Vec<(String, Vec<String>)> = arr
        .iter()
        .filter(|n| drop_ids.contains(&mm_str(n, "id")))
        .map(|n| {
            (
                target[&mm_str(n, "id")].clone(),
                n.get("facets")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();
    for (keep, facets) in dropped_facets {
        if let Some(k) = arr.iter_mut().find(|n| mm_str(n, "id") == keep) {
            for f in facets {
                mm_push_facet(k, &f);
            }
        }
    }
    arr.retain(|n| !drop_ids.contains(&mm_str(n, "id")));
    for n in arr.iter_mut() {
        if let Some(p) = mm_parent(n) {
            if let Some(t) = target.get(&p) {
                n["parent_id"] = Value::String(t.clone());
            }
        }
    }
    remap.len()
}

/// Marks leaves below the top-level branches as `expandable` (they can be refined on demand)
/// unless they are known to be atomic; clears the flag on parents.
fn mm_mark_expandable(payload: &mut Value) {
    let nodes = mm_nodes(payload);
    let Some(root) = mm_root_id(&nodes) else {
        return;
    };
    let parents: HashSet<String> = nodes.iter().filter_map(mm_parent).collect();
    let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for n in arr.iter_mut() {
        let id = mm_str(n, "id");
        let leaf = !parents.contains(&id) && id != root;
        let atomic = n.get("atomic").and_then(Value::as_bool) == Some(true);
        if let Some(obj) = n.as_object_mut() {
            if leaf && !atomic {
                obj.insert("expandable".into(), Value::Bool(true));
            } else {
                obj.remove("expandable");
            }
        }
    }
}

/// Applies review suggestions: a topic (child of a top-level branch) may move to another
/// top-level branch; a topic may be merged into another topic (children and facets move).
fn mm_apply_review(
    payload: &mut Value,
    moves: &[(String, String)],
    merges: &[(String, String)],
) -> usize {
    let nodes = mm_nodes(payload);
    let Some(root) = mm_root_id(&nodes) else {
        return 0;
    };
    let is_branch = |id: &str| {
        nodes
            .iter()
            .any(|n| mm_str(n, "id") == id && mm_parent(n).as_deref() == Some(root.as_str()))
    };
    let is_topic = |id: &str| {
        nodes
            .iter()
            .any(|n| mm_str(n, "id") == id && mm_parent(n).is_some_and(|p| is_branch(&p)))
    };
    let mut applied = 0;
    let mut dropped: HashSet<String> = HashSet::new();
    for (keep, drop) in merges.iter().take(6) {
        if keep == drop || !is_topic(keep) || !is_topic(drop) || dropped.contains(keep) {
            continue;
        }
        let facets: Vec<String> = nodes
            .iter()
            .find(|n| mm_str(n, "id") == *drop)
            .map(|n| {
                let mut f: Vec<String> = n
                    .get("facets")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                f.insert(0, mm_str(n, "label"));
                f
            })
            .unwrap_or_default();
        let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
            return applied;
        };
        for n in arr.iter_mut() {
            if mm_parent(n).as_deref() == Some(drop.as_str()) {
                n["parent_id"] = Value::String(keep.clone());
            }
            if mm_str(n, "id") == *keep {
                for f in &facets {
                    mm_push_facet(n, f);
                }
            }
        }
        arr.retain(|n| mm_str(n, "id") != *drop);
        dropped.insert(drop.clone());
        applied += 1;
    }
    for (topic, branch) in moves.iter().take(6) {
        if dropped.contains(topic) || !is_topic(topic) || !is_branch(branch) {
            continue;
        }
        if let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) {
            if let Some(n) = arr.iter_mut().find(|n| mm_str(n, "id") == *topic) {
                if mm_parent(n).as_deref() != Some(branch.as_str()) {
                    n["parent_id"] = Value::String(branch.clone());
                    applied += 1;
                }
            }
        }
    }
    applied
}

/// Most children a mind-map parent should have before they get grouped under topics.
const MM_MAX_CHILDREN: usize = 7;

fn mm_push_facet(node: &mut Value, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Some(obj) = node.as_object_mut() {
        let facets = obj
            .entry("facets")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(arr) = facets.as_array_mut() {
            if !arr.iter().any(|f| f.as_str() == Some(text)) {
                arr.push(Value::String(text.to_string()));
            }
        }
    }
}

/// Enforces the shape rules on the subtree under `anchor_id` without dropping content:
/// nodes deeper than `max_below` levels are folded into their ancestor's facets, a parent
/// with a single child absorbs it, and model-set `atomic` flags are cleared (leaves stay
/// expandable on demand). Returns the parents that still have too many children.
fn mm_enforce_shape(payload: &mut Value, anchor_id: &str, max_below: usize) -> Vec<String> {
    let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    for _ in 0..8 {
        let mut changed = false;
        // Depth of every node in the anchor's subtree, relative to the anchor.
        let mut depth: HashMap<String, usize> = HashMap::new();
        depth.insert(anchor_id.to_string(), 0);
        loop {
            let before = depth.len();
            for n in arr.iter() {
                if let Some(p) = mm_parent(n) {
                    if let Some(d) = depth.get(&p).copied() {
                        depth.entry(mm_str(n, "id")).or_insert(d + 1);
                    }
                }
            }
            if depth.len() == before {
                break;
            }
        }
        // Fold the deepest over-limit node into its parent's facets.
        if let Some((deep_id, _)) = depth
            .iter()
            .filter(|(_, d)| **d > max_below)
            .max_by_key(|(_, d)| **d)
            .map(|(k, d)| (k.clone(), *d))
        {
            let idx = arr.iter().position(|n| mm_str(n, "id") == deep_id);
            if let Some(i) = idx {
                let n = arr.remove(i);
                let label = mm_str(&n, "label");
                let parent = mm_parent(&n).unwrap_or_default();
                for c in arr.iter_mut() {
                    if mm_parent(c).as_deref() == Some(deep_id.as_str()) {
                        if let Some(o) = c.as_object_mut() {
                            o.insert("parent_id".into(), Value::String(parent.clone()));
                        }
                    }
                }
                if let Some(p) = arr.iter_mut().find(|c| mm_str(c, "id") == parent) {
                    mm_push_facet(p, &label);
                }
                changed = true;
            }
        }
        if changed {
            continue;
        }
        // A parent with exactly one child absorbs it (the child's children move up).
        let ids: Vec<String> = depth.keys().cloned().collect();
        for pid in ids {
            let kids: Vec<String> = arr
                .iter()
                .filter(|n| mm_parent(n).as_deref() == Some(pid.as_str()))
                .map(|n| mm_str(n, "id"))
                .collect();
            if kids.len() != 1 {
                continue;
            }
            let only = &kids[0];
            let Some(i) = arr.iter().position(|n| &mm_str(n, "id") == only) else {
                continue;
            };
            let n = arr.remove(i);
            for c in arr.iter_mut() {
                if mm_parent(c).as_deref() == Some(only.as_str()) {
                    if let Some(o) = c.as_object_mut() {
                        o.insert("parent_id".into(), Value::String(pid.clone()));
                    }
                }
            }
            if let Some(p) = arr.iter_mut().find(|c| mm_str(c, "id") == pid) {
                mm_push_facet(p, &mm_str(&n, "label"));
                for f in n
                    .get("facets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    mm_push_facet(p, f);
                }
            }
            changed = true;
            break;
        }
        if !changed {
            break;
        }
    }
    let mut subtree: std::collections::HashSet<String> = std::collections::HashSet::new();
    subtree.insert(anchor_id.to_string());
    loop {
        let before = subtree.len();
        for n in arr.iter() {
            if mm_parent(n).is_some_and(|p| subtree.contains(&p)) {
                subtree.insert(mm_str(n, "id"));
            }
        }
        if subtree.len() == before {
            break;
        }
    }
    for n in arr.iter_mut() {
        if subtree.contains(&mm_str(n, "id")) {
            if let Some(o) = n.as_object_mut() {
                o.remove("atomic");
            }
        }
    }
    let mut crowded: Vec<String> = subtree
        .iter()
        .filter(|id| {
            arr.iter()
                .filter(|n| mm_parent(n).as_deref() == Some(id.as_str()))
                .count()
                > MM_MAX_CHILDREN
        })
        .cloned()
        .collect();
    crowded.sort();
    crowded
}

/// Applies a model-proposed grouping: each group with 2+ members becomes a new topic node
/// under `parent_id` and its members move under it. Returns the number of groups created.
fn mm_apply_groups(payload: &mut Value, parent_id: &str, groups: &[Value]) -> usize {
    let Some(arr) = payload.get_mut("nodes").and_then(Value::as_array_mut) else {
        return 0;
    };
    let children: std::collections::HashSet<String> = arr
        .iter()
        .filter(|n| mm_parent(n).as_deref() == Some(parent_id))
        .map(|n| mm_str(n, "id"))
        .collect();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut created = 0;
    for (k, g) in groups.iter().enumerate() {
        let label = mm_str(g, "label");
        let members: Vec<String> = g
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|m| children.contains(*m) && !taken.contains(*m))
            .map(String::from)
            .collect();
        if label.trim().is_empty() || members.len() < 2 || members.len() == children.len() {
            continue;
        }
        let mut gid = format!("{parent_id}.g{}", k + 1);
        let mut j = 2;
        while arr.iter().any(|n| mm_str(n, "id") == gid) {
            gid = format!("{parent_id}.g{}_{j}", k + 1);
            j += 1;
        }
        for n in arr.iter_mut() {
            if members.contains(&mm_str(n, "id")) {
                if let Some(o) = n.as_object_mut() {
                    o.insert("parent_id".into(), Value::String(gid.clone()));
                }
            }
        }
        taken.extend(members);
        arr.push(json!({
            "id": gid,
            "parent_id": parent_id,
            "label": label,
            "summary": mm_str(g, "summary"),
            "importance": "key",
            "citations": []
        }));
        created += 1;
    }
    created
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
        "lecture" => ArtifactType::Lecture,
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
    if artifact_type == ArtifactType::Slides {
        if let Some(p) = payload.as_mut() {
            texfmt::convert_slides_payload(p);
        }
    }
    if artifact_type == ArtifactType::Report {
        if let Some(p) = payload.as_mut() {
            texfmt::convert_report_headings(p);
        }
    }
    if artifact_type == ArtifactType::Flashcards {
        if let Some(p) = payload.as_mut() {
            texfmt::convert_flashcards_payload(p);
        }
    }
    if artifact_type == ArtifactType::Audio {
        // Turns refer to speakers by id ("host"); every client shows the script, so give them
        // the role names the script defines and keep the id for anything that needs it.
        if let Some(p) = payload.as_mut() {
            let roles: HashMap<String, String> = p
                .get("speakers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| {
                            let id = s.get("id")?.as_str()?.to_string();
                            let role = s.get("role")?.as_str()?.trim().to_string();
                            (!role.is_empty()).then_some((id, role))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(Value::Array(turns)) = p.get_mut("turns") {
                for t in turns {
                    let id = t.get("speaker").and_then(Value::as_str).map(String::from);
                    if let (Some(id), Some(obj)) = (id, t.as_object_mut()) {
                        if let Some(role) = roles.get(&id) {
                            obj.insert("speaker_id".into(), json!(id));
                            obj.insert("speaker".into(), json!(role));
                        }
                    }
                }
            }
        }
    }
    if artifact_type == ArtifactType::Lecture {
        if let Some(Value::Array(slides)) = payload.as_mut().and_then(|p| p.get_mut("slides")) {
            for s in slides {
                lecture::texify_slide_text(s);
            }
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
        attachments: Vec::new(),
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
    let label_hits = label_tokens
        .iter()
        .filter(|t| quiz_tokens.contains(*t))
        .count();
    let label_ratio = label_hits as f64 / label_tokens.len() as f64;
    let summary_ratio = if summary_tokens.is_empty() {
        0.0
    } else {
        let hits = summary_tokens
            .iter()
            .filter(|t| quiz_tokens.contains(*t))
            .count();
        hits as f64 / summary_tokens.len().min(12) as f64
    };
    (label_ratio * 0.75 + summary_ratio * 0.25).min(0.8)
}

/// The request carries the user's own material (pictures or files), so it can be generated
/// without any selected source.
/// Scope discipline for generation that starts from the user's own material (a photo of one
/// exercise, a file, a link). Without it the model maps the material to the chapter that contains
/// it and covers the whole chapter.
/// Ids of the sources whose content is in `evidence`. Evidence blocks are labelled with their
/// source's title; the catalog of every source that on-demand reading puts first is not use.
/// Longer titles are matched first so "A" is not counted because "A 第二版" was read.
pub(crate) fn sources_used_in(evidence: &str, refs: &[ArtifactSourceRef]) -> Vec<String> {
    let mut body = evidence
        .split_once("On-Demand Retrieved Excerpts:")
        .map(|(_, b)| b)
        .unwrap_or(evidence)
        .to_string();
    let mut by_len: Vec<&ArtifactSourceRef> =
        refs.iter().filter(|r| !r.title.trim().is_empty()).collect();
    by_len.sort_by_key(|r| std::cmp::Reverse(r.title.chars().count()));
    let mut used = Vec::new();
    for r in by_len {
        if body.contains(r.title.as_str()) {
            used.push(r.id.clone());
            body = body.replace(r.title.as_str(), "\u{0}");
        }
    }
    // keep the artifact's own order
    refs.iter()
        .filter(|r| used.contains(&r.id))
        .map(|r| r.id.clone())
        .collect()
}

/// How a source reader ends when its closing reply should be a structured result (e.g. the
/// lecture outline) instead of "DONE".
pub(crate) struct ReaderFinish {
    /// Appended to the reader's instructions: when to stop and what to reply with.
    pub instruction: String,
    pub schema: Value,
    pub schema_name: &'static str,
    /// Title of the reasoning shown if the result has to be asked for once more.
    pub thought_title: String,
}

pub(crate) const USER_MATERIAL_SCOPE: &str = "USER MATERIAL FIRST: the user attached their own material (<user_images> / <user_files>, or described in the topic above). The scope of this artifact is EXACTLY what that material shows or asks, e.g. one exercise means that one exercise. Do NOT widen it to the chapter, section or neighbouring topics that contain it, and do NOT cover related material the user did not mention. The user's material comes first; relate it to the sources only when that actually helps (a definition, theorem or formula it relies on that you must get right, or a connection worth pointing out): read just those passages, and never produce slides whose only purpose is to locate the material in the book, list prerequisites or recap the chapter. After the material itself is fully covered, a short closing part that grows out of it — a method summary, an extension or variant, common mistakes — is welcome when it adds value.";

fn has_user_material(config: &Value) -> bool {
    ["images", "files"].iter().any(|k| {
        config
            .get(*k)
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    })
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn partial_json_is_closed_and_cut_back() {
        let v = close_partial_json(
            r#"{"title":"T","cards":[{"front":"a","back":"b"},{"front":"c","ba"#,
        )
        .unwrap();
        assert_eq!(v["title"], "T");
        assert_eq!(v["cards"].as_array().unwrap().len(), 2);
        let v = close_partial_json(r#"noise {"q":[1,2,"#).unwrap();
        assert_eq!(v["q"], json!([1, 2]));
        assert!(close_partial_json("no json here").is_none());
    }

    #[test]
    fn quiz_preview_hides_answers() {
        let mut v = json!({"questions":[{"id":"1","type":"single","prompt":"p","options":["a"],
            "answer_indices":[0],"explanation":"e","rubric":"r","citations":[1]}]});
        sanitize_preview(ArtifactType::Quiz, &mut v);
        let q = &v["questions"][0];
        assert!(q.get("answer_indices").is_none() && q.get("explanation").is_none());
        assert_eq!(q["prompt"], "p");
    }
}

#[cfg(test)]
mod mindmap_merge_tests {
    use super::*;

    fn skeleton() -> Value {
        json!({"title":"t","nodes":[
            {"id":"r","parent_id":null,"label":"线性代数","citations":[]},
            {"id":"b1","parent_id":"r","label":"矩阵","citations":[]},
            {"id":"b1x","parent_id":"b1","label":"矩阵的逆","citations":[]},
            {"id":"b2","parent_id":"r","label":"向量空间","citations":[]}
        ]})
    }

    fn node<'a>(p: &'a Value, id: &str) -> Option<&'a Value> {
        p["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
    }

    fn mk(id: &str, parent: Option<&str>, label: &str) -> Value {
        json!({"id": id, "parent_id": parent, "label": label, "citations": []})
    }

    #[test]
    fn review_moves_and_merges_topics() {
        let mut p = json!({"nodes":[
            mk("r", None, "根"), mk("b1", Some("r"), "一"), mk("b2", Some("r"), "二"),
            mk("t1", Some("b1"), "甲"), mk("t2", Some("b1"), "乙"), mk("t3", Some("b2"), "丙"),
            mk("k1", Some("t2"), "子")]});
        let n = mm_apply_review(
            &mut p,
            &[("t1".into(), "b2".into()), ("zz".into(), "b2".into())],
            &[("t3".into(), "t2".into())],
        );
        assert_eq!(n, 2);
        let nodes = mm_nodes(&p);
        let by = |id: &str| nodes.iter().find(|n| mm_str(n, "id") == id).cloned();
        assert_eq!(mm_parent(&by("t1").unwrap()).as_deref(), Some("b2"));
        assert!(by("t2").is_none());
        assert_eq!(mm_parent(&by("k1").unwrap()).as_deref(), Some("t3"));
    }

    #[test]
    fn branches_adopt_dedup_and_expandable() {
        let mut main = json!({"nodes":[mk("r", None, "根"), mk("b1", Some("r"), "一"), mk("b2", Some("r"), "二")]});
        let mut l1 = main.clone();
        merge_mindmap_subtree(
            &mut l1,
            "b1",
            vec![mk("t", Some("b1"), "运算"), mk("k", Some("t"), "向量加法")],
        );
        let mut l2 = main.clone();
        merge_mindmap_subtree(
            &mut l2,
            "b2",
            vec![
                mk("t", Some("b2"), "性质"),
                mk("k", Some("t"), "向量 加法!"),
            ],
        );
        mm_adopt_branch(&mut main, &l2, "b2");
        mm_adopt_branch(&mut main, &l1, "b1");
        mm_sort_by_branch_order(&mut main, &["b1".to_string(), "b2".to_string()]);
        let ids: Vec<String> = mm_nodes(&main).iter().map(|n| mm_str(n, "id")).collect();
        assert_eq!(ids.first().map(String::as_str), Some("r"));
        assert_eq!(ids.len(), 7);
        assert_eq!(mm_dedup_labels(&mut main), 1);
        mm_mark_expandable(&mut main);
        let nodes = mm_nodes(&main);
        assert_eq!(nodes.len(), 6);
        let leaves: Vec<&Value> = nodes
            .iter()
            .filter(|n| n["expandable"] == json!(true))
            .collect();
        assert_eq!(leaves.len(), 2);
    }

    #[test]
    fn shape_folds_deep_nodes_and_single_children_into_facets() {
        let mut p = json!({"title":"t","nodes":[
            {"id":"r","parent_id":null,"label":"根","citations":[]},
            {"id":"b","parent_id":"r","label":"向量","citations":[]},
            {"id":"t1","parent_id":"b","label":"运算","citations":[]},
            {"id":"t2","parent_id":"b","label":"长度","citations":[]},
            {"id":"k1","parent_id":"t1","label":"加法","atomic":true,"citations":[]},
            {"id":"k2","parent_id":"t1","label":"数乘","citations":[]},
            {"id":"d1","parent_id":"k1","label":"交换律","citations":[]},
            {"id":"d2","parent_id":"k1","label":"结合律","citations":[]},
            {"id":"s1","parent_id":"t2","label":"单位向量","citations":[]}
        ]});
        let crowded = mm_enforce_shape(&mut p, "b", 2);
        assert!(crowded.is_empty());
        // level-3 details became facets of their knowledge point
        assert!(node(&p, "d1").is_none() && node(&p, "d2").is_none());
        let k1 = node(&p, "k1").unwrap();
        let mut f: Vec<&str> = k1["facets"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        f.sort();
        assert_eq!(f, vec!["交换律", "结合律"]);
        assert!(k1.get("atomic").is_none());
        // a topic with a single child absorbed it
        assert!(node(&p, "s1").is_none());
        assert_eq!(node(&p, "t2").unwrap()["facets"], json!(["单位向量"]));
    }

    #[test]
    fn crowded_parent_is_reported_and_groups_apply() {
        let mut nodes = vec![
            json!({"id":"r","parent_id":null,"label":"根","citations":[]}),
            json!({"id":"b","parent_id":"r","label":"向量","citations":[]}),
        ];
        for i in 0..9 {
            nodes.push(json!({"id":format!("c{i}"),"parent_id":"b","label":format!("点{i}"),"citations":[]}));
        }
        let mut p = json!({"title":"t","nodes":nodes});
        assert_eq!(mm_enforce_shape(&mut p, "b", 2), vec!["b".to_string()]);
        let groups = json!([
            {"label":"基础运算","summary":"s","members":["c0","c1","c2","c3"]},
            {"label":"度量","summary":"s","members":["c4","c5","nope"]},
            {"label":"单个","summary":"s","members":["c6"]}
        ]);
        let made = mm_apply_groups(&mut p, "b", groups.as_array().unwrap());
        assert_eq!(made, 2);
        assert_eq!(node(&p, "c0").unwrap()["parent_id"], "b.g1");
        assert_eq!(node(&p, "c5").unwrap()["parent_id"], "b.g2");
        assert_eq!(node(&p, "c6").unwrap()["parent_id"], "b");
        assert!(mm_enforce_shape(&mut p, "b", 3).is_empty());
    }

    #[test]
    fn merge_namespaces_ids_and_resolves_parents() {
        let mut p = skeleton();
        let added = merge_mindmap_subtree(
            &mut p,
            "b1",
            vec![
                json!({"id":"n1","parent_id":"b1x","label":"伴随矩阵法","citations":[3]}),
                json!({"id":"n2","parent_id":"b1x","label":"初等变换法","citations":[]}),
                json!({"id":"n3","parent_id":"n2","label":"[A|I]→[I|A⁻¹]","citations":[]}),
                json!({"id":"n4","parent_id":"b2","label":"越界节点","citations":[]}),
                json!({"id":"b1x","parent_id":"b1","label":"矩阵的逆","citations":[]}),
                json!({"id":"n5","parent_id":"b1","label":" 矩阵的逆 ","citations":[]}),
                json!({"id":"n6","parent_id":"n5","label":"分块求逆","citations":[]}),
            ],
        );
        let nodes = mm_nodes(&p);
        let by = |label: &str| {
            nodes
                .iter()
                .find(|n| mm_str(n, "label") == label)
                .unwrap()
                .clone()
        };
        assert_eq!(added, 5);
        assert_eq!(by("伴随矩阵法")["id"], "b1.n1");
        assert_eq!(by("伴随矩阵法")["parent_id"], "b1x");
        assert_eq!(by("伴随矩阵法")["citations"], json!([]));
        assert_eq!(by("[A|I]→[I|A⁻¹]")["parent_id"], "b1.n2");
        // a parent outside the anchor's subtree falls back to the anchor
        assert_eq!(by("越界节点")["parent_id"], "b1");
        // a duplicate of an existing sibling is merged; its children attach to the original
        assert_eq!(
            nodes
                .iter()
                .filter(|n| mm_str(n, "label").trim() == "矩阵的逆")
                .count(),
            1
        );
        assert_eq!(by("分块求逆")["parent_id"], "b1x");
    }

    #[test]
    fn tree_helpers_follow_paths() {
        let nodes = mm_nodes(&skeleton());
        assert_eq!(mm_root_id(&nodes).as_deref(), Some("r"));
        assert_eq!(mm_ancestor_labels(&nodes, "b1x"), vec!["线性代数", "矩阵"]);
        assert_eq!(mm_top_branch(&nodes, "b1x", "r").as_deref(), Some("b1"));
        let mut outline = String::new();
        mm_outline(&nodes, "b1", 0, &mut outline);
        assert_eq!(outline, "- [b1x] 矩阵的逆\n");
    }
}

#[cfg(test)]
mod ui_language_tests {
    use super::*;

    #[test]
    fn explicit_ui_language_wins_over_the_system_language() {
        assert!(ui_language_is_english("en-US"));
        assert!(ui_language_is_english("EN"));
        assert!(!ui_language_is_english("zh-CN"));
        assert!(!ui_language_is_english(" zh-Hans "));
    }

    #[test]
    fn status_messages_carry_both_languages() {
        let msg = format!("中文{}English", lna_llm::STATUS_LANG_SEP);
        assert_eq!(lna_llm::pick_status_lang(&msg, true), "English");
        assert_eq!(lna_llm::pick_status_lang(&msg, false), "中文");
        assert_eq!(lna_llm::pick_status_lang("plain", true), "plain");
    }
}

#[cfg(test)]
mod used_sources_tests {
    use super::*;

    fn r(id: &str, title: &str) -> ArtifactSourceRef {
        ArtifactSourceRef {
            id: id.into(),
            title: title.into(),
            kind: "pdf".into(),
        }
    }

    #[test]
    fn only_sources_with_read_excerpts_count() {
        let refs = [
            r("a", "大学物理"),
            r("b", "大学物理 第二版"),
            r("c", "线性代数"),
        ];
        let ev = "Source Outline:\n大学物理\n线性代数\n大学物理 第二版\n\nOn-Demand Retrieved Excerpts:\n[1] (大学物理 第二版, p.3) 力矩……";
        assert_eq!(sources_used_in(ev, &refs), vec!["b".to_string()]);
        // evidence without a catalog: every labelled block counts
        let ev = "[1] (线性代数, p.1) 矩阵\n[2] (大学物理, p.9) 转动";
        assert_eq!(
            sources_used_in(ev, &refs),
            vec!["a".to_string(), "c".to_string()]
        );
        assert!(sources_used_in("", &refs).is_empty());
    }
}
