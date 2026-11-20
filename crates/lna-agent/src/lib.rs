pub mod citations;

use lna_llm::*;
use lna_retrieval::{RetrievalEngine, RetrievedEvidence};
use lna_secrets::SecretStore;
use lna_storage::{db::map_sql_err, Database};
use lna_types::*;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const CHAT_SYSTEM_PROMPT: &str = include_str!("../../../prompts/chat_system_v1.md");
#[allow(dead_code)]
const QUERY_REWRITE_PROMPT: &str = include_str!("../../../prompts/query_rewrite_v1.md");
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
                return Err(LnaError::new(
                    ErrorCode::ModelNotConfigured,
                    "No active model profile configured",
                ));
            }
        };
        let resolved = self.resolve_profile(&profile)?;

        let mut thoughts = Vec::new();
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

        // REWRITE (optional)
        let queries = vec![user_text.to_string()];

        // RETRIEVE
        let outcome = self
            .retrieval
            .search(notebook_id, &queries, &selected_sources, evidence_budget, 8)
            .await?;

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
        let history_msgs = self.load_recent_history(notebook_id, &user_msg_id, history_budget)?;
        let sources_xml = if outcome.grounded && !outcome.evidences.is_empty() {
            let mut s = String::from("<sources>\n");
            for ev in &outcome.evidences {
                let page_attr = ev.page_start.map(|p| p.to_string()).unwrap_or_default();
                let sec_attr = ev.section_path.as_deref().unwrap_or("");
                s.push_str(&format!(
                    "<source n=\"{}\" title=\"{}\" page=\"{}\" section=\"{}\">\n{}\n</source>\n",
                    ev.citation_no, ev.source_title, page_attr, sec_attr, ev.content
                ));
            }
            s.push_str("</sources>");
            s
        } else {
            "<sources>\n(未检索到与问题直接相关的来源片段；请明确告知用户在已选来源中未找到依据)\n</sources>".to_string()
        };

        let mut llm_messages = history_msgs;
        llm_messages.push(LlmMessage {
            role: Role::User,
            parts: vec![Part::Text(format!(
                "{sources_xml}\n<question>{user_text}</question>"
            ))],
        });

        let tools = if resolved.capabilities.tools {
            vec![ToolDef {
                name: "search_sources".into(),
                description: "在已选来源中补充检索关键词".into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
            }]
        } else {
            vec![]
        };

        let mut full_text = String::new();
        let mut _thought_buf = String::new();
        let mut usage_total = Usage::default();
        let valid_nos: HashSet<i32> = evidence_map.keys().copied().collect();
        let mut norm = citations::CitationStreamNormalizer::new(valid_nos);
        let mut emitted_cites: HashSet<i32> = HashSet::new();
        let mut next_cite_no = (evidence_map.len() as i32) + 1;

        // GENERATE + TOOL_LOOP (max 3 rounds)
        for round in 0..=3 {
            let req = LlmRequest {
                system: CHAT_SYSTEM_PROMPT.to_string(),
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

            let mut pending_calls: HashMap<u32, (String, String, String)> = HashMap::new();
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
                        let q = args
                            .get("query")
                            .and_then(|x| x.as_str())
                            .unwrap_or(user_text);
                        let extra = self
                            .retrieval
                            .search(notebook_id, &[q.to_string()], &selected_sources, 1600, 4)
                            .await?;
                        let mut res_txt = String::new();
                        for mut ev in extra.evidences {
                            ev.citation_no = next_cite_no;
                            next_cite_no += 1;
                            res_txt.push_str(&format!("[{}] {}\n", ev.citation_no, ev.content));
                            norm.add_valid(ev.citation_no);
                            evidence_map.insert(ev.citation_no, ev);
                        }
                        tool_results.push(Part::ToolResult {
                            call_id: cid,
                            content: res_txt,
                            is_error: false,
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

                    let final_cites: Vec<CitationRef> = emitted_cites
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
                        .collect();

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
                    self.emit(
                        &topic,
                        "chat.finished",
                        json!({ "status": "COMPLETE", "stop_reason": format!("{reason:?}") }),
                    );
                    return self.get_message(&asst_msg_id);
                }
                Err(e) => {
                    let status = if e.code == ErrorCode::Cancelled {
                        "CANCELLED"
                    } else if !full_text.is_empty() {
                        "INCOMPLETE"
                    } else {
                        "FAILED"
                    };
                    self.finalize_message(
                        &asst_msg_id,
                        status,
                        &full_text,
                        &thoughts,
                        &[],
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

        self.finalize_message(
            &asst_msg_id,
            "INCOMPLETE",
            &full_text,
            &thoughts,
            &[],
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

    fn load_recent_history(
        &self,
        notebook_id: &str,
        exclude_id: &str,
        _budget: usize,
    ) -> LnaResult<Vec<LlmMessage>> {
        self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM chat_messages
                     WHERE notebook_id = ?1 AND id != ?2 AND status = 'COMPLETE'
                     ORDER BY created_at DESC LIMIT 8",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map(params![notebook_id, exclude_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(map_sql_err)?;
            let mut msgs = Vec::new();
            for r in rows {
                let (role, content) = r.map_err(map_sql_err)?;
                msgs.push(LlmMessage {
                    role: if role == "user" {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    parts: vec![Part::Text(content)],
                });
            }
            msgs.reverse();
            Ok(msgs)
        })
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
