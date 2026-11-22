use crate::db::map_sql_err;
use crate::Database;
use lna_types::*;
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeMap;

pub fn list_notebooks(db: &Database) -> LnaResult<Vec<Notebook>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT n.id, n.title, n.emoji, n.summary_md, n.summary_fingerprint,
                        n.suggested_questions_json, n.title_user_edited, n.created_at, n.updated_at,
                        (SELECT COUNT(*) FROM sources s WHERE s.notebook_id = n.id) AS source_count
                 FROM notebooks n
                 ORDER BY n.updated_at DESC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |r| {
                let sq_json: Option<String> = r.get(5)?;
                let suggested_questions = sq_json
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                Ok(Notebook {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    emoji: r.get(2)?,
                    summary_md: r.get(3)?,
                    summary_fingerprint: r.get(4)?,
                    suggested_questions,
                    title_user_edited: r.get::<_, i64>(6)? == 1,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    source_count: r.get(9)?,
                })
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(map_sql_err)?);
        }
        Ok(out)
    })
}

pub fn get_notebook(db: &Database, id: &str) -> LnaResult<Notebook> {
    db.read(|conn| {
        conn.query_row(
            "SELECT n.id, n.title, n.emoji, n.summary_md, n.summary_fingerprint,
                    n.suggested_questions_json, n.title_user_edited, n.created_at, n.updated_at,
                    (SELECT COUNT(*) FROM sources s WHERE s.notebook_id = n.id) AS source_count
             FROM notebooks n WHERE n.id = ?1",
            [id],
            |r| {
                let sq_json: Option<String> = r.get(5)?;
                let suggested_questions = sq_json
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                Ok(Notebook {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    emoji: r.get(2)?,
                    summary_md: r.get(3)?,
                    summary_fingerprint: r.get(4)?,
                    suggested_questions,
                    title_user_edited: r.get::<_, i64>(6)? == 1,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    source_count: r.get(9)?,
                })
            },
        )
        .optional()
        .map_err(map_sql_err)?
        .ok_or_else(|| LnaError::not_found(format!("Notebook not found: {id}")))
    })
}

pub fn create_notebook(db: &Database, title: &str, emoji: Option<&str>) -> LnaResult<Notebook> {
    let id = new_ulid();
    let now = now_ms();
    let emoji = emoji.unwrap_or("");
    db.write(|conn| {
        conn.execute(
            "INSERT INTO notebooks (id, title, emoji, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, emoji, now, now],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })?;
    get_notebook(db, &id)
}

pub fn rename_notebook(db: &Database, id: &str, title: &str, user_edited: bool) -> LnaResult<()> {
    let now = now_ms();
    db.write(|conn| {
        let changed = conn
            .execute(
                "UPDATE notebooks SET title = ?1, title_user_edited = ?2, updated_at = ?3 WHERE id = ?4",
                params![title, if user_edited { 1 } else { 0 }, now, id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!("Notebook not found: {id}")));
        }
        Ok(())
    })
}

pub fn delete_notebook(db: &Database, id: &str) -> LnaResult<()> {
    let vec_tables: Vec<String> = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT vec_table FROM embedding_models")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    let artifact_dirs: Vec<String> = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT output_dir FROM studio_artifacts WHERE notebook_id = ?1 AND output_dir IS NOT NULL")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([id], |r| r.get::<_, String>(0))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        for tbl in &vec_tables {
            let sql = format!(
                "DELETE FROM {tbl} WHERE chunk_id IN (SELECT id FROM source_chunks WHERE notebook_id = ?1)"
            );
            let _ = tx.execute(&sql, [id]);
        }
        let changed = tx
            .execute("DELETE FROM notebooks WHERE id = ?1", [id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!("Notebook not found: {id}")));
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    })?;

    let nb_blobs = db.paths().blobs_dir.join(id);
    if nb_blobs.exists() {
        let _ = std::fs::remove_dir_all(nb_blobs);
    }
    for d in artifact_dirs {
        let p = std::path::Path::new(&d);
        if p.exists() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
    Ok(())
}

pub fn list_sources(db: &Database, notebook_id: &str) -> LnaResult<Vec<Source>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, notebook_id, title, kind, origin_uri, blob_path, content_hash,
                        byte_size, page_count, token_count, language, is_selected, status,
                        error_code, error_detail, guide_summary_md, guide_topics_json, ai_indexed,
                        created_at, updated_at
                 FROM sources WHERE notebook_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], map_source_row)
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })
}

pub fn get_source(db: &Database, source_id: &str) -> LnaResult<Source> {
    db.read(|conn| {
        conn.query_row(
            "SELECT id, notebook_id, title, kind, origin_uri, blob_path, content_hash,
                    byte_size, page_count, token_count, language, is_selected, status,
                    error_code, error_detail, guide_summary_md, guide_topics_json, ai_indexed,
                    created_at, updated_at
             FROM sources WHERE id = ?1",
            [source_id],
            map_source_row,
        )
        .optional()
        .map_err(map_sql_err)?
        .ok_or_else(|| LnaError::not_found(format!("Source not found: {source_id}")))
    })
}

fn map_source_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Source> {
    let kind_s: String = r.get(3)?;
    let kind = SourceKind::from_db_str(&kind_s);
    let status_s: String = r.get(12)?;
    let status = match status_s.as_str() {
        "QUEUED" => SourceStatus::Queued,
        "PARSING" => SourceStatus::Parsing,
        "CHUNKING" => SourceStatus::Chunking,
        "EMBEDDING" => SourceStatus::Embedding,
        "READY" => SourceStatus::Ready,
        _ => SourceStatus::Failed,
    };
    let topics_json: Option<String> = r.get(16)?;
    let guide_topics = topics_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(Source {
        id: r.get(0)?,
        notebook_id: r.get(1)?,
        title: r.get(2)?,
        kind,
        origin_uri: r.get(4)?,
        blob_path: r.get(5)?,
        content_hash: r.get(6)?,
        byte_size: r.get(7)?,
        page_count: r.get(8)?,
        token_count: r.get(9)?,
        language: r.get(10)?,
        is_selected: r.get::<_, i64>(11)? == 1,
        status,
        error_code: r.get(13)?,
        error_detail: r.get(14)?,
        guide_summary_md: r.get(15)?,
        guide_topics,
        ai_indexed: r.get::<_, i64>(17)? == 1,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
    })
}

pub fn update_source_guide(
    db: &Database,
    source_id: &str,
    guide_summary_md: Option<&str>,
    guide_topics: &[String],
    ai_indexed: bool,
) -> LnaResult<()> {
    let topics_json = serde_json::to_string(guide_topics).unwrap_or_else(|_| "[]".into());
    db.write(|conn| {
        conn.execute(
            "UPDATE sources
             SET guide_summary_md = ?1, guide_topics_json = ?2, ai_indexed = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                guide_summary_md,
                topics_json,
                if ai_indexed { 1 } else { 0 },
                now_ms(),
                source_id
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn replace_source_index_nodes(
    db: &Database,
    source_id: &str,
    nodes: &[SourceIndexNode],
) -> LnaResult<()> {
    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        tx.execute(
            "DELETE FROM source_index_nodes WHERE source_id = ?1",
            [source_id],
        )
        .map_err(map_sql_err)?;
        for n in nodes {
            let kw_json = serde_json::to_string(&n.keywords).unwrap_or_else(|_| "[]".into());
            tx.execute(
                "INSERT INTO source_index_nodes
                 (id, source_id, parent_id, ordinal, level, title, summary, keywords_json,
                  page_start, page_end, char_start, char_end)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    n.id,
                    source_id,
                    n.parent_id,
                    n.ordinal,
                    n.level,
                    n.title,
                    n.summary,
                    kw_json,
                    n.page_start,
                    n.page_end,
                    n.char_start,
                    n.char_end
                ],
            )
            .map_err(map_sql_err)?;
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn list_source_index_nodes(db: &Database, source_id: &str) -> LnaResult<Vec<SourceIndexNode>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, source_id, parent_id, ordinal, level, title, summary, keywords_json,
                        page_start, page_end, char_start, char_end
                 FROM source_index_nodes
                 WHERE source_id = ?1
                 ORDER BY ordinal ASC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([source_id], |r| {
                let kw_s: String = r.get(7)?;
                let keywords = serde_json::from_str::<Vec<String>>(&kw_s).unwrap_or_default();
                Ok(SourceIndexNode {
                    id: r.get(0)?,
                    source_id: r.get(1)?,
                    parent_id: r.get(2)?,
                    ordinal: r.get(3)?,
                    level: r.get(4)?,
                    title: r.get(5)?,
                    summary: r.get(6)?,
                    keywords,
                    page_start: r.get(8)?,
                    page_end: r.get(9)?,
                    char_start: r.get(10)?,
                    char_end: r.get(11)?,
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

pub fn get_source_index_node(
    db: &Database,
    source_id: &str,
    node_id: &str,
) -> LnaResult<Option<SourceIndexNode>> {
    db.read(|conn| {
        conn.query_row(
            "SELECT id, source_id, parent_id, ordinal, level, title, summary, keywords_json,
                    page_start, page_end, char_start, char_end
             FROM source_index_nodes
             WHERE source_id = ?1 AND id = ?2",
            params![source_id, node_id],
            |r| {
                let kw_s: String = r.get(7)?;
                let keywords = serde_json::from_str::<Vec<String>>(&kw_s).unwrap_or_default();
                Ok(SourceIndexNode {
                    id: r.get(0)?,
                    source_id: r.get(1)?,
                    parent_id: r.get(2)?,
                    ordinal: r.get(3)?,
                    level: r.get(4)?,
                    title: r.get(5)?,
                    summary: r.get(6)?,
                    keywords,
                    page_start: r.get(8)?,
                    page_end: r.get(9)?,
                    char_start: r.get(10)?,
                    char_end: r.get(11)?,
                })
            },
        )
        .optional()
        .map_err(map_sql_err)
    })
}

pub fn set_source_selected(db: &Database, source_id: &str, selected: bool) -> LnaResult<()> {
    db.write(|conn| {
        let changed = conn
            .execute(
                "UPDATE sources SET is_selected = ?1, updated_at = ?2 WHERE id = ?3",
                params![if selected { 1 } else { 0 }, now_ms(), source_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!(
                "Source not found: {source_id}"
            )));
        }
        Ok(())
    })
}

pub fn set_all_sources_selected(db: &Database, notebook_id: &str, selected: bool) -> LnaResult<()> {
    db.write(|conn| {
        conn.execute(
            "UPDATE sources SET is_selected = ?1, updated_at = ?2 WHERE notebook_id = ?3",
            params![if selected { 1 } else { 0 }, now_ms(), notebook_id],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn rename_source(db: &Database, source_id: &str, title: &str) -> LnaResult<()> {
    db.write(|conn| {
        let changed = conn
            .execute(
                "UPDATE sources SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, now_ms(), source_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!(
                "Source not found: {source_id}"
            )));
        }
        Ok(())
    })
}

pub fn remove_source(db: &Database, source_id: &str) -> LnaResult<()> {
    let src = get_source(db, source_id)?;
    let vec_tables: Vec<String> = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT vec_table FROM embedding_models")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        for tbl in &vec_tables {
            let sql = format!(
                "DELETE FROM {tbl} WHERE chunk_id IN (SELECT id FROM source_chunks WHERE source_id = ?1)"
            );
            let _ = tx.execute(&sql, [source_id]);
        }
        tx.execute("DELETE FROM sources WHERE id = ?1", [source_id])
            .map_err(map_sql_err)?;
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    })?;

    if let Some(bp) = src.blob_path {
        let p = std::path::Path::new(&bp);
        if let Some(parent) = p.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
    Ok(())
}

pub fn list_profiles(db: &Database) -> LnaResult<Vec<ModelProfile>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, display_name, protocol, base_url, model_id, keychain_ref,
                        show_thinking, effort, context_window, max_output_tokens,
                        capabilities_json, extra_headers_json, token_ratio,
                        input_price_per_m, output_price_per_m, is_active, created_at, updated_at
                 FROM model_profiles ORDER BY updated_at DESC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt.query_map([], map_profile_row).map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })
}

pub fn get_active_profile(db: &Database) -> LnaResult<Option<ModelProfile>> {
    db.read(|conn| {
        conn.query_row(
            "SELECT id, display_name, protocol, base_url, model_id, keychain_ref,
                    show_thinking, effort, context_window, max_output_tokens,
                    capabilities_json, extra_headers_json, token_ratio,
                    input_price_per_m, output_price_per_m, is_active, created_at, updated_at
             FROM model_profiles WHERE is_active = 1 LIMIT 1",
            [],
            map_profile_row,
        )
        .optional()
        .map_err(map_sql_err)
    })
}

pub fn get_profile(db: &Database, id: &str) -> LnaResult<ModelProfile> {
    db.read(|conn| {
        conn.query_row(
            "SELECT id, display_name, protocol, base_url, model_id, keychain_ref,
                    show_thinking, effort, context_window, max_output_tokens,
                    capabilities_json, extra_headers_json, token_ratio,
                    input_price_per_m, output_price_per_m, is_active, created_at, updated_at
             FROM model_profiles WHERE id = ?1",
            [id],
            map_profile_row,
        )
        .optional()
        .map_err(map_sql_err)?
        .ok_or_else(|| LnaError::not_found(format!("Profile not found: {id}")))
    })
}

fn map_profile_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ModelProfile> {
    let proto_s: String = r.get(2)?;
    let protocol = match proto_s.as_str() {
        "responses" => ProtocolKind::Responses,
        "anthropic" => ProtocolKind::Anthropic,
        "gemini" => ProtocolKind::Gemini,
        _ => ProtocolKind::ChatCompletion,
    };
    let caps_json: String = r.get(10)?;
    let hdrs_json: String = r.get(11)?;
    let keychain_ref: Option<String> = r.get(5)?;
    Ok(ModelProfile {
        id: r.get(0)?,
        display_name: r.get(1)?,
        protocol,
        base_url: r.get(3)?,
        model_id: r.get(4)?,
        has_secret: false,
        keychain_ref,
        show_thinking: r.get::<_, i64>(6)? == 1,
        effort: r.get(7)?,
        context_window: r.get::<_, i64>(8)? as u32,
        max_output_tokens: r.get::<_, i64>(9)? as u32,
        capabilities: serde_json::from_str(&caps_json).unwrap_or_default(),
        extra_headers: serde_json::from_str::<BTreeMap<String, String>>(&hdrs_json)
            .unwrap_or_default(),
        token_ratio: r.get(12)?,
        input_price_per_m: r.get(13)?,
        output_price_per_m: r.get(14)?,
        is_active: r.get::<_, i64>(15)? == 1,
        created_at: r.get(16)?,
        updated_at: r.get(17)?,
    })
}

pub fn upsert_profile(db: &Database, params_in: &ProfileUpsertParams) -> LnaResult<ModelProfile> {
    let now = now_ms();
    let id = params_in.id.clone().unwrap_or_else(new_ulid);
    let extra_json = serde_json::to_string(&params_in.extra_headers)
        .map_err(|e| LnaError::invalid(e.to_string()))?;

    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        if params_in.activate {
            tx.execute("UPDATE model_profiles SET is_active = 0 WHERE is_active = 1", [])
                .map_err(map_sql_err)?;
        }
        let existing_caps_str: Option<String> = tx
            .query_row(
                "SELECT capabilities_json FROM model_profiles WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)?;
        let exists = existing_caps_str.is_some();

        let has_modality_patch = params_in.supports_vision.is_some()
            || params_in.supports_audio.is_some()
            || params_in.supports_video.is_some();

        let merged_caps_json: Option<String> = if let Some(ref c) = params_in.capabilities {
            let mut merged = c.clone();
            if let Some(v) = params_in.supports_vision {
                merged.supports_vision = v;
            }
            if let Some(v) = params_in.supports_audio {
                merged.supports_audio = v;
            }
            if let Some(v) = params_in.supports_video {
                merged.supports_video = v;
            }
            Some(serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into()))
        } else if has_modality_patch || !exists {
            let mut merged: ModelCapabilities = existing_caps_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            if let Some(v) = params_in.supports_vision {
                merged.supports_vision = v;
            }
            if let Some(v) = params_in.supports_audio {
                merged.supports_audio = v;
            }
            if let Some(v) = params_in.supports_video {
                merged.supports_video = v;
            }
            Some(serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into()))
        } else {
            None
        };

        if exists {
            tx.execute(
                "UPDATE model_profiles SET display_name=?1, protocol=?2, base_url=?3, model_id=?4,
                 show_thinking=?5, effort=?6, context_window=?7, max_output_tokens=?8,
                 extra_headers_json=?9, input_price_per_m=?10, output_price_per_m=?11,
                 capabilities_json=COALESCE(?15, capabilities_json), is_active=CASE WHEN ?12=1 THEN 1 ELSE is_active END, updated_at=?13
                 WHERE id=?14",
                params![
                    params_in.display_name,
                    params_in.protocol.as_str(),
                    params_in.base_url,
                    params_in.model_id,
                    if params_in.show_thinking { 1 } else { 0 },
                    params_in.effort,
                    params_in.context_window as i64,
                    params_in.max_output_tokens as i64,
                    extra_json,
                    params_in.input_price_per_m,
                    params_in.output_price_per_m,
                    if params_in.activate { 1 } else { 0 },
                    now,
                    id,
                    merged_caps_json,
                ],
            )
            .map_err(map_sql_err)?;
        } else {
            let init_caps = merged_caps_json.as_deref().unwrap_or("{}");
            tx.execute(
                "INSERT INTO model_profiles
                 (id, display_name, protocol, base_url, model_id, keychain_ref, show_thinking,
                  effort, context_window, max_output_tokens, capabilities_json, extra_headers_json,
                  input_price_per_m, output_price_per_m, is_active, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    id,
                    params_in.display_name,
                    params_in.protocol.as_str(),
                    params_in.base_url,
                    params_in.model_id,
                    format!("profile_{id}"),
                    if params_in.show_thinking { 1 } else { 0 },
                    params_in.effort,
                    params_in.context_window as i64,
                    params_in.max_output_tokens as i64,
                    init_caps,
                    extra_json,
                    params_in.input_price_per_m,
                    params_in.output_price_per_m,
                    if params_in.activate { 1 } else { 0 },
                    now,
                    now,
                ],
            )
            .map_err(map_sql_err)?;
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    })?;
    get_profile(db, &id)
}

pub fn activate_profile(db: &Database, id: &str) -> LnaResult<()> {
    db.write(|conn| {
        let tx = conn.transaction().map_err(map_sql_err)?;
        tx.execute(
            "UPDATE model_profiles SET is_active = 0 WHERE is_active = 1",
            [],
        )
        .map_err(map_sql_err)?;
        let changed = tx
            .execute(
                "UPDATE model_profiles SET is_active = 1, updated_at = ?1 WHERE id = ?2",
                params![now_ms(), id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!("Profile not found: {id}")));
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn delete_profile(db: &Database, id: &str) -> LnaResult<()> {
    db.write(|conn| {
        let changed = conn
            .execute("DELETE FROM model_profiles WHERE id = ?1", [id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(LnaError::not_found(format!("Profile not found: {id}")));
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
pub fn record_usage(
    db: &Database,
    notebook_id: Option<&str>,
    profile_id: Option<&str>,
    purpose: &str,
    subject_id: Option<&str>,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
    is_estimated: bool,
) -> LnaResult<()> {
    db.write(|conn| {
        conn.execute(
            "INSERT INTO usage_events
             (id, notebook_id, profile_id, purpose, subject_id, input_tokens, output_tokens, cached_tokens, is_estimated, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                new_ulid(),
                notebook_id,
                profile_id,
                purpose,
                subject_id,
                input_tokens,
                output_tokens,
                cached_tokens,
                if is_estimated { 1 } else { 0 },
                now_ms()
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn get_usage_summary(db: &Database, notebook_id: Option<&str>) -> LnaResult<UsageTotals> {
    db.read(|conn| {
        let (inp, out, cached): (i64, i64, i64) = match notebook_id {
            Some(nid) => conn
                .query_row(
                    "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cached_tokens),0)
                     FROM usage_events WHERE notebook_id = ?1",
                    [nid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(map_sql_err)?,
            None => conn
                .query_row(
                    "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cached_tokens),0)
                     FROM usage_events",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(map_sql_err)?,
        };
        let (cost, priced_rows): (f64, i64) = match notebook_id {
            Some(nid) => conn.query_row(
                "SELECT COALESCE(SUM((u.input_tokens * p.input_price_per_m + u.output_tokens * p.output_price_per_m) / 1000000.0),0),
                        COALESCE(SUM(CASE WHEN p.input_price_per_m IS NOT NULL AND p.output_price_per_m IS NOT NULL THEN 1 ELSE 0 END),0)
                 FROM usage_events u JOIN model_profiles p ON p.id=u.profile_id WHERE u.notebook_id=?1",
                [nid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(map_sql_err)?,
            None => conn.query_row(
                "SELECT COALESCE(SUM((u.input_tokens * p.input_price_per_m + u.output_tokens * p.output_price_per_m) / 1000000.0),0),
                        COALESCE(SUM(CASE WHEN p.input_price_per_m IS NOT NULL AND p.output_price_per_m IS NOT NULL THEN 1 ELSE 0 END),0)
                 FROM usage_events u JOIN model_profiles p ON p.id=u.profile_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(map_sql_err)?,
        };
        Ok(UsageTotals {
            input_tokens: inp,
            output_tokens: out,
            cached_tokens: cached,
            estimated_cost_usd: if priced_rows > 0 { Some(cost) } else { None },
        })
    })
}

pub fn update_profile_capabilities(
    db: &Database,
    id: &str,
    caps: &ModelCapabilities,
) -> LnaResult<()> {
    let caps_json = serde_json::to_string(caps).map_err(|e| LnaError::invalid(e.to_string()))?;
    db.write(|conn| {
        conn.execute(
            "UPDATE model_profiles SET capabilities_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![caps_json, now_ms(), id],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })
}

pub fn get_setting_json(db: &Database, key: &str) -> LnaResult<Option<serde_json::Value>> {
    let raw: Option<String> = db.read(|conn| {
        conn.query_row(
            "SELECT value_json FROM app_settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sql_err)
    })?;
    Ok(raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()))
}

pub fn set_setting_json(db: &Database, key: &str, value: &serde_json::Value) -> LnaResult<()> {
    let val = value.to_string();
    db.write(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO app_settings (key, value_json) VALUES (?1, ?2)",
            params![key, val],
        )
        .map_err(map_sql_err)?;
        Ok(())
    })
}

/// Selects a model profile supporting the requested modality (`"vision" | "audio" | "video"`).
/// Priority: active profile (if it supports the modality) -> most recently updated profile that supports it.
pub fn resolve_modality_profile(db: &Database, modality: &str) -> LnaResult<Option<ModelProfile>> {
    let profiles = list_profiles(db)?;
    let matches_modality = |p: &ModelProfile| -> bool {
        match modality {
            "vision" => p.capabilities.supports_vision,
            "audio" => p.capabilities.supports_audio,
            "video" => p.capabilities.supports_video,
            _ => false,
        }
    };
    if let Some(active) = profiles.iter().find(|p| p.is_active && matches_modality(p)) {
        return Ok(Some(active.clone()));
    }
    Ok(profiles.into_iter().find(matches_modality))
}
