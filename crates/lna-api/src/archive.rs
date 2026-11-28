use lna_jobs::JobQueue;
use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, now_ms, ErrorCode, LnaError, LnaResult};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

const MAX_IMPORT_ENTRIES: usize = 2000;
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

/// Export notebook to .lna ZIP (manifest.json, notebook.json, original blobs, studio artifacts; NO API keys or vectors, F2).
pub fn export_notebook_lna(db: &Database, notebook_id: &str, out_zip: &Path) -> LnaResult<()> {
    let nb = lna_storage::get_notebook(db, notebook_id)?;
    let sources = lna_storage::list_sources(db, notebook_id)?;
    let notes = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, title, body_md, origin, citations_json, created_at, updated_at FROM notes WHERE notebook_id = ?1")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                    "body_md": r.get::<_, String>(2)?,
                    "origin": r.get::<_, String>(3)?,
                    "citations_json": r.get::<_, Option<String>>(4)?,
                    "created_at": r.get::<_, i64>(5)?,
                    "updated_at": r.get::<_, i64>(6)?,
                }))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    let artifacts = db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, artifact_type, title, status, stage, progress, config_json,
                        source_ids_json, sources_fingerprint, draft_json, payload_json,
                        output_dir, created_at, updated_at, finished_at
                 FROM studio_artifacts WHERE notebook_id = ?1",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "artifact_type": r.get::<_, String>(1)?,
                    "title": r.get::<_, String>(2)?,
                    "status": r.get::<_, String>(3)?,
                    "stage": r.get::<_, Option<String>>(4)?,
                    "progress": r.get::<_, f64>(5)?,
                    "config_json": r.get::<_, String>(6)?,
                    "source_ids_json": r.get::<_, String>(7)?,
                    "sources_fingerprint": r.get::<_, String>(8)?,
                    "draft_json": r.get::<_, Option<String>>(9)?,
                    "payload_json": r.get::<_, Option<String>>(10)?,
                    "output_dir": r.get::<_, Option<String>>(11)?,
                    "created_at": r.get::<_, i64>(12)?,
                    "updated_at": r.get::<_, i64>(13)?,
                    "finished_at": r.get::<_, Option<i64>>(14)?,
                }))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    if let Some(p) = out_zip.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let f = std::fs::File::create(out_zip).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();

    let manifest = json!({
        "format": "lna",
        "version": 1,
        "exported_at": now_ms(),
    });
    zip.start_file("manifest.json", opts).map_err(z_err)?;
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
        .map_err(io_err)?;

    // Absolute local paths are machine-specific (and reveal the user name); blobs travel inside the ZIP.
    let mut sources_json = serde_json::to_value(&sources).unwrap_or_else(|_| json!([]));
    if let Some(items) = sources_json.as_array_mut() {
        for item in items {
            if let Some(obj) = item.as_object_mut() {
                obj.remove("blob_path");
            }
        }
    }
    let bundle = json!({
        "notebook": nb,
        "sources": sources_json,
        "notes": notes,
        "artifacts": artifacts,
    });
    zip.start_file("notebook.json", opts).map_err(z_err)?;
    zip.write_all(serde_json::to_string_pretty(&bundle).unwrap().as_bytes())
        .map_err(io_err)?;

    for src in &sources {
        if let Some(ref bp) = src.blob_path {
            let p = Path::new(bp);
            if p.exists() {
                if let Ok(bytes) = std::fs::read(p) {
                    let fname = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("original.bin");
                    zip.start_file(format!("blobs/{}/{fname}", src.id), opts)
                        .map_err(z_err)?;
                    zip.write_all(&bytes).map_err(io_err)?;
                }
            }
        }
    }

    for art in &artifacts {
        if let (Some(aid), Some(out_dir)) = (
            art.get("id").and_then(|x| x.as_str()),
            art.get("output_dir").and_then(|x| x.as_str()),
        ) {
            let dir = Path::new(out_dir);
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for ent in entries.flatten() {
                        let ep = ent.path();
                        if ep.is_file() {
                            if let (Some(fname), Ok(bytes)) =
                                (ep.file_name().and_then(|n| n.to_str()), std::fs::read(&ep))
                            {
                                zip.start_file(format!("artifacts/{aid}/{fname}"), opts)
                                    .map_err(z_err)?;
                                zip.write_all(&bytes).map_err(io_err)?;
                            }
                        }
                    }
                }
            }
        }
    }

    zip.finish().map_err(z_err)?;
    Ok(())
}

/// Import .lna ZIP with path-traversal, symlink, and zip-bomb guards (F2).
pub fn import_notebook_lna(db: &Database, jobs: &JobQueue, in_zip: &Path) -> LnaResult<String> {
    import_notebook_lna_with_limits(db, jobs, in_zip, MAX_IMPORT_ENTRIES, MAX_IMPORT_BYTES)
}

/// Same as `import_notebook_lna` with explicit limits (the public entry point uses the defaults).
/// Anything created before a failure is rolled back so a bad archive leaves no half notebook.
pub fn import_notebook_lna_with_limits(
    db: &Database,
    jobs: &JobQueue,
    in_zip: &Path,
    max_entries: usize,
    max_bytes: u64,
) -> LnaResult<String> {
    let created: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let res = import_inner(db, jobs, in_zip, max_entries, max_bytes, &created);
    if res.is_err() {
        if let Some(id) = created.borrow().as_deref() {
            let _ = lna_storage::delete_notebook(db, id);
        }
    }
    res
}

fn import_inner(
    db: &Database,
    jobs: &JobQueue,
    in_zip: &Path,
    max_entries: usize,
    max_bytes: u64,
    created: &std::cell::RefCell<Option<String>>,
) -> LnaResult<String> {
    let f = std::fs::File::open(in_zip).map_err(|e| LnaError::invalid(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| LnaError::invalid(e.to_string()))?;

    if archive.len() > max_entries {
        return Err(LnaError::invalid("ZIP exceeds maximum entry count"));
    }

    let mut total_uncompressed = 0u64;
    let mut manifest_json: Option<Value> = None;
    let mut notebook_json: Option<Value> = None;
    let mut blob_entries: HashMap<String, (String, Vec<u8>)> = HashMap::new();
    let mut artifact_entries: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(z_err)?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        if name.starts_with('/') || name.contains('\\') || name.contains("..") || name.contains(':')
        {
            return Err(LnaError::new(
                ErrorCode::InvalidArgument,
                format!("Rejected unsafe ZIP path: {name}"),
            ));
        }
        if entry.is_symlink() {
            return Err(LnaError::new(
                ErrorCode::InvalidArgument,
                format!("Rejected symlink in ZIP: {name}"),
            ));
        }
        if entry.is_dir() {
            continue;
        }
        let remaining = max_bytes.saturating_sub(total_uncompressed);
        if entry.size() > remaining {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                "ZIP uncompressed size exceeds limit (zip bomb guard)",
            ));
        }
        let mut buf = Vec::new();
        let mut limited = entry.take(remaining + 1);
        limited.read_to_end(&mut buf).map_err(io_err)?;
        total_uncompressed = total_uncompressed.saturating_add(buf.len() as u64);
        if total_uncompressed > max_bytes {
            return Err(LnaError::new(
                ErrorCode::SourceTooLarge,
                "ZIP uncompressed size exceeds limit (zip bomb guard)",
            ));
        }

        if name == "manifest.json" {
            let v: Value =
                serde_json::from_slice(&buf).map_err(|e| LnaError::invalid(e.to_string()))?;
            manifest_json = Some(v);
        } else if name == "notebook.json" {
            let v: Value =
                serde_json::from_slice(&buf).map_err(|e| LnaError::invalid(e.to_string()))?;
            notebook_json = Some(v);
        } else if let Some(rest) = name.strip_prefix("blobs/") {
            if let Some((old_sid, fname)) = rest.split_once('/') {
                if !old_sid.is_empty() && !fname.is_empty() && fname != "." && !fname.contains('/')
                {
                    blob_entries.insert(old_sid.to_string(), (fname.to_string(), buf));
                }
            }
        } else if let Some(rest) = name.strip_prefix("artifacts/") {
            if let Some((old_aid, fname)) = rest.split_once('/') {
                if !old_aid.is_empty() && !fname.is_empty() && fname != "." && !fname.contains('/')
                {
                    artifact_entries
                        .entry(old_aid.to_string())
                        .or_default()
                        .push((fname.to_string(), buf));
                }
            }
        }
    }

    let manifest =
        manifest_json.ok_or_else(|| LnaError::invalid("Missing manifest.json in .lna"))?;
    if manifest.get("format").and_then(|x| x.as_str()) != Some("lna")
        || manifest.get("version").and_then(|x| x.as_i64()) != Some(1)
    {
        return Err(LnaError::invalid(
            "Unsupported .lna manifest format or version",
        ));
    }

    let bundle = notebook_json.ok_or_else(|| LnaError::invalid("Missing notebook.json in .lna"))?;
    let title = bundle
        .pointer("/notebook/title")
        .and_then(|x| x.as_str())
        .unwrap_or("导入的笔记本");
    let emoji = bundle.pointer("/notebook/emoji").and_then(|x| x.as_str());
    let nb = lna_storage::create_notebook(db, title, emoji)?;
    *created.borrow_mut() = Some(nb.id.clone());

    let mut source_id_map: HashMap<String, String> = HashMap::new();
    let mut queued_source_ids: Vec<String> = Vec::new();

    if let Some(sources) = bundle.get("sources").and_then(|x| x.as_array()) {
        for s in sources {
            let old_sid = s.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let new_sid = new_ulid();
            if !old_sid.is_empty() {
                source_id_map.insert(old_sid.to_string(), new_sid.clone());
            }
            let stitle = s.get("title").and_then(|x| x.as_str()).unwrap_or("来源");
            let kind_str = s.get("kind").and_then(|x| x.as_str()).unwrap_or("txt");
            let kind_db = lna_types::SourceKind::from_db_str(kind_str).as_str();
            let origin_uri = s.get("origin_uri").and_then(|x| x.as_str());
            let content_hash = s.get("content_hash").and_then(|x| x.as_str()).unwrap_or("");
            let byte_size = s.get("byte_size").and_then(|x| x.as_i64()).unwrap_or(0);
            let is_selected = if s
                .get("is_selected")
                .and_then(|x| x.as_bool())
                .unwrap_or(true)
            {
                1i64
            } else {
                0i64
            };

            let mut new_blob_path: Option<String> = None;
            if let Some((fname, bytes)) = blob_entries.get(old_sid) {
                let bdir = db.paths().blobs_dir.join(&nb.id).join(&new_sid);
                std::fs::create_dir_all(&bdir).map_err(|e| LnaError::internal(e.to_string()))?;
                let bpath = bdir.join(fname);
                std::fs::write(&bpath, bytes).map_err(|e| LnaError::internal(e.to_string()))?;
                new_blob_path = Some(bpath.to_string_lossy().to_string());
            }

            let status = if new_blob_path.is_some() {
                "QUEUED"
            } else {
                "FAILED"
            };
            let now = now_ms();
            db.write(|conn| {
                conn.execute(
                    "INSERT INTO sources
                     (id, notebook_id, title, kind, origin_uri, blob_path, content_hash, byte_size, is_selected, status, created_at, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    params![
                        new_sid,
                        nb.id,
                        stitle,
                        kind_db,
                        origin_uri,
                        new_blob_path,
                        content_hash,
                        byte_size,
                        is_selected,
                        status,
                        now,
                        now
                    ],
                )
                .map_err(map_sql_err)?;
                Ok(())
            })?;
            if status == "QUEUED" {
                queued_source_ids.push(new_sid);
            }
        }
    }

    for sid in &queued_source_ids {
        jobs.enqueue("ingest_source", Some(&nb.id), sid, 10)?;
    }

    if let Some(notes) = bundle.get("notes").and_then(|x| x.as_array()) {
        db.write(|conn| {
            for n in notes {
                let nid = new_ulid();
                let nt = n.get("title").and_then(|x| x.as_str()).unwrap_or("笔记");
                let body = n.get("body_md").and_then(|x| x.as_str()).unwrap_or("");
                let origin = n.get("origin").and_then(|x| x.as_str()).unwrap_or("manual");
                let cites = n.get("citations_json").and_then(|x| x.as_str());
                let now = now_ms();
                conn.execute(
                    "INSERT INTO notes (id, notebook_id, title, body_md, origin, citations_json, created_at, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![nid, nb.id, nt, body, origin, cites, now, now],
                )
                .map_err(map_sql_err)?;
            }
            Ok(())
        })?;
    }

    if let Some(artifacts) = bundle.get("artifacts").and_then(|x| x.as_array()) {
        for art in artifacts {
            let old_aid = art.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let new_aid = new_ulid();
            let atype = art
                .get("artifact_type")
                .and_then(|x| x.as_str())
                .unwrap_or("report");
            let atitle = art.get("title").and_then(|x| x.as_str()).unwrap_or("产物");
            // Only finished artifacts keep their status; an in-flight one has no job after import.
            let status = match art.get("status").and_then(|x| x.as_str()) {
                Some("READY") => "READY",
                Some("FAILED") => "FAILED",
                _ => "CANCELLED",
            };
            let stage = art.get("stage").and_then(|x| x.as_str());
            let progress = art.get("progress").and_then(|x| x.as_f64()).unwrap_or(1.0);
            let cfg = art
                .get("config_json")
                .and_then(|x| x.as_str())
                .unwrap_or("{}");
            let old_sids: Vec<String> = art
                .get("source_ids_json")
                .and_then(|x| x.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let remapped_sids: Vec<String> = old_sids
                .iter()
                .filter_map(|id| source_id_map.get(id).cloned())
                .collect();
            let sids_json = serde_json::to_string(&remapped_sids).unwrap_or_else(|_| "[]".into());
            let fp = art
                .get("sources_fingerprint")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let draft = art.get("draft_json").and_then(|x| x.as_str());
            let payload = art.get("payload_json").and_then(|x| x.as_str());

            let mut out_dir_str: Option<String> = None;
            if let Some(files) = artifact_entries.get(old_aid) {
                let dir = db.paths().artifacts_dir.join(&new_aid);
                std::fs::create_dir_all(&dir).map_err(|e| LnaError::internal(e.to_string()))?;
                for (fname, bytes) in files {
                    std::fs::write(dir.join(fname), bytes)
                        .map_err(|e| LnaError::internal(e.to_string()))?;
                }
                out_dir_str = Some(dir.to_string_lossy().to_string());
            }

            let now = now_ms();
            db.write(|conn| {
                conn.execute(
                    "INSERT INTO studio_artifacts
                     (id, notebook_id, artifact_type, title, status, stage, progress, config_json,
                      source_ids_json, sources_fingerprint, draft_json, payload_json, output_dir,
                      created_at, updated_at, finished_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                    params![
                        new_aid,
                        nb.id,
                        atype,
                        atitle,
                        status,
                        stage,
                        progress,
                        cfg,
                        sids_json,
                        fp,
                        draft,
                        payload,
                        out_dir_str,
                        now,
                        now,
                        Some(now)
                    ],
                )
                .map_err(map_sql_err)?;
                Ok(())
            })?;
        }
    }

    Ok(nb.id)
}

/// Export notebook notes, summary, and chat as Markdown with citations converted to footnotes (F3).
pub fn export_notebook_markdown(db: &Database, notebook_id: &str, out_md: &Path) -> LnaResult<()> {
    let nb = lna_storage::get_notebook(db, notebook_id)?;
    let mut md = format!("# {}\n\n", nb.title);
    if let Some(ref sum) = nb.summary_md {
        md.push_str("## 摘要\n\n");
        md.push_str(sum);
        md.push_str("\n\n");
    }

    let mut footnotes: Vec<(i32, String)> = Vec::new();
    let msgs: Vec<(String, String, String)> = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, role, content FROM chat_messages WHERE notebook_id = ?1 ORDER BY created_at ASC")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;

    if !msgs.is_empty() {
        md.push_str("## 对话记录\n\n");
        for (mid, role, content) in msgs {
            let mut txt = content;
            let cites: Vec<(i32, String, String)> = db.read(|conn| {
                let mut stmt = conn
                    .prepare("SELECT citation_no, source_title, quote FROM message_citations WHERE message_id = ?1 ORDER BY citation_no ASC")
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([&mid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(map_sql_err)?);
                }
                Ok(out)
            })?;
            for (no, stitle, quote) in cites {
                let fn_idx = (footnotes.len() + 1) as i32;
                txt = txt.replace(&format!("[{no}]"), &format!("[^{fn_idx}]"));
                footnotes.push((fn_idx, format!("{stitle}: {quote}")));
            }
            md.push_str(&format!("**{role}**: {txt}\n\n"));
        }
    }

    let notes: Vec<(String, String)> = db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT title, body_md FROM notes WHERE notebook_id = ?1 ORDER BY updated_at DESC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;
    if !notes.is_empty() {
        md.push_str("## 笔记\n\n");
        for (title, body) in notes {
            md.push_str(&format!("### {title}\n\n{body}\n\n"));
        }
    }

    let text_artifacts: Vec<(String, String, String)> = db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT title, artifact_type, payload_json
                 FROM studio_artifacts
                 WHERE notebook_id = ?1 AND status = 'READY' AND payload_json IS NOT NULL
                   AND artifact_type IN ('report','quiz','flashcards','mindmap')
                 ORDER BY created_at ASC",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([notebook_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)?);
        }
        Ok(out)
    })?;
    if !text_artifacts.is_empty() {
        md.push_str("## Studio 产物\n\n");
        for (title, atype, payload_s) in text_artifacts {
            let payload: Value = serde_json::from_str(&payload_s).unwrap_or(Value::Null);
            let body = lna_studio::render::artifact_markdown(&atype, &title, &payload);
            // demote the artifact's own "# title" so it nests under "## Studio 产物"
            let body = body.replacen("# ", "### ", 1).replace(
                &format!("{nl}## ", nl = char::from(10)),
                &format!("{nl}#### ", nl = char::from(10)),
            );
            md.push_str(&body);
            md.push('\n');
        }
    }

    if !footnotes.is_empty() {
        md.push_str("## 引用脚注\n\n");
        for (idx, body) in footnotes {
            md.push_str(&format!("[^{idx}]: {body}\n"));
        }
    }

    if let Some(p) = out_md.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    std::fs::write(out_md, md).map_err(io_err)
}

/// Export sanitized diagnostics (metadata, counts, error codes only — no content, prompts, or keys, S2).
pub fn export_diagnostics(db: &Database, out_json: &Path) -> LnaResult<()> {
    let counts = db.read(|conn| {
        let nb: i64 = conn
            .query_row("SELECT COUNT(*) FROM notebooks", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let src: i64 = conn
            .query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let msg: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let art: i64 = conn
            .query_row("SELECT COUNT(*) FROM studio_artifacts", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        Ok(json!({ "notebooks": nb, "sources": src, "messages": msg, "artifacts": art }))
    })?;
    // Only codes, statuses and capability flags: never titles, text, prompts, answers or URLs.
    let breakdown = db.read(|conn| {
        let group = |sql: &str| -> LnaResult<Value> {
            let mut stmt = conn.prepare(sql).map_err(map_sql_err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))
                .map_err(map_sql_err)?;
            let mut m = serde_json::Map::new();
            for r in rows {
                let (k, n) = r.map_err(map_sql_err)?;
                m.insert(k.unwrap_or_else(|| "none".into()), json!(n));
            }
            Ok(Value::Object(m))
        };
        Ok(json!({
            "source_status": group("SELECT status, COUNT(*) FROM sources GROUP BY status")?,
            "source_errors": group("SELECT error_code, COUNT(*) FROM sources WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "studio_status": group("SELECT status, COUNT(*) FROM studio_artifacts GROUP BY status")?,
            "studio_errors": group("SELECT error_code, COUNT(*) FROM studio_artifacts WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "chat_status": group("SELECT status, COUNT(*) FROM chat_messages GROUP BY status")?,
            "chat_errors": group("SELECT error_code, COUNT(*) FROM chat_messages WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "job_status": group("SELECT status, COUNT(*) FROM jobs GROUP BY status")?,
        }))
    })?;
    let profiles: Vec<Value> = lna_storage::list_profiles(db)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            json!({
                "protocol": p.protocol.as_str(),
                "context_window": p.context_window,
                "max_output_tokens": p.max_output_tokens,
                "capabilities": p.capabilities,
                "active": p.is_active,
            })
        })
        .collect();
    let diag = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "exported_at": now_ms(),
        "counts": counts,
        "breakdown": breakdown,
        "profiles": profiles,
    });
    if let Some(p) = out_json.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    std::fs::write(out_json, serde_json::to_string_pretty(&diag).unwrap()).map_err(io_err)
}


fn is_sensitive_header_or_key(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    if low == "max_token_limits" {
        return false;
    }
    ["authorization", "api-key", "api_key", "apikey", "secret", "password", "cookie", "keychain", "x-goog-api-key", "token"]
        .iter()
        .any(|k| low.contains(k))
}

/// Export complete redacted Debug diagnostics bundle as a ZIP archive:
/// - `manifest.json`: summary metadata, counts, breakdown, sanitized settings & model profiles (NO API keys), indices
/// - `notebooks/<id>.json`: per-notebook sources, chat messages, studio artifacts (with errors/thoughts), notes
/// - `api_traces/<seq>.json`: per-request full Debug API endpoint input & output (base64 images stripped to `$media_ref`)
/// - `media/<hash>.<ext>`: deduplicated image/media attachments referenced by `api_traces`
/// - `logs/runtime.log`: full persistent INFO+ and Debug runtime logs
pub fn export_diagnostics_zip(
    db: &Database,
    llm: &lna_llm::LlmClient,
    out_zip: &Path,
) -> LnaResult<Value> {
    let counts = db.read(|conn| {
        let nb: i64 = conn
            .query_row("SELECT COUNT(*) FROM notebooks", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let src: i64 = conn
            .query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let msg: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        let art: i64 = conn
            .query_row("SELECT COUNT(*) FROM studio_artifacts", [], |r| r.get(0))
            .map_err(map_sql_err)?;
        Ok(json!({ "notebooks": nb, "sources": src, "messages": msg, "artifacts": art }))
    })?;

    let breakdown = db.read(|conn| {
        let group = |sql: &str| -> LnaResult<Value> {
            let mut stmt = conn.prepare(sql).map_err(map_sql_err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))
                .map_err(map_sql_err)?;
            let mut m = serde_json::Map::new();
            for r in rows {
                let (k, n) = r.map_err(map_sql_err)?;
                m.insert(k.unwrap_or_else(|| "none".into()), json!(n));
            }
            Ok(Value::Object(m))
        };
        Ok(json!({
            "source_status": group("SELECT status, COUNT(*) FROM sources GROUP BY status")?,
            "source_errors": group("SELECT error_code, COUNT(*) FROM sources WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "studio_status": group("SELECT status, COUNT(*) FROM studio_artifacts GROUP BY status")?,
            "studio_errors": group("SELECT error_code, COUNT(*) FROM studio_artifacts WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "chat_status": group("SELECT status, COUNT(*) FROM chat_messages GROUP BY status")?,
            "chat_errors": group("SELECT error_code, COUNT(*) FROM chat_messages WHERE error_code IS NOT NULL GROUP BY error_code")?,
            "job_status": group("SELECT status, COUNT(*) FROM jobs GROUP BY status")?,
        }))
    })?;

    // Sanitized app_settings (exclude any secret/key/token entries except max_token_limits)
    let settings = db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT key, value_json FROM app_settings ORDER BY key")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(map_sql_err)?;
        let mut m = serde_json::Map::new();
        for r in rows {
            let (k, vj) = r.map_err(map_sql_err)?;
            if is_sensitive_header_or_key(&k) {
                m.insert(k, json!("[REDACTED]"));
            } else {
                let parsed = serde_json::from_str::<Value>(&vj).unwrap_or_else(|_| Value::String(vj));
                m.insert(k, parsed);
            }
        }
        Ok(Value::Object(m))
    })?;

    // Sanitized model profiles (NEVER include API keys/secrets; redact sensitive headers & URL params)
    let profiles: Vec<Value> = lna_storage::list_profiles(db)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let safe_headers: serde_json::Map<String, Value> = p
                .extra_headers
                .iter()
                .map(|(k, v)| {
                    if is_sensitive_header_or_key(k) {
                        (k.clone(), json!("[REDACTED]"))
                    } else {
                        (k.clone(), json!(v))
                    }
                })
                .collect();
            json!({
                "id": p.id,
                "display_name": p.display_name,
                "protocol": p.protocol.as_str(),
                "base_url": lna_llm::sanitize_url_for_log(&p.base_url),
                "model_id": p.model_id,
                "context_window": p.context_window,
                "max_output_tokens": p.max_output_tokens,
                "max_parallel": p.max_parallel,
                "preset": p.preset,
                "shrink_media": p.shrink_media,
                "capabilities": p.capabilities,
                "extra_headers": safe_headers,
                "active": p.is_active,
            })
        })
        .collect();

    if let Some(parent) = out_zip.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let f = std::fs::File::create(out_zip).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // 1. Per-notebook sub-JSONs under notebooks/<id>.json
    let notebooks = lna_storage::list_notebooks(db).unwrap_or_default();
    let mut notebooks_index = Vec::new();
    for nb in &notebooks {
        let sources = lna_storage::list_sources(db, &nb.id).unwrap_or_default();
        let messages = db
            .read(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, role, content, status, stop_reason, error_code, created_at
                         FROM chat_messages WHERE notebook_id = ?1 ORDER BY created_at ASC",
                    )
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([&nb.id], |r| {
                        Ok(json!({
                            "id": r.get::<_, String>(0)?,
                            "role": r.get::<_, String>(1)?,
                            "content": r.get::<_, String>(2)?,
                            "status": r.get::<_, String>(3)?,
                            "stop_reason": r.get::<_, Option<String>>(4)?,
                            "error_code": r.get::<_, Option<String>>(5)?,
                            "created_at": r.get::<_, i64>(6)?,
                        }))
                    })
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(map_sql_err)?);
                }
                Ok(out)
            })
            .unwrap_or_default();
        let artifacts = db
            .read(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, artifact_type, title, status, stage, progress, config_json,
                                source_ids_json, draft_json, payload_json, error_code, error_detail,
                                error_log, created_at, updated_at, finished_at
                         FROM studio_artifacts WHERE notebook_id = ?1 ORDER BY updated_at DESC",
                    )
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([&nb.id], |r| {
                        let parse_opt = |s: Option<String>| -> Value {
                            s.and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                                .unwrap_or(Value::Null)
                        };
                        let cfg_raw = r.get::<_, String>(6)?;
                        let sids_raw = r.get::<_, String>(7)?;
                        Ok(json!({
                            "id": r.get::<_, String>(0)?,
                            "artifact_type": r.get::<_, String>(1)?,
                            "title": r.get::<_, String>(2)?,
                            "status": r.get::<_, String>(3)?,
                            "stage": r.get::<_, Option<String>>(4)?,
                            "progress": r.get::<_, f64>(5)?,
                            "config": serde_json::from_str::<Value>(&cfg_raw).unwrap_or_else(|_| Value::String(cfg_raw)),
                            "source_ids": serde_json::from_str::<Value>(&sids_raw).unwrap_or_else(|_| Value::String(sids_raw)),
                            "draft": parse_opt(r.get::<_, Option<String>>(8)?),
                            "payload": parse_opt(r.get::<_, Option<String>>(9)?),
                            "error_code": r.get::<_, Option<String>>(10)?,
                            "error_detail": r.get::<_, Option<String>>(11)?,
                            "error_log": r.get::<_, Option<String>>(12)?,
                            "created_at": r.get::<_, i64>(13)?,
                            "updated_at": r.get::<_, i64>(14)?,
                            "finished_at": r.get::<_, Option<i64>>(15)?,
                        }))
                    })
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(map_sql_err)?);
                }
                Ok(out)
            })
            .unwrap_or_default();

        let notes = db
            .read(|conn| {
                let mut stmt = conn
                    .prepare("SELECT id, title, body_md, origin, created_at, updated_at FROM notes WHERE notebook_id = ?1")
                    .map_err(map_sql_err)?;
                let rows = stmt
                    .query_map([&nb.id], |r| {
                        Ok(json!({
                            "id": r.get::<_, String>(0)?,
                            "title": r.get::<_, String>(1)?,
                            "body_md": r.get::<_, String>(2)?,
                            "origin": r.get::<_, String>(3)?,
                            "created_at": r.get::<_, i64>(4)?,
                            "updated_at": r.get::<_, i64>(5)?,
                        }))
                    })
                    .map_err(map_sql_err)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(map_sql_err)?);
                }
                Ok(out)
            })
            .unwrap_or_default();

        let rel_path = format!("notebooks/{}.json", nb.id);
        let nb_doc = json!({
            "notebook": nb,
            "sources": sources,
            "messages": messages,
            "artifacts": artifacts,
            "notes": notes,
        });
        zip.start_file(&rel_path, opts).map_err(z_err)?;
        zip.write_all(serde_json::to_string_pretty(&nb_doc).unwrap().as_bytes())
            .map_err(io_err)?;
        notebooks_index.push(json!({
            "id": nb.id,
            "title": nb.title,
            "file": rel_path,
            "sources": sources.len(),
            "messages": messages.len(),
            "artifacts": artifacts.len(),
            "notes": notes.len(),
        }));
    }

    // 2. Structured Debug API traces (api_traces/*.json) and extracted images (media/*)
    let mut api_traces_index = Vec::new();
    let mut media_files = Vec::new();
    let mut log_bytes: u64 = 0;

    if let Some(log_dir) = llm.log_dir() {
        let traces_dir = log_dir.join("traces");
        if let Ok(rd) = std::fs::read_dir(&traces_dir) {
            let mut trace_paths: Vec<std::path::PathBuf> = rd
                .filter_map(|e| e.ok().map(|x| x.path()))
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect();
            trace_paths.sort();
            for tp in trace_paths {
                let Some(fname) = tp.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Ok(bytes) = std::fs::read(&tp) {
                    let rel = format!("api_traces/{fname}");
                    zip.start_file(&rel, opts).map_err(z_err)?;
                    zip.write_all(&bytes).map_err(io_err)?;
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        api_traces_index.push(json!({
                            "seq": v["seq"],
                            "timestamp_ms": v["timestamp_ms"],
                            "protocol": v["protocol"],
                            "profile_id": v["profile_id"],
                            "model_id": v["model_id"],
                            "endpoint": v["endpoint"],
                            "http_status": v["http_status"],
                            "stop_reason": v["stop_reason"],
                            "elapsed_ms": v["elapsed_ms"],
                            "usage": v["usage"],
                            "media_refs": v["media_refs"],
                            "file": rel,
                        }));
                    } else {
                        api_traces_index.push(json!({ "file": rel }));
                    }
                }
            }
        }

        let media_dir = log_dir.join("media");
        if let Ok(rd) = std::fs::read_dir(&media_dir) {
            let mut mpaths: Vec<std::path::PathBuf> = rd
                .filter_map(|e| e.ok().map(|x| x.path()))
                .filter(|p| p.is_file())
                .collect();
            mpaths.sort();
            for mp in mpaths {
                let Some(fname) = mp.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Ok(bytes) = std::fs::read(&mp) {
                    let rel = format!("media/{fname}");
                    zip.start_file(&rel, opts).map_err(z_err)?;
                    zip.write_all(&bytes).map_err(io_err)?;
                    media_files.push(json!({ "file": rel, "bytes": bytes.len() }));
                }
            }
        }
    }

    // 3. Full persistent runtime logs (INFO+ and Debug wire logs) under logs/runtime.log
    if let Ok(log_res) = llm.export_logs(None) {
        let content = log_res
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        log_bytes = content.len() as u64;
        zip.start_file("logs/runtime.log", opts).map_err(z_err)?;
        zip.write_all(content.as_bytes()).map_err(io_err)?;
    }

    // 4. Root manifest.json (总 JSON)
    let manifest = json!({
        "format": "lna-diagnostics-zip",
        "schema_version": 2,
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "exported_at": now_ms(),
        "debug_mode": llm.debug_mode(),
        "counts": counts,
        "breakdown": breakdown,
        "max_token_limits": llm.token_limits(),
        "settings": settings,
        "profiles": profiles,
        "logs_file": "logs/runtime.log",
        "log_bytes": log_bytes,
        "notebooks_index": notebooks_index,
        "api_traces_index": api_traces_index,
        "media_files": media_files,
    });
    zip.start_file("manifest.json", opts).map_err(z_err)?;
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
        .map_err(io_err)?;

    zip.finish().map_err(z_err)?;
    Ok(json!({
        "ok": true,
        "format": "zip",
        "path": out_zip.to_string_lossy(),
        "notebooks": notebooks_index.len(),
        "api_traces": api_traces_index.len(),
        "media_files": media_files.len(),
        "log_bytes": log_bytes,
    }))
}

fn z_err(e: zip::result::ZipError) -> LnaError {
    LnaError::invalid(format!("ZIP error: {e}"))
}

fn io_err(e: std::io::Error) -> LnaError {
    LnaError::internal(format!("IO error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lna_storage::StoragePaths;

    #[test]
    fn rejects_path_traversal_in_lna_import() {
        let dir = std::env::temp_dir().join(format!("lna_arch_test_{}", std::process::id()));
        let paths = StoragePaths::from_root(&dir).unwrap();
        let db = Database::open(paths).unwrap();
        let jobs = JobQueue::new(db.clone());

        let zip_path = dir.join("evil.lna");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("../escape.txt", opts).unwrap();
            zw.write_all(b"evil").unwrap();
            zw.finish().unwrap();
        }

        let err = import_notebook_lna(&db, &jobs, &zip_path).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
