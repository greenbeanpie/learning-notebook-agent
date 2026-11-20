pub mod citations;
pub mod dialog;

use lna_llm::*;
use lna_retrieval::{RetrievalEngine, RetrievedEvidence};
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_types::*;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const CHAT_SYSTEM_PROMPT: &str = include_str!("../../../prompts/chat_system_v1.md");
const QUERY_REWRITE_PROMPT: &str = include_str!("../../../prompts/query_rewrite_v1.md");
const FOLLOWUPS_PROMPT: &str = include_str!("../../../prompts/followups_v1.md");
const HISTORY_SUMMARY_PROMPT: &str = include_str!("../../../prompts/history_summary_v1.md");
const SUMMARY_PROMPT: &str = include_str!("../../../prompts/notebook_summary_v1.md");

#[derive(Clone)]
pub struct AgentService {
    db: Database,
    secrets: Arc<dyn SecretStore>,
    llm: LlmClient,
    retrieval: Arc<RetrievalEngine>,
    events: broadcast::Sender<ApiEvent>,
    seq: Arc<std::sync::atomic::AtomicU64>,
    active_runs: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl AgentService {
    pub fn new(
        db: Database,
        secrets: Arc<dyn SecretStore>,
        llm: LlmClient,
        retrieval: Arc<RetrievalEngine>,
        events: broadcast::Sender<ApiEvent>,
        seq: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            db,
            secrets,
            llm,
            retrieval,
            events,
            seq,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn emit(&self, topic: &str, event: &str, data: serde_json::Value) {
        let s = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.events.send(ApiEvent {
            seq: s,
            topic: topic.to_string(),
            event: event.to_string(),
            data,
        });
    }

    pub fn clear_history(&self, notebook_id: &str) -> LnaResult<()> {
        if let Ok(g) = self.active_runs.lock() {
            if g.contains_key(notebook_id) {
                return Err(LnaError::new(
                    ErrorCode::Conflict,
                    "Cannot clear chat while streaming",
                ));
            }
        }
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute(
                "DELETE FROM chat_messages WHERE notebook_id = ?1",
                [notebook_id],
            )
            .map_err(map_sql_err)?;
            tx.execute(
                "DELETE FROM conversation_summaries WHERE notebook_id = ?1",
                [notebook_id],
            )
            .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn set_feedback(&self, message_id: &str, feedback: i32) -> LnaResult<()> {
        self.db.write(|conn| {
            conn.execute(
                "UPDATE chat_messages SET feedback = ?1 WHERE id = ?2",
                params![feedback, message_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn citation_detail(
        &self,
        message_id: &str,
        citation_no: i32,
    ) -> LnaResult<serde_json::Value> {
        self.db.read(|conn| {
            let row = conn.query_row(
                "SELECT chunk_id, source_id, source_title, page_start, page_end, section_path, quote
                 FROM message_citations WHERE message_id = ?1 AND citation_no = ?2",
                params![message_id, citation_no],
                |r| Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i32>>(3)?,
                    r.get::<_, Option<i32>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                )),
            ).optional().map_err(map_sql_err)?
             .ok_or_else(|| LnaError::not_found("Citation not found"))?;

            let (chunk_id, source_id, source_title, page_start, page_end, section_path, quote) =
                row;
            let chunk_meta: Option<(i32, String)> = if let Some(cid) = chunk_id {
                conn.query_row(
                    "SELECT chunk_index, content FROM source_chunks WHERE id = ?1",
                    [cid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sql_err)?
            } else {
                None
            };
            let (prev_chunks, next_chunks) = match (&source_id, &chunk_meta) {
                (Some(sid), Some((cidx, _))) => {
                    let prev = self
                        .retrieval
                        .read_adjacent_chunks(sid, *cidx, "prev", 1)
                        .unwrap_or_default();
                    let next = self
                        .retrieval
                        .read_adjacent_chunks(sid, *cidx, "next", 1)
                        .unwrap_or_default();
                    (
                        prev.into_iter()
                            .map(|(_, idx, c)| json!({ "chunk_index": idx, "content": c }))
                            .collect::<Vec<_>>(),
                        next.into_iter()
                            .map(|(_, idx, c)| json!({ "chunk_index": idx, "content": c }))
                            .collect::<Vec<_>>(),
                    )
                }
                _ => (Vec::new(), Vec::new()),
            };
            let full_content = chunk_meta.map(|(_, c)| c);
            Ok(json!({
                "no": citation_no,
                "source_title": source_title,
                "source_removed": source_id.is_none() || full_content.is_none(),
                "page_start": page_start,
                "page_end": page_end,
                "section_path": section_path,
                "content": full_content.unwrap_or(quote),
                "prev_chunks": prev_chunks,
                "next_chunks": next_chunks,
            }))
        })
    }

    pub async fn regenerate_last(&self, notebook_id: &str) -> LnaResult<ChatMessage> {
        {
            let g = self
                .active_runs
                .lock()
                .map_err(|_| LnaError::internal("Active runs lock poisoned"))?;
            if g.contains_key(notebook_id) {
                return Err(LnaError::new(
                    ErrorCode::Conflict,
                    "Cannot regenerate while streaming",
                ));
            }
        }
        let last_user: Option<(String, String)> = self.db.read(|conn| {
            conn.query_row(
                "SELECT id, content FROM chat_messages
                 WHERE notebook_id = ?1 AND role = 'user'
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [notebook_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        let (uid, user_text) =
            last_user.ok_or_else(|| LnaError::not_found("No user message to regenerate"))?;
        self.db.write(|conn| {
            conn.execute(
                "DELETE FROM chat_messages
                 WHERE notebook_id = ?1 AND role = 'assistant'
                   AND created_at >= (SELECT created_at FROM chat_messages WHERE id = ?2)",
                params![notebook_id, uid],
            )
            .map_err(map_sql_err)?;
            conn.execute("DELETE FROM chat_messages WHERE id = ?1", [&uid])
                .map_err(map_sql_err)?;
            Ok(())
        })?;
        self.send_message(notebook_id, &user_text).await
    }

    pub fn cancel(&self, notebook_id: &str) -> LnaResult<()> {
        if let Ok(mut g) = self.active_runs.lock() {
            if let Some(tok) = g.remove(notebook_id) {
                tok.cancel();
            }
        }
        Ok(())
    }

    pub fn list_history(&self, notebook_id: &str) -> LnaResult<Vec<ChatMessage>> {
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, notebook_id, role, status, content, thoughts_json, followups_json,
                            profile_id, stop_reason, error_code, feedback, created_at
                     FROM chat_messages WHERE notebook_id = ?1 ORDER BY created_at ASC, id ASC",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([notebook_id], |r| {
                    let status_s: String = r.get(3)?;
                    let status = match status_s.as_str() {
                        "STREAMING" => MessageStatus::Streaming,
                        "INCOMPLETE" => MessageStatus::Incomplete,
                        "FAILED" => MessageStatus::Failed,
                        "CANCELLED" => MessageStatus::Cancelled,
                        _ => MessageStatus::Complete,
                    };
                    let thoughts_s: Option<String> = r.get(5)?;
                    let followups_s: Option<String> = r.get(6)?;
                    Ok(ChatMessage {
                        id: r.get(0)?,
                        notebook_id: r.get(1)?,
                        role: r.get(2)?,
                        status,
                        content: r.get(4)?,
                        thoughts: thoughts_s
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        thought_summary: None,
                        citations: Vec::new(),
                        followups: followups_s
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        profile_id: r.get(7)?,
                        stop_reason: r.get(8)?,
                        error_code: r.get(9)?,
                        feedback: r.get(10)?,
                        created_at: r.get(11)?,
                    })
                })
                .map_err(map_sql_err)?;

            let mut out = Vec::new();
            for item in rows {
                let mut msg = item.map_err(map_sql_err)?;
                msg.citations = load_citations(conn, &msg.id)?;
                out.push(msg);
            }
            Ok(out)
        })
    }

    pub async fn send_message(&self, notebook_id: &str, user_text: &str) -> LnaResult<ChatMessage> {
        // Only 1 active generation per notebook (Section 9.1)
        let cancel = CancellationToken::new();
        {
            let mut g = self
                .active_runs
                .lock()
                .map_err(|_| LnaError::internal("Active runs lock poisoned"))?;
            if g.contains_key(notebook_id) {
                return Err(LnaError::new(
                    ErrorCode::Conflict,
                    "A response is already streaming for this notebook",
                ));
            }
            g.insert(notebook_id.to_string(), cancel.clone());
        }

        let res = self.run_chat_pipeline(notebook_id, user_text, cancel).await;

        if let Ok(mut g) = self.active_runs.lock() {
            g.remove(notebook_id);
        }
        res
    }

    async fn run_chat_pipeline(
        &self,
        notebook_id: &str,
        user_text: &str,
        cancel: CancellationToken,
    ) -> LnaResult<ChatMessage> {
        let topic = format!("chat:{notebook_id}");
        let user_msg_id = new_ulid();
        let asst_msg_id = new_ulid();
        let now = now_ms();

        // Selected sources
        let selected_sources: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM sources WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY'")
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
        let selected_json =
            serde_json::to_string(&selected_sources).unwrap_or_else(|_| "[]".into());

        // Write user message + assistant placeholder
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute(
                "INSERT INTO chat_messages (id, notebook_id, role, status, content, selected_source_ids_json, created_at)
                 VALUES (?1,?2,'user','COMPLETE',?3,?4,?5)",
                params![user_msg_id, notebook_id, user_text, selected_json, now],
            )
            .map_err(map_sql_err)?;
            tx.execute(
                "INSERT INTO chat_messages (id, notebook_id, role, status, content, prompt_version, created_at)
                 VALUES (?1,?2,'assistant','STREAMING','','chat_system_v1',?3)",
                params![asst_msg_id, notebook_id, now + 1],
            )
            .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })?;

        self.emit(
            &topic,
            "chat.started",
            json!({ "message_id": asst_msg_id, "user_message_id": user_msg_id }),
        );

        // PRECHECK: no sources selected
        if selected_sources.is_empty() {
            let msg = "当前未勾选任何已就绪的来源。请在左侧来源栏勾选至少一个来源后再提问。";
            self.finalize_message(&asst_msg_id, "COMPLETE", msg, &[], &[], None, None, None)?;
            self.emit(&topic, "chat.text_delta", json!({ "text": msg }));
            self.emit(
                &topic,
                "chat.finished",
                json!({ "status": "COMPLETE", "stop_reason": "no_source" }),
            );
            return self.get_message(&asst_msg_id);
        }

        // PRECHECK: active model profile + secret
        let profile = match lna_storage::get_active_profile(&self.db)? {
            Some(p) => p,
            None => {
                let err = LnaError::new(
                    ErrorCode::ModelNotConfigured,
                    "No active model profile configured",
                );
                self.finalize_message(
                    &asst_msg_id,
                    "FAILED",
                    "",
                    &[],
                    &[],
                    None,
                    Some("model_not_configured"),
                    Some("MODEL_NOT_CONFIGURED"),
                )?;
                self.emit(
                    &topic,
                    "chat.finished",
                    json!({ "status": "FAILED", "error": err }),
                );
                return Err(err);
            }
        };
        let resolved = match self.resolve_profile(&profile) {
            Ok(r) => r,
            Err(e) => {
                let _ = self.finalize_message(
                    &asst_msg_id,
                    "FAILED",
                    "",
                    &[],
                    &[],
                    Some(&profile.id),
                    None,
                    Some(e.code.as_str()),
                );
                self.emit(
                    &topic,
                    "chat.finished",
                    json!({ "status": "FAILED", "error": e }),
                );
                return Err(e);
            }
        };

        let mut thoughts = Vec::new();
        let mut queries = vec![user_text.to_string()];
        let source_langs: Vec<String> = selected_sources
            .iter()
            .filter_map(|id| lna_storage::get_source(&self.db, id).ok())
            .filter_map(|src| src.language)
            .filter(|lang| !lang.is_empty())
            .collect();
        let has_history = self.db.read(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE notebook_id=?1 AND id<>?2 AND role='user' AND status='COMPLETE')",
                params![notebook_id, user_msg_id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(map_sql_err)
        }).unwrap_or(false);
        if let Some(reason) = dialog::rewrite_reason(user_text, has_history, &source_langs) {
            let step_id = new_ulid();
            let running = ThoughtStep {
                step_id: step_id.clone(),
                kind: "query_rewrite".into(),
                title: "正在整理检索问题".into(),
                detail: Some(reason.to_string()),
                state: "running".into(),
            };
            self.emit(
                &topic,
                "chat.thought_step",
                serde_json::to_value(&running).unwrap_or_default(),
            );
            let history_text = self.db.read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT role, content FROM chat_messages WHERE notebook_id=?1 AND id<>?2 AND status='COMPLETE' ORDER BY created_at DESC LIMIT 6"
                ).map_err(map_sql_err)?;
                let rows = stmt.query_map(params![notebook_id, user_msg_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                }).map_err(map_sql_err)?;
                let mut lines = Vec::new();
                for row in rows { let (role, content) = row.map_err(map_sql_err)?; lines.push(format!("{role}: {content}")); }
                lines.reverse();
                Ok(lines.join("\n"))
            }).unwrap_or_default();
            let rewrite_req = LlmRequest {
                system: QUERY_REWRITE_PROMPT.to_string(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "对话历史：\n{history_text}\n\n当前问题：\n{user_text}"
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::JsonObject,
                max_output_tokens: 512,
                reasoning: ReasoningOptions::default(),
            };
            let rewrite_result = tokio::time::timeout(
                Duration::from_secs(3),
                self.llm
                    .complete_text(&resolved, &rewrite_req, cancel.clone()),
            )
            .await;
            let (detail, state) = match rewrite_result {
                Ok(Ok((raw, _))) => {
                    queries = dialog::parse_rewrite(&raw, user_text);
                    (
                        format!("{reason}；生成 {} 条检索查询", queries.len()),
                        "done",
                    )
                }
                Ok(Err(err)) => (
                    format!("{reason}；改写失败，使用原问题：{}", err.message),
                    "skipped",
                ),
                Err(_) => (format!("{reason}；超过 3 秒，使用原问题"), "skipped"),
            };
            let finished = ThoughtStep {
                step_id,
                kind: "query_rewrite".into(),
                title: if state == "done" {
                    "检索问题已整理".into()
                } else {
                    "使用原问题检索".into()
                },
                detail: Some(detail),
                state: state.into(),
            };
            self.emit(
                &topic,
                "chat.thought_step",
                serde_json::to_value(&finished).unwrap_or_default(),
            );
            thoughts.push(finished);
        }

        let step1 = ThoughtStep {
            step_id: new_ulid(),
            kind: "retrieve".into(),
            title: "正在检索您的来源".into(),
            detail: Some(format!("已勾选 {} 个来源", selected_sources.len())),
            state: "running".into(),
        };
        self.emit(
            &topic,
            "chat.thought_step",
            serde_json::to_value(&step1).unwrap_or_default(),
        );

        // Context budget calculation (Section 9.4)
        let w = resolved.context_window as usize;
        let r = (resolved.max_output_tokens as usize).min(w / 4);
        let available = w.saturating_sub(r + 800 + w / 20);
        let evidence_budget = (available * 60) / 100;
        let history_budget = (available * 30) / 100;

        // RETRIEVE
        let outcome = match self
            .retrieval
            .search(notebook_id, &queries, &selected_sources, evidence_budget, 8)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                let _ = self.finalize_message(
                    &asst_msg_id,
                    "FAILED",
                    "",
                    &thoughts,
                    &[],
                    Some(&profile.id),
                    None,
                    Some(e.code.as_str()),
                );
                self.emit(
                    &topic,
                    "chat.finished",
                    json!({ "status": "FAILED", "error": e }),
                );
                return Err(e);
            }
        };

        let mut step1_done = step1;
        step1_done.state = "done".into();
        step1_done.detail = Some(format!("检索到 {} 个证据片段", outcome.evidences.len()));
        self.emit(
            &topic,
            "chat.thought_step",
            serde_json::to_value(&step1_done).unwrap_or_default(),
        );
        thoughts.push(step1_done);
        if let Some(ref reason) = outcome.vector_fallback_reason {
            let fb_step = ThoughtStep {
                step_id: new_ulid(),
                kind: "retrieve_fallback".into(),
                title: "已降级为关键词检索".into(),
                detail: Some(reason.clone()),
                state: "done".into(),
            };
            self.emit(
                &topic,
                "chat.thought_step",
                serde_json::to_value(&fb_step).unwrap_or_default(),
            );
            thoughts.push(fb_step);
        }

        let mut evidence_map: HashMap<i32, RetrievedEvidence> = HashMap::new();
        for ev in &outcome.evidences {
            evidence_map.insert(ev.citation_no, ev.clone());
        }

        // Build prompt messages
        let (history_msgs, history_summary) = match self
            .load_history_context(
                notebook_id,
                &user_msg_id,
                history_budget,
                &resolved,
                cancel.clone(),
            )
            .await
        {
            Ok(h) => h,
            Err(e) => {
                let _ = self.finalize_message(
                    &asst_msg_id,
                    "FAILED",
                    "",
                    &thoughts,
                    &[],
                    Some(&profile.id),
                    None,
                    Some(e.code.as_str()),
                );
                self.emit(
                    &topic,
                    "chat.finished",
                    json!({ "status": "FAILED", "error": e }),
                );
                return Err(e);
            }
        };
        let mut sources_xml = format_sources_xml(&outcome.evidences, outcome.grounded);
        let system_prompt = history_summary
            .as_deref()
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| format!("{CHAT_SYSTEM_PROMPT}\n\n[历史摘要]\n{summary}"))
            .unwrap_or_else(|| CHAT_SYSTEM_PROMPT.to_string());

        let mut llm_messages = history_msgs;
        llm_messages.push(LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(format!(
                "{sources_xml}\n<question>{}</question>",
                xml_text_escape(user_text)
            ))],
        });

        let tools = if resolved.capabilities.tools {
            vec![
                ToolDef {
                    name: "search_sources".into(),
                    description: "在已选来源中补充检索关键词".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }),
                },
                ToolDef {
                    name: "read_context".into(),
                    description:
                        "读取指定引用附近的上下文；direction 为 prev、next 或 both，count 最大为 3"
                            .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "citation_no": { "type": "integer", "minimum": 1 },
                            "direction": { "type": "string", "enum": ["prev", "next", "both"] },
                            "count": { "type": "integer", "minimum": 1, "maximum": 3 }
                        },
                        "required": ["citation_no"]
                    }),
                },
                ToolDef {
                    name: "list_sources".into(),
                    description: "列出当前已选来源的名称、状态与语言".into(),
                    parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                },
            ]
        } else {
            vec![]
        };

        let mut full_text = String::new();
        let mut _thought_buf = String::new();
        let mut usage_total = Usage::default();
        let mut last_checkpoint_at = Instant::now();
        let mut last_checkpoint_bytes = 0usize;
        let mut context_retried = false;
        let valid_nos: HashSet<i32> = evidence_map.keys().copied().collect();
        let mut norm = citations::CitationStreamNormalizer::new(valid_nos);
        let mut emitted_cites: HashSet<i32> = HashSet::new();
        let mut next_cite_no = (evidence_map.len() as i32) + 1;

        // GENERATE + TOOL_LOOP (max 3 rounds)
        for round in 0..=3 {
            let req = LlmRequest {
                system: system_prompt.clone(),
                messages: llm_messages.clone(),
                tools: if round < 3 { tools.clone() } else { vec![] },
                tool_mode: if round < 3 && !tools.is_empty() {
                    ToolMode::Auto
                } else {
                    ToolMode::None
                },
                output: OutputMode::Text,
                max_output_tokens: resolved.max_output_tokens,
                reasoning: ReasoningOptions {
                    show_summary: resolved.show_thinking,
                    effort: resolved.effort.clone(),
                },
            };

            let mut pending_calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            let mut round_text = String::new();
            let mut opaque_parts: Vec<Part> = Vec::new();

            let stop_res = self
                .llm
                .stream(&resolved, &req, cancel.clone(), |ev| match ev {
                    LlmEvent::ThoughtDelta(t) => {
                        _thought_buf.push_str(&t);
                        if resolved.show_thinking {
                            self.emit(&topic, "chat.thought_delta", json!({ "text": t }));
                        }
                    }
                    LlmEvent::TextDelta(t) => {
                        let (out_chunk, cited_nos) = norm.push(&t);
                        for n in cited_nos {
                            if let Some(ev_ref) = evidence_map.get(&n) {
                                if emitted_cites.insert(n) {
                                    self.emit(
                                        &topic,
                                        "chat.citation",
                                        json!({
                                            "no": n,
                                            "chunk_id": ev_ref.chunk_id,
                                            "source_id": ev_ref.source_id,
                                            "title": ev_ref.source_title,
                                            "page": ev_ref.page_start,
                                            "quote": ev_ref.content.chars().take(220).collect::<String>(),
                                        }),
                                    );
                                }
                            }
                        }
                        if !out_chunk.is_empty() {
                            round_text.push_str(&out_chunk);
                            full_text.push_str(&out_chunk);
                            if full_text.len().saturating_sub(last_checkpoint_bytes) >= 2048
                                || last_checkpoint_at.elapsed() >= Duration::from_secs(1)
                            {
                                let checkpoint = full_text.clone();
                                let _ = self.db.write(|conn| {
                                    conn.execute(
                                        "UPDATE chat_messages SET content=?1 WHERE id=?2 AND status='STREAMING'",
                                        params![checkpoint, asst_msg_id],
                                    ).map_err(map_sql_err)?;
                                    Ok(())
                                });
                                last_checkpoint_bytes = full_text.len();
                                last_checkpoint_at = Instant::now();
                            }
                            self.emit(&topic, "chat.text_delta", json!({ "text": out_chunk }));
                        }
                    }
                    LlmEvent::ToolCallStart { index, id, name } => {
                        pending_calls.insert(index, (id, name, String::new()));
                    }
                    LlmEvent::ToolCallDelta { index, json } => {
                        if let Some(entry) = pending_calls.get_mut(&index) {
                            entry.2.push_str(&json);
                        }
                    }
                    LlmEvent::Opaque(op) => {
                        opaque_parts.push(Part::Opaque(op));
                    }
                    LlmEvent::Usage(u) => {
                        usage_total.input_tokens += u.input_tokens;
                        usage_total.output_tokens += u.output_tokens;
                        usage_total.cached_tokens += u.cached_tokens;
                    }
                    _ => {}
                })
                .await;

            let (tail, tail_cites) = norm.finish();
            if !tail.is_empty() {
                round_text.push_str(&tail);
                full_text.push_str(&tail);
                self.emit(&topic, "chat.text_delta", json!({ "text": tail }));
            }
            for n in tail_cites {
                if let Some(ev_ref) = evidence_map.get(&n) {
                    if emitted_cites.insert(n) {
                        self.emit(
                            &topic,
                            "chat.citation",
                            json!({
                                "no": n,
                                "chunk_id": ev_ref.chunk_id,
                                "source_id": ev_ref.source_id,
                                "title": ev_ref.source_title,
                                "page": ev_ref.page_start,
                                "quote": ev_ref.content.chars().take(220).collect::<String>(),
                            }),
                        );
                    }
                }
            }

            if full_text.len() > last_checkpoint_bytes {
                let checkpoint = full_text.clone();
                let _ = self.db.write(|conn| {
                    conn.execute(
                        "UPDATE chat_messages SET content=?1 WHERE id=?2 AND status='STREAMING'",
                        params![checkpoint, asst_msg_id],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                });
                last_checkpoint_bytes = full_text.len();
                last_checkpoint_at = Instant::now();
            }

            if !context_retried && round == 0 && full_text.is_empty() {
                if let Err(ref err) = stop_res {
                    if err.code == ErrorCode::ModelContextExceeded {
                        context_retried = true;
                        let keep = (outcome.evidences.len() / 2)
                            .max(1)
                            .min(outcome.evidences.len());
                        sources_xml =
                            format_sources_xml(&outcome.evidences[..keep], outcome.grounded);
                        if let Some(last_user) = llm_messages.last_mut() {
                            last_user.parts = vec![Part::Text(format!(
                                "{sources_xml}\n<question>{}</question>",
                                xml_text_escape(user_text)
                            ))];
                        }
                        let step = ThoughtStep {
                            step_id: new_ulid(),
                            kind: "context_retry".into(),
                            title: "上下文超限，缩减证据后重试".into(),
                            detail: Some(format!(
                                "证据片段由 {} 条缩减为 {} 条",
                                outcome.evidences.len(),
                                keep
                            )),
                            state: "done".into(),
                        };
                        self.emit(
                            &topic,
                            "chat.thought_step",
                            serde_json::to_value(&step).unwrap_or_default(),
                        );
                        thoughts.push(step);
                        continue;
                    }
                }
            }

            match stop_res {
                Ok(StopReason::ToolUse) if !pending_calls.is_empty() => {
                    let mut asst_parts = opaque_parts;
                    if !round_text.is_empty() {
                        asst_parts.push(Part::Text(round_text));
                    }
                    let mut tool_results = Vec::new();
                    for (_, (cid, name, args_s)) in pending_calls {
                        let args: serde_json::Value =
                            serde_json::from_str(&args_s).unwrap_or_else(|_| json!({}));
                        asst_parts.push(Part::ToolCall {
                            id: cid.clone(),
                            name: name.clone(),
                            arguments: args.clone(),
                        });
                        let mut is_error = false;
                        let result_content = match name.as_str() {
                            "search_sources" => {
                                let q = args
                                    .get("query")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or(user_text);
                                match self
                                    .retrieval
                                    .search(
                                        notebook_id,
                                        &[q.to_string()],
                                        &selected_sources,
                                        1600,
                                        4,
                                    )
                                    .await
                                {
                                    Ok(extra) => {
                                        let mut result = String::new();
                                        for mut ev in extra.evidences {
                                            if let Some(existing) = evidence_map
                                                .values()
                                                .find(|old| old.chunk_id == ev.chunk_id)
                                            {
                                                result.push_str(&format!(
                                                    "[{}] {}\n",
                                                    existing.citation_no, ev.content
                                                ));
                                            } else {
                                                ev.citation_no = next_cite_no;
                                                next_cite_no += 1;
                                                result.push_str(&format!(
                                                    "[{}] {}\n",
                                                    ev.citation_no, ev.content
                                                ));
                                                norm.add_valid(ev.citation_no);
                                                evidence_map.insert(ev.citation_no, ev);
                                            }
                                        }
                                        if result.is_empty() {
                                            "未检索到更多相关片段。".to_string()
                                        } else {
                                            result
                                        }
                                    }
                                    Err(err) => {
                                        is_error = true;
                                        format!("补充检索失败：{}", err.message)
                                    }
                                }
                            }
                            "read_context" => {
                                let no = args
                                    .get("citation_no")
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(0) as i32;
                                let direction = args
                                    .get("direction")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("both");
                                let count = args
                                    .get("count")
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(1)
                                    .clamp(1, 3) as i32;
                                if let Some(ev) = evidence_map.get(&no).cloned() {
                                    match self.retrieval.read_adjacent_chunks(
                                        &ev.source_id,
                                        ev.chunk_index,
                                        direction,
                                        count,
                                    ) {
                                        Ok(adjacent) => {
                                            let mut result =
                                                format!("引用 [{}] 当前片段：\n{}", no, ev.content);
                                            for (_, index, content) in adjacent {
                                                result.push_str(&format!(
                                                    "\n\n[相邻分块 {}]\n{}",
                                                    index, content
                                                ));
                                            }
                                            result
                                        }
                                        Err(err) => {
                                            is_error = true;
                                            format!("读取上下文失败：{}", err.message)
                                        }
                                    }
                                } else {
                                    is_error = true;
                                    format!("不存在引用编号 [{no}]。")
                                }
                            }
                            "list_sources" => {
                                match lna_storage::list_sources(&self.db, notebook_id) {
                                    Ok(sources) => {
                                        let selected: HashSet<&str> =
                                            selected_sources.iter().map(String::as_str).collect();
                                        let lines: Vec<String> = sources
                                            .iter()
                                            .filter(|src| selected.contains(src.id.as_str()))
                                            .map(|src| {
                                                format!(
                                                    "{} | 状态：{:?} | 语言：{}",
                                                    src.title,
                                                    src.status,
                                                    src.language.as_deref().unwrap_or("未知")
                                                )
                                            })
                                            .collect();
                                        if lines.is_empty() {
                                            "当前没有已选来源。".to_string()
                                        } else {
                                            lines.join("\n")
                                        }
                                    }
                                    Err(err) => {
                                        is_error = true;
                                        format!("读取来源列表失败：{}", err.message)
                                    }
                                }
                            }
                            _ => {
                                is_error = true;
                                format!("不支持的只读工具：{name}")
                            }
                        };
                        let step = ThoughtStep {
                            step_id: new_ulid(),
                            kind: "tool_call".into(),
                            title: match name.as_str() {
                                "search_sources" => "补充检索来源".into(),
                                "read_context" => "读取引用上下文".into(),
                                "list_sources" => "查看已选来源".into(),
                                _ => "执行只读工具".into(),
                            },
                            detail: Some(name.clone()),
                            state: "done".into(),
                        };
                        self.emit(
                            &topic,
                            "chat.thought_step",
                            serde_json::to_value(&step).unwrap_or_default(),
                        );
                        thoughts.push(step);
                        tool_results.push(Part::ToolResult {
                            call_id: cid,
                            content: format!(
                                "<tool_result trust=\"untrusted\">{}</tool_result>",
                                xml_text_escape(&result_content)
                            ),
                            is_error,
                        });
                    }

                    llm_messages.push(LlmMessage {
                        role: Role::Assistant,
                        parts: asst_parts,
                    });
                    llm_messages.push(LlmMessage {
                        role: Role::Tool,
                        parts: tool_results,
                    });
                    continue;
                }
                Ok(reason) => {
                    let _ = lna_storage::record_usage(
                        &self.db,
                        Some(notebook_id),
                        Some(&profile.id),
                        "chat",
                        Some(&asst_msg_id),
                        usage_total.input_tokens as i64,
                        usage_total.output_tokens as i64,
                        usage_total.cached_tokens as i64,
                        false,
                    );
                    self.emit(
                        &topic,
                        "chat.usage",
                        json!({
                            "input": usage_total.input_tokens,
                            "output": usage_total.output_tokens,
                            "cached": usage_total.cached_tokens,
                            "estimated": false
                        }),
                    );

                    let final_cites = collect_final_citations(&emitted_cites, &evidence_map);

                    self.finalize_message(
                        &asst_msg_id,
                        "COMPLETE",
                        &full_text,
                        &thoughts,
                        &final_cites,
                        Some(&profile.id),
                        Some(&format!("{reason:?}")),
                        None,
                    )?;
                    if full_text.trim().chars().count() > 20 && emitted_cites.is_empty() {
                        let missing = ThoughtStep {
                            step_id: new_ulid(),
                            kind: "citation_missing".into(),
                            title: "未找到引用".into(),
                            detail: Some(
                                "回答包含实质内容，但没有引用已选来源中的证据片段。".into(),
                            ),
                            state: "done".into(),
                        };
                        self.emit(
                            &topic,
                            "chat.thought_step",
                            serde_json::to_value(&missing).unwrap_or_default(),
                        );
                        thoughts.push(missing);
                        let _ = self.db.write(|conn| {
                            let serialized =
                                serde_json::to_string(&thoughts).unwrap_or_else(|_| "[]".into());
                            conn.execute(
                                "UPDATE chat_messages SET thoughts_json=?1 WHERE id=?2",
                                params![serialized, asst_msg_id],
                            )
                            .map_err(map_sql_err)?;
                            Ok(())
                        });
                    }
                    self.emit(
                        &topic,
                        "chat.finished",
                        json!({ "status": "COMPLETE", "stop_reason": format!("{reason:?}") }),
                    );
                    let completed = self.get_message(&asst_msg_id)?;
                    self.spawn_followups(
                        notebook_id.to_string(),
                        asst_msg_id.clone(),
                        user_text.to_string(),
                        full_text.clone(),
                        resolved.clone(),
                    );
                    return Ok(completed);
                }
                Err(e) => {
                    let status = if e.code == ErrorCode::Cancelled {
                        "CANCELLED"
                    } else if !full_text.is_empty() {
                        "INCOMPLETE"
                    } else {
                        "FAILED"
                    };
                    let final_cites = collect_final_citations(&emitted_cites, &evidence_map);
                    self.finalize_message(
                        &asst_msg_id,
                        status,
                        &full_text,
                        &thoughts,
                        &final_cites,
                        Some(&profile.id),
                        None,
                        Some(e.code.as_str()),
                    )?;
                    self.emit(
                        &topic,
                        "chat.finished",
                        json!({ "status": status, "error": e }),
                    );
                    return Err(e);
                }
            }
        }

        let final_cites = collect_final_citations(&emitted_cites, &evidence_map);
        self.finalize_message(
            &asst_msg_id,
            "INCOMPLETE",
            &full_text,
            &thoughts,
            &final_cites,
            Some(&profile.id),
            Some("tool_loop_limit"),
            None,
        )?;
        self.emit(
            &topic,
            "chat.finished",
            json!({ "status": "INCOMPLETE", "stop_reason": "tool_loop_limit" }),
        );
        self.get_message(&asst_msg_id)
    }

    pub async fn refresh_notebook_summary(&self, notebook_id: &str) -> LnaResult<()> {
        let profile = match lna_storage::get_active_profile(&self.db)? {
            Some(p) => p,
            None => return Ok(()), // Section 7.5: skip if no model configured
        };
        let resolved = self.resolve_profile(&profile)?;
        let sources = lna_storage::list_sources(&self.db, notebook_id)?;
        let ready: Vec<&Source> = sources
            .iter()
            .filter(|s| s.status == SourceStatus::Ready)
            .collect();
        if ready.is_empty() {
            return Ok(());
        }

        let mut excerpts = Vec::new();
        for s in &ready {
            let chunks: Vec<String> = self.db.read(|conn| {
                let mut stmt = conn.prepare("SELECT content FROM source_chunks WHERE source_id = ?1 ORDER BY chunk_index ASC LIMIT 2").map_err(map_sql_err)?;
                let rows = stmt.query_map([&s.id], |r| r.get::<_, String>(0)).map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows { out.push(r.map_err(map_sql_err)?); }
                Ok(out)
            }).unwrap_or_default();
            excerpts.push(format!(
                "### {} ({} tokens)\n{}",
                s.title,
                s.token_count,
                chunks.join("\n")
            ));
        }
        let titles_list = excerpts.join("\n\n");

        let req = LlmRequest {
            system: SUMMARY_PROMPT.to_string(),
            messages: vec![LlmMessage {
                role: Role::User,
                parts: vec![Part::Text(titles_list)],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::JsonObject,
            max_output_tokens: 1200,
            reasoning: ReasoningOptions::default(),
        };

        let (raw, _) = self
            .llm
            .complete_text(&resolved, &req, CancellationToken::new())
            .await?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let summary_md = v.get("summary_md").and_then(|x| x.as_str()).unwrap_or("");
            let title_sugg = v.get("title_suggestion").and_then(|x| x.as_str());
            let sq = v
                .get("suggested_questions")
                .map(|x| x.to_string())
                .unwrap_or_else(|| "[]".into());
            self.db.write(|conn| {
                conn.execute(
                    "UPDATE notebooks SET summary_md=?1, suggested_questions_json=?2,
                     title=CASE WHEN title_user_edited=0 AND ?3 IS NOT NULL THEN ?3 ELSE title END,
                     updated_at=?4 WHERE id=?5",
                    params![summary_md, sq, title_sugg, now_ms(), notebook_id],
                )
                .map_err(map_sql_err)?;
                Ok(())
            })?;
            self.emit(
                &format!("notebook:{notebook_id}"),
                "notebook.summary_updated",
                json!({ "notebook_id": notebook_id, "summary": summary_md }),
            );
        }
        Ok(())
    }

    pub fn resolve_profile(&self, profile: &ModelProfile) -> LnaResult<ResolvedProfile> {
        let api_key = match profile.keychain_ref {
            Some(ref kref) => self.secrets.get(kref)?,
            None => None,
        };
        Ok(ResolvedProfile {
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
        })
    }

    async fn load_history_context(
        &self,
        notebook_id: &str,
        exclude_id: &str,
        budget: usize,
        profile: &ResolvedProfile,
        cancel: CancellationToken,
    ) -> LnaResult<(Vec<LlmMessage>, Option<String>)> {
        let messages = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, role, content FROM chat_messages
                 WHERE notebook_id=?1 AND id<>?2 AND status='COMPLETE'
                 ORDER BY created_at DESC LIMIT 500",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map(params![notebook_id, exclude_id], |r| {
                    Ok(dialog::HistoryMsg {
                        id: r.get(0)?,
                        role: r.get(1)?,
                        content: r.get(2)?,
                    })
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sql_err)?);
            }
            out.reverse();
            Ok(out)
        })?;
        let plan = dialog::plan_history(&messages, budget);
        let existing: Option<(String, String)> = self.db.read(|conn| {
            conn.query_row(
                "SELECT through_message_id, summary FROM conversation_summaries WHERE notebook_id=?1",
                [notebook_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(map_sql_err)
        })?;
        let mut summary = existing.as_ref().map(|(_, text)| text.clone());
        let mut overflow_to_summarize = plan.overflow.clone();
        if let Some((through_id, _)) = existing.as_ref() {
            if let Some(pos) = overflow_to_summarize
                .iter()
                .position(|m| &m.id == through_id)
            {
                overflow_to_summarize = overflow_to_summarize.into_iter().skip(pos + 1).collect();
            } else if messages.iter().any(|m| &m.id == through_id) {
                overflow_to_summarize.clear();
            }
        }
        if !overflow_to_summarize.is_empty() {
            let transcript = overflow_to_summarize
                .iter()
                .map(|m| format!("{}: {}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            let previous = summary.as_deref().unwrap_or("");
            let req = LlmRequest {
                system: HISTORY_SUMMARY_PROMPT.to_string(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "已有摘要：\n{previous}\n\n需要合并的新对话：\n{transcript}"
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::JsonObject,
                max_output_tokens: 512,
                reasoning: ReasoningOptions::default(),
            };
            if let Ok((raw, _)) = self.llm.complete_text(profile, &req, cancel).await {
                let new_summary = dialog::extract_json(&raw)
                    .and_then(|v| {
                        v.get("summary")
                            .and_then(|x| x.as_str())
                            .map(str::to_string)
                    })
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| raw.trim().to_string());
                if !new_summary.trim().is_empty() {
                    let through_id = overflow_to_summarize
                        .last()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    let token_count = dialog::estimate_tokens(&new_summary) as i64;
                    self.db.write(|conn| {
                        conn.execute(
                            "INSERT INTO conversation_summaries (notebook_id, through_message_id, summary, token_count, updated_at)
                             VALUES (?1,?2,?3,?4,?5)
                             ON CONFLICT(notebook_id) DO UPDATE SET through_message_id=excluded.through_message_id,
                             summary=excluded.summary, token_count=excluded.token_count, updated_at=excluded.updated_at",
                            params![notebook_id, through_id, new_summary, token_count, now_ms()],
                        ).map_err(map_sql_err)?;
                        Ok(())
                    })?;
                    summary = Some(new_summary);
                }
            }
        }
        let history = plan
            .kept
            .into_iter()
            .map(|m| LlmMessage {
                role: if m.role == "user" {
                    Role::User
                } else {
                    Role::Assistant
                },
                parts: vec![Part::Text(m.content)],
            })
            .collect();
        Ok((history, summary))
    }

    fn spawn_followups(
        &self,
        notebook_id: String,
        message_id: String,
        question: String,
        answer: String,
        profile: ResolvedProfile,
    ) {
        let service = self.clone();
        tokio::spawn(async move {
            let req = LlmRequest {
                system: FOLLOWUPS_PROMPT.to_string(),
                messages: vec![LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(format!(
                        "问题：\n{question}\n\n回答：\n{answer}"
                    ))],
                }],
                tools: vec![],
                tool_mode: ToolMode::None,
                output: OutputMode::JsonObject,
                max_output_tokens: 256,
                reasoning: ReasoningOptions::default(),
            };
            let Ok((raw, _)) = service
                .llm
                .complete_text(&profile, &req, CancellationToken::new())
                .await
            else {
                return;
            };
            let questions = dialog::parse_followups(&raw);
            if questions.is_empty() {
                return;
            }
            let serialized = serde_json::to_string(&questions).unwrap_or_else(|_| "[]".into());
            if service
                .db
                .write(|conn| {
                    conn.execute(
                        "UPDATE chat_messages SET followups_json=?1 WHERE id=?2",
                        params![serialized, message_id],
                    )
                    .map_err(map_sql_err)?;
                    Ok(())
                })
                .is_ok()
            {
                service.emit(
                    &format!("chat:{notebook_id}"),
                    "chat.followups",
                    json!({
                        "message_id": message_id,
                        "questions": questions
                    }),
                );
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_message(
        &self,
        msg_id: &str,
        status: &str,
        content: &str,
        thoughts: &[ThoughtStep],
        citations: &[CitationRef],
        profile_id: Option<&str>,
        stop_reason: Option<&str>,
        error_code: Option<&str>,
    ) -> LnaResult<()> {
        let thoughts_json = serde_json::to_string(thoughts).unwrap_or_else(|_| "[]".into());
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute(
                "UPDATE chat_messages SET status=?1, content=?2, thoughts_json=?3,
                 profile_id=?4, stop_reason=?5, error_code=?6 WHERE id=?7",
                params![status, content, thoughts_json, profile_id, stop_reason, error_code, msg_id],
            )
            .map_err(map_sql_err)?;
            for c in citations {
                tx.execute(
                    "INSERT OR REPLACE INTO message_citations
                     (message_id, citation_no, chunk_id, source_id, source_title, page_start, page_end, section_path, quote)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        msg_id,
                        c.no,
                        c.chunk_id,
                        c.source_id,
                        c.source_title,
                        c.page_start,
                        c.page_end,
                        c.section_path,
                        c.quote
                    ],
                )
                .map_err(map_sql_err)?;
            }
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn get_message(&self, msg_id: &str) -> LnaResult<ChatMessage> {
        self.db.read(|conn| {
            let mut msg = conn
                .query_row(
                    "SELECT id, notebook_id, role, status, content, thoughts_json, followups_json,
                            profile_id, stop_reason, error_code, feedback, created_at
                     FROM chat_messages WHERE id = ?1",
                    [msg_id],
                    |r| {
                        let status_s: String = r.get(3)?;
                        let status = match status_s.as_str() {
                            "STREAMING" => MessageStatus::Streaming,
                            "INCOMPLETE" => MessageStatus::Incomplete,
                            "FAILED" => MessageStatus::Failed,
                            "CANCELLED" => MessageStatus::Cancelled,
                            _ => MessageStatus::Complete,
                        };
                        let thoughts_s: Option<String> = r.get(5)?;
                        let followups_s: Option<String> = r.get(6)?;
                        Ok(ChatMessage {
                            id: r.get(0)?,
                            notebook_id: r.get(1)?,
                            role: r.get(2)?,
                            status,
                            content: r.get(4)?,
                            thoughts: thoughts_s
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_default(),
                            thought_summary: None,
                            citations: Vec::new(),
                            followups: followups_s
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_default(),
                            profile_id: r.get(7)?,
                            stop_reason: r.get(8)?,
                            error_code: r.get(9)?,
                            feedback: r.get(10)?,
                            created_at: r.get(11)?,
                        })
                    },
                )
                .optional()
                .map_err(map_sql_err)?
                .ok_or_else(|| LnaError::not_found(format!("Message not found: {msg_id}")))?;
            msg.citations = load_citations(conn, &msg.id)?;
            Ok(msg)
        })
    }
}

fn load_citations(conn: &rusqlite::Connection, msg_id: &str) -> LnaResult<Vec<CitationRef>> {
    let mut stmt = conn
        .prepare(
            "SELECT citation_no, chunk_id, source_id, source_title, page_start, page_end, section_path, quote
             FROM message_citations WHERE message_id = ?1 ORDER BY citation_no ASC",
        )
        .map_err(map_sql_err)?;
    let rows = stmt
        .query_map([msg_id], |r| {
            let sid: Option<String> = r.get(2)?;
            Ok(CitationRef {
                no: r.get(0)?,
                chunk_id: r.get(1)?,
                source_removed: sid.is_none(),
                source_id: sid,
                source_title: r.get(3)?,
                page_start: r.get(4)?,
                page_end: r.get(5)?,
                section_path: r.get(6)?,
                quote: r.get(7)?,
            })
        })
        .map_err(map_sql_err)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(map_sql_err)?);
    }
    Ok(out)
}

fn format_sources_xml(evidences: &[RetrievedEvidence], grounded: bool) -> String {
    if !grounded || evidences.is_empty() {
        return "<sources>\n(未检索到与问题直接相关的来源片段；请明确告知用户在已选来源中未找到依据)\n</sources>".to_string();
    }
    let mut out = String::from("<sources>\n");
    for ev in evidences {
        let page = ev.page_start.map(|p| p.to_string()).unwrap_or_default();
        let section = xml_attr_escape(ev.section_path.as_deref().unwrap_or(""));
        let title = xml_attr_escape(&ev.source_title);
        out.push_str(&format!(
            "<source n=\"{}\" title=\"{}\" page=\"{}\" section=\"{}\">\n{}\n</source>\n",
            ev.citation_no,
            title,
            page,
            section,
            xml_text_escape(&ev.content)
        ));
    }
    out.push_str("</sources>");
    out
}

fn xml_text_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn collect_final_citations(
    emitted_cites: &HashSet<i32>,
    evidence_map: &HashMap<i32, RetrievedEvidence>,
) -> Vec<CitationRef> {
    emitted_cites
        .iter()
        .filter_map(|n| evidence_map.get(n))
        .map(|ev| CitationRef {
            no: ev.citation_no,
            chunk_id: Some(ev.chunk_id),
            source_id: Some(ev.source_id.clone()),
            source_title: ev.source_title.clone(),
            page_start: ev.page_start,
            page_end: ev.page_end,
            section_path: ev.section_path.clone(),
            quote: ev.content.chars().take(240).collect(),
            source_removed: false,
        })
        .collect()
}
