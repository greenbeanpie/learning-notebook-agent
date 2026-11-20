pub mod archive;

use lna_agent::AgentService;
use lna_embedding::EmbeddingService;
use lna_ingest::IngestService;
use lna_jobs::JobQueue;
use lna_llm::{
    LlmClient, LlmEvent, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions, Role, ToolDef,
    ToolMode,
};
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
}

impl CoreRuntime {
    pub fn open(data_dir: PathBuf, allow_memory_secret_fallback: bool) -> LnaResult<Arc<Self>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2) // Section 3: max 2 worker threads
            .enable_all()
            .build()
            .map_err(|e| LnaError::internal(format!("Failed to build tokio runtime: {e}")))?;

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
            llm.set_capability_sink(move |profile_id, caps| {
                let _ = lna_storage::update_profile_capabilities(&db, profile_id, caps);
            });
        }
        let embedding = EmbeddingService::new(db.clone(), secrets.clone());
        let retrieval = Arc::new(RetrievalEngine::new(db.clone(), embedding.clone()));
        let ingest = IngestService::new(db.clone(), jobs.clone(), embedding.clone());

        let (events, _) = broadcast::channel(512);
        let seq = Arc::new(AtomicU64::new(0));
        let agent = AgentService::new(
            db.clone(),
            secrets.clone(),
            llm.clone(),
            retrieval.clone(),
            events.clone(),
            seq.clone(),
        );
        let studio = StudioService::new(
            db.clone(),
            secrets.clone(),
            jobs.clone(),
            llm.clone(),
            retrieval.clone(),
            events.clone(),
            seq.clone(),
        );

        let core = Arc::new(Self {
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

    fn spawn_background_loops(self: &Arc<Self>) {
        // S3: Delayed 5s orphan file cleanup
        let weak_clean = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(core) = weak_clean.upgrade() {
                let _ = lna_ingest::IngestService::cleanup_orphan_files(&core.db);
            }
        });

        // Worker 1: Fast ingest & summary loop (never blocked by Studio, B-14)
        let weak1 = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut tick = 0u64;
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
                if let Ok(Some(job)) =
                    core.jobs
                        .claim_next(&["ingest_source", "refresh_summary", "embed_source"])
                {
                    let _tok = core.jobs.register_token(&job.id);
                    core.emit_source_status(&job);
                    let res = match job.kind.as_str() {
                        "ingest_source" => core.ingest.process_ingest_job(&job.subject_id).await,
                        "refresh_summary" => {
                            core.agent.refresh_notebook_summary(&job.subject_id).await
                        }
                        "embed_source" => {
                            match lna_storage::get_source(&core.db, &job.subject_id) {
                                Ok(src) => {
                                    core.ingest
                                        .embed_source_chunks(&src.id, &src.notebook_id, &src.title)
                                        .await
                                }
                                Err(e) => Err(e),
                            }
                        }
                        _ => Ok(()),
                    };
                    match res {
                        Ok(()) => {
                            let _ = core.jobs.mark_done(&job.id);
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
                } else {
                    tokio::select! {
                        _ = core.shutdown.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                    }
                }
            }
        });

        // Worker 2: Dedicated Studio generation loop (updates artifact FAILED on terminal error, B-07)
        let weak2 = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            loop {
                let Some(core) = weak2.upgrade() else { break };
                if core.shutdown.is_cancelled() {
                    break;
                }
                if let Ok(Some(job)) = core.jobs.claim_next(&["studio_generate"]) {
                    let tok = core.jobs.register_token(&job.id);
                    match core.studio.process_job(&job.subject_id, tok).await {
                        Ok(()) => {
                            let _ = core.jobs.mark_done(&job.id);
                        }
                        Err(e) => {
                            let terminal = core
                                .jobs
                                .mark_failed_or_retry(&job.id, &e.message, 5000, e.retryable)
                                .unwrap_or(true);
                            if e.code == ErrorCode::Cancelled {
                                let _ = core.studio.mark_cancelled(&job.subject_id);
                            } else if terminal {
                                let _ = core.studio.mark_failed(&job.subject_id, &e);
                            }
                        }
                    }
                } else {
                    tokio::select! {
                        _ = core.shutdown.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {}
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

    pub async fn dispatch(&self, req: ApiRequest) -> LnaResult<Value> {
        let p = &req.params;
        match req.method.as_str() {
            // --- System ---
            "app.info" => Ok(json!({
                "name": "Local Notebook Agent",
                "version": env!("CARGO_PKG_VERSION"),
                "data_dir": self.db.paths().root,
                "studio_modalities": ["audio","slides","mindmap","report","flashcards","quiz","infographic"]
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

            // --- Notebook ---
            "notebook.list" => Ok(json!({ "notebooks": lna_storage::list_notebooks(&self.db)? })),
            "notebook.create" => {
                let title = p
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or("未命名笔记本");
                let emoji = p.get("emoji").and_then(|x| x.as_str());
                Ok(
                    serde_json::to_value(lna_storage::create_notebook(&self.db, title, emoji)?)
                        .unwrap(),
                )
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
                lna_storage::delete_notebook(&self.db, id)?;
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
                let text = req_str(p, "text")?;
                let msg = self.agent.send_message(nid, text).await?;
                Ok(serde_json::to_value(msg).unwrap())
            }
            "chat.cancel" => {
                let nid = req_str(p, "notebook_id")?;
                self.agent.cancel(nid)?;
                Ok(json!({ "ok": true }))
            }

            // --- Profile & Secrets ---
            "profile.list" => {
                let mut profiles = lna_storage::list_profiles(&self.db)?;
                for prof in profiles.iter_mut() {
                    self.refresh_has_secret(prof);
                }
                Ok(json!({ "profiles": profiles }))
            }
            "profile.upsert" => {
                let params_in: ProfileUpsertParams = serde_json::from_value(p.clone())
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
                let mut prof = lna_storage::upsert_profile(&self.db, &params_in)?;
                self.refresh_has_secret(&mut prof);
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
                let req = LlmRequest {
                    system: "Reply with OK.".into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text("ping".into())],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::Text,
                    max_output_tokens: 16,
                    reasoning: ReasoningOptions::default(),
                };
                let (txt, ping_usage) = self
                    .llm
                    .complete_text_strict(&resolved, &req, CancellationToken::new())
                    .await?;

                let mut caps = prof.capabilities.clone();
                caps.streaming = true;
                caps.stream_usage = ping_usage.input_tokens > 0 || ping_usage.output_tokens > 0;

                let tool_req = LlmRequest {
                    system: "You must call the lna_capability_probe tool exactly once with {\"ok\":true}. Do not answer in natural language.".into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text("Call the capability probe now.".into())],
                    }],
                    tools: vec![ToolDef {
                        name: "lna_capability_probe".into(),
                        description: "Capability detection probe".into(),
                        parameters: json!({
                            "type": "object",
                            "properties": { "ok": { "type": "boolean" } },
                            "required": ["ok"],
                            "additionalProperties": false
                        }),
                    }],
                    tool_mode: ToolMode::Auto,
                    output: OutputMode::Text,
                    max_output_tokens: 64,
                    reasoning: ReasoningOptions::default(),
                };
                let mut tool_called = false;
                if self
                    .llm
                    .stream_strict(&resolved, &tool_req, CancellationToken::new(), |event| {
                        if matches!(event, LlmEvent::ToolCallStart { .. }) {
                            tool_called = true;
                        }
                    })
                    .await
                    .is_ok()
                {
                    caps.tools = tool_called;
                } else {
                    caps.tools = false;
                }

                let probe_schema = json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                    "additionalProperties": false
                });
                let schema_req = LlmRequest {
                    system: "Return only a JSON object with ok set to true.".into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text("Return the probe object.".into())],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::JsonSchema {
                        name: "lna_capability_probe".into(),
                        schema: probe_schema.clone(),
                    },
                    max_output_tokens: 64,
                    reasoning: ReasoningOptions::default(),
                };
                caps.json_schema = self
                    .llm
                    .complete_text_strict(&resolved, &schema_req, CancellationToken::new())
                    .await
                    .ok()
                    .and_then(|(raw, _)| serde_json::from_str::<Value>(&raw).ok())
                    .map(|value| value.get("ok").and_then(Value::as_bool) == Some(true))
                    .unwrap_or(false);

                let json_mode_req = LlmRequest {
                    system: "Return only a JSON object with ok set to true.".into(),
                    messages: vec![LlmMessage {
                        role: Role::User,
                        parts: vec![Part::Text("Return the probe object.".into())],
                    }],
                    tools: vec![],
                    tool_mode: ToolMode::None,
                    output: OutputMode::JsonObject,
                    max_output_tokens: 64,
                    reasoning: ReasoningOptions::default(),
                };
                caps.json_mode = self
                    .llm
                    .complete_text_strict(&resolved, &json_mode_req, CancellationToken::new())
                    .await
                    .ok()
                    .and_then(|(raw, _)| serde_json::from_str::<Value>(&raw).ok())
                    .map(|value| value.get("ok").and_then(Value::as_bool) == Some(true))
                    .unwrap_or(false);

                lna_storage::update_profile_capabilities(&self.db, id, &caps)?;
                Ok(json!({ "ok": true, "reply": txt, "capabilities": caps }))
            }
            "profile.discover_models" => {
                let id = req_str(p, "profile_id")?;
                let prof = lna_storage::get_profile(&self.db, id)?;
                let resolved = self.agent.resolve_profile(&prof)?;
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
                let rec = self
                    .embedding
                    .configure(provider, model_name, dim, qp, pp, ep)?;
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
                self.studio.estimate(nid, at)
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
                let cfg = p.get("config").cloned().unwrap_or_else(|| json!({}));
                let art = self.studio.create(nid, at, title, cfg)?;
                Ok(serde_json::to_value(art).unwrap())
            }
            "studio.get" => {
                let id = req_str(p, "artifact_id")?;
                Ok(serde_json::to_value(self.studio.get(id)?).unwrap())
            }
            "studio.approve_draft" => {
                let id = req_str(p, "artifact_id")?;
                let draft = p.get("draft").cloned();
                Ok(serde_json::to_value(self.studio.approve_draft(id, draft)?).unwrap())
            }
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
                let chunks = self.ingest.source_preview(sid, page)?;
                Ok(json!({ "chunks": chunks }))
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
                let out_path = std::path::Path::new(req_str(p, "out_path")?);
                archive::export_diagnostics(&self.db, out_path)?;
                Ok(json!({ "ok": true }))
            }
            "embedding.rebuild" => {
                let force = p.get("force").and_then(|x| x.as_bool()).unwrap_or(false);
                let count = queue_embeddings(&self.db, &self.embedding, &self.jobs, force)?;
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
                                queue_embeddings(&db, &embedding, &jobs, false).map(|_| ())
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

            // --- Search / Research (deferred per user request) ---
            m if m.starts_with("research.") => Err(LnaError::new(
                ErrorCode::CapabilityMissing,
                "Web/Local search feature is deferred in this release",
            )),

            _ => Err(LnaError::invalid(format!("Unknown method: {}", req.method))),
        }
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        let _ = self.db.checkpoint_truncate();
    }
}

/// Queues embedding jobs for every ready source (all vectors are recomputed with `force`).
fn queue_embeddings(
    db: &Database,
    embedding: &EmbeddingService,
    jobs: &JobQueue,
    force: bool,
) -> LnaResult<usize> {
    let model = embedding.active_model()?.ok_or_else(|| {
        LnaError::new(
            ErrorCode::CapabilityMissing,
            "No active embedding model configured",
        )
    })?;
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
        jobs.enqueue("embed_source", Some(&nid), &sid, 50)?;
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
