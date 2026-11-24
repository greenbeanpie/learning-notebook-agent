pub mod citations;
pub mod dialog;

use lna_llm::*;
use lna_retrieval::{RetrievalEngine, RetrievedEvidence};
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_studio::StudioService;
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
    studio: StudioService,
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
        studio: StudioService,
        events: broadcast::Sender<ApiEvent>,
        seq: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            db,
            secrets,
            llm,
            retrieval,
            studio,
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
        let msg_ids: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM chat_messages WHERE notebook_id = ?1")
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
        self.studio.delete_unimported_for_messages(&msg_ids)?;
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
            let source_removed =
                source_id.is_none() || (chunk_id.is_some() && full_content.is_none());
            Ok(json!({
                "no": citation_no,
                "source_title": source_title,
                "source_removed": source_removed,
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
        let asst_ids: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM chat_messages
                     WHERE notebook_id = ?1 AND role = 'assistant'
                       AND created_at >= (SELECT created_at FROM chat_messages WHERE id = ?2)",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map(params![notebook_id, uid], |r| r.get::<_, String>(0))
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        self.studio.delete_unimported_for_messages(&asst_ids)?;
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

    /// Cancels every running answer (shutdown).
    pub fn cancel_all(&self) {
        if let Ok(g) = self.active_runs.lock() {
            for tok in g.values() {
                tok.cancel();
            }
        }
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
        let mut msgs = self.db.read(|conn| {
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
                        artifacts: Vec::new(),
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
        })?;
        for msg in &mut msgs {
            if msg.role == "assistant" {
                msg.artifacts = self.studio.list_by_message(&msg.id).unwrap_or_default();
            }
        }
        Ok(msgs)
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

        // Selected sources (id, title, kind)
        let selected_source_meta: Vec<(String, String, String)> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, title, kind FROM sources WHERE notebook_id = ?1 AND is_selected = 1 AND status = 'READY' ORDER BY created_at ASC")
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([notebook_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;
        let selected_sources: Vec<String> =
            selected_source_meta.iter().map(|(id, _, _)| id.clone()).collect();
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

        // Context budget calculation (Section 9.4) — tightened for minimal token cost + high precision
        let w = resolved.context_window as usize;
        let r = (resolved.max_output_tokens as usize).min(w / 4);
        let available = w.saturating_sub(r + 800 + w / 20);
        let evidence_budget = ((available * 60) / 100).min(2800);
        let history_budget = (available * 30) / 100;

        // RETRIEVE (Tree-Boosted Hybrid Top-5 high-precision anchors)
        let mut outcome = match self
            .retrieval
            .search(notebook_id, &queries, &selected_sources, evidence_budget, 5)
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

        if outcome.evidences.is_empty()
            && !selected_sources.is_empty()
            && is_overview_question(user_text)
        {
            if let Ok(overview) =
                self.retrieval
                    .read_source_overview(&selected_sources, evidence_budget, 4)
            {
                if !overview.is_empty() {
                    outcome.evidences = overview;
                    outcome.grounded = true;
                }
            }
        }

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
        let mut sources_xml = format_sources_xml(
            &self.db,
            &outcome.evidences,
            outcome.grounded,
            &selected_source_meta,
        );
        let system_prompt = history_summary
            .as_deref()
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| format!("{CHAT_SYSTEM_PROMPT}\n\n[历史摘要]\n{summary}"))
            .unwrap_or_else(|| CHAT_SYSTEM_PROMPT.to_string());

        let studio_hint_xml = if resolved.capabilities.tools {
            detect_studio_artifact_hint(user_text).map(|(art_type, zh_label)| {
                format!(
                    "\n<system_hint>检测到用户本轮在请求生成「{zh_label} ({art_type})」。若确属生成学习成品需求，请直接调用 generate_studio_artifact(artifact_type=\"{art_type}\", ...) 异步后台生成（在 topic 中写明章节/主题与要求，无需先通读全文，数量保持适中如 quiz 5~8/flashcards 8~12，调用后立即返回再用 1~2 句告知用户即可），严禁在对话正文中手写完整题目或长篇内容代替。</system_hint>"
                )
            })
        } else {
            None
        }
        .unwrap_or_default();

        let mut llm_messages = history_msgs;
        llm_messages.push(LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(format!(
                "{sources_xml}\n<question>{}</question>{studio_hint_xml}",
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
                    description: "列出当前已选来源的 ID、名称、状态与语言".into(),
                    parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                },
                ToolDef {
                    name: "read_source".into(),
                    description: "按顺序读取已选来源的少量连续分块（仅在检索未命中或需要确认某小节上下文时使用，每次建议读取 2~4 块，避免整章全量通读）".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "从 list_sources 获取的来源 ID；省略则读取已选来源" },
                            "start_chunk": { "type": "integer", "minimum": 0, "description": "起始分块序号，默认 0" },
                            "count": { "type": "integer", "minimum": 1, "maximum": 6, "description": "读取分块数量，默认 4" }
                        }
                    }),
                },
                ToolDef {
                    name: "read_index_node".into(),
                    description: "按 AI 索引目录树的 node_id 精准读取该小节的核心正文片段（比翻页更省 Token 且更精准，按需读取相关节点，无需一次性遍历全章）".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "来源 ID" },
                            "node_id": { "type": "string", "description": "从 <selected_files> 目录树中获取的节点 id" },
                            "limit": { "type": "integer", "minimum": 1, "maximum": 6, "description": "最多读取分块数，默认 4" }
                        },
                        "required": ["source_id", "node_id"]
                    }),
                },
                ToolDef {
                    name: "inspect_source_structure".into(),
                    description: "查看来源的页面/章节与多媒体视觉索引（总页数、哪些页面是扫描页/空白页、每页含有的图片数量以及文档内嵌图片清单）".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "来源 ID（可通过 list_sources 获取）" }
                        },
                        "required": ["source_id"]
                    }),
                },
                ToolDef {
                    name: "inspect_source_pages".into(),
                    description: "针对图文混合或扫描版 PDF/PPTX/文档，按页码按需读取最关键的 1~3 页（单次最多 5 页）的文字分块并渲染页面图片分析其中的图表与公式，避免一次性拉取大范围连续页码。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "来源 ID" },
                            "pages": {
                                "type": "array",
                                "items": { "type": "integer", "minimum": 1 },
                                "maxItems": 5,
                                "description": "需要查看的 1-based 页码或幻灯片编号列表（推荐 1~3 页，最多 5 页）"
                            },
                            "focus_question": { "type": "string", "description": "你想从这些页面中获取的具体问题、图表或公式细节" }
                        },
                        "required": ["source_id", "pages", "focus_question"]
                    }),
                },
                ToolDef {
                    name: "inspect_source_images".into(),
                    description: "提取并查看独立图片源、PDF 指定页内的嵌入图像或 Office/EPUB 文档内嵌图片，由视觉模型分析细节并总结。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "来源 ID" },
                            "page": { "type": "integer", "minimum": 1, "description": "PDF 或 PPTX 的 1-based 页码（查看该页内的嵌入图片时填写）" },
                            "image_ids": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "从 inspect_source_structure 获取的图片 ID 列表（如 img_1），省略则提取该页或文档前几张图片"
                            },
                            "focus_question": { "type": "string", "description": "需要从图片中识别或分析的具体问题" }
                        },
                        "required": ["source_id", "focus_question"]
                    }),
                },
                ToolDef {
                    name: "generate_studio_artifact".into(),
                    description: "【必用·异步非阻塞】当用户要求生成测验/测试/出题/练习题(quiz)、闪卡(flashcards)、思维导图(mindmap)、研究报告/学习指南(report)、演示文稿/PPT(slides)、信息图(infographic)或音频概览/播客(audio)时，必须调用此工具；调用后会立即入队并在后台生成（不阻塞主对话），不要直接在对话中手写题目或长篇正文代替。除非用户明确要求更大数量，默认保持适中规模（quiz 5~8 题、flashcards 8~12 张、slides 6~10 页）。此工具内部会自动从已选来源检索证据，无需先逐页通读全文，只需在 topic 中写明目标章节/主题与具体要求即可。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "artifact_type": {
                                "type": "string",
                                "enum": ["quiz", "flashcards", "mindmap", "report", "slides", "infographic", "audio"],
                                "description": "成品类型：quiz(测验)、flashcards(闪卡)、mindmap(思维导图)、report(研究报告)、slides(演示文稿)、infographic(信息图)、audio(音频概览)"
                            },
                            "title": { "type": "string", "description": "成品标题（可选，如「第4章正交性测验」）" },
                            "topic": { "type": "string", "description": "聚焦的主题、章节或具体出题/编写要求（可选）" },
                            "lang": { "type": "string", "description": "输出语言代码，如 zh-CN 或 en（可选）" },
                            "count": { "type": "integer", "minimum": 3, "maximum": 40, "description": "题目数量（quiz 建议 5~8，默认 5）或闪卡数量（flashcards 建议 8~12，默认 10）" },
                            "difficulty": { "type": "string", "enum": ["auto", "mixed", "easy", "medium", "hard"], "description": "测验难度（仅 quiz）：auto 为参考知识点图谱理解进度自适应难度（默认）；若用户未显式指定难度请省略或传 auto" },
                            "types": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["single", "multi", "fill", "short"] },
                                "description": "测验题型（仅 quiz）：single 单选、multi 多选、fill 填空、short 简答"
                            },
                            "format": { "type": "string", "description": "格式：report 可选 document/interactive；audio 可选 deep_dive/debate/brief/critique" },
                            "pages": { "type": "integer", "minimum": 4, "maximum": 30, "description": "演示文稿页数（仅 slides，建议 6~10，默认 8）" },
                            "orientation": { "type": "string", "enum": ["landscape", "portrait", "square"], "description": "信息图方向（仅 infographic）" }
                        },
                        "required": ["artifact_type"]
                    }),
                },
                ToolDef {
                    name: "get_knowledge_progress".into(),
                    description: "查询当前笔记本（或指定材料 source_id）的知识点图谱理解进度统计，包括已掌握/学习中/待巩固/未学习的概念列表、总体掌握率百分比与推荐测验难度。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "可选：指定材料 ID，仅查询该材料依附的知识点图谱进度" }
                        }
                    }),
                },
                ToolDef {
                    name: "build_source_knowledge_graph".into(),
                    description: "为指定材料（source_id）构建依附的知识点图谱（异步后台生成，不作为 Studio 产物展现）。当用户学习结构化教材/讲义并需要知识点图谱或前置依赖梳理时调用。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "source_id": { "type": "string", "description": "目标材料 ID（可先通过 list_sources 获取）" },
                            "detail": { "type": "string", "enum": ["concise", "standard", "detailed"], "description": "图谱细致度，默认 standard" }
                        },
                        "required": ["source_id"]
                    }),
                },
                ToolDef {
                    name: "report_thought_stage".into(),
                    description: "每完成一个阶段的思考或分析时调用，向用户汇报该阶段的思考流程概括（标题+概述），不暴露内部原始推理。".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "简短的思考阶段标题" },
                            "summary": { "type": "string", "description": "1~2句该阶段思考流程概述" }
                        },
                        "required": ["title", "summary"],
                        "additionalProperties": false
                    }),
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
        let mut generated_studio_artifact = false;
        let valid_nos: HashSet<i32> = evidence_map.keys().copied().collect();
        let mut norm = citations::CitationStreamNormalizer::new(valid_nos);
        let mut emitted_cites: HashSet<i32> = HashSet::new();
        let mut next_cite_no = (evidence_map.len() as i32) + 1;

        // GENERATE + TOOL_LOOP (max 12 tool rounds + 1 final synthesis round). Rounds that only
        // report thought stages don't consume the reading budget (capped separately).
        const MAX_TOOL_ROUNDS: usize = 12;
        const MAX_FREE_STAGE_ROUNDS: usize = 6;
        let mut free_stage_rounds: usize = 0;
        let mut final_round_nudged = false;
        let mut final_empty_retried = false;
        let mut round: usize = 0;
        while round <= MAX_TOOL_ROUNDS {
            if round == MAX_TOOL_ROUNDS && !final_round_nudged {
                final_round_nudged = true;
                llm_messages.push(LlmMessage {
                    role: Role::User,
                    parts: vec![Part::Text(
                        "（系统提示：本轮为最终总结轮次，已关闭所有工具调用。请勿输出任何工具调用或 DSML/XML 标签，直接依据上方已获得的证据片段与引用编号 [n] 回答用户问题；如某部分证据尚未覆盖，请如实说明。）"
                            .to_string(),
                    )],
                });
            }
            let req = LlmRequest {
                system: system_prompt.clone(),
                messages: llm_messages.clone(),
                tools: if round < MAX_TOOL_ROUNDS { tools.clone() } else { vec![] },
                tool_mode: if round < MAX_TOOL_ROUNDS && !tools.is_empty() {
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
            let mut thought_filter = ThoughtTagStreamFilter::new();

            let mut emit_visible = |vis: &str| {
                if vis.is_empty() {
                    return;
                }
                let (out_chunk, cited_nos) = norm.push(vis);
                for n in cited_nos {
                    if let Some(ev_ref) = evidence_map.get(&n) {
                        if emitted_cites.insert(n) {
                            self.emit(
                                &topic,
                                "chat.citation",
                                json!({
                                    "no": n,
                                    "chunk_id": if ev_ref.chunk_id > 0 { Some(ev_ref.chunk_id) } else { None },
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
                    let out_slice = if full_text.is_empty() {
                        out_chunk.trim_start()
                    } else {
                        out_chunk.as_str()
                    };
                    if out_slice.is_empty() {
                        return;
                    }
                    round_text.push_str(out_slice);
                    full_text.push_str(out_slice);
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
                    self.emit(&topic, "chat.text_delta", json!({ "text": out_slice }));
                }
            };

            let mut record_stages = |stages: Vec<(String, String)>| {
                for (st_title, st_summary) in stages {
                    let st = ThoughtStep {
                        step_id: new_ulid(),
                        kind: "thought_stage".into(),
                        title: st_title,
                        detail: if st_summary.is_empty() { None } else { Some(st_summary) },
                        state: "done".into(),
                    };
                    self.emit(&topic, "chat.thought_step", serde_json::to_value(&st).unwrap_or_default());
                    thoughts.push(st);
                }
            };

            let stop_res = self
                .llm
                .stream(&resolved, &req, cancel.clone(), |ev| match ev {
                    LlmEvent::ThoughtDelta(t) => {
                        // Internal CoT is intentionally hidden from UI; only stage summaries are displayed.
                        _thought_buf.push_str(&t);
                    }
                    LlmEvent::TextDelta(t) => {
                        let (vis, stages) = thought_filter.push(&t);
                        record_stages(stages);
                        emit_visible(&vis);
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

            let (vis_tail, tail_stages) = thought_filter.finish();
            record_stages(tail_stages);
            emit_visible(&vis_tail);
            let leaked_calls = thought_filter.take_leaked_tool_calls();
            if pending_calls.is_empty() && !leaked_calls.is_empty() && round < MAX_TOOL_ROUNDS {
                for (idx, (cid, name, args_val)) in leaked_calls.iter().enumerate() {
                    pending_calls.insert(
                        idx as u32,
                        (cid.clone(), name.clone(), args_val.to_string()),
                    );
                }
            }

            let (tail, tail_cites) = norm.finish();
            if !tail.is_empty() {
                let tail_slice = if full_text.is_empty() {
                    tail.trim_start()
                } else {
                    tail.as_str()
                };
                if !tail_slice.is_empty() {
                    round_text.push_str(tail_slice);
                    full_text.push_str(tail_slice);
                    self.emit(&topic, "chat.text_delta", json!({ "text": tail_slice }));
                }
            }
            for n in tail_cites {
                if let Some(ev_ref) = evidence_map.get(&n) {
                    if emitted_cites.insert(n) {
                        self.emit(
                            &topic,
                            "chat.citation",
                            json!({
                                "no": n,
                                "chunk_id": if ev_ref.chunk_id > 0 { Some(ev_ref.chunk_id) } else { None },
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
                        sources_xml = format_sources_xml(
                            &self.db,
                            &outcome.evidences[..keep],
                            outcome.grounded,
                            &selected_source_meta,
                        );
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
                Ok(StopReason::ToolUse) | Ok(StopReason::EndTurn)
                    if !pending_calls.is_empty() && round < MAX_TOOL_ROUNDS =>
                {
                    let mut asst_parts = opaque_parts;
                    if !round_text.is_empty() {
                        asst_parts.push(Part::Text(round_text));
                    }
                    let mut tool_results = Vec::new();
                    let only_stage_reports = pending_calls
                        .values()
                        .all(|(_, name, _)| name == "report_thought_stage");
                    for (_, (cid, name, args_s)) in pending_calls {
                        let args: serde_json::Value =
                            serde_json::from_str(&args_s).unwrap_or_else(|_| json!({}));
                        asst_parts.push(Part::ToolCall {
                            id: cid.clone(),
                            name: name.clone(),
                            arguments: args.clone(),
                        });
                        let mut is_error = false;
                        let mut custom_step_title: Option<String> = None;
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
                                                    "ID: {} | 标题: {} | 状态: {:?} | 语言: {}",
                                                    src.id,
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
                            "read_source" => {
                                let req_sid = args
                                    .get("source_id")
                                    .and_then(|x| x.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty());
                                let start_chunk = args
                                    .get("start_chunk")
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(0)
                                    .max(0) as i32;
                                let count = args
                                    .get("count")
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(4)
                                    .clamp(1, 6) as i32;
                                let target_ids: Vec<String> = match req_sid {
                                    Some(sid) => {
                                        if selected_sources.iter().any(|s| s == sid) {
                                            vec![sid.to_string()]
                                        } else {
                                            Vec::new()
                                        }
                                    }
                                    None => selected_sources.clone(),
                                };
                                if target_ids.is_empty() {
                                    is_error = true;
                                    "指定的来源未选中或不存在。".to_string()
                                } else {
                                    let mut result = String::new();
                                    let per_src = if req_sid.is_some() {
                                        count
                                    } else {
                                        (count / target_ids.len() as i32).max(1)
                                    };
                                    for sid in &target_ids {
                                        match self.retrieval.read_source_chunks(sid, start_chunk, per_src) {
                                            Ok(chunks) => {
                                                for mut ev in chunks {
                                                    if let Some(existing) = evidence_map
                                                        .values()
                                                        .find(|old| old.chunk_id == ev.chunk_id)
                                                    {
                                                        result.push_str(&format!(
                                                            "[{}] ({} #chunk {})\n{}\n\n",
                                                            existing.citation_no,
                                                            ev.source_title,
                                                            ev.chunk_index,
                                                            ev.content
                                                        ));
                                                    } else {
                                                        ev.citation_no = next_cite_no;
                                                        next_cite_no += 1;
                                                        result.push_str(&format!(
                                                            "[{}] ({} #chunk {})\n{}\n\n",
                                                            ev.citation_no,
                                                            ev.source_title,
                                                            ev.chunk_index,
                                                            ev.content
                                                        ));
                                                        norm.add_valid(ev.citation_no);
                                                        evidence_map.insert(ev.citation_no, ev);
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                is_error = true;
                                                result.push_str(&format!(
                                                    "读取来源 {sid} 失败：{}\n",
                                                    err.message
                                                ));
                                            }
                                        }
                                    }
                                    if result.is_empty() {
                                        "该来源暂无已索引的正文分块。".to_string()
                                    } else {
                                        result
                                    }
                                }
                            }
                            "read_index_node" => {
                                let sid = args
                                    .get("source_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .trim();
                                let nid = args
                                    .get("node_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .trim();
                                let limit = args
                                    .get("limit")
                                    .and_then(|x| x.as_i64())
                                    .unwrap_or(4)
                                    .clamp(1, 6) as i32;
                                if !selected_sources.iter().any(|s| s == sid) {
                                    is_error = true;
                                    "指定的来源未选中或不存在。".to_string()
                                } else {
                                    match self.retrieval.read_index_node_chunks(sid, nid, limit) {
                                        Ok(chunks) if !chunks.is_empty() => {
                                            let mut result = String::new();
                                            for mut ev in chunks {
                                                if let Some(existing) = evidence_map
                                                    .values()
                                                    .find(|old| old.chunk_id == ev.chunk_id)
                                                {
                                                    result.push_str(&format!(
                                                        "[{}] ({} · {})\n{}\n\n",
                                                        existing.citation_no,
                                                        ev.source_title,
                                                        ev.section_path.as_deref().unwrap_or(""),
                                                        ev.content
                                                    ));
                                                } else {
                                                    ev.citation_no = next_cite_no;
                                                    next_cite_no += 1;
                                                    result.push_str(&format!(
                                                        "[{}] ({} · {})\n{}\n\n",
                                                        ev.citation_no,
                                                        ev.source_title,
                                                        ev.section_path.as_deref().unwrap_or(""),
                                                        ev.content
                                                    ));
                                                    norm.add_valid(ev.citation_no);
                                                    evidence_map.insert(ev.citation_no, ev);
                                                }
                                            }
                                            result
                                        }
                                        Ok(_) => "该索引节点下暂无已提取文本分块；若是扫描版 PDF，请使用 inspect_source_pages 按该节点的页码范围视觉读取。".to_string(),
                                        Err(err) => {
                                            is_error = true;
                                            format!("读取索引节点失败：{}", err.message)
                                        }
                                    }
                                }
                            }
                            "inspect_source_structure" => {
                                let sid = args
                                    .get("source_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .trim();
                                match self
                                    .tool_inspect_source_structure(sid, &selected_sources)
                                    .await
                                {
                                    Ok(s) => s,
                                    Err(e) => {
                                        is_error = true;
                                        e
                                    }
                                }
                            }
                            "inspect_source_pages" => {
                                let sid = args
                                    .get("source_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .trim();
                                let pages = parse_page_numbers(args.get("pages"));
                                let focus = args
                                    .get("focus_question")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("请识别并总结这些页面的文字、图表与公式内容");
                                match self
                                    .tool_inspect_source_pages(
                                        notebook_id,
                                        sid,
                                        &pages,
                                        focus,
                                        &selected_sources,
                                        &mut evidence_map,
                                        &mut next_cite_no,
                                        &mut norm,
                                    )
                                    .await
                                {
                                    Ok(s) => s,
                                    Err(e) => {
                                        is_error = true;
                                        e
                                    }
                                }
                            }
                            "inspect_source_images" => {
                                let sid = args
                                    .get("source_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .trim();
                                let page = args.get("page").and_then(parse_single_page);
                                let image_ids: Vec<String> = args
                                    .get("image_ids")
                                    .and_then(|x| x.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(str::to_string))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let focus = args
                                    .get("focus_question")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("请提取并分析图片中的关键文字、图表数据与内容");
                                match self
                                    .tool_inspect_source_images(
                                        notebook_id,
                                        sid,
                                        page,
                                        &image_ids,
                                        focus,
                                        &selected_sources,
                                        &mut evidence_map,
                                        &mut next_cite_no,
                                        &mut norm,
                                    )
                                    .await
                                {
                                    Ok(s) => s,
                                    Err(e) => {
                                        is_error = true;
                                        e
                                    }
                                }
                            }
                            "generate_studio_artifact" => {
                                match self
                                    .tool_generate_studio_artifact(
                                        notebook_id,
                                        &asst_msg_id,
                                        &args,
                                        cancel.clone(),
                                    )
                                    .await
                                {
                                    Ok((step_title, summary)) => {
                                        generated_studio_artifact = true;
                                        custom_step_title = Some(step_title);
                                        summary
                                    }
                                    Err(err_msg) => {
                                        is_error = true;
                                        custom_step_title = Some("生成 Studio 成品失败".into());
                                        err_msg
                                    }
                                }
                            }
                            "get_knowledge_progress" => {
                                let sid = args.get("source_id").and_then(|x| x.as_str());
                                match self.studio.compute_kg_progress_summary(notebook_id, sid) {
                                    Ok(v) => {
                                        custom_step_title = Some("已查询知识点图谱理解进度".into());
                                        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
                                    }
                                    Err(err_msg) => {
                                        is_error = true;
                                        err_msg.message
                                    }
                                }
                            }
                            "build_source_knowledge_graph" => {
                                let sid = args.get("source_id").and_then(|x| x.as_str()).unwrap_or("").trim();
                                let detail = args.get("detail").and_then(|x| x.as_str()).unwrap_or("standard");
                                if sid.is_empty() {
                                    is_error = true;
                                    "缺少 source_id 参数。".to_string()
                                } else {
                                    let cfg = json!({
                                        "mode": "knowledge_graph",
                                        "source_id": sid,
                                        "detail": detail,
                                    });
                                    match self.studio.create(notebook_id, ArtifactType::Mindmap, None, cfg) {
                                        Ok(art) => {
                                            custom_step_title = Some("已开始构建材料知识点图谱".into());
                                            json!({ "ok": true, "artifact_id": art.id, "source_id": sid, "status": "PENDING" }).to_string()
                                        }
                                        Err(e) => {
                                            is_error = true;
                                            e.message
                                        }
                                    }
                                }
                            }
                            "report_thought_stage" => {
                                "已记录当前思考阶段概括。".to_string()
                            }
                            _ => {
                                is_error = true;
                                format!("不支持的只读工具：{name}")
                            }
                        };
                        let step = if name == "report_thought_stage" {
                            let stage_title = args
                                .get("title")
                                .and_then(|x| x.as_str())
                                .unwrap_or("思考阶段")
                                .trim()
                                .to_string();
                            let stage_summary = args
                                .get("summary")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            ThoughtStep {
                                step_id: new_ulid(),
                                kind: "thought_stage".into(),
                                title: stage_title,
                                detail: if stage_summary.is_empty() { None } else { Some(stage_summary) },
                                state: "done".into(),
                            }
                        } else {
                            ThoughtStep {
                                step_id: new_ulid(),
                                kind: match name.as_str() {
                                    "search_sources" => "tool_search".into(),
                                    "read_context"
                                    | "read_source"
                                    | "read_index_node"
                                    | "inspect_source_structure"
                                    | "inspect_source_pages"
                                    | "inspect_source_images" => "tool_read_context".into(),
                                    "list_sources" => "tool_list_sources".into(),
                                    "generate_studio_artifact" => "tool_studio".into(),
                                    _ => "tool_call".into(),
                                },
                                title: custom_step_title.unwrap_or_else(|| match name.as_str() {
                                    "search_sources" => "已搜索您的来源".into(),
                                    "read_context" => "已读取引用上下文".into(),
                                    "read_source" => "已读取来源正文".into(),
                                    "read_index_node" => "已读取目录节点正文".into(),
                                    "list_sources" => "已查看所选来源".into(),
                                    "inspect_source_structure" => "已查看文档视觉结构".into(),
                                    "inspect_source_pages" => "已视觉阅读指定页面".into(),
                                    "inspect_source_images" => "已分析文档内嵌图片".into(),
                                    _ => "已执行工具调用".into(),
                                }),
                                detail: args
                                    .get("query")
                                    .or_else(|| args.get("topic"))
                                    .and_then(|x| x.as_str())
                                    .map(|q| q.to_string()),
                                state: if is_error { "failed".into() } else { "done".into() },
                            }
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
                    if only_stage_reports && free_stage_rounds < MAX_FREE_STAGE_ROUNDS {
                        free_stage_rounds += 1;
                    } else {
                        round += 1;
                    }
                    continue;
                }
                Ok(reason) => {
                    if full_text.trim().is_empty() && !leaked_calls.is_empty() && !final_empty_retried {
                        final_empty_retried = true;
                        llm_messages.push(LlmMessage {
                            role: Role::User,
                            parts: vec![Part::Text(
                                "你刚才输出了工具调用标签，但当前已达工具调用上限，无法继续调用工具。请不要再输出任何 DSML、XML 或工具调用格式，直接基于目前已经读取到的内容用中文给出总结回答并标注 [n] 引用；如有未覆盖的页码范围，请直接说明。"
                                    .to_string(),
                            )],
                        });
                        continue;
                    }
                    let mut suppressed_missing_citation = generated_studio_artifact;
                    if full_text.trim().is_empty() && !leaked_calls.is_empty() {
                        suppressed_missing_citation = true;
                        let fallback = "本次多步查阅已达到工具调用上限，模型未能输出最终总结。请尝试缩小问题范围或直接指定具体章节/页码后重试。";
                        full_text = fallback.to_string();
                        self.emit(&topic, "chat.text_delta", json!({ "text": fallback }));
                    }
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

                    let (clean_text, parsed_stages) = extract_thought_stages(&full_text);
                    for (st_title, st_summary) in parsed_stages {
                        let st = ThoughtStep {
                            step_id: new_ulid(),
                            kind: "thought_stage".into(),
                            title: st_title,
                            detail: if st_summary.is_empty() { None } else { Some(st_summary) },
                            state: "done".into(),
                        };
                        self.emit(&topic, "chat.thought_step", serde_json::to_value(&st).unwrap_or_default());
                        thoughts.push(st);
                    }
                    full_text = clean_text;
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
                    if !suppressed_missing_citation
                        && full_text.trim().chars().count() > 20
                        && emitted_cites.is_empty()
                    {
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

    pub async fn refresh_notebook_summary(
        &self,
        notebook_id: &str,
        cancel: CancellationToken,
    ) -> LnaResult<()> {
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

        // Fingerprint of the ready source set: unchanged sources must not trigger another model call.
        let fingerprint = {
            use sha2::{Digest, Sha256};
            let mut parts: Vec<String> = ready
                .iter()
                .map(|s| format!("{}:{}", s.id, s.content_hash))
                .collect();
            parts.sort();
            hex::encode(Sha256::digest(parts.join("|").as_bytes()))
        };
        let current: Option<(Option<String>, Option<String>)> = self.db.read(|conn| {
            conn.query_row(
                "SELECT summary_fingerprint, summary_md FROM notebooks WHERE id=?1",
                [notebook_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(map_sql_err)
        })?;
        if let Some((Some(fp), Some(md))) = &current {
            if fp == &fingerprint && !md.is_empty() {
                return Ok(());
            }
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

        let sum_tid = format!("summary:{notebook_id}");
        self.emit(
            &format!("notebook:{notebook_id}"),
            "ai.activity",
            json!({
                "task_id": sum_tid,
                "notebook_id": notebook_id,
                "scope": "notebook_summary",
                "kind": "tool",
                "title": "AI 生成笔记本概览与推荐问题",
                "body": "正在综合已选来源的核心摘要，提炼主题并生成推荐问题…",
                "active": true,
            }),
        );
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
            reasoning: ReasoningOptions {
                show_summary: resolved.show_thinking,
                effort: resolved.effort.clone(),
            },
        };

        let mut thought_buf = String::new();
        let mut last_emit = std::time::Instant::now();
        let sum_res = self
            .llm
            .complete_text_with_thinking(&resolved, &req, cancel, |delta| {
                if delta.is_empty() { return; }
                thought_buf.push_str(delta);
                if last_emit.elapsed() >= std::time::Duration::from_millis(250) {
                    let preview: String = thought_buf.chars().rev().take(120).collect::<String>().chars().rev().collect();
                    self.emit(
                        &format!("notebook:{notebook_id}"),
                        "ai.activity",
                        json!({
                            "task_id": sum_tid,
                            "notebook_id": notebook_id,
                            "scope": "notebook_summary",
                            "kind": "reasoning",
                            "title": "AI 思考 · 笔记本概览",
                            "body": preview,
                            "active": true,
                        }),
                    );
                    last_emit = std::time::Instant::now();
                }
            })
            .await;
        self.emit(
            &format!("notebook:{notebook_id}"),
            "ai.activity",
            json!({
                "task_id": sum_tid,
                "notebook_id": notebook_id,
                "scope": "notebook_summary",
                "kind": "done",
                "title": "笔记本概览已更新",
                "body": "",
                "active": false,
            }),
        );
        let (raw, _) = sum_res?;
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
                     updated_at=?4, summary_fingerprint=?6 WHERE id=?5",
                    params![
                        summary_md,
                        sq,
                        title_sugg,
                        now_ms(),
                        notebook_id,
                        fingerprint
                    ],
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
            let fu_tid = format!("followups:{message_id}");
            service.emit(
                &format!("notebook:{notebook_id}"),
                "ai.activity",
                json!({
                    "task_id": fu_tid,
                    "notebook_id": notebook_id,
                    "scope": "chat_followups",
                    "kind": "stage",
                    "title": "AI 生成延伸追问建议",
                    "body": "正在结合本轮问答生成后续探索问题…",
                    "active": true,
                }),
            );
            let res_fu = service
                .llm
                .complete_text(&profile, &req, CancellationToken::new())
                .await;
            service.emit(
                &format!("notebook:{notebook_id}"),
                "ai.activity",
                json!({
                    "task_id": fu_tid,
                    "notebook_id": notebook_id,
                    "scope": "chat_followups",
                    "kind": "done",
                    "title": "延伸追问已就绪",
                    "body": "",
                    "active": false,
                }),
            );
            let Ok((raw, _)) = res_fu else {
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
        let mut msg = self.db.read(|conn| {
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
                            artifacts: Vec::new(),
                            created_at: r.get(11)?,
                        })
                    },
                )
                .optional()
                .map_err(map_sql_err)?
                .ok_or_else(|| LnaError::not_found(format!("Message not found: {msg_id}")))?;
            msg.citations = load_citations(conn, &msg.id)?;
            Ok(msg)
        })?;
        if msg.role == "assistant" {
            msg.artifacts = self.studio.list_by_message(&msg.id).unwrap_or_default();
        }
        Ok(msg)
    }

    async fn tool_generate_studio_artifact(
        &self,
        notebook_id: &str,
        message_id: &str,
        args: &serde_json::Value,
        _cancel: CancellationToken,
    ) -> Result<(String, String), String> {
        let kind_str = args
            .get("artifact_type")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let artifact_type = match kind_str.as_str() {
            "quiz" => ArtifactType::Quiz,
            "flashcards" => ArtifactType::Flashcards,
            "mindmap" => ArtifactType::Mindmap,
            "report" => ArtifactType::Report,
            "slides" => ArtifactType::Slides,
            "infographic" => ArtifactType::Infographic,
            "audio" => ArtifactType::Audio,
            _ => {
                return Err(format!(
                    "不支持的 artifact_type：{kind_str}。可选值为 quiz、flashcards、mindmap、report、slides、infographic、audio。"
                ));
            }
        };
        let kind_label = match artifact_type {
            ArtifactType::Quiz => "知识测验",
            ArtifactType::Flashcards => "学习闪卡",
            ArtifactType::Mindmap => "思维导图",
            ArtifactType::Report => "研究报告",
            ArtifactType::Slides => "演示文稿",
            ArtifactType::Infographic => "信息图",
            ArtifactType::Audio => "音频概览",
        };
        let title = args
            .get("title")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let parse_int_arg = |key: &str| -> Option<i64> {
            args.get(key).and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
            })
        };

        let mut cfg = serde_json::Map::new();
        if let Some(t) = args
            .get("topic")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            cfg.insert("topic".into(), json!(t));
        }
        if let Some(l) = args
            .get("lang")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            cfg.insert("lang".into(), json!(l));
        }

        match artifact_type {
            ArtifactType::Quiz => {
                let count = parse_int_arg("count").unwrap_or(5).clamp(3, 30);
                cfg.insert("count".into(), json!(count));
                let difficulty = args
                    .get("difficulty")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| matches!(*s, "auto" | "mixed" | "easy" | "medium" | "hard"))
                    .unwrap_or("auto");
                cfg.insert("difficulty".into(), json!(difficulty));

                let mut parsed_types: Vec<String> = Vec::new();
                if let Some(raw_types) = args.get("types") {
                    if let Some(arr) = raw_types.as_array() {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                let t = s.trim();
                                if matches!(t, "single" | "multi" | "fill" | "short") {
                                    parsed_types.push(t.to_string());
                                }
                            }
                        }
                    } else if let Some(s) = raw_types.as_str() {
                        let trimmed = s.trim();
                        if trimmed.starts_with('[') {
                            if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
                                for item in arr {
                                    let t = item.trim();
                                    if matches!(t, "single" | "multi" | "fill" | "short") {
                                        parsed_types.push(t.to_string());
                                    }
                                }
                            }
                        } else {
                            for part in trimmed.split([',', '，', ' ']) {
                                let t = part.trim();
                                if matches!(t, "single" | "multi" | "fill" | "short") {
                                    parsed_types.push(t.to_string());
                                }
                            }
                        }
                    }
                }
                if parsed_types.is_empty() {
                    parsed_types = vec!["single".into(), "multi".into()];
                }
                cfg.insert("types".into(), json!(parsed_types));
            }
            ArtifactType::Flashcards => {
                let count = parse_int_arg("count").unwrap_or(10).clamp(3, 40);
                cfg.insert("count".into(), json!(count));
            }
            ArtifactType::Report => {
                let fmt = args
                    .get("format")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| matches!(*s, "document" | "interactive"))
                    .unwrap_or("document");
                cfg.insert("format".into(), json!(fmt));
            }
            ArtifactType::Slides => {
                let pages = parse_int_arg("pages").unwrap_or(8).clamp(4, 30);
                cfg.insert("pages".into(), json!(pages));
                cfg.insert("reviewOutline".into(), json!(false));
            }
            ArtifactType::Infographic => {
                let ori = args
                    .get("orientation")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| matches!(*s, "landscape" | "portrait" | "square"))
                    .unwrap_or("landscape");
                cfg.insert("orientation".into(), json!(ori));
            }
            ArtifactType::Audio => {
                let fmt = args
                    .get("format")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| matches!(*s, "deep_dive" | "debate" | "brief" | "critique"))
                    .unwrap_or("deep_dive");
                cfg.insert("format".into(), json!(fmt));
            }
            ArtifactType::Mindmap => {}
        }

        let topic_ev = format!("chat:{notebook_id}");
        let art = self
            .studio
            .create_for_chat(notebook_id, message_id, artifact_type, title, json!(cfg))
            .map_err(|e| format!("创建{kind_label}失败：{}", e.message))?;
        self.emit(
            &topic_ev,
            "chat.artifact",
            json!({ "message_id": message_id, "artifact": art }),
        );

        let summary = format!(
            "已将{}《{}》（artifact_id: {}）加入后台生成队列并立即返回。对话中已展示实时进度卡片，用户可点击「查看过程」实时查看后台思考与生成进度；完成后可在对话内直接预览/作答或一键导入右侧 Studio。主对话无需等待完成，请用 1~2 句话简要告知用户已开始在后台生成即可，切勿在正文中写出完整题目、闪卡或长篇内容。",
            kind_label, art.title, art.id
        );
        Ok((format!("已启动生成{kind_label}"), summary))
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

fn format_sources_xml(
    db: &Database,
    evidences: &[RetrievedEvidence],
    grounded: bool,
    selected_files: &[(String, String, String)],
) -> String {
    let mut out = format!("<sources selected_file_count=\"{}\">\n", selected_files.len());
    if !selected_files.is_empty() {
        out.push_str("<selected_files>\n");
        for (id, title, kind) in selected_files {
            let src_opt = lna_storage::get_source(db, id).ok();
            let nodes = lna_storage::list_source_index_nodes(db, id).unwrap_or_default();
            let guide = src_opt
                .as_ref()
                .and_then(|s| s.guide_summary_md.as_deref())
                .unwrap_or("")
                .trim();
            if guide.is_empty() && nodes.is_empty() {
                out.push_str(&format!(
                    "<file id=\"{}\" title=\"{}\" kind=\"{}\" />\n",
                    xml_attr_escape(id),
                    xml_attr_escape(title),
                    xml_attr_escape(kind)
                ));
            } else {
                out.push_str(&format!(
                    "<file id=\"{}\" title=\"{}\" kind=\"{}\">\n",
                    xml_attr_escape(id),
                    xml_attr_escape(title),
                    xml_attr_escape(kind)
                ));
                if !guide.is_empty() {
                    out.push_str(&format!("<guide>{}</guide>\n", xml_text_escape(guide)));
                }
                if !nodes.is_empty() {
                    out.push_str("<toc>\n");
                    for n in nodes.iter().take(24) {
                        let pages = match (n.page_start, n.page_end) {
                            (Some(s), Some(e)) if e >= s => format!("p.{s}-{e}"),
                            (Some(s), _) => format!("p.{s}"),
                            _ => String::new(),
                        };
                        out.push_str(&format!(
                            "<node id=\"{}\" title=\"{}\" pages=\"{}\" />\n",
                            xml_attr_escape(&n.id),
                            xml_attr_escape(&n.title),
                            xml_attr_escape(&pages)
                        ));
                    }
                    out.push_str("</toc>\n");
                }
                out.push_str("</file>\n");
            }
        }
        out.push_str("</selected_files>\n");
    }
    if !grounded || evidences.is_empty() {
        out.push_str("(未检索到与问题直接相关的来源片段；请结合 <selected_files> 目录树调用 read_index_node / read_source / inspect_source_pages，或在确实无依据时告知用户)\n</sources>");
        return out;
    }
    for ev in evidences {
        let page = ev.page_start.map(|p| p.to_string()).unwrap_or_default();
        let section = xml_attr_escape(ev.section_path.as_deref().unwrap_or(""));
        let title = xml_attr_escape(&ev.source_title);
        let sid = xml_attr_escape(&ev.source_id);
        out.push_str(&format!(
            "<source n=\"{}\" source_id=\"{}\" title=\"{}\" page=\"{}\" section=\"{}\">\n{}\n</source>\n",
            ev.citation_no,
            sid,
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
            chunk_id: if ev.chunk_id > 0 { Some(ev.chunk_id) } else { None },
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

impl AgentService {
    async fn load_or_build_media_index(
        &self,
        src: &Source,
        bp: &std::path::Path,
    ) -> Option<lna_ingest::media::MediaIndex> {
        if let Some(existing) = lna_ingest::media::read_media_index(bp) {
            return Some(existing);
        }
        let kind = src.kind;
        let bp_clone = bp.to_path_buf();
        let sid = src.id.clone();
        match kind {
            SourceKind::Pdf => {
                if let Ok(Ok(ex)) =
                    tokio::task::spawn_blocking(move || lna_ingest::parser::parse_pdf_detailed(&bp_clone))
                        .await
                {
                    let idx = lna_ingest::media::MediaIndex {
                        source_id: sid,
                        kind: "pdf".into(),
                        page_count: ex.doc.page_count,
                        ocr_pages: ex.ocr_needed_pages,
                        pdf_pages: ex.page_meta,
                        embedded_images: vec![],
                    };
                    lna_ingest::media::write_media_index(bp, &idx);
                    return Some(idx);
                }
            }
            SourceKind::Docx
            | SourceKind::Pptx
            | SourceKind::Epub
            | SourceKind::Odt
            | SourceKind::Hwpx => {
                if let Ok(Ok(rich)) = tokio::task::spawn_blocking(move || match kind {
                    SourceKind::Docx => lna_ingest::office::parse_docx(&bp_clone),
                    SourceKind::Pptx => lna_ingest::office::parse_pptx(&bp_clone),
                    SourceKind::Epub => lna_ingest::office::parse_epub(&bp_clone),
                    SourceKind::Odt => lna_ingest::office::parse_odt(&bp_clone),
                    SourceKind::Hwpx => lna_ingest::office::parse_hwpx(&bp_clone),
                    _ => unreachable!(),
                })
                .await
                {
                    let idx = lna_ingest::media::MediaIndex {
                        source_id: sid,
                        kind: kind.as_str().into(),
                        page_count: rich.doc.page_count,
                        ocr_pages: vec![],
                        pdf_pages: vec![],
                        embedded_images: rich.embedded_images,
                    };
                    lna_ingest::media::write_media_index(bp, &idx);
                    return Some(idx);
                }
            }
            _ => {}
        }
        None
    }

    fn verify_selected_source(
        &self,
        source_id: &str,
        selected_sources: &[String],
    ) -> Result<(Source, std::path::PathBuf), String> {
        if source_id.is_empty() || !selected_sources.iter().any(|s| s == source_id) {
            return Err("指定的来源未选中或不存在。".to_string());
        }
        let src = lna_storage::get_source(&self.db, source_id)
            .map_err(|e| format!("读取来源失败：{}", e.message))?;
        let bp = src
            .blob_path
            .as_deref()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "来源缺少本地文件路径。".to_string())?;
        Ok((src, bp))
    }

    async fn tool_inspect_source_structure(
        &self,
        source_id: &str,
        selected_sources: &[String],
    ) -> Result<String, String> {
        let (src, bp) = self.verify_selected_source(source_id, selected_sources)?;
        let idx = self.load_or_build_media_index(&src, &bp).await;

        let chunk_count: i64 = self
            .db
            .read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM source_chunks WHERE source_id = ?1",
                    [&src.id],
                    |r| r.get(0),
                )
                .map_err(map_sql_err)
            })
            .unwrap_or(0);

        let mut lines = vec![format!(
            "来源：{} (ID: {}, 类型: {}, 已索引文本分块数: {})",
            src.title,
            src.id,
            src.kind.as_str(),
            chunk_count
        )];
        if let Some(m) = idx {
            if let Some(pc) = m.page_count {
                lines.push(format!("总页数/幻灯片数: {pc}"));
            }
            if !m.ocr_pages.is_empty() {
                let sample_ocr: Vec<String> = m
                    .ocr_pages
                    .iter()
                    .take(30)
                    .map(|p| p.to_string())
                    .collect();
                lines.push(format!(
                    "纯图片/需 OCR 页码 ({} 页): {}{}",
                    m.ocr_pages.len(),
                    sample_ocr.join(", "),
                    if m.ocr_pages.len() > 30 { " ..." } else { "" }
                ));
            }
            if !m.pdf_pages.is_empty() {
                let img_pages: Vec<String> = m
                    .pdf_pages
                    .iter()
                    .filter(|p| p.image_count > 0 || p.needs_ocr)
                    .take(40)
                    .map(|p| {
                        let sec = if p.section_path.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", p.section_path.join(" > "))
                        };
                        format!(
                            "第{}页(字符:{}, 图像:{}{})",
                            p.page,
                            p.char_count,
                            p.image_count,
                            if p.needs_ocr { ", 扫描/图页" } else { "" }
                        ) + &sec
                    })
                    .collect();
                if !img_pages.is_empty() {
                    lines.push(format!("含图像或扫描内容的页面:\n{}", img_pages.join("\n")));
                }
            }
            if !m.embedded_images.is_empty() {
                let img_list: Vec<String> = m
                    .embedded_images
                    .iter()
                    .take(40)
                    .map(|im| {
                        let pg = im
                            .page
                            .map(|p| format!("第{p}页"))
                            .unwrap_or_else(|| "文档内".into());
                        format!("{} ({pg}, {}, {})", im.id, im.zip_entry, im.mime_type)
                    })
                    .collect();
                lines.push(format!(
                    "内嵌图片 ({} 张):\n{}",
                    m.embedded_images.len(),
                    img_list.join("\n")
                ));
            }
        }
        Ok(lines.join("\n\n"))
    }

    #[allow(clippy::too_many_arguments)]
    async fn tool_inspect_source_pages(
        &self,
        notebook_id: &str,
        source_id: &str,
        pages: &[u32],
        focus_question: &str,
        selected_sources: &[String],
        evidence_map: &mut HashMap<i32, RetrievedEvidence>,
        next_cite_no: &mut i32,
        norm: &mut citations::CitationStreamNormalizer,
    ) -> Result<String, String> {
        let (src, bp) = self.verify_selected_source(source_id, selected_sources)?;
        if pages.is_empty() {
            return Err("请至少提供一个 1-based 页码。".to_string());
        }
        let mut clean_pages: Vec<u32> = pages.iter().copied().filter(|&p| p >= 1).collect();
        clean_pages.sort_unstable();
        clean_pages.dedup();
        if clean_pages.is_empty() {
            return Err("页码必须大于等于 1。".to_string());
        }
        let truncated_note = if clean_pages.len() > 5 {
            let skipped: Vec<String> = clean_pages[5..].iter().map(|p| p.to_string()).collect();
            let kept: Vec<String> = clean_pages[..5].iter().map(|p| p.to_string()).collect();
            clean_pages.truncate(5);
            format!(
                "（注意：单次最多视觉读取 5 页，本次已读取第 {} 页；其余第 {} 页未读取，如需继续查看请分批调用 inspect_source_pages。）\n\n",
                kept.join(","),
                skipped.join(",")
            )
        } else {
            String::new()
        };

        // Gather matching indexed text chunks for those pages
        let page_set: HashSet<i32> = clean_pages.iter().map(|&p| p as i32).collect();
        let all_chunks = self
            .retrieval
            .read_source_chunks(&src.id, 0, 200)
            .unwrap_or_default();
        let mut text_sections = Vec::new();
        for c in &all_chunks {
            let ps = c.page_start.unwrap_or(0);
            let pe = c.page_end.unwrap_or(ps);
            if ps > 0 && (ps..=pe).any(|p| page_set.contains(&p)) {
                text_sections.push(format!(
                    "[已提取第{}-{}页文字]\n{}",
                    ps, pe, c.content
                ));
            }
        }

        let visual_text = match src.kind {
            SourceKind::Pdf => {
                let bp_c = bp.clone();
                let pages_c = clean_pages.clone();
                let rendered = tokio::task::spawn_blocking(move || {
                    lna_ingest::parser::render_pdf_pages_to_jpeg(&bp_c, &pages_c, 1400)
                })
                .await
                .map_err(|e| format!("渲染 PDF 页面失败：{e}"))?
                .map_err(|e| format!("渲染 PDF 页面失败：{}", e.message))?;
                let payloads: Vec<(String, String, Vec<u8>)> = rendered
                    .into_iter()
                    .map(|(pno, jpg)| (format!("{} 第 {pno} 页", src.title), "jpg".into(), jpg))
                    .collect();
                lna_ingest::media::vision_inspect_images(
                    &self.db,
                    &self.secrets,
                    &self.llm,
                    notebook_id,
                    payloads,
                    focus_question,
                )
                .await
                .map_err(|e| format!("视觉读取页面失败：{}", e.message))?
            }
            SourceKind::Image => {
                let raw = std::fs::read(&bp).map_err(|e| format!("读取图片失败：{e}"))?;
                let ext = bp
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_string();
                lna_ingest::media::vision_inspect_images(
                    &self.db,
                    &self.secrets,
                    &self.llm,
                    notebook_id,
                    vec![(src.title.clone(), ext, raw)],
                    focus_question,
                )
                .await
                .map_err(|e| format!("视觉分析图片失败：{}", e.message))?
            }
            SourceKind::Pptx
            | SourceKind::Docx
            | SourceKind::Epub
            | SourceKind::Odt
            | SourceKind::Hwpx => {
                let idx = self.load_or_build_media_index(&src, &bp).await;
                let mut payloads = Vec::new();
                if let Some(m) = idx {
                    let any_paged = m.embedded_images.iter().any(|x| x.page.is_some());
                    for im in &m.embedded_images {
                        if let Some(ipg) = im.page {
                            if !clean_pages.contains(&ipg) {
                                continue;
                            }
                        } else if any_paged {
                            continue;
                        }
                        if let Ok(bytes) =
                            lna_ingest::office::extract_zip_embedded_image(&bp, &im.zip_entry)
                        {
                            let im_ext = im
                                .zip_entry
                                .rsplit('.')
                                .next()
                                .unwrap_or("png")
                                .to_string();
                            let label = format!(
                                "{} {} ({})",
                                src.title,
                                im.page
                                    .map(|p| format!("第{p}页"))
                                    .unwrap_or_else(|| "内嵌图片".into()),
                                im.id
                            );
                            payloads.push((label, im_ext, bytes));
                            if payloads.len() >= 5 {
                                break;
                            }
                        }
                    }
                }
                if payloads.is_empty() {
                    String::new()
                } else {
                    lna_ingest::media::vision_inspect_images(
                        &self.db,
                        &self.secrets,
                        &self.llm,
                        notebook_id,
                        payloads,
                        focus_question,
                    )
                    .await
                    .map_err(|e| format!("视觉读取嵌入图片失败：{}", e.message))?
                }
            }
            _ => String::new(),
        };

        let mut combined = String::new();
        if !visual_text.is_empty() {
            // Permanently cache visually-read PDF pages into local index so future questions spend 0 vision tokens
            if src.kind == SourceKind::Pdf {
                if let Some(&first_page) = clean_pages.first() {
                    let _ = lna_ingest::ai_indexer::cache_visual_page_result(
                        &self.db,
                        &src.id,
                        notebook_id,
                        first_page,
                        &visual_text,
                    );
                }
            }
            combined.push_str(&format!("[视觉读取结果]\n{visual_text}\n\n"));
        }
        if !text_sections.is_empty() {
            combined.push_str(&text_sections.join("\n\n"));
        }
        if combined.is_empty() {
            return Ok("指定页面暂无可提取的文字或视觉内容。".to_string());
        }

        let cite_no = *next_cite_no;
        *next_cite_no += 1;
        let ev = RetrievedEvidence {
            citation_no: cite_no,
            chunk_id: -(cite_no as i64),
            source_id: src.id.clone(),
            source_title: src.title.clone(),
            chunk_index: 0,
            page_start: clean_pages.first().map(|&p| p as i32),
            page_end: clean_pages.last().map(|&p| p as i32),
            section_path: Some(format!(
                "第 {} 页",
                clean_pages
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            content: combined.clone(),
            token_count: 0,
            rrf_score: 1.0,
        };
        norm.add_valid(cite_no);
        evidence_map.insert(cite_no, ev);

        Ok(format!("{truncated_note}[{cite_no}] ({})\n{combined}", src.title))
    }

    #[allow(clippy::too_many_arguments)]
    async fn tool_inspect_source_images(
        &self,
        notebook_id: &str,
        source_id: &str,
        page: Option<u32>,
        image_ids: &[String],
        focus_question: &str,
        selected_sources: &[String],
        evidence_map: &mut HashMap<i32, RetrievedEvidence>,
        next_cite_no: &mut i32,
        norm: &mut citations::CitationStreamNormalizer,
    ) -> Result<String, String> {
        let (src, bp) = self.verify_selected_source(source_id, selected_sources)?;
        let mut payloads: Vec<(String, String, Vec<u8>)> = Vec::new();
        let mut used_page = page;

        match src.kind {
            SourceKind::Image => {
                let raw = std::fs::read(&bp).map_err(|e| format!("读取图片失败：{e}"))?;
                let ext = bp
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_string();
                payloads.push((src.title.clone(), ext, raw));
                used_page = Some(1);
            }
            SourceKind::Pdf => {
                let target_page = page.unwrap_or(1).max(1);
                used_page = Some(target_page);
                let bp_c = bp.clone();
                let imgs = tokio::task::spawn_blocking(move || {
                    lna_ingest::parser::extract_pdf_page_images(&bp_c, target_page, 5)
                })
                .await
                .map_err(|e| format!("提取 PDF 图片失败：{e}"))?
                .map_err(|e| format!("提取 PDF 图片失败：{}", e.message))?;

                if imgs.is_empty() {
                    // Fallback to full page render in case chart/diagram is vector graphics or Form XObject
                    let bp_r = bp.clone();
                    let rendered = tokio::task::spawn_blocking(move || {
                        lna_ingest::parser::render_pdf_pages_to_jpeg(&bp_r, &[target_page], 1600)
                    })
                    .await
                    .map_err(|e| format!("渲染 PDF 页面失败：{e}"))?
                    .map_err(|e| format!("渲染 PDF 页面失败：{}", e.message))?;
                    for (pno, jpg) in rendered {
                        payloads.push((
                            format!("{} 第 {pno} 页整页图像", src.title),
                            "jpg".into(),
                            jpg,
                        ));
                    }
                } else {
                    for (idx, png) in imgs {
                        payloads.push((
                            format!("{} 第 {target_page} 页图 {idx}", src.title),
                            "png".into(),
                            png,
                        ));
                    }
                }
            }
            SourceKind::Docx
            | SourceKind::Pptx
            | SourceKind::Epub
            | SourceKind::Odt
            | SourceKind::Hwpx => {
                let idx = self
                    .load_or_build_media_index(&src, &bp)
                    .await
                    .ok_or_else(|| "无法读取文档视觉索引。".to_string())?;
                for im in &idx.embedded_images {
                    if !image_ids.is_empty() && !image_ids.contains(&im.id) {
                        continue;
                    }
                    if let Some(pg) = page {
                        if im.page.is_some() && im.page != Some(pg) {
                            continue;
                        }
                    }
                    if let Ok(bytes) =
                        lna_ingest::office::extract_zip_embedded_image(&bp, &im.zip_entry)
                    {
                        let ext = im
                            .zip_entry
                            .rsplit('.')
                            .next()
                            .unwrap_or("png")
                            .to_string();
                        if used_page.is_none() {
                            used_page = im.page;
                        }
                        let label = format!("{} {} ({})", src.title, im.id, im.zip_entry);
                        payloads.push((label, ext, bytes));
                        if payloads.len() >= 5 {
                            break;
                        }
                    }
                }
            }
            _ => {
                return Err("该类型来源不包含可单独提取的图片。".to_string());
            }
        }

        if payloads.is_empty() {
            return Ok("未在指定位置找到可提取的图片。".to_string());
        }

        let visual_text = lna_ingest::media::vision_inspect_images(
            &self.db,
            &self.secrets,
            &self.llm,
            notebook_id,
            payloads,
            focus_question,
        )
        .await
        .map_err(|e| format!("视觉分析图片失败：{}", e.message))?;

        let cite_no = *next_cite_no;
        *next_cite_no += 1;
        let ev = RetrievedEvidence {
            citation_no: cite_no,
            chunk_id: -(cite_no as i64),
            source_id: src.id.clone(),
            source_title: src.title.clone(),
            chunk_index: 0,
            page_start: used_page.map(|p| p as i32),
            page_end: used_page.map(|p| p as i32),
            section_path: Some("视觉图像分析".into()),
            content: visual_text.clone(),
            token_count: 0,
            rrf_score: 1.0,
        };
        norm.add_valid(cite_no);
        evidence_map.insert(cite_no, ev);

        Ok(format!("[{cite_no}] ({})\n{visual_text}", src.title))
    }
}


/// Returns true when the user asks a broad overview/summary question about the selected sources
/// rather than a narrow factual lookup.
pub fn is_overview_question(text: &str) -> bool {
    let lower = text.to_lowercase();
    const CUES: &[&str] = &[
        "讲了什么",
        "讲的是什么",
        "说了什么",
        "写了什么",
        "什么内容",
        "内容是什么",
        "主要内容",
        "主题是什么",
        "总结",
        "概括",
        "概述",
        "摘要",
        "大纲",
        "目录",
        "导读",
        "介绍一下",
        "概览",
        "summarize",
        "summary",
        "overview",
        "what is this",
        "what does the",
        "what is in",
        "main points",
        "key takeaways",
        "about this",
    ];
    CUES.iter().any(|cue| lower.contains(cue))
}

/// Detects when a user message explicitly requests generating a Studio artifact (e.g., quiz,
/// flashcards, mindmap, slides, infographic, audio overview, or study report) so the model can be
/// nudged to call `generate_studio_artifact` instead of manually writing out items in plain text.
pub fn detect_studio_artifact_hint(user_text: &str) -> Option<(&'static str, &'static str)> {
    let lower = user_text.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }

    const OPT_OUT: &[&str] = &[
        "不要生成卡片",
        "不用卡片",
        "不要卡片",
        "不要studio",
        "不用studio",
        "纯文本",
        "只用文字",
        "直接在对话里",
        "plain text only",
        "no card",
    ];
    if OPT_OUT.iter().any(|cue| lower.contains(cue)) {
        return None;
    }

    const ACTION_CUES: &[&str] = &[
        "生成",
        "出一",
        "出几",
        "出个",
        "出些",
        "出点",
        "出题",
        "出卷",
        "做个",
        "做一份",
        "做套",
        "来个",
        "来一份",
        "来套",
        "给我",
        "帮我",
        "创建",
        "制作",
        "准备",
        "整理一份",
        "考考",
        "考我",
        "自测",
        "小测",
        "刷题",
        "测一下",
        "测测",
        "generate",
        "create",
        "make",
        "build",
        "give me",
        "quiz me",
    ];
    let has_action = ACTION_CUES.iter().any(|cue| lower.contains(cue));

    const DIRECT_QUIZ_CUES: &[&str] = &[
        "考考我",
        "自测",
        "刷题",
        "出几道题",
        "出题",
        "quiz me",
    ];
    const QUIZ_NOUNS: &[&str] = &[
        "测验",
        "测试",
        "练习题",
        "习题",
        "考题",
        "选择题",
        "填空题",
        "测试题",
        "测验题",
        "小测",
        "题目",
        "quiz",
        "practice test",
        "mock test",
    ];
    if DIRECT_QUIZ_CUES.iter().any(|cue| lower.contains(cue))
        || (has_action && QUIZ_NOUNS.iter().any(|cue| lower.contains(cue)))
    {
        return Some(("quiz", "知识测验"));
    }

    if !has_action {
        return None;
    }

    const FLASHCARD_NOUNS: &[&str] = &["闪卡", "记忆卡", "抽认卡", "背诵卡", "flashcard"];
    if FLASHCARD_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("flashcards", "学习闪卡"));
    }

    const MINDMAP_NOUNS: &[&str] = &["思维导图", "脑图", "知识图谱", "mindmap", "mind map"];
    if MINDMAP_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("mindmap", "思维导图"));
    }

    const SLIDES_NOUNS: &[&str] = &["ppt", "幻灯片", "演示文稿", "slides", "presentation"];
    if SLIDES_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("slides", "演示文稿"));
    }

    const INFOGRAPHIC_NOUNS: &[&str] = &["信息图", "一图读懂", "infographic"];
    if INFOGRAPHIC_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("infographic", "信息图"));
    }

    const AUDIO_NOUNS: &[&str] = &["音频概览", "播客", "语音讲解", "audio overview", "podcast"];
    if AUDIO_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("audio", "音频概览"));
    }

    const REPORT_NOUNS: &[&str] = &[
        "研究报告",
        "学习指南",
        "复习报告",
        "总结报告",
        "study guide",
        "research report",
    ];
    if REPORT_NOUNS.iter().any(|cue| lower.contains(cue)) {
        return Some(("report", "研究报告"));
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeakedToolClose {
    DsmlCalls,
    ToolCallTag,
    FunctionCallsTag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TagFilterState {
    Normal,
    InHiddenBlock { close_tag: &'static str },
    InStageBody { title: String },
    InLeakedToolBlock { body: String, close_kind: LeakedToolClose },
}

/// Streaming filter that strips `<think>`, `<thought>`, `<thinking>`, `<thought_stage>`,
/// and leaked tool-call markup (`< | | DSML | | calls>`, `<｜DSML｜calls>`, `<tool_call>`,
/// `<function_calls>`) in real time during token streaming so internal reasoning or raw tool XML
/// never appears in the chat message, while emitting parsed `<thought_stage>` items live and
/// capturing leaked tool calls for recovery.
pub struct ThoughtTagStreamFilter {
    buf: String,
    state: TagFilterState,
    leaked_tool_blocks: Vec<String>,
}

impl ThoughtTagStreamFilter {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            state: TagFilterState::Normal,
            leaked_tool_blocks: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &str) -> (String, Vec<(String, String)>) {
        self.buf.push_str(chunk);
        self.process(false)
    }

    pub fn finish(&mut self) -> (String, Vec<(String, String)>) {
        self.process(true)
    }

    pub fn take_leaked_tool_calls(&mut self) -> Vec<(String, String, serde_json::Value)> {
        let blocks = std::mem::take(&mut self.leaked_tool_blocks);
        let mut calls = Vec::new();
        for block in blocks {
            calls.extend(parse_leaked_tool_calls(&block, calls.len()));
        }
        calls
    }

    fn process(&mut self, eof: bool) -> (String, Vec<(String, String)>) {
        let mut out = String::new();
        let mut stages = Vec::new();

        loop {
            match &mut self.state {
                TagFilterState::Normal => {
                    let Some(lt) = self.buf.find('<') else {
                        out.push_str(&self.buf);
                        self.buf.clear();
                        break;
                    };
                    if lt > 0 {
                        out.push_str(&self.buf[..lt]);
                        self.buf.drain(..lt);
                    }
                    // Now self.buf starts with '<'
                    let lower = self.buf.to_ascii_lowercase();
                    const HIDDEN_TAGS: &[(&str, &str)] = &[
                        ("<think>", "</think>"),
                        ("<thought>", "</thought>"),
                        ("<thinking>", "</thinking>"),
                    ];
                    let mut matched_hidden = false;
                    for (open, close) in HIDDEN_TAGS {
                        if lower.starts_with(open) {
                            self.buf.drain(..open.len());
                            self.state = TagFilterState::InHiddenBlock { close_tag: close };
                            matched_hidden = true;
                            break;
                        }
                    }
                    if matched_hidden {
                        continue;
                    }

                    // Stray closing tags (just in case)
                    const STRAY_CLOSES: &[&str] = &[
                        "</think>",
                        "</thought>",
                        "</thinking>",
                        "</thought_stage>",
                    ];
                    let mut matched_stray = false;
                    for close in STRAY_CLOSES {
                        if lower.starts_with(close) {
                            self.buf.drain(..close.len());
                            matched_stray = true;
                            break;
                        }
                    }
                    if matched_stray {
                        continue;
                    }

                    // Check <thought_stage ...>
                    if lower.starts_with("<thought_stage") {
                        let next_char = lower.as_bytes().get("<thought_stage".len()).copied();
                        if next_char.is_none() {
                            if eof {
                                self.buf.clear();
                            }
                            break;
                        }
                        if matches!(next_char, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')) {
                            if let Some(gt) = self.buf.find('>') {
                                let header = &self.buf[..gt];
                                let title = parse_thought_stage_title(header);
                                self.buf.drain(..gt + 1);
                                self.state = TagFilterState::InStageBody { title };
                                continue;
                            } else if !eof && self.buf.len() < 512 {
                                break;
                            } else {
                                // Malformed unclosed <thought_stage; drop it
                                self.buf.clear();
                                break;
                            }
                        }
                    }

                    // Check leaked tool-call open tags (<tool_call>, <function_calls>, < | | DSML | | calls>)
                    if lower.starts_with("<tool_call>") {
                        self.buf.drain(.."<tool_call>".len());
                        self.state = TagFilterState::InLeakedToolBlock {
                            body: String::new(),
                            close_kind: LeakedToolClose::ToolCallTag,
                        };
                        continue;
                    }
                    if lower.starts_with("<function_calls>") {
                        self.buf.drain(.."<function_calls>".len());
                        self.state = TagFilterState::InLeakedToolBlock {
                            body: String::new(),
                            close_kind: LeakedToolClose::FunctionCallsTag,
                        };
                        continue;
                    }
                    if let Some(end_idx) = match_dsml_calls_tag(&self.buf, false) {
                        self.buf.drain(..end_idx);
                        self.state = TagFilterState::InLeakedToolBlock {
                            body: String::new(),
                            close_kind: LeakedToolClose::DsmlCalls,
                        };
                        continue;
                    }

                    // Check if `lower` is still a strict prefix of any recognized tag
                    const ALL_PREFIXES: &[&str] = &[
                        "<think>",
                        "<thought>",
                        "<thinking>",
                        "<thought_stage>",
                        "<tool_call>",
                        "<function_calls>",
                        "</think>",
                        "</thought>",
                        "</thinking>",
                        "</thought_stage>",
                        "</tool_call>",
                        "</function_calls>",
                    ];
                    let is_prefix = ALL_PREFIXES.iter().any(|tag| tag.starts_with(&lower))
                        || is_dsml_open_prefix(&self.buf);
                    if is_prefix && !eof {
                        break;
                    }
                    if is_prefix && eof {
                        // Incomplete trailing `<thought` / `<think` / `< | | DSML` tag at EOF: drop rather than leak
                        self.buf.clear();
                        break;
                    }

                    // Not a thought tag (e.g. `<br>` or `a < b`): emit '<' and continue
                    out.push('<');
                    self.buf.drain(..1);
                }
                TagFilterState::InHiddenBlock { close_tag } => {
                    let close_tag = *close_tag;
                    let lower = self.buf.to_ascii_lowercase();
                    if let Some(idx) = lower.find(close_tag) {
                        self.buf.drain(..idx + close_tag.len());
                        self.state = TagFilterState::Normal;
                        continue;
                    }
                    if eof {
                        self.buf.clear();
                        break;
                    }
                    let keep = close_tag.len().saturating_sub(1);
                    if self.buf.len() > keep {
                        let mut cut = self.buf.len() - keep;
                        while cut > 0 && !self.buf.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        if cut > 0 {
                            self.buf.drain(..cut);
                        }
                    }
                    break;
                }
                TagFilterState::InStageBody { title } => {
                    let lower = self.buf.to_ascii_lowercase();
                    let close = "</thought_stage>";
                    if let Some(idx) = lower.find(close) {
                        let summary = self.buf[..idx].trim().to_string();
                        stages.push((title.clone(), summary));
                        self.buf.drain(..idx + close.len());
                        self.state = TagFilterState::Normal;
                        continue;
                    }
                    if eof {
                        let summary = self.buf.trim().to_string();
                        if !summary.is_empty() {
                            stages.push((title.clone(), summary));
                        }
                        self.buf.clear();
                        self.state = TagFilterState::Normal;
                    }
                    break;
                }
                TagFilterState::InLeakedToolBlock { body, close_kind } => {
                    let close_kind = *close_kind;
                    let lower = self.buf.to_ascii_lowercase();
                    let closed = match close_kind {
                        LeakedToolClose::ToolCallTag => {
                            lower.find("</tool_call>").map(|i| (i, i + "</tool_call>".len()))
                        }
                        LeakedToolClose::FunctionCallsTag => lower
                            .find("</function_calls>")
                            .map(|i| (i, i + "</function_calls>".len())),
                        LeakedToolClose::DsmlCalls => find_dsml_calls_close(&self.buf),
                    };
                    if let Some((start_idx, end_idx)) = closed {
                        body.push_str(&self.buf[..start_idx]);
                        let captured = std::mem::take(body);
                        if !captured.trim().is_empty() {
                            self.leaked_tool_blocks.push(captured);
                        }
                        self.buf.drain(..end_idx);
                        self.state = TagFilterState::Normal;
                        continue;
                    }
                    if eof {
                        body.push_str(&self.buf);
                        let captured = std::mem::take(body);
                        if !captured.trim().is_empty() {
                            self.leaked_tool_blocks.push(captured);
                        }
                        self.buf.clear();
                        self.state = TagFilterState::Normal;
                        break;
                    }
                    let keep = 48usize;
                    if self.buf.len() > keep {
                        let mut cut = self.buf.len() - keep;
                        while cut > 0 && !self.buf.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        if cut > 0 {
                            body.push_str(&self.buf[..cut]);
                            self.buf.drain(..cut);
                        }
                    }
                    break;
                }
            }
        }

        (out, stages)
    }
}

fn parse_single_page(val: &serde_json::Value) -> Option<u32> {
    if let Some(n) = val.as_u64() {
        return u32::try_from(n).ok().filter(|&p| p >= 1);
    }
    if let Some(s) = val.as_str() {
        return s.trim().parse::<u32>().ok().filter(|&p| p >= 1);
    }
    None
}

fn push_page_token(tok: &str, out: &mut Vec<u32>) {
    let t = tok.trim();
    if t.is_empty() {
        return;
    }
    if let Some((a, b)) = t.split_once('-') {
        if let (Ok(start), Ok(end)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
            if start >= 1 && end >= start && (end - start) <= 200 {
                out.extend(start..=end);
                return;
            }
        }
    }
    if let Ok(n) = t.parse::<u32>() {
        if n >= 1 {
            out.push(n);
        }
    }
}

fn parse_page_numbers(val: Option<&serde_json::Value>) -> Vec<u32> {
    let Some(val) = val else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match val {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(n) = item.as_u64() {
                    if let Ok(p) = u32::try_from(n) {
                        if p >= 1 {
                            out.push(p);
                        }
                    }
                } else if let Some(s) = item.as_str() {
                    push_page_token(s, &mut out);
                }
            }
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.starts_with('[') {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    return parse_page_numbers(Some(&parsed));
                }
            }
            for part in trimmed.split([',', '，', ' ']) {
                push_page_token(part, &mut out);
            }
        }
        serde_json::Value::Number(n) => {
            if let Some(p) = n.as_u64().and_then(|v| u32::try_from(v).ok()) {
                if p >= 1 {
                    out.push(p);
                }
            }
        }
        _ => {}
    }
    out
}

/// Strips spaces and vertical bars (`|`, `｜`) from a DSML tag header and lowercases it,
/// e.g. `"< | | DSML | | calls>"` -> `"<dsmlcalls>"`, `"</ | | DSML | | invoke>"` -> `"</dsmlinvoke>"`.
fn normalize_dsml_tag(tag_slice: &str) -> String {
    let mut out = String::with_capacity(tag_slice.len());
    for ch in tag_slice.chars() {
        if ch.is_whitespace() || ch == '|' || ch == '｜' {
            continue;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn match_dsml_calls_tag(s: &str, closing: bool) -> Option<usize> {
    if !s.starts_with('<') {
        return None;
    }
    let gt = s.find('>')?;
    if gt > 64 {
        return None;
    }
    let norm = normalize_dsml_tag(&s[..=gt]);
    let ok = if closing {
        norm == "</dsmlcalls>" || norm == "</dsmlfunction_calls>"
    } else {
        norm == "<dsmlcalls>" || norm == "<dsmlfunction_calls>"
    };
    if ok { Some(gt + 1) } else { None }
}

fn is_dsml_open_prefix(s: &str) -> bool {
    if !s.starts_with('<') || s.len() > 48 || s.contains('>') {
        return false;
    }
    let norm = normalize_dsml_tag(s);
    "<dsmlcalls>".starts_with(&norm) || "<dsmlfunction_calls>".starts_with(&norm)
}

fn find_dsml_calls_close(s: &str) -> Option<(usize, usize)> {
    let mut idx = 0;
    while let Some(rel) = s[idx..].find("</") {
        let pos = idx + rel;
        if let Some(len) = match_dsml_calls_tag(&s[pos..], true) {
            return Some((pos, pos + len));
        }
        idx = pos + 2;
    }
    None
}

fn extract_xml_attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let dq = format!("{key}=\"");
    if let Some(pos) = attrs.find(&dq) {
        let rest = &attrs[pos + dq.len()..];
        if let Some(end) = rest.find('"') {
            return Some(&rest[..end]);
        }
    }
    let sq = format!("{key}='");
    if let Some(pos) = attrs.find(&sq) {
        let rest = &attrs[pos + sq.len()..];
        if let Some(end) = rest.find('\'') {
            return Some(&rest[..end]);
        }
    }
    None
}

fn parse_leaked_tool_calls(block: &str, base_idx: usize) -> Vec<(String, String, serde_json::Value)> {
    let trimmed = block.trim();
    // 1. Try JSON `<tool_call>{"name": ..., "arguments": ...}</tool_call>`
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(name) = v.get("name").and_then(|x| x.as_str()) {
                let args = v
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                return vec![(
                    format!("recovered_{}", base_idx + 1),
                    name.to_string(),
                    args,
                )];
            }
        }
    }

    // 2. Parse DSML `<...invoke name="..."> ... <...parameter name="..." ...>VAL</...parameter> ... </...invoke>`
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel_lt) = block[pos..].find('<') {
        let tag_start = pos + rel_lt;
        let Some(rel_gt) = block[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + rel_gt + 1;
        let tag_header = &block[tag_start..tag_end];
        let norm_header = normalize_dsml_tag(tag_header);
        if norm_header.starts_with("<dsmlinvoke") || norm_header.starts_with("<invoke") {
            let tool_name = extract_xml_attr(tag_header, "name")
                .unwrap_or("")
                .trim()
                .to_string();
            // Find matching invoke close tag
            let mut invoke_end = block.len();
            let mut body_end = block.len();
            let mut scan = tag_end;
            while let Some(c_rel) = block[scan..].find("</") {
                let c_start = scan + c_rel;
                if let Some(c_gt) = block[c_start..].find('>') {
                    let c_end = c_start + c_gt + 1;
                    let norm_close = normalize_dsml_tag(&block[c_start..c_end]);
                    if norm_close == "</dsmlinvoke>" || norm_close == "</invoke>" {
                        body_end = c_start;
                        invoke_end = c_end;
                        break;
                    }
                    scan = c_end;
                } else {
                    break;
                }
            }
            let invoke_body = &block[tag_end..body_end];
            let mut args_map = serde_json::Map::new();
            let mut p_pos = 0usize;
            while let Some(p_rel) = invoke_body[p_pos..].find('<') {
                let p_start = p_pos + p_rel;
                let Some(p_gt) = invoke_body[p_start..].find('>') else {
                    break;
                };
                let p_head_end = p_start + p_gt + 1;
                let p_head = &invoke_body[p_start..p_head_end];
                let norm_p = normalize_dsml_tag(p_head);
                if norm_p.starts_with("<dsmlparameter") || norm_p.starts_with("<parameter") {
                    let p_name = extract_xml_attr(p_head, "name").unwrap_or("").trim();
                    let is_string = extract_xml_attr(p_head, "string")
                        .map(|v| v.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                    let mut val_end = invoke_body.len();
                    let mut next_p_pos = invoke_body.len();
                    let mut c_scan = p_head_end;
                    while let Some(pc_rel) = invoke_body[c_scan..].find("</") {
                        let pc_start = c_scan + pc_rel;
                        if let Some(pc_gt) = invoke_body[pc_start..].find('>') {
                            let pc_end = pc_start + pc_gt + 1;
                            let norm_pc = normalize_dsml_tag(&invoke_body[pc_start..pc_end]);
                            if norm_pc == "</dsmlparameter>" || norm_pc == "</parameter>" {
                                val_end = pc_start;
                                next_p_pos = pc_end;
                                break;
                            }
                            c_scan = pc_end;
                        } else {
                            break;
                        }
                    }
                    let raw_val = invoke_body[p_head_end..val_end].trim();
                    if !p_name.is_empty() {
                        let parsed_val = if is_string {
                            serde_json::Value::String(raw_val.to_string())
                        } else {
                            serde_json::from_str::<serde_json::Value>(raw_val)
                                .unwrap_or_else(|_| serde_json::Value::String(raw_val.to_string()))
                        };
                        args_map.insert(p_name.to_string(), parsed_val);
                    }
                    p_pos = next_p_pos;
                } else {
                    p_pos = p_head_end;
                }
            }
            if !tool_name.is_empty() {
                out.push((
                    format!("recovered_{}", base_idx + out.len() + 1),
                    tool_name,
                    serde_json::Value::Object(args_map),
                ));
            }
            pos = invoke_end;
        } else {
            pos = tag_end;
        }
    }
    out
}

fn parse_thought_stage_title(header: &str) -> String {
    header
        .split("title=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .or_else(|| {
            header
                .split("title='")
                .nth(1)
                .and_then(|s| s.split('\'').next())
        })
        .unwrap_or("思考阶段")
        .trim()
        .to_string()
}

/// Extracts `<thought_stage title="...">summary</thought_stage>` blocks from text
/// (fallback for models without tool calling), returning cleaned text and parsed stages.
pub fn extract_thought_stages(raw: &str) -> (String, Vec<(String, String)>) {
    let mut filter = ThoughtTagStreamFilter::new();
    let (mut cleaned, mut stages) = filter.push(raw);
    let (tail, tail_stages) = filter.finish();
    cleaned.push_str(&tail);
    stages.extend(tail_stages);
    (cleaned.trim_start().to_string(), stages)
}

#[cfg(test)]
mod thought_filter_tests {
    use super::*;

    #[test]
    fn strips_thought_and_stage_tags_even_in_one_char_chunks() {
        let input = "<thought>internal cot</thought>\n<thought_stage title=\"分析问题\">梳理了核心要点</thought_stage>\n\n正文开始 x < y 且 <thinking>hidden</thinking> 结束。";
        let mut f = ThoughtTagStreamFilter::new();
        let mut vis = String::new();
        let mut stages = Vec::new();
        for ch in input.chars() {
            let mut s = String::new();
            s.push(ch);
            let (v, st) = f.push(&s);
            vis.push_str(&v);
            stages.extend(st);
        }
        let (tail, tail_st) = f.finish();
        vis.push_str(&tail);
        stages.extend(tail_st);

        assert_eq!(stages, vec![("分析问题".to_string(), "梳理了核心要点".to_string())]);
        assert!(!vis.contains("<thought"));
        assert!(!vis.contains("<thinking"));
        assert!(!vis.contains("internal cot"));
        assert!(!vis.contains("hidden"));
        assert!(vis.contains("正文开始 x < y 且  结束。"));
    }

    #[test]
    fn detects_overview_questions() {
        assert!(is_overview_question("测试：来源中的文件讲了什么？"));
        assert!(is_overview_question("请总结一下这份讲义的主要内容"));
        assert!(!is_overview_question("矩阵的秩是如何定义的？"));
    }

    #[test]
    fn strips_and_recovers_leaked_dsml_tool_calls_and_string_pages() {
        let input = "前置说明。\n< | | DSML | | calls>\n< | | DSML | | invoke name=\"inspect_source_pages\">\n< | | DSML | | parameter name=\"pages\" string=\"false\">[\"220\", \"221\", \"222\"]</ | | DSML | | parameter>\n< | | DSML | | parameter name=\"source_id\" string=\"true\">01M7ZANXFXCEH2P1ZSMPMK160P</ | | DSML | | parameter>\n</ | | DSML | | invoke>\n</ | | DSML | | calls>\n";
        let mut f = ThoughtTagStreamFilter::new();
        let mut vis = String::new();
        for ch in input.chars() {
            let mut s = String::new();
            s.push(ch);
            let (v, _) = f.push(&s);
            vis.push_str(&v);
        }
        let (tail, _) = f.finish();
        vis.push_str(&tail);

        assert!(!vis.contains("DSML"));
        assert!(!vis.contains("inspect_source_pages"));
        assert_eq!(vis.trim(), "前置说明。");

        let recovered = f.take_leaked_tool_calls();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].1, "inspect_source_pages");
        assert_eq!(
            recovered[0].2.get("source_id").and_then(|x| x.as_str()),
            Some("01M7ZANXFXCEH2P1ZSMPMK160P")
        );
        let pages = parse_page_numbers(recovered[0].2.get("pages"));
        assert_eq!(pages, vec![220, 221, 222]);
    }

    #[test]
    fn detects_studio_artifact_hints_accurately() {
        assert_eq!(
            detect_studio_artifact_hint("根据第4章给我出一份5道题的测验"),
            Some(("quiz", "知识测验"))
        );
        assert_eq!(
            detect_studio_artifact_hint("Quiz me on orthogonal projections"),
            Some(("quiz", "知识测验"))
        );
        assert_eq!(
            detect_studio_artifact_hint("帮我制作10张线性代数闪卡"),
            Some(("flashcards", "学习闪卡"))
        );
        assert_eq!(
            detect_studio_artifact_hint("生成一个本章知识点的思维导图"),
            Some(("mindmap", "思维导图"))
        );
        assert_eq!(
            detect_studio_artifact_hint("Create slides covering least squares"),
            Some(("slides", "演示文稿"))
        );
        assert_eq!(
            detect_studio_artifact_hint("帮我整理一份第4章学习指南"),
            Some(("report", "研究报告"))
        );

        assert_eq!(detect_studio_artifact_hint("这本书的测验答案在哪一页？"), None);
        assert_eq!(detect_studio_artifact_hint("解释一下正交投影"), None);
        assert_eq!(detect_studio_artifact_hint("测试：来源中的文件讲了什么？"), None);
        assert_eq!(
            detect_studio_artifact_hint("只用文字给我出3道题，不要生成卡片"),
            None
        );
    }
}
