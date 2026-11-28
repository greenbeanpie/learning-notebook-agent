pub mod archive;
mod probe_media;

use lna_agent::AgentService;
use lna_embedding::EmbeddingService;
use lna_ingest::IngestService;
use lna_jobs::JobQueue;
use lna_llm::{LlmClient, LlmEvent, LlmRequest, OutputMode, Part, ToolDef, ToolMode};
use lna_retrieval::RetrievalEngine;
use lna_secrets::{KeyringSecretStore, SecretStore};
use lna_storage::{db::map_sql_err, Database, StoragePaths};
use lna_studio::StudioService;
use lna_types::*;
use rusqlite::params;
use secrecy::SecretString;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub struct CoreRuntime {
    pub db: Database,
    pub secrets: Arc<dyn SecretStore>,
    pub jobs: JobQueue,
    pub llm: LlmClient,
    pub embedding: EmbeddingService,
    pub ingest: IngestService,
    pub retrieval: Arc<RetrievalEngine>,
    pub agent: AgentService,
    pub studio: StudioService,
    pub events: broadcast::Sender<ApiEvent>,
    seq: Arc<AtomicU64>,
    shutdown: CancellationToken,
    /// The model download in progress, if any: (cancel token, latest progress).
    model_download: Arc<std::sync::Mutex<Option<(CancellationToken, Value)>>>,
    rt: tokio::runtime::Runtime,
    /// Handle to this runtime for background work started from `&self` (see `spawn_profile_probe`).
    self_ref: std::sync::Weak<Self>,
}

impl CoreRuntime {
    pub fn open(data_dir: PathBuf, allow_memory_secret_fallback: bool) -> LnaResult<Arc<Self>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2) // Section 3: max 2 worker threads
            .enable_all()
            .build()
            .map_err(|e| LnaError::internal(format!("Failed to build tokio runtime: {e}")))?;

        lna_ingest::ffmpeg::set_tools_dir(&data_dir);
        let paths = StoragePaths::from_root(data_dir)?;
        let db = Database::open(paths)?;
        db.recover_on_startup()?;

        let secrets: Arc<dyn SecretStore> =
            Arc::new(KeyringSecretStore::new(allow_memory_secret_fallback));
        let jobs = JobQueue::new(db.clone());
        let llm = LlmClient::new();
        {
            // D5: persist capabilities learned from "unsupported parameter" responses.
            let db = db.clone();
            llm.set_capability_sink(move |profile_id, wire| {
                let _ = lna_storage::update_profile_wire_params(
                    &db,
                    profile_id,
                    wire.stream_usage,
                    wire.max_tokens_field.as_deref(),
                );
            });
        }
        // Per-profile limit on requests in flight (user setting in the AI settings).
        for prof in lna_storage::list_profiles(&db).unwrap_or_default() {
            llm.set_parallel_limit(&prof.id, prof.max_parallel);
        }
        let saved_debug: bool = db
            .read(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT value_json FROM app_settings WHERE key='debug_mode'",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .ok()
                    .and_then(|s| serde_json::from_str::<bool>(&s).ok())
                    .unwrap_or(false))
            })
            .unwrap_or(false);
        llm.configure_logging(db.paths().root.join("logs").join("lna.log"), saved_debug);
        if let Ok(Some(raw_limits)) = db.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key='max_token_limits'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok())
        }) {
            if let Ok(parsed) = serde_json::from_str::<lna_llm::MaxTokenLimits>(&raw_limits) {
                llm.set_token_limits(parsed);
            }
        }
        llm.append_log(
            "INFO",
            "lna_api::core",
            &format!(
                "Core opened (version={}, debug_mode={})",
                env!("CARGO_PKG_VERSION"),
                saved_debug
            ),
        );
        let embedding = EmbeddingService::new(db.clone(), secrets.clone());
        let retrieval = Arc::new(RetrievalEngine::new(db.clone(), embedding.clone()));
        let (events, _) = broadcast::channel(512);
        let seq = Arc::new(AtomicU64::new(0));
        let ingest = IngestService::new(
            db.clone(),
            jobs.clone(),
            embedding.clone(),
            llm.clone(),
            secrets.clone(),
        )
        .with_events(events.clone(), seq.clone());

        let studio = StudioService::new(
            db.clone(),
            secrets.clone(),
            jobs.clone(),
            llm.clone(),
            retrieval.clone(),
            events.clone(),
            seq.clone(),
        );
        let agent = AgentService::new(
            db.clone(),
            secrets.clone(),
            llm.clone(),
            retrieval.clone(),
            studio.clone(),
            events.clone(),
            seq.clone(),
        );

        let core = Arc::new_cyclic(|me| Self {
            db,
            secrets,
            jobs,
            llm,
            embedding,
            ingest,
            retrieval,
            agent,
            studio,
            events,
            seq,
            shutdown: CancellationToken::new(),
            model_download: Arc::new(std::sync::Mutex::new(None)),
            rt,
            self_ref: me.clone(),
        });

        core.spawn_background_loops();
        Ok(core)
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

    /// `has_secret` must reflect whether a key is actually stored, not merely
    /// whether the profile carries a keychain reference (B-25).
    fn refresh_has_secret(&self, prof: &mut ModelProfile) {
        prof.has_secret = match prof.keychain_ref.as_deref() {
            Some(kref) => self.secrets.exists(kref).unwrap_or(false),
            None => false,
        };
    }

    /// Tell clients that a notebook's sources changed state (QUEUED → PROCESSING → READY/FAILED),
    /// so they can refresh without polling.
    fn emit_source_status(&self, job: &lna_jobs::JobRecord) {
        if job.kind != "ingest_source" {
            return;
        }
        let nid = job.notebook_id.clone().unwrap_or_default();
        self.emit(
            &format!("sources:{nid}"),
            "source.status",
            json!({ "notebook_id": nid, "source_id": job.subject_id }),
        );
    }

    /// Probes streaming, tool calling, JSON schema and JSON mode against the live endpoint, and
    /// detects image / audio / video input with a small sample each (all three, whatever was
    /// stored). A modality is only set when its sample is understood. The third value lists the
    /// modalities whose probe failed unexpectedly (server, network, auth), not the ones the model
    /// simply does not take. Connection/auth failures are returned as errors (there is no separate
    /// ping: the first real probe request shows them). JSON mode is only tried when JSON Schema
    /// is not available, since either one is enough to use the model.
    async fn probe_capabilities(
        &self,
        resolved: &lna_llm::ResolvedProfile,
        base: ModelCapabilities,
    ) -> LnaResult<(ModelCapabilities, String, Vec<Value>)> {
        let mut caps = base;
        caps.streaming = true;

        let probe_schema = json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        });
        // Room for models that think before answering: a small cap is spent on reasoning and
        // leaves no tool call / JSON, which would wrongly mark the model as unsupported.
        let tool_req = LlmRequest::simple(
            "You must call the lna_capability_probe tool exactly once with {\"ok\":true}. Do not answer in natural language.",
            "Call the capability probe now.",
            4096,
        )
        .with_tools(
            vec![ToolDef {
                name: "lna_capability_probe".into(),
                description: "Capability detection probe".into(),
                parameters: probe_schema.clone(),
            }],
            ToolMode::Auto,
        );
        let schema_req = LlmRequest::simple(
            "Return only a JSON object with ok set to true.",
            "Return the probe object.",
            4096,
        )
        .with_output(OutputMode::JsonSchema {
            name: "lna_capability_probe".into(),
            schema: probe_schema,
        });
        let json_mode_req = LlmRequest::simple(
            "Return only a JSON object with ok set to true.",
            "Return the probe object.",
            4096,
        )
        .with_output(OutputMode::JsonObject);

        let tool_fut =
            async {
                let mut tool_called = false;
                let mut usage_seen = false;
                match self
                    .llm
                    .stream_strict(resolved, &tool_req, CancellationToken::new(), |event| {
                        match event {
                            LlmEvent::ToolCallStart { .. } => tool_called = true,
                            LlmEvent::Usage(u) if u.input_tokens > 0 || u.output_tokens > 0 => {
                                usage_seen = true
                            }
                            _ => {}
                        }
                    })
                    .await
                {
                    Ok(_) => Ok((tool_called, usage_seen)),
                    Err(e) if is_connection_failure(&e) => Err(e),
                    Err(_) => Ok((false, false)),
                }
            };
        // One structured-output check: Ok(true) when the model answered `{"ok":true}`.
        let structured = |req: LlmRequest| async move {
            match self
                .llm
                .complete_text_strict(resolved, &req, CancellationToken::new())
                .await
            {
                Ok((raw, _)) => Ok(serde_json::from_str::<Value>(&raw)
                    .ok()
                    .map(|value| value.get("ok").and_then(Value::as_bool) == Some(true))
                    .unwrap_or(false)),
                Err(e) if is_connection_failure(&e) => Err(e),
                Err(_) => Ok(false),
            }
        };
        // JSON Schema first; JSON mode only when the schema did not work (either is enough).
        let structured_fut = async {
            if structured(schema_req.clone()).await? {
                return Ok((true, None));
            }
            let mode = structured(json_mode_req.clone()).await?;
            Ok::<_, LnaError>((false, Some(mode)))
        };

        // The capability probes are independent and bounded by LlmClient's per-profile gate.
        let (tool_res, structured_res, vision, audio, video) = tokio::join!(
            tool_fut,
            structured_fut,
            self.probe_media(resolved, MediaProbe::Image),
            self.probe_media(resolved, MediaProbe::Audio),
            self.probe_media(resolved, MediaProbe::Video),
        );
        let (tools_ok, usage_seen) = tool_res?;
        let (schema_ok, json_mode_ok) = structured_res?;
        caps.stream_usage = usage_seen;
        caps.tools = tools_ok;
        caps.json_schema = schema_ok;
        if let Some(mode) = json_mode_ok {
            caps.json_mode = mode;
        }
        let mut failed = Vec::new();
        let mut settle = |kind: &str, verdict: Result<bool, String>| match verdict {
            Ok(seen) => seen,
            Err(detail) => {
                failed.push(json!({ "kind": kind, "detail": detail }));
                false
            }
        };
        caps.supports_vision = settle("vision", vision);
        caps.supports_audio = settle("audio", audio);
        caps.supports_video = settle("video", video);

        caps.probed_at = Some(lna_types::now_ms());
        Ok((caps, resolved.model_id.clone(), failed))
    }

    /// Sends one media sample and checks the reply describes it. `Ok(true)`: the sample was
    /// understood. `Ok(false)`: not understood, or the protocol / endpoint does not take this kind
    /// of input (a clean "not supported"). `Err`: an unexpected failure (server, network, auth).
    async fn probe_media(
        &self,
        resolved: &lna_llm::ResolvedProfile,
        kind: MediaProbe,
    ) -> Result<bool, String> {
        use probe_media as pm;
        let (question, mime, data, audio_format) = match kind {
            MediaProbe::Image => (
                "What single colour fills the attached image? Reply with one word.".to_string(),
                "image/png",
                pm::b64(&pm::red_png()),
                None,
            ),
            MediaProbe::Audio => (
                format!(
                    "How many separate beeps are in the attached audio? Reply with just the number. If no audio is attached, reply exactly {}.",
                    pm::NO_AUDIO
                ),
                "audio/wav",
                pm::b64(&pm::tone_wav()),
                Some("wav".to_string()),
            ),
            MediaProbe::Video => (
                "What single colour fills every frame of the attached video? Reply with one word."
                    .to_string(),
                "video/mp4",
                pm::b64(pm::RED_MP4),
                None,
            ),
        };
        let req = LlmRequest::from_parts(
            "You are checking which kinds of input reach you. Answer briefly and honestly; never guess about an attachment you did not receive.",
            vec![
                Part::Text(question),
                Part::Media {
                    mime_type: mime.into(),
                    data_base64: Some(data),
                    file_uri: None,
                    audio_format,
                },
            ],
            // room for models that think before answering: a small cap is spent on reasoning and
            // leaves no answer, which would wrongly mark the input as unsupported
            4096,
        );
        let reply = match self
            .llm
            .complete_text_strict(resolved, &req, CancellationToken::new())
            .await
        {
            Ok((reply, _)) => reply,
            // the protocol cannot build this input, or the endpoint rejects it as a bad request
            Err(e)
                if matches!(
                    e.code,
                    ErrorCode::CapabilityMissing | ErrorCode::ModelBadRequest
                ) =>
            {
                return Ok(false)
            }
            Err(e) => return Err(e.message),
        };
        Ok(match kind {
            MediaProbe::Audio => pm::heard_audio(&reply),
            MediaProbe::Image | MediaProbe::Video => pm::names_sample_color(&reply),
        })
    }

    /// Probes a saved profile and stores what the endpoint showed, then tells clients the profile
    /// changed. Nothing is stored when the probe fails (the profile stays "not checked yet") or when
    /// its connection was edited while the probe ran, since the result would describe the old one.
    async fn probe_stored_profile(&self, saved: &ModelProfile) {
        let Ok(resolved) = self.agent.resolve_profile(saved) else {
            return;
        };
        let Ok((caps, _, _)) = self
            .probe_capabilities(&resolved, saved.capabilities.clone())
            .await
        else {
            return;
        };
        let Ok(now) = lna_storage::get_profile(&self.db, &saved.id) else {
            return;
        };
        let same_connection = now.protocol == saved.protocol
            && now.base_url == saved.base_url
            && now.model_id == saved.model_id
            && now.extra_headers == saved.extra_headers
            && now.preset == saved.preset;
        if same_connection
            && lna_storage::update_profile_capabilities(&self.db, &saved.id, &caps).is_ok()
        {
            self.emit(
                "profiles",
                "profile.updated",
                json!({ "profile_id": saved.id }),
            );
        }
    }

    /// Probes a profile that has not been checked yet in the background, so its capabilities show
    /// up without pressing "test". Runs after a save; the caller has stored the API key already.
    fn spawn_profile_probe(&self, saved: ModelProfile) {
        let weak = self.self_ref.clone();
        self.rt.spawn(async move {
            if let Some(core) = weak.upgrade() {
                core.probe_stored_profile(&saved).await;
            }
        });
    }

    async fn ping(
        &self,
        resolved: &lna_llm::ResolvedProfile,
    ) -> LnaResult<(String, lna_llm::Usage)> {
        let req = LlmRequest::simple("Reply with OK.", "ping", 512);
        self.llm
            .complete_text_strict(resolved, &req, CancellationToken::new())
            .await
    }

    fn spawn_background_loops(self: &Arc<Self>) {
        // S3: Delayed 5s orphan file cleanup
        let weak_clean = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(core) = weak_clean.upgrade() {
                let _ = lna_ingest::IngestService::cleanup_orphan_files(&core.db);
                // pictures from lecture questions and failed sends, once a day old
                lna_ingest::attachments::sweep_orphans(
                    &core.db,
                    std::time::Duration::from_secs(24 * 3600),
                );
            }
        });

        // Profiles saved before the tool / structured-output requirement were never probed:
        // probe them once so unqualified ones are refused from then on.
        let weak_probe = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let Some(core) = weak_probe.upgrade() else {
                return;
            };
            let pending: Vec<ModelProfile> = lna_storage::list_profiles(&core.db)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| {
                    let c = &p.capabilities;
                    !c.embedding_only && c.probed_at.is_none() && c.chat_missing().is_some()
                })
                .collect();
            let _ = lna_llm::run_parallel_unordered(
                pending,
                4,
                &core.shutdown,
                |_, prof| {
                    let core = core.clone();
                    async move { core.probe_stored_profile(&prof).await }
                },
                |_, ()| Ok(()),
            )
            .await;
        });

        // Worker 1: Fast ingest & summary loop (never blocked by Studio, B-14)
        let weak1 = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut tick = 0u64;
            // Imports run side by side; their model calls are bounded by the profile's own limit.
            let bg_slots = Arc::new(tokio::sync::Semaphore::new(8));
            loop {
                let Some(core) = weak1.upgrade() else { break };
                if core.shutdown.is_cancelled() {
                    break;
                }
                if tick.is_multiple_of(60) {
                    core.db.evict_idle_readers();
                    core.embedding.evict_if_idle();
                }
                tick += 1;
                let Ok(bg_permit) = bg_slots.clone().acquire_owned().await else {
                    break;
                };
                if let Ok(Some(job)) =
                    core.jobs
                        .claim_next(&["ingest_source", "refresh_summary", "embed_source"])
                {
                    let _tok = core.jobs.register_token(&job.id);
                    core.emit_source_status(&job);
                    let core = core.clone();
                    tokio::spawn(async move {
                        let _bg_permit = bg_permit;
                        let res = match job.kind.as_str() {
                            "ingest_source" => {
                                core.ingest.process_ingest_job(&job.subject_id).await
                            }
                            "refresh_summary" => {
                                core.agent
                                    .refresh_notebook_summary(&job.subject_id, _tok.clone())
                                    .await
                            }
                            "embed_source" => {
                                let (sid, target_mid) = match job.subject_id.split_once('@') {
                                    Some((s, m)) => (s, Some(m)),
                                    None => (job.subject_id.as_str(), None),
                                };
                                match lna_storage::get_source(&core.db, sid) {
                                    Ok(src) => {
                                        if let Some(mid) = target_mid {
                                            match core.embedding.get_model(mid) {
                                                Ok(Some(model)) => {
                                                    core.ingest
                                                        .embed_source_chunks_for_model(
                                                            &src.id,
                                                            &src.notebook_id,
                                                            &src.title,
                                                            &model,
                                                        )
                                                        .await
                                                }
                                                Ok(None) => Err(LnaError::not_found(
                                                    "Target embedding model not found",
                                                )),
                                                Err(e) => Err(e),
                                            }
                                        } else {
                                            core.ingest
                                                .embed_source_chunks(
                                                    &src.id,
                                                    &src.notebook_id,
                                                    &src.title,
                                                )
                                                .await
                                        }
                                    }
                                    Err(e) => Err(e),
                                }
                            }
                            _ => Ok(()),
                        };
                        match res {
                            Ok(()) => {
                                let _ = core.jobs.mark_done(&job.id);
                                if job.kind == "embed_source" {
                                    if let Some((_, mid)) = job.subject_id.split_once('@') {
                                        let suffix = format!("%@{mid}");
                                        let remaining: i64 = core
                                            .db
                                            .read(|conn| {
                                                conn.query_row(
                                                    "SELECT COUNT(*) FROM jobs
                                                 WHERE kind = 'embed_source'
                                                   AND subject_id LIKE ?1
                                                   AND status IN ('QUEUED','RUNNING')",
                                                    [suffix],
                                                    |r| r.get(0),
                                                )
                                                .map_err(map_sql_err)
                                            })
                                            .unwrap_or(1);
                                        if remaining == 0 {
                                            let _ = core.embedding.activate_model(mid);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = core.jobs.mark_failed_or_retry(
                                    &job.id,
                                    &e.message,
                                    5000,
                                    e.retryable,
                                );
                            }
                        }
                        core.emit_source_status(&job);
                    });
                } else {
                    drop(bg_permit);
                    tokio::select! {
                        _ = core.shutdown.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                    }
                }
            }
        });

        // Worker 2: Dedicated Studio parallel dispatcher. Artifacts are not capped here any more:
        // what is bounded is the number of model requests in flight per profile (AI settings), so
        // the cap below only guards against runaway queues.
        let weak2 = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let studio_slots = Arc::new(tokio::sync::Semaphore::new(64));
            loop {
                let Some(core) = weak2.upgrade() else { break };
                if core.shutdown.is_cancelled() {
                    break;
                }
                let Ok(permit) = studio_slots.clone().acquire_owned().await else {
                    break;
                };
                if core.shutdown.is_cancelled() {
                    drop(permit);
                    break;
                }
                if let Ok(Some(job)) = core.jobs.claim_next(&["studio_generate"]) {
                    let tok = core.jobs.register_token(&job.id);
                    let core_job = core.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        match core_job.studio.process_job(&job.subject_id, tok).await {
                            Ok(()) => {
                                let _ = core_job.jobs.mark_done(&job.id);
                            }
                            Err(e) => {
                                // StudioService::process_job owns the 3-retry rollback loop and records error_log on terminal failure.
                                let _ = core_job
                                    .jobs
                                    .mark_failed_or_retry(&job.id, &e.message, 0, false);
                                if e.code == ErrorCode::Cancelled {
                                    let _ = core_job.studio.mark_cancelled(&job.subject_id);
                                }
                            }
                        }
                    });
                } else {
                    drop(permit);
                    tokio::select! {
                        _ = core.shutdown.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                    }
                }
            }
        });
    }

    /// Notebooks that already have sources but no summary get one once a model is available (B-13).
    fn enqueue_missing_summaries(&self) {
        if let Ok(nbs) = lna_storage::list_notebooks(&self.db) {
            for nb in nbs {
                if nb.summary_md.is_none() && nb.source_count > 0 {
                    let _ = self
                        .jobs
                        .enqueue("refresh_summary", Some(&nb.id), &nb.id, 200);
                }
            }
        }
    }

    pub fn handle_json(&self, raw_req: &str) -> String {
        let req: ApiRequest = match serde_json::from_str(raw_req) {
            Ok(r) => r,
            Err(e) => {
                let resp = ApiResponse::err(
                    "".into(),
                    LnaError::invalid(format!("Invalid request JSON: {e}")),
                );
                return serde_json::to_string(&resp).unwrap_or_default();
            }
        };
        let id = req.id.clone();
        let res = self.rt.block_on(self.dispatch(req));
        let resp = match res {
            Ok(v) => ApiResponse::ok(id, v),
            Err(e) => ApiResponse::err(id, e),
        };
        serde_json::to_string(&resp).unwrap_or_default()
    }

    /// Stores the pictures of a request's `images` parameter (see `lna_ingest::attachments`).
    /// Pictures (`images`), one-off reference files (`files`) and web links (`links`) of a
    /// request, stored. Nothing is kept when one of them is rejected.
    fn save_request_images(&self, p: &Value) -> LnaResult<Vec<Attachment>> {
        let non_empty = |k: &str| {
            p.get(k)
                .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        };
        let mut out = Vec::new();
        let res = (|| -> LnaResult<()> {
            if let Some(items) = non_empty("images") {
                out.extend(lna_ingest::attachments::save_images(
                    &self.db.paths().root,
                    items,
                )?);
            }
            if let Some(items) = non_empty("files") {
                let saved = lna_ingest::context_files::save_files(&self.db, items, out.len())?;
                out.extend(saved);
            }
            if let Some(items) = non_empty("links") {
                let saved = lna_ingest::context_files::save_links(&self.db, items, out.len())?;
                out.extend(saved);
            }
            Ok(())
        })();
        if let Err(e) = res {
            lna_ingest::attachments::delete_files(&out);
            return Err(e);
        }
        Ok(out)
    }

    pub async fn dispatch(&self, req: ApiRequest) -> LnaResult<Value> {
        let p = &req.params;
        match req.method.as_str() {
            // --- System ---
            "app.info" => Ok(json!({
                "name": "Local Notebook Agent",
                "version": env!("CARGO_PKG_VERSION"),
                "data_dir": self.db.paths().root,
                "studio_modalities": ["audio","slides","mindmap","report","flashcards","quiz","infographic","lecture"]
            })),
            "usage.summary" => {
                let nid = p.get("notebook_id").and_then(|x| x.as_str());
                let totals = lna_storage::get_usage_summary(&self.db, nid)?;
                Ok(serde_json::to_value(totals).unwrap_or_default())
            }
            "settings.get" => {
                let key = req_str(p, "key")?;
                let val: Option<String> = self.db.read(|conn| {
                    conn.query_row(
                        "SELECT value_json FROM app_settings WHERE key=?1",
                        [key],
                        |r| r.get(0),
                    )
                    .ok()
                    .map(Ok)
                    .transpose()
                })?;
                Ok(
                    json!({ "key": key, "value": val.and_then(|s| serde_json::from_str::<Value>(&s).ok()) }),
                )
            }
            "settings.set" => {
                let key = req_str(p, "key")?;
                let val = p.get("value").cloned().unwrap_or(Value::Null).to_string();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO app_settings (key, value_json) VALUES (?1, ?2)",
                        params![key, val],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(json!({ "ok": true }))
            }
            "settings.token_limits.get" => Ok(json!({
                "limits": self.llm.token_limits(),
                "defaults": lna_llm::MaxTokenLimits::default(),
            })),
            "settings.token_limits.set" => {
                let val = p.get("limits").cloned().unwrap_or_else(|| p.clone());
                let limits: lna_llm::MaxTokenLimits = serde_json::from_value(val)
                    .map_err(|e| LnaError::invalid(format!("Invalid max_token_limits: {e}")))?;
                let raw = serde_json::to_string(&limits).unwrap_or_default();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO app_settings (key, value_json) VALUES ('max_token_limits', ?1)",
                        [raw],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                self.llm.set_token_limits(limits.clone());
                self.llm.append_log(
                    "INFO",
                    "lna_api::settings",
                    &format!("Updated max_token_limits: {:?}", limits),
                );
                Ok(json!({
                    "ok": true,
                    "limits": limits,
                    "defaults": lna_llm::MaxTokenLimits::default(),
                }))
            }
            "settings.token_limits.reset" => {
                let defaults = lna_llm::MaxTokenLimits::default();
                self.db.write(|conn| {
                    conn.execute("DELETE FROM app_settings WHERE key='max_token_limits'", [])
                        .map_err(map_sql_err)?;
                    Ok(())
                })?;
                self.llm.set_token_limits(defaults.clone());
                self.llm.append_log(
                    "INFO",
                    "lna_api::settings",
                    "Reset max_token_limits to official defaults",
                );
                Ok(json!({
                    "ok": true,
                    "limits": defaults,
                    "defaults": lna_llm::MaxTokenLimits::default(),
                }))
            }
            "settings.debug.get" => Ok(json!({
                "enabled": self.llm.debug_mode(),
                "log_path": self.llm.log_path().map(|p| p.to_string_lossy().into_owned()),
                "size_bytes": self.llm.log_size_bytes(),
            })),
            "settings.debug.set" => {
                let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO app_settings (key, value_json) VALUES ('debug_mode', ?1)",
                        [if enabled { "true" } else { "false" }],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                self.llm.set_debug_mode(enabled);
                self.llm.append_log(
                    "INFO",
                    "lna_api::settings",
                    &format!("Debug mode set to {enabled}"),
                );
                Ok(json!({ "ok": true, "enabled": enabled }))
            }
            "logs.export" => {
                let out_path = p
                    .get("out_path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(std::path::Path::new);
                self.llm.export_logs(out_path)
            }
            "logs.clear" => {
                self.llm.clear_logs()?;
                Ok(json!({ "ok": true }))
            }

            // --- Notebook ---
            "notebook.list" => Ok(json!({ "notebooks": lna_storage::list_notebooks(&self.db)? })),
            "notebook.create" => {
                // A title the user typed is theirs: the AI summary never renames it. Only a
                // notebook created without a title gets a placeholder the AI may replace.
                let typed = p
                    .get("title")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty());
                let emoji = p.get("emoji").and_then(|x| x.as_str());
                let nb =
                    lna_storage::create_notebook(&self.db, typed.unwrap_or("未命名笔记本"), emoji)?;
                if let Some(t) = typed {
                    lna_storage::rename_notebook(&self.db, &nb.id, t, true)?;
                }
                Ok(serde_json::to_value(lna_storage::get_notebook(&self.db, &nb.id)?).unwrap())
            }
            "notebook.get" => {
                let id = req_str(p, "notebook_id")?;
                Ok(serde_json::to_value(lna_storage::get_notebook(&self.db, id)?).unwrap())
            }
            "notebook.rename" => {
                let id = req_str(p, "notebook_id")?;
                let title = req_str(p, "title")?;
                lna_storage::rename_notebook(&self.db, id, title, true)?;
                Ok(json!({ "ok": true }))
            }
            "notebook.delete" => {
                let id = req_str(p, "notebook_id")?;
                // pictures of its chat messages and Studio requests go with it
                let pictures = self.agent.attachments_of_notebook(id);
                let studio_pics = self.studio.attachments_of_notebook(id);
                lna_storage::delete_notebook(&self.db, id)?;
                for atts in pictures.iter().chain(studio_pics.iter()) {
                    lna_ingest::attachments::delete_files(atts);
                }
                Ok(json!({ "ok": true }))
            }

            // --- Source ---
            "source.list" => {
                let nid = req_str(p, "notebook_id")?;
                Ok(json!({ "sources": lna_storage::list_sources(&self.db, nid)? }))
            }
            "source.add_files" => {
                let nid = req_str(p, "notebook_id")?;
                let paths = p
                    .get("paths")
                    .and_then(|x| x.as_array())
                    .ok_or_else(|| LnaError::invalid("paths array required"))?;
                let mut added = Vec::new();
                for path_val in paths {
                    if let Some(ps) = path_val.as_str() {
                        let (src, dup) = self.ingest.add_file(nid, std::path::Path::new(ps))?;
                        added.push(json!({ "source": src, "duplicate": dup }));
                    }
                }
                Ok(json!({ "items": added }))
            }
            "source.add_text" => {
                let nid = req_str(p, "notebook_id")?;
                let title = p
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or("粘贴文本");
                let text = req_str(p, "text")?;
                let (src, dup) = self.ingest.add_text(nid, title, text, SourceKind::Txt)?;
                Ok(json!({ "source": src, "duplicate": dup }))
            }
            "source.add_url" => {
                let nid = req_str(p, "notebook_id")?;
                let url = req_str(p, "url")?;
                let (src, dup) = self.ingest.add_url(nid, url).await?;
                Ok(json!({ "source": src, "duplicate": dup }))
            }
            "source.set_selected" => {
                let sid = req_str(p, "source_id")?;
                let sel = p.get("selected").and_then(|x| x.as_bool()).unwrap_or(true);
                lna_storage::set_source_selected(&self.db, sid, sel)?;
                Ok(json!({ "ok": true }))
            }
            "source.set_all_selected" => {
                let nid = req_str(p, "notebook_id")?;
                let sel = p.get("selected").and_then(|x| x.as_bool()).unwrap_or(true);
                lna_storage::set_all_sources_selected(&self.db, nid, sel)?;
                Ok(json!({ "ok": true }))
            }
            "source.rename" => {
                let sid = req_str(p, "source_id")?;
                let title = req_str(p, "title")?;
                lna_storage::rename_source(&self.db, sid, title)?;
                Ok(json!({ "ok": true }))
            }
            "source.get" => {
                let sid = req_str(p, "source_id")?;
                Ok(serde_json::to_value(lna_storage::get_source(&self.db, sid)?).unwrap())
            }
            "source.retry" => {
                let sid = req_str(p, "source_id")?;
                let src = lna_storage::get_source(&self.db, sid)?;
                self.db.write(|conn| {
                    conn.execute(
                        "UPDATE sources SET status='QUEUED', error_code=NULL, error_detail=NULL, updated_at=?1 WHERE id=?2",
                        params![now_ms(), sid],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                self.emit(
                    &format!("sources:{}", src.notebook_id),
                    "source.status",
                    json!({ "notebook_id": src.notebook_id, "source_id": sid }),
                );
                self.jobs
                    .enqueue("ingest_source", Some(&src.notebook_id), sid, 10)?;
                Ok(json!({ "ok": true }))
            }
            "source.remove" => {
                let sid = req_str(p, "source_id")?;
                lna_storage::remove_source(&self.db, sid)?;
                Ok(json!({ "ok": true }))
            }

            // --- Chat ---
            "chat.history" => {
                let nid = req_str(p, "notebook_id")?;
                Ok(json!({ "messages": self.agent.list_history(nid)? }))
            }
            "chat.send" => {
                let nid = req_str(p, "notebook_id")?;
                let atts = self.save_request_images(p)?;
                let text = match p.get("text").and_then(Value::as_str).map(str::trim) {
                    Some(t) if !t.is_empty() => t,
                    // a picture on its own is a valid message
                    _ if !atts.is_empty() => "",
                    _ => return Err(LnaError::invalid("text required")),
                };
                // On failure the user message (with its pictures) is usually already stored, so the
                // files are kept; pictures no message refers to are removed by the orphan sweep.
                let msg = self.agent.send_message(nid, text, atts).await?;
                Ok(serde_json::to_value(msg).unwrap())
            }
            "attachment.promote" => {
                let nid = req_str(p, "notebook_id")?;
                let id = req_str(p, "attachment_id")?;
                let found = lna_ingest::attachments::resolve(&self.db.paths().root, &json!([id]));
                let att = found
                    .into_iter()
                    .next()
                    .ok_or_else(|| LnaError::not_found("附件不存在"))?;
                let (src, dup) = if att.kind == "link" {
                    self.ingest
                        .add_url(nid, att.url.as_deref().unwrap_or_default())
                        .await?
                } else {
                    // imported under its original name; the attachment itself stays with the message
                    let tmp = std::env::temp_dir().join(format!("lna_promote_{}", att.id));
                    std::fs::create_dir_all(&tmp).map_err(|e| LnaError::internal(e.to_string()))?;
                    let named = tmp.join(&att.name);
                    std::fs::copy(&att.path, &named)
                        .map_err(|e| LnaError::internal(e.to_string()))?;
                    let r = self.ingest.add_file(nid, &named);
                    let _ = std::fs::remove_dir_all(&tmp);
                    r?
                };
                Ok(json!({ "source": src, "duplicate": dup }))
            }
            "chat.cancel" => {
                let nid = req_str(p, "notebook_id")?;
                self.agent.cancel(nid)?;
                Ok(json!({ "ok": true }))
            }

            // --- Profile & Secrets ---
            "profile.presets" => Ok(json!({ "presets": builtin_provider_presets() })),
            "profile.list" => {
                let mut profiles = lna_storage::list_profiles(&self.db)?;
                for prof in profiles.iter_mut() {
                    self.refresh_has_secret(prof);
                }
                let rows: Vec<Value> = profiles
                    .iter()
                    .map(|prof| {
                        let mut v = serde_json::to_value(prof).unwrap_or_default();
                        let caps = &prof.capabilities;
                        v["purpose"] = json!(if caps.embedding_only {
                            "embedding"
                        } else {
                            "chat"
                        });
                        v["eligible"] = json!(caps.embedding_only || caps.chat_missing().is_none());
                        v["probe_pending"] = json!(
                            !caps.embedding_only
                                && caps.probed_at.is_none()
                                && caps.chat_missing().is_some()
                        );
                        v["unsupported_reason"] = json!(if caps.probed_at.is_some() {
                            caps.chat_missing()
                        } else {
                            None
                        });
                        v
                    })
                    .collect();
                Ok(json!({ "profiles": rows }))
            }
            "profile.upsert" => {
                let mut params_in: ProfileUpsertParams = serde_json::from_value(p.clone())
                    .map_err(|e| LnaError::invalid(e.to_string()))?;
                let allow_insecure_http = p
                    .get("allow_insecure_http")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if is_non_loopback_http_endpoint(&params_in.base_url) && !allow_insecure_http {
                    return Err(LnaError::invalid(
                        "Non-loopback http:// model endpoints transmit credentials in plaintext. Set allow_insecure_http=true to confirm explicitly.",
                    ));
                }
                let api_key = p
                    .get("api_key")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_string);
                let existing = match params_in.id.as_deref() {
                    Some(id) => lna_storage::get_profile(&self.db, id).ok(),
                    None => None,
                };
                let embedding_only = p.get("purpose").and_then(Value::as_str) == Some("embedding")
                    || (p.get("purpose").is_none()
                        && existing
                            .as_ref()
                            .is_some_and(|e| e.capabilities.embedding_only));
                // Tool / JSON flags are never taken from the client: they come from the probe.
                let mut base = existing
                    .as_ref()
                    .map(|e| e.capabilities.clone())
                    .unwrap_or_default();
                // Media inputs (`supports_*`) are never taken from the client: the probe detects them.
                if let Some(c) = params_in.capabilities.as_ref() {
                    base.max_tokens_field = c.max_tokens_field.clone().or(base.max_tokens_field);
                    base.anthropic_thinking =
                        c.anthropic_thinking.clone().or(base.anthropic_thinking);
                }
                base.embedding_only = embedding_only;
                if embedding_only && params_in.activate {
                    return Err(LnaError::invalid("仅用于向量化的配置不能设为对话模型"));
                }
                // Saving stores the profile and the API key; the endpoint is not contacted here. When
                // the connection changed, the old measurements no longer describe it, so they are
                // cleared and the profile is probed again in the background (see below).
                let mut caps = base;
                if !embedding_only {
                    let connection_changed = match existing.as_ref() {
                        None => true,
                        Some(e) => {
                            // an omitted preset keeps the stored one; "" clears it
                            let preset_changed = params_in.preset.as_deref().is_some_and(|p| {
                                e.preset.as_deref() != Some(p).filter(|p| !p.is_empty())
                            });
                            e.protocol != params_in.protocol
                                || e.base_url != params_in.base_url
                                || e.model_id != params_in.model_id
                                || e.extra_headers != params_in.extra_headers
                                || preset_changed
                                || api_key.is_some()
                        }
                    };
                    if connection_changed {
                        // optimistic: streaming with usage is tried, and relaxed at run time
                        // when the endpoint rejects `stream_options`
                        caps.streaming = true;
                        caps.stream_usage = true;
                        caps.tools = false;
                        caps.json_schema = false;
                        caps.json_mode = false;
                        caps.supports_vision = false;
                        caps.supports_audio = false;
                        caps.supports_video = false;
                        caps.probed_at = None;
                    }
                }
                params_in.capabilities = Some(caps);
                // Storage would merge the flat flags over the capabilities: clear them so only the probe's result is stored.
                params_in.supports_vision = None;
                params_in.supports_audio = None;
                params_in.supports_video = None;
                // Shrinking media is a MiMo-only switch: any other resulting preset stores it off.
                // An omitted preset keeps the stored one.
                let resulting_is_mimo = match params_in.preset.as_deref() {
                    Some(preset) => preset == "mimo",
                    None => existing.as_ref().and_then(|e| e.preset.as_deref()) == Some("mimo"),
                };
                if !resulting_is_mimo {
                    params_in.shrink_media = Some(false);
                }
                let mut prof = lna_storage::upsert_profile(&self.db, &params_in)?;
                self.llm.set_parallel_limit(&prof.id, prof.max_parallel);
                if let (Some(k), Some(kref)) = (api_key, prof.keychain_ref.as_deref()) {
                    self.secrets.set(kref, SecretString::from(k))?;
                }
                self.refresh_has_secret(&mut prof);
                // A profile not checked yet (new, or its connection changed) is probed in the background.
                if !prof.capabilities.embedding_only && prof.capabilities.probed_at.is_none() {
                    self.spawn_profile_probe(prof.clone());
                }
                if params_in.activate {
                    self.enqueue_missing_summaries();
                }
                Ok(serde_json::to_value(prof).unwrap())
            }
            "profile.activate" => {
                let id = req_str(p, "profile_id")?;
                lna_storage::activate_profile(&self.db, id)?;
                self.enqueue_missing_summaries();
                Ok(json!({ "ok": true }))
            }
            "profile.delete" => {
                let id = req_str(p, "profile_id")?;
                lna_storage::delete_profile(&self.db, id)?;
                Ok(json!({ "ok": true }))
            }
            "profile.test" => {
                let id = req_str(p, "profile_id")?;
                let prof = lna_storage::get_profile(&self.db, id)?;
                let resolved = self.agent.resolve_profile(&prof)?;
                if prof.capabilities.embedding_only {
                    let (txt, _) = self.ping(&resolved).await?;
                    return Ok(
                        json!({ "ok": true, "reply": txt, "capabilities": prof.capabilities }),
                    );
                }
                let (caps, txt, media_failed) = self
                    .probe_capabilities(&resolved, prof.capabilities.clone())
                    .await?;
                lna_storage::update_profile_capabilities(&self.db, id, &caps)?;
                self.emit("profiles", "profile.updated", json!({ "profile_id": id }));
                if let Some(reason) = caps.chat_missing() {
                    return Err(LnaError::new(ErrorCode::CapabilityMissing, reason));
                }
                Ok(json!({
                    "ok": true,
                    "reply": txt,
                    "capabilities": caps,
                    // only probes that failed unexpectedly; a modality the model lacks is `false`
                    "media_failed": media_failed,
                    "detected": {
                        "vision": caps.supports_vision,
                        "audio": caps.supports_audio,
                        "video": caps.supports_video,
                    },
                }))
            }
            "profile.discover_models" => {
                let resolved = if let Some(id) = p
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    let prof = lna_storage::get_profile(&self.db, id)?;
                    let mut r = self.agent.resolve_profile(&prof)?;
                    if let Some(url) = p
                        .get("base_url")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        r.base_url = url.to_string();
                    }
                    if let Some(key) = p
                        .get("api_key")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        r.api_key = Some(SecretString::from(key.to_string()));
                    }
                    r
                } else {
                    let base_url = req_str(p, "base_url")?.to_string();
                    let protocol = match p
                        .get("protocol")
                        .and_then(|v| v.as_str())
                        .unwrap_or("chat_completion")
                    {
                        "responses" => lna_types::ProtocolKind::Responses,
                        "anthropic" => lna_types::ProtocolKind::Anthropic,
                        "gemini" => lna_types::ProtocolKind::Gemini,
                        _ => lna_types::ProtocolKind::ChatCompletion,
                    };
                    let api_key = p
                        .get("api_key")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| SecretString::from(s.to_string()));
                    lna_llm::ResolvedProfile {
                        id: String::new(),
                        display_name: String::new(),
                        protocol,
                        base_url,
                        model_id: String::new(),
                        api_key,
                        show_thinking: false,
                        effort: None,
                        context_window: 0,
                        max_output_tokens: 0,
                        capabilities: Default::default(),
                        extra_headers: Default::default(),
                        anthropic_version: "2023-06-01".to_string(),
                        preset: None,
                        shrink_media: false,
                    }
                };
                let models = self.llm.discover_models(&resolved).await?;
                Ok(json!({ "models": models }))
            }
            "secret.set" => {
                let kref = req_str(p, "keychain_ref")?;
                let val = req_str(p, "secret")?;
                self.secrets
                    .set(kref, SecretString::from(val.to_string()))?;
                Ok(json!({ "ok": true }))
            }
            "secret.clear" => {
                let kref = req_str(p, "keychain_ref")?;
                self.secrets.clear(kref)?;
                Ok(json!({ "ok": true }))
            }
            "secret.exists" => {
                let kref = req_str(p, "keychain_ref")?;
                Ok(json!({ "exists": self.secrets.exists(kref)? }))
            }

            // --- Embedding ---
            "embedding.status" => Ok(serde_json::to_value(self.embedding.status()?).unwrap()),
            "embedding.configure" => {
                let provider = req_str(p, "provider")?;
                let model_name = req_str(p, "model_name")?;
                let dim = p.get("dimension").and_then(|x| x.as_u64()).unwrap_or(512) as usize;
                let qp = p.get("query_prefix").and_then(|x| x.as_str()).unwrap_or("");
                let pp = p
                    .get("passage_prefix")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let ep = p.get("endpoint_profile_id").and_then(|x| x.as_str());
                let defer_activate = p
                    .get("defer_activate")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                let rec = self.embedding.configure_with_activation(
                    provider,
                    model_name,
                    dim,
                    qp,
                    pp,
                    ep,
                    !defer_activate,
                )?;
                Ok(serde_json::to_value(rec).unwrap())
            }

            // --- Studio (7 modalities; video excluded) ---
            "studio.list" => {
                let nid = req_str(p, "notebook_id")?;
                Ok(json!({ "artifacts": self.studio.list(nid)? }))
            }
            "studio.estimate" => {
                let nid = req_str(p, "notebook_id")?;
                let at: ArtifactType = serde_json::from_value(
                    p.get("artifact_type")
                        .cloned()
                        .ok_or_else(|| LnaError::invalid("artifact_type required"))?,
                )
                .map_err(|e| LnaError::invalid(e.to_string()))?;
                let cfg = p.get("config").cloned().unwrap_or_else(|| json!({}));
                self.studio.estimate_with_config(nid, at, &cfg)
            }
            "i18n.translate_notebook_content" => {
                let nid = req_str(p, "notebook_id")?;
                let target_lang = req_str(p, "target_lang")?;
                self.translate_notebook_ui_content(nid, target_lang)
            }
            "studio.create" => {
                let nid = req_str(p, "notebook_id")?;
                let at: ArtifactType = serde_json::from_value(
                    p.get("artifact_type")
                        .cloned()
                        .ok_or_else(|| LnaError::invalid("artifact_type required"))?,
                )
                .map_err(|e| LnaError::invalid(e.to_string()))?;
                let title = p.get("title").and_then(|x| x.as_str());
                let mut cfg = p.get("config").cloned().unwrap_or_else(|| json!({}));
                let atts = self.save_request_images(p)?;
                if !atts.is_empty() {
                    if let Some(obj) = cfg.as_object_mut() {
                        // only ids: the config is echoed into prompts (see attachments::resolve)
                        let pics = lna_ingest::attachments::images(&atts);
                        let files = lna_ingest::context_files::context_items(&atts);
                        if !pics.is_empty() {
                            obj.insert("images".into(), lna_ingest::attachments::ids_of(&pics));
                        }
                        if !files.is_empty() {
                            obj.insert("files".into(), lna_ingest::attachments::ids_of(&files));
                        }
                    }
                }
                let art = match self.studio.create(nid, at, title, cfg) {
                    Ok(a) => a,
                    Err(e) => {
                        lna_ingest::attachments::delete_files(&atts);
                        return Err(e);
                    }
                };
                Ok(serde_json::to_value(art).unwrap())
            }
            "studio.get" => {
                let id = req_str(p, "artifact_id")?;
                Ok(serde_json::to_value(self.studio.get(id)?).unwrap())
            }
            "studio.preview.get" => {
                let id = req_str(p, "artifact_id")?;
                self.studio.preview_get(id)
            }
            "studio.approve_draft" => {
                let id = req_str(p, "artifact_id")?;
                let draft = p.get("draft").cloned();
                Ok(serde_json::to_value(self.studio.approve_draft(id, draft)?).unwrap())
            }
            "studio.revise_draft" => {
                let id = req_str(p, "artifact_id")?.to_string();
                let instruction = req_str(p, "instruction")?.to_string();
                let current_draft = p.get("draft").and_then(Value::as_str).map(String::from);
                let atts = self.save_request_images(p)?;
                let studio = self.studio.clone();
                // the pictures only inform this one revision; they are not kept afterwards
                let updated = studio
                    .revise_draft(&id, &instruction, current_draft.as_deref(), &atts)
                    .await;
                lna_ingest::attachments::delete_files(&atts);
                let updated = updated?;
                Ok(serde_json::to_value(updated).unwrap())
            }
            "studio.refine_node" => {
                let id = req_str(p, "artifact_id")?.to_string();
                let node_id = req_str(p, "node_id")?.to_string();
                let studio = self.studio.clone();
                studio.refine_node(&id, &node_id).await
            }
            // --- Lecture (讲解) ---
            "lecture.ask" => {
                let id = req_str(p, "artifact_id")?.to_string();
                let slide_id = req_str(p, "slide_id")?.to_string();
                let question = req_str(p, "question")?.to_string();
                let sentence_id = p
                    .get("sentence_id")
                    .and_then(Value::as_str)
                    .map(String::from);
                let history: Vec<Value> = p
                    .get("history")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let atts = self.save_request_images(p)?;
                let studio = self.studio.clone();
                // the pictures stay on disk so the question panel can keep showing them; the
                // periodic attachment sweep removes them later
                let mut out = studio
                    .lecture_ask(
                        &id,
                        &slide_id,
                        sentence_id.as_deref(),
                        &question,
                        &history,
                        &atts,
                    )
                    .await?;
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "attachments".into(),
                        serde_json::to_value(&atts).unwrap_or_default(),
                    );
                }
                Ok(out)
            }
            "lecture.adapt" => {
                let id = req_str(p, "artifact_id")?.to_string();
                let slide_id = req_str(p, "slide_id")?.to_string();
                let kind = req_str(p, "kind")?.to_string();
                if !matches!(
                    kind.as_str(),
                    "extend" | "slower" | "simpler" | "skip_known" | "more_examples"
                ) {
                    return Err(LnaError::invalid(
                        "kind must be extend, slower, simpler, skip_known or more_examples",
                    ));
                }
                let reason = p
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let question = p
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let studio = self.studio.clone();
                studio
                    .lecture_adapt(&id, &slide_id, &kind, &reason, &question)
                    .await
            }
            "lecture.undo" => {
                let id = req_str(p, "artifact_id")?;
                let adaptation_id = p.get("adaptation_id").and_then(Value::as_str);
                let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
                self.studio.lecture_undo(id, adaptation_id, all)
            }
            // --- Text-to-speech ---
            "tts.get_settings" => self.studio.tts_settings(),
            "tts.set_settings" => self.studio.set_tts_settings(p),
            "tts.synthesize" => {
                let text = req_str(p, "text")?.to_string();
                let voice = p.get("voice").and_then(Value::as_str).map(String::from);
                let studio = self.studio.clone();
                studio.tts_synthesize(&text, voice.as_deref()).await
            }
            "tts.clear_cache" => Ok(json!({ "removed": self.studio.tts_clear_cache()? })),
            "studio.save_progress" => {
                let id = req_str(p, "artifact_id")?;
                let state = p.get("state").cloned().unwrap_or_else(|| json!({}));
                self.studio.save_progress(id, state)?;
                Ok(json!({ "ok": true }))
            }
            "studio.cancel" => {
                let id = req_str(p, "artifact_id")?;
                self.jobs.cancel_by_subject(id)?;
                self.studio.mark_cancelled(id)?;
                Ok(json!({ "ok": true }))
            }
            "studio.pause" => {
                let id = req_str(p, "artifact_id")?;
                self.jobs.cancel_by_subject(id)?;
                self.studio.mark_paused(id)?;
                Ok(json!({ "ok": true }))
            }
            "studio.delete" => {
                let id = req_str(p, "artifact_id")?;
                self.studio.delete(id)?;
                Ok(json!({ "ok": true }))
            }
            "studio.rename" => {
                let id = req_str(p, "artifact_id")?;
                let title = req_str(p, "title")?;
                self.studio.rename(id, title)?;
                Ok(json!({ "ok": true }))
            }
            "studio.retry" => {
                let id = req_str(p, "artifact_id")?;
                Ok(serde_json::to_value(self.studio.retry(id)?).unwrap())
            }
            "studio.regenerate" => {
                let id = req_str(p, "artifact_id")?;
                let config = p.get("config").cloned().filter(|v| !v.is_null());
                let use_selected = p
                    .get("use_selected_sources")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(true);
                let atts = self.save_request_images(p)?;
                Ok(
                    serde_json::to_value(self.studio.regenerate(id, config, use_selected, atts)?)
                        .unwrap(),
                )
            }
            "studio.import_to_studio" => {
                let id = req_str(p, "artifact_id")?;
                Ok(serde_json::to_value(self.studio.import_to_studio(id)?).unwrap())
            }
            "studio.set_feedback" => {
                let id = req_str(p, "artifact_id")?;
                let fb = p.get("feedback").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
                self.studio.set_feedback(id, fb)?;
                Ok(json!({ "ok": true }))
            }

            // --- Notes ---
            "note.list" => {
                let nid = req_str(p, "notebook_id")?;
                let notes = self.db.read(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT id, notebook_id, title, body_md, origin, origin_message_id,
                                    citations_json, converted_source_id, created_at, updated_at
                             FROM notes WHERE notebook_id = ?1 ORDER BY updated_at DESC",
                        )
                        .map_err(map_sql_err)?;
                    let rows = stmt
                        .query_map([nid], |r| {
                            Ok(Note {
                                id: r.get(0)?,
                                notebook_id: r.get(1)?,
                                title: r.get(2)?,
                                body_md: r.get(3)?,
                                origin: r.get(4)?,
                                origin_message_id: r.get(5)?,
                                citations_json: r.get(6)?,
                                converted_source_id: r.get(7)?,
                                created_at: r.get(8)?,
                                updated_at: r.get(9)?,
                            })
                        })
                        .map_err(map_sql_err)?;
                    let mut out = Vec::new();
                    for r in rows {
                        out.push(r.map_err(map_sql_err)?);
                    }
                    Ok(out)
                })?;
                Ok(json!({ "notes": notes }))
            }
            "note.save" => {
                let nid = req_str(p, "notebook_id")?;
                let id = p
                    .get("id")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(new_ulid);
                let title = p.get("title").and_then(|x| x.as_str()).unwrap_or("新笔记");
                let body_md = p.get("body_md").and_then(|x| x.as_str()).unwrap_or("");
                let now = now_ms();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT INTO notes (id, notebook_id, title, body_md, origin, created_at, updated_at)
                         VALUES (?1,?2,?3,?4,'manual',?5,?6)
                         ON CONFLICT(id) DO UPDATE SET title=excluded.title, body_md=excluded.body_md, updated_at=excluded.updated_at",
                        params![id, nid, title, body_md, now, now],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(json!({ "id": id }))
            }
            "note.delete" => {
                let id = req_str(p, "note_id")?;
                self.db.write(|conn| {
                    conn.execute("DELETE FROM notes WHERE id = ?1", [id])
                        .map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(json!({ "ok": true }))
            }
            "note.to_source" => {
                let id = req_str(p, "note_id")?;
                let (nid, title, body): (String, String, String) = self.db.read(|conn| {
                    conn.query_row(
                        "SELECT notebook_id, title, body_md FROM notes WHERE id = ?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(map_sql_err)
                })?;
                let (src, _) = self
                    .ingest
                    .add_text(&nid, &title, &body, SourceKind::Note)?;
                self.db.write(|conn| {
                    conn.execute(
                        "UPDATE notes SET converted_source_id=?1, updated_at=?2 WHERE id=?3",
                        params![src.id, now_ms(), id],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(serde_json::to_value(src).unwrap())
            }

            "notebook.export" => {
                let id = req_str(p, "notebook_id")?;
                let fmt = p.get("format").and_then(|x| x.as_str()).unwrap_or("lna");
                let out_path = std::path::Path::new(req_str(p, "out_path")?);
                if fmt == "markdown" || fmt == "md" {
                    archive::export_notebook_markdown(&self.db, id, out_path)?;
                } else {
                    archive::export_notebook_lna(&self.db, id, out_path)?;
                }
                Ok(json!({ "ok": true }))
            }
            "notebook.import" => {
                let in_path = std::path::Path::new(req_str(p, "in_path")?);
                let new_id = archive::import_notebook_lna(&self.db, &self.jobs, in_path)?;
                Ok(json!({ "notebook_id": new_id }))
            }
            "source.preview" => {
                let sid = req_str(p, "source_id")?;
                let page = p.get("page").and_then(|x| x.as_i64()).map(|n| n as i32);
                let src = lna_storage::get_source(&self.db, sid)?;
                lna_ingest::ensure_source_viewable(&src)?;
                let chunks = self.ingest.source_preview(sid, page)?;
                let index_nodes =
                    lna_storage::list_source_index_nodes(&self.db, sid).unwrap_or_default();
                let preview_html_path = self
                    .ingest
                    .ensure_preview_html(sid)
                    .ok()
                    .flatten()
                    .map(|p| p.to_string_lossy().to_string());
                Ok(json!({
                    "id": src.id,
                    "title": src.title,
                    "kind": src.kind.as_str(),
                    "origin_uri": src.origin_uri,
                    "blob_path": src.blob_path,
                    "preview_html_path": preview_html_path,
                    "page_count": src.page_count,
                    "guide_summary_md": src.guide_summary_md,
                    "guide_topics": src.guide_topics,
                    "ai_indexed": src.ai_indexed,
                    "index_nodes": index_nodes,
                    "chunks": chunks
                }))
            }
            "source.generate_guide" => {
                let sid = req_str(p, "source_id")?;
                let src = self.ingest.regenerate_source_guide(sid).await?;
                let index_nodes =
                    lna_storage::list_source_index_nodes(&self.db, sid).unwrap_or_default();
                Ok(json!({
                    "source": src,
                    "index_nodes": index_nodes
                }))
            }
            "chat.regenerate" => {
                let nid = req_str(p, "notebook_id")?;
                let msg = self.agent.regenerate_last(nid).await?;
                Ok(serde_json::to_value(msg).unwrap())
            }
            "chat.clear" => {
                let nid = req_str(p, "notebook_id")?;
                self.agent.clear_history(nid)?;
                Ok(json!({ "ok": true }))
            }
            "chat.set_feedback" => {
                let mid = req_str(p, "message_id")?;
                let fb = p.get("feedback").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
                self.agent.set_feedback(mid, fb)?;
                Ok(json!({ "ok": true }))
            }
            "chat.citation_detail" => {
                let mid = req_str(p, "message_id")?;
                let no = p.get("citation_no").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
                self.agent.citation_detail(mid, no)
            }
            "studio.grade_answer" => {
                let aid = req_str(p, "artifact_id")?;
                let qid = req_str(p, "question_id")?;
                let ans = p.get("answer").cloned().unwrap_or(Value::Null);
                self.studio.grade_answer(aid, qid, &ans)
            }
            "source.knowledge_graph" => {
                let sid = req_str(p, "source_id")?;
                let art = self.studio.get_source_knowledge_graph(sid)?;
                Ok(json!({ "artifact": art }))
            }
            "source.knowledge_progress" => {
                let nid = req_str(p, "notebook_id")?;
                let sid = p.get("source_id").and_then(|x| x.as_str());
                self.studio.compute_kg_progress_summary(nid, sid)
            }
            "studio.apply_quiz_to_kg" => {
                let nid = req_str(p, "notebook_id")?;
                let aid = req_str(p, "artifact_id")?;
                let results = p.get("results").cloned().unwrap_or_else(|| json!([]));
                self.studio
                    .apply_quiz_to_knowledge_graphs(nid, aid, results)
            }
            "studio.add_to_sources" => {
                let aid = req_str(p, "artifact_id")?;
                let (art, md) = self.studio.render_markdown(aid)?;
                let title = p
                    .get("title")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| format!("[Studio] {}", art.title));
                let (src, dup) = self
                    .ingest
                    .add_text(&art.notebook_id, &title, &md, SourceKind::Md)?;
                Ok(json!({ "source": src, "duplicate": dup }))
            }
            "studio.export" => {
                let aid = req_str(p, "artifact_id")?;
                let fmt = p
                    .get("format")
                    .and_then(|x| x.as_str())
                    .unwrap_or("markdown");
                let out_path = std::path::Path::new(req_str(p, "out_path")?);
                self.studio.export_artifact(aid, fmt, out_path)?;
                Ok(json!({ "ok": true }))
            }
            "note.get" => {
                let id = req_str(p, "note_id")?;
                let note = self.db.read(|conn| {
                    conn.query_row(
                        "SELECT id, notebook_id, title, body_md, origin, origin_message_id, citations_json, converted_source_id, created_at, updated_at FROM notes WHERE id=?1",
                        [id],
                        |r| Ok(Note {
                            id: r.get(0)?, notebook_id: r.get(1)?, title: r.get(2)?, body_md: r.get(3)?,
                            origin: r.get(4)?, origin_message_id: r.get(5)?, citations_json: r.get(6)?,
                            converted_source_id: r.get(7)?, created_at: r.get(8)?, updated_at: r.get(9)?,
                        })
                    ).map_err(map_sql_err)
                })?;
                Ok(serde_json::to_value(note).unwrap())
            }
            "note.from_message" => {
                let mid = req_str(p, "message_id")?;
                let msg = self.agent.get_message(mid)?;
                let id = new_ulid();
                let now = now_ms();
                let cites = serde_json::to_string(&msg.citations).ok();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT INTO notes (id, notebook_id, title, body_md, origin, origin_message_id, citations_json, created_at, updated_at)
                         VALUES (?1,?2,'对话摘录',?3,'chat',?4,?5,?6,?7)",
                        params![id, msg.notebook_id, msg.content, mid, cites, now, now]
                    ).map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(json!({ "id": id }))
            }
            "note.from_summary" => {
                let nid = req_str(p, "notebook_id")?;
                let nb = lna_storage::get_notebook(&self.db, nid)?;
                let body = nb.summary_md.unwrap_or_default();
                let id = new_ulid();
                let now = now_ms();
                self.db.write(|conn| {
                    conn.execute(
                        "INSERT INTO notes (id, notebook_id, title, body_md, origin, created_at, updated_at)
                         VALUES (?1,?2,?3,?4,'summary',?5,?6)",
                        params![id, nid, format!("{} - 摘要", nb.title), body, now, now]
                    ).map_err(map_sql_err)?;
                    Ok(())
                })?;
                Ok(json!({ "id": id }))
            }
            "diagnostics.export" => {
                let path_str = p
                    .get("out_path")
                    .or_else(|| p.get("output_path"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| LnaError::invalid("Missing out_path"))?;
                let out_path = std::path::Path::new(path_str);
                let want_zip = out_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("zip"))
                    .unwrap_or(false)
                    || p.get("format").and_then(|v| v.as_str()) == Some("zip");
                if want_zip {
                    archive::export_diagnostics_zip(&self.db, &self.llm, out_path)
                } else {
                    archive::export_diagnostics(&self.db, out_path)?;
                    Ok(json!({ "ok": true }))
                }
            }
            "embedding.rebuild" => {
                let force = p.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
                let model_id = p.get("model_id").and_then(|x| x.as_str());
                let count =
                    queue_embeddings(&self.db, &self.embedding, &self.jobs, force, model_id)?;
                Ok(json!({ "ok": true, "queued": count > 0, "sources_queued": count }))
            }

            "embedding.default_model" => {
                let spec = lna_embedding::download::ModelSpec::default_multilingual();
                Ok(json!({
                    "id": spec.id,
                    "display_name": spec.display_name,
                    "dimension": spec.dimension,
                    "total_bytes": spec.files.iter().map(|f| f.size).sum::<u64>(),
                    "installed": self.embedding.model_files_present(&spec),
                }))
            }
            "embedding.download_status" => {
                let g = self
                    .model_download
                    .lock()
                    .map_err(|_| LnaError::internal("lock"))?;
                Ok(match g.as_ref() {
                    Some((_, progress)) => json!({ "running": true, "progress": progress }),
                    None => json!({ "running": false }),
                })
            }
            "embedding.download_cancel" => {
                if let Some((tok, _)) = self
                    .model_download
                    .lock()
                    .map_err(|_| LnaError::internal("lock"))?
                    .as_ref()
                {
                    tok.cancel();
                }
                Ok(json!({ "ok": true }))
            }
            "embedding.download" => {
                // `spec` lets advanced users (and tests) point at another model; default is the built-in one.
                let spec: lna_embedding::download::ModelSpec = match p.get("spec") {
                    Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                        .map_err(|e| LnaError::invalid(format!("invalid model spec: {e}")))?,
                    _ => lna_embedding::download::ModelSpec::default_multilingual(),
                };
                spec.validate()?;
                let activate = p.get("activate").and_then(|x| x.as_bool()).unwrap_or(true);
                let cancel = CancellationToken::new();
                {
                    let mut g = self
                        .model_download
                        .lock()
                        .map_err(|_| LnaError::internal("lock"))?;
                    if g.is_some() {
                        return Err(LnaError::new(
                            ErrorCode::Conflict,
                            "A model download is already running",
                        ));
                    }
                    *g = Some((cancel.clone(), json!({})));
                }
                let embedding = self.embedding.clone();
                let db = self.db.clone();
                let jobs = self.jobs.clone();
                let events = self.events.clone();
                let seq = self.seq.clone();
                let state = self.model_download.clone();
                let emit = move |event: &str, data: Value| {
                    let s = seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let _ = events.send(ApiEvent {
                        topic: "embedding".into(),
                        event: event.into(),
                        seq: s,
                        data,
                    });
                };
                self.rt.spawn(async move {
                    let state_for_progress = state.clone();
                    let emit_progress = emit.clone();
                    let res = embedding
                        .download_model(&spec, cancel, move |pr| {
                            let v = serde_json::to_value(&pr).unwrap_or_default();
                            if let Ok(mut g) = state_for_progress.lock() {
                                if let Some((_, slot)) = g.as_mut() {
                                    *slot = v.clone();
                                }
                            }
                            emit_progress("embedding.download_progress", v);
                        })
                        .await;
                    let outcome = match res {
                        Ok(_) if activate => embedding
                            .configure(
                                "local_onnx",
                                &spec.id,
                                spec.dimension,
                                &spec.query_prefix,
                                &spec.passage_prefix,
                                None,
                            )
                            .and_then(|_| {
                                queue_embeddings(&db, &embedding, &jobs, false, None).map(|_| ())
                            }),
                        Ok(_) => Ok(()),
                        Err(e) => Err(e),
                    };
                    if let Ok(mut g) = state.lock() {
                        *g = None;
                    }
                    match outcome {
                        Ok(()) => emit("embedding.download_done", json!({ "ok": true })),
                        Err(e) => emit(
                            "embedding.download_done",
                            json!({ "ok": false, "error": e }),
                        ),
                    }
                });
                Ok(json!({ "ok": true, "started": true }))
            }

            // --- ffmpeg: the app-managed copy (lna_ingest::ffmpeg_install) ---
            "ffmpeg.status" => Ok(json!(lna_ingest::ffmpeg_install::status())),
            "ffmpeg.install" => {
                // Runs to completion (the call returns when it is done). Progress goes out on the
                // `ffmpeg` topic; the caller should not run this on its UI thread.
                let res = lna_ingest::ffmpeg_install::install(|downloaded, total| {
                    self.emit(
                        "ffmpeg",
                        "ffmpeg.download_progress",
                        json!({ "downloaded": downloaded, "total": total }),
                    );
                })
                .await;
                let done = match &res {
                    Ok(_) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": e }),
                };
                self.emit("ffmpeg", "ffmpeg.download_done", done);
                res?;
                Ok(json!(lna_ingest::ffmpeg_install::status()))
            }
            "ffmpeg.remove" => {
                lna_ingest::ffmpeg_install::remove()?;
                Ok(json!(lna_ingest::ffmpeg_install::status()))
            }

            // --- Search / Research (deferred per user request) ---
            m if m.starts_with("research.") => Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                "Web/Local search feature is deferred in this release",
            )),

            _ => Err(LnaError::invalid(format!("Unknown method: {}", req.method))),
        }
    }

    /// Translates the notebook's overview (`summary_md` and `suggested_questions`) into `target_lang`
    /// (`zh-CN` or `en-US`), leaving source book guides, Studio artifacts, and chat history untouched.
    fn translate_notebook_ui_content(
        &self,
        notebook_id: &str,
        target_lang: &str,
    ) -> LnaResult<Value> {
        let to_en = target_lang.to_lowercase().starts_with("en");
        let lang_name = if to_en {
            "English"
        } else {
            "Simplified Chinese (简体中文)"
        };

        let Some(prof) = lna_storage::get_active_profile(&self.db)? else {
            let _ = self.events.send(ApiEvent {
                event: "notebook.summary_updated".into(),
                topic: format!("notebook:{notebook_id}"),
                seq: 0,
                data: json!({ "notebook_id": notebook_id }),
            });
            return Ok(json!({ "ok": true, "translated_with_llm": false }));
        };
        let mut resolved = self.agent.resolve_profile(&prof)?;
        resolved.show_thinking = false;
        resolved.effort = None;

        let (summary_md, sq_json): (Option<String>, Option<String>) = self.db.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT summary_md, suggested_questions_json FROM notebooks WHERE id = ?1",
                    [notebook_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok()
                .unwrap_or((None, None)))
        })?;
        let suggested_questions: Vec<String> = sq_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        if summary_md.as_deref().unwrap_or("").trim().is_empty() && suggested_questions.is_empty() {
            let _ = self.events.send(ApiEvent {
                event: "notebook.summary_updated".into(),
                topic: format!("notebook:{notebook_id}"),
                seq: 0,
                data: json!({ "notebook_id": notebook_id }),
            });
            return Ok(json!({ "ok": true, "translated_with_llm": false }));
        }

        let input_payload = json!({
            "summary_md": summary_md.unwrap_or_default(),
            "suggested_questions": suggested_questions,
        });

        let req = LlmRequest::simple(
            format!(
                "You are a UI content translator. Translate all text values in the provided JSON into {lang_name}. Keep JSON keys strictly unchanged, preserve Markdown **bold** emphasis, and output valid JSON only with no code fences."
            ),
            input_payload.to_string(),
            resolved.bounded_output(8192),
        )
        .with_output(OutputMode::JsonObject);

        let tr_tid = format!("translate:{notebook_id}");
        self.emit(
            &format!("notebook:{notebook_id}"),
            "ai.activity",
            json!({
                "task_id": tr_tid,
                "notebook_id": notebook_id,
                "scope": "ui_translate",
                "kind": "tool",
                "title": format!("AI 翻译界面摘要（{lang_name}）"),
                "body": "正在翻译笔记本概览与推荐问题…",
                "active": true,
            }),
        );
        let llm = self.llm.clone();
        // called from the async dispatcher: leave the worker thread before blocking on the call
        let handle = self.rt.handle().clone();
        let translated_res = tokio::task::block_in_place(move || {
            handle.block_on(async move {
                let schema = json!({
                    "type": "object",
                    "properties": {
                        "summary_md": { "type": "string" },
                        "suggested_questions": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["summary_md", "suggested_questions"],
                    "additionalProperties": false
                });
                llm.complete_structured(
                    &resolved,
                    &req,
                    "ui_translation",
                    &schema,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            })
        });
        self.emit(
            &format!("notebook:{notebook_id}"),
            "ai.activity",
            json!({
                "task_id": tr_tid,
                "notebook_id": notebook_id,
                "scope": "ui_translate",
                "kind": "done",
                "title": "界面翻译完成",
                "body": "",
                "active": false,
            }),
        );

        if let Ok((parsed, _)) = translated_res {
            {
                let new_sum = parsed
                    .get("summary_md")
                    .and_then(Value::as_str)
                    .map(String::from);
                let new_sq: Option<String> = parsed
                    .get("suggested_questions")
                    .and_then(Value::as_array)
                    .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
                let ts = now_ms();
                let _ = self.db.write(|conn| {
                    if let Some(ref sm) = new_sum {
                        if !sm.trim().is_empty() {
                            let _ = conn.execute(
                                "UPDATE notebooks SET summary_md = ?1, updated_at = ?2 WHERE id = ?3",
                                params![sm, ts, notebook_id],
                            );
                        }
                    }
                    if let Some(ref sq) = new_sq {
                        let _ = conn.execute(
                            "UPDATE notebooks SET suggested_questions_json = ?1, updated_at = ?2 WHERE id = ?3",
                            params![sq, ts, notebook_id],
                        );
                    }
                    Ok(())
                });
            }
        }

        let _ = self.events.send(ApiEvent {
            event: "notebook.summary_updated".into(),
            topic: format!("notebook:{notebook_id}"),
            seq: 0,
            data: json!({ "notebook_id": notebook_id }),
        });
        Ok(json!({ "ok": true, "translated_with_llm": true }))
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        // Stop in-flight answers, Studio jobs and downloads so the runtime can wind down quickly (B-16).
        self.agent.cancel_all();
        self.jobs.cancel_all_running();
        if let Ok(g) = self.model_download.lock() {
            if let Some((tok, _)) = g.as_ref() {
                tok.cancel();
            }
        }
        let _ = self.db.checkpoint_truncate();
    }
}

/// Queues embedding jobs for every ready source (all vectors are recomputed with `force`).
fn queue_embeddings(
    db: &Database,
    embedding: &EmbeddingService,
    jobs: &JobQueue,
    force: bool,
    target_model_id: Option<&str>,
) -> LnaResult<usize> {
    let model = match target_model_id {
        Some(mid) => embedding
            .get_model(mid)?
            .ok_or_else(|| LnaError::not_found("Target embedding model not found"))?,
        None => embedding.active_model()?.ok_or_else(|| {
            LnaError::new(
                ErrorCode::CapabilityMissing,
                "No active embedding model configured",
            )
        })?,
    };
    if force {
        db.write(|conn| {
            // The vec0 table has a primary key per chunk, so old vectors must go before new ones are written.
            conn.execute(&format!("DELETE FROM {}", model.vec_table), [])
                .map_err(map_sql_err)?;
            conn.execute(
                "DELETE FROM chunk_embedding_state WHERE embedding_model_id = ?1",
                [&model.id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })?;
    }
    let ready_sources: Vec<(String, String)> = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, notebook_id FROM sources WHERE status = 'READY'")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;
    let count = ready_sources.len();
    for (sid, nid) in ready_sources {
        let subject = if target_model_id.is_some() {
            format!("{sid}@{}", model.id)
        } else {
            sid
        };
        jobs.enqueue("embed_source", Some(&nid), &subject, 50)?;
    }
    Ok(count)
}

fn is_non_loopback_http_endpoint(base_url: &str) -> bool {
    let Some((scheme, rest)) = base_url.trim().split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    }
    .trim()
    .trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return !ip.is_loopback();
    }
    !host.is_empty()
}

fn req_str<'a>(p: &'a Value, key: &str) -> LnaResult<&'a str> {
    p.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| LnaError::invalid(format!("Missing string parameter: {key}")))
}

/// Built-in AI provider presets (official providers and preconfigured relays).
fn builtin_provider_presets() -> Value {
    json!([
        {
            "id": "openai",
            "name": "OpenAI 官方",
            "name_en": "OpenAI",
            "category": "official",
            "protocol": "responses",
            "base_url": "https://api.openai.com/v1",
            "default_model": "gpt-4.1",
            "suggested_models": ["gpt-4.1", "gpt-4o", "o4-mini"],
            "context_window": 1047576,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "anthropic",
            "name": "Anthropic 官方",
            "name_en": "Anthropic",
            "category": "official",
            "protocol": "anthropic",
            "base_url": "https://api.anthropic.com/v1",
            "default_model": "claude-sonnet-4-20250514",
            "suggested_models": ["claude-sonnet-4-20250514", "claude-opus-4-20250514", "claude-3-7-sonnet-20250219"],
            "context_window": 200000,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "google",
            "name": "Google Gemini 官方",
            "name_en": "Google Gemini",
            "category": "official",
            "protocol": "gemini",
            "base_url": "https://generativelanguage.googleapis.com/v1beta",
            "default_model": "gemini-2.5-flash",
            "suggested_models": ["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash"],
            "context_window": 1000000,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "deepseek",
            "name": "DeepSeek 官方",
            "name_en": "DeepSeek",
            "category": "official",
            "protocol": "chat_completion",
            "base_url": "https://api.deepseek.com/v1",
            "default_model": "deepseek-chat",
            "suggested_models": ["deepseek-chat", "deepseek-reasoner"],
            "context_window": 64000,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "glm",
            "name": "GLM 智谱官方",
            "name_en": "GLM (Zhipu)",
            "category": "official",
            "protocol": "chat_completion",
            "base_url": "https://open.bigmodel.cn/api/paas/v4",
            "default_model": "glm-4.5",
            "suggested_models": ["glm-4.5", "glm-4.5-air", "glm-4-plus"],
            "context_window": 128000,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "mimo",
            "name": "MiMo 小米官方",
            "name_en": "MiMo (Xiaomi)",
            "category": "official",
            "protocol": "chat_completion",
            "base_url": "https://api.xiaomimimo.com/v1",
            "default_model": "mimo-v2.6-flash",
            "suggested_models": ["mimo-v2.6-flash", "mimo-v2.6-pro", "mimo-v2.6-pro-ultraspeed"],
            "context_window": 128000,
            "max_output_tokens": 0,
            "extra_headers": {}
        },
        {
            "id": "opencode_go",
            "name": "Opencode Go (中转)",
            "name_en": "Opencode Go (Relay)",
            "category": "relay",
            "protocol": "chat_completion",
            "base_url": "https://opencode.ai/zen/go/v1",
            "default_model": "glm-5",
            "suggested_models": ["glm-5", "kimi-k2.5", "minimax-m2.5"],
            "context_window": 128000,
            "max_output_tokens": 0,
            "extra_headers": {
                "x-opencode-session": "local-notebook-agent",
                "User-Agent": "opencode/1.0"
            }
        },
        {
            // Provider keys are stored in the Cloudflare dashboard (BYOK / Unified Billing), so
            // the only secret filled in here is the gateway's token. It travels in
            // cf-aig-authorization; no Authorization header is sent (the gateway would forward it
            // to the provider). Replace {account_id} / {gateway_id} in the address ("default" is
            // a valid gateway id); models are named {provider}/{model}.
            "id": "cloudflare_ai_gateway",
            "name": "Cloudflare AI Gateway (云端托管密钥)",
            "name_en": "Cloudflare AI Gateway (keys stored in Cloudflare)",
            "category": "relay",
            "protocol": "chat_completion",
            "base_url": "https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}/compat",
            "default_model": "openai/gpt-4.1",
            "suggested_models": [
                "openai/gpt-4.1",
                "anthropic/claude-sonnet-4-5",
                "google-ai-studio/gemini-2.5-flash",
                "deepseek/deepseek-chat"
            ],
            "context_window": 128000,
            "max_output_tokens": 0,
            "extra_headers": {
                "cf-aig-authorization": "Bearer {api_key}"
            }
        }
    ])
}

/// A failure of the connection itself (credentials, address, rate limit, network), as opposed to
/// the model declining a capability probe.
fn is_connection_failure(e: &LnaError) -> bool {
    matches!(
        e.code,
        ErrorCode::ModelNotConfigured
            | ErrorCode::ModelAuthFailed
            | ErrorCode::ModelNotFound
            | ErrorCode::ModelRateLimited
            | ErrorCode::ModelOverloaded
            | ErrorCode::ModelStreamInterrupted
            | ErrorCode::KeychainUnavailable
            | ErrorCode::FetchFailed
            | ErrorCode::Internal
            | ErrorCode::Cancelled
    )
}

/// Which declared input `probe_media` checks.
#[derive(Clone, Copy)]
enum MediaProbe {
    Image,
    Audio,
    Video,
}
