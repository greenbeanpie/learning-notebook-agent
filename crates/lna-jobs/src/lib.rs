use lna_storage::{db::map_sql_err, Database};
use lna_types::{new_ulid, now_ms, LnaResult};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub notebook_id: Option<String>,
    pub subject_id: String,
    pub attempt: i32,
    pub max_attempts: i32,
}

#[derive(Clone)]
pub struct JobQueue {
    db: Database,
    tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl JobQueue {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn enqueue(
        &self,
        kind: &str,
        notebook_id: Option<&str>,
        subject_id: &str,
        priority: i32,
    ) -> LnaResult<String> {
        let id = new_ulid();
        let now = now_ms();
        self.db.write(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM jobs WHERE kind = ?1 AND subject_id = ?2 AND status IN ('QUEUED','RUNNING')",
                    params![kind, subject_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sql_err)?;
            if let Some(eid) = existing {
                return Ok(eid);
            }
            conn.execute(
                "INSERT INTO jobs (id, kind, notebook_id, subject_id, status, priority, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'QUEUED', ?5, ?6, ?7)",
                params![id, kind, notebook_id, subject_id, priority, now, now],
            )
            .map_err(map_sql_err)?;
            Ok(id)
        })
    }

    pub fn claim_next(&self, kinds: &[&str]) -> LnaResult<Option<JobRecord>> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let now = now_ms();
        let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            let sql = format!(
                "SELECT id, kind, notebook_id, subject_id, attempt, max_attempts
                 FROM jobs
                 WHERE status = 'QUEUED' AND run_after <= ?1 AND kind IN ({placeholders})
                 ORDER BY priority ASC, created_at ASC
                 LIMIT 1"
            );
            let mut stmt = tx.prepare(&sql).map_err(map_sql_err)?;
            let mut param_vals: Vec<rusqlite::types::Value> = vec![now.into()];
            for k in kinds {
                param_vals.push((*k).to_string().into());
            }
            let job = stmt
                .query_row(rusqlite::params_from_iter(param_vals), |r| {
                    Ok(JobRecord {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        notebook_id: r.get(2)?,
                        subject_id: r.get(3)?,
                        attempt: r.get(4)?,
                        max_attempts: r.get(5)?,
                    })
                })
                .optional()
                .map_err(map_sql_err)?;
            drop(stmt);

            if let Some(ref j) = job {
                tx.execute(
                    "UPDATE jobs SET status = 'RUNNING', attempt = attempt + 1, updated_at = ?1 WHERE id = ?2",
                    params![now, j.id],
                )
                .map_err(map_sql_err)?;
            }
            tx.commit().map_err(map_sql_err)?;
            Ok(job)
        })
    }

    pub fn mark_done(&self, job_id: &str) -> LnaResult<()> {
        self.remove_token(job_id);
        self.db.write(|conn| {
            conn.execute(
                "UPDATE jobs SET status = 'DONE', updated_at = ?1 WHERE id = ?2 AND status = 'RUNNING'",
                params![now_ms(), job_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    /// Returns true if the job reached terminal FAILED state (false if re-queued or already CANCELLED).
    pub fn mark_failed_or_retry(
        &self,
        job_id: &str,
        err: &str,
        backoff_ms: i64,
        retryable: bool,
    ) -> LnaResult<bool> {
        self.remove_token(job_id);
        let now = now_ms();
        self.db.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            let cur: Option<(String, i32, i32)> = tx
                .query_row(
                    "SELECT status, attempt, max_attempts FROM jobs WHERE id = ?1",
                    [job_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(map_sql_err)?;

            let Some((status, attempt, max_attempts)) = cur else {
                return Ok(false);
            };
            if status != "RUNNING" {
                // Respect CANCELLED or already terminal state (B-07)
                return Ok(false);
            }

            let terminal = !retryable || attempt >= max_attempts;
            let next_status = if terminal { "FAILED" } else { "QUEUED" };
            tx.execute(
                "UPDATE jobs SET status = ?1, run_after = ?2, last_error = ?3, updated_at = ?4 WHERE id = ?5",
                params![next_status, now + backoff_ms, err, now, job_id],
            )
            .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(terminal)
        })
    }

    pub fn cancel_by_subject(&self, subject_id: &str) -> LnaResult<()> {
        let now = now_ms();
        let active_ids: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM jobs WHERE subject_id = ?1 AND status IN ('QUEUED','RUNNING')",
                )
                .map_err(map_sql_err)?;
            let rows = stmt
                .query_map([subject_id], |r| r.get::<_, String>(0))
                .map_err(map_sql_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sql_err)?);
            }
            Ok(out)
        })?;

        if let Ok(mut g) = self.tokens.lock() {
            for id in &active_ids {
                if let Some(tok) = g.remove(id) {
                    tok.cancel();
                }
            }
        }

        self.db.write(|conn| {
            conn.execute(
                "UPDATE jobs SET status = 'CANCELLED', updated_at = ?1 WHERE subject_id = ?2 AND status IN ('QUEUED','RUNNING')",
                params![now, subject_id],
            )
            .map_err(map_sql_err)?;
            Ok(())
        })
    }

    pub fn register_token(&self, job_id: &str) -> CancellationToken {
        let tok = CancellationToken::new();
        if let Ok(mut g) = self.tokens.lock() {
            g.insert(job_id.to_string(), tok.clone());
        }
        tok
    }

    fn remove_token(&self, job_id: &str) {
        if let Ok(mut g) = self.tokens.lock() {
            g.remove(job_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lna_storage::StoragePaths;

    #[test]
    fn cancelled_job_is_not_requeued_and_mark_done_only_updates_running() {
        let dir = std::env::temp_dir().join(format!("lna_jobs_b07_{}", lna_types::new_ulid()));
        let paths = StoragePaths::from_root(&dir).unwrap();
        let db = Database::open(paths).unwrap();
        let q = JobQueue::new(db);

        let jid = q.enqueue("studio_generate", None, "art_1", 100).unwrap();
        // Cancel before claim -> mark_done should not change CANCELLED to DONE
        q.cancel_by_subject("art_1").unwrap();
        q.mark_done(&jid).unwrap();
        let changed = q.mark_failed_or_retry(&jid, "err", 1000, true).unwrap();
        assert!(!changed);

        let status: String =
            q.db.read(|conn| {
                conn.query_row("SELECT status FROM jobs WHERE id = ?1", [&jid], |r| {
                    r.get(0)
                })
                .map_err(map_sql_err)
            })
            .unwrap();
        assert_eq!(status, "CANCELLED");
        assert!(q.claim_next(&["studio_generate"]).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
