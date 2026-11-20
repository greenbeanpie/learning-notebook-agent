use lna_types::{ErrorCode, LnaError, LnaResult};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

static REGISTER_VEC: Once = Once::new();
const MIGRATION_0001: &str = include_str!("../../../migrations/0001_init.sql");

#[derive(Debug, Clone)]
pub struct StoragePaths {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub blobs_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub models_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl StoragePaths {
    pub fn from_root(root: impl AsRef<Path>) -> LnaResult<Self> {
        let root = root.as_ref().to_path_buf();
        let blobs_dir = root.join("blobs");
        let artifacts_dir = root.join("artifacts");
        let models_dir = root.join("models");
        let logs_dir = root.join("logs");
        for d in [&root, &blobs_dir, &artifacts_dir, &models_dir, &logs_dir] {
            std::fs::create_dir_all(d).map_err(|e| {
                LnaError::internal(format!("Failed to create directory {}: {e}", d.display()))
            })?;
        }
        Ok(Self {
            db_path: root.join("notebook.db"),
            root,
            blobs_dir,
            artifacts_dir,
            models_dir,
            logs_dir,
        })
    }
}

struct ReadSlot {
    conn: Connection,
    last_used: Instant,
}

#[derive(Clone)]
pub struct Database {
    paths: StoragePaths,
    writer: Arc<Mutex<Connection>>,
    readers: Arc<Mutex<Vec<ReadSlot>>>,
}

impl Database {
    pub fn open(paths: StoragePaths) -> LnaResult<Self> {
        Self::register_sqlite_vec();

        let conn = Connection::open_with_flags(
            &paths.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql_err)?;

        apply_pragmas(&conn)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_sql_err)?;

        run_migrations(&conn, &paths.db_path)?;

        Ok(Self {
            paths,
            writer: Arc::new(Mutex::new(conn)),
            readers: Arc::new(Mutex::new(Vec::with_capacity(3))),
        })
    }

    fn register_sqlite_vec() {
        REGISTER_VEC.call_once(|| unsafe {
            type AutoExt = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::os::raw::c_int;
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), AutoExt>(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        });
    }

    pub fn paths(&self) -> &StoragePaths {
        &self.paths
    }

    pub fn write<F, T>(&self, f: F) -> LnaResult<T>
    where
        F: FnOnce(&mut Connection) -> LnaResult<T>,
    {
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| LnaError::internal("Database writer lock poisoned"))?;
        f(&mut guard)
    }

    pub fn read<F, T>(&self, f: F) -> LnaResult<T>
    where
        F: FnOnce(&Connection) -> LnaResult<T>,
    {
        let mut slot = {
            let mut pool = self
                .readers
                .lock()
                .map_err(|_| LnaError::internal("Database reader lock poisoned"))?;
            pool.pop()
        };

        let conn = match slot.take() {
            Some(s) => s.conn,
            None => {
                let c = Connection::open_with_flags(
                    &self.paths.db_path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(map_sql_err)?;
                apply_pragmas(&c)?;
                c
            }
        };

        let res = f(&conn);

        if let Ok(mut pool) = self.readers.lock() {
            if pool.len() < 3 {
                pool.push(ReadSlot {
                    conn,
                    last_used: Instant::now(),
                });
            }
        }
        res
    }

    /// Close read-only connections idle for >= 60s.
    pub fn evict_idle_readers(&self) {
        if let Ok(mut pool) = self.readers.lock() {
            let now = Instant::now();
            pool.retain(|s| now.duration_since(s.last_used) < Duration::from_secs(60));
        }
    }

    /// Graceful shutdown checkpoint.
    pub fn checkpoint_truncate(&self) -> LnaResult<()> {
        if let Ok(mut pool) = self.readers.lock() {
            pool.clear();
        }
        self.write(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(map_sql_err)
        })
    }

    /// Crash recovery on startup: reset RUNNING jobs, STREAMING messages, RUNNING studio artifacts.
    pub fn recover_on_startup(&self) -> LnaResult<()> {
        self.write(|conn| {
            let tx = conn.transaction().map_err(map_sql_err)?;
            tx.execute(
                "UPDATE jobs SET status = 'QUEUED', updated_at = ?1 WHERE status = 'RUNNING'",
                [lna_types::now_ms()],
            )
            .map_err(map_sql_err)?;
            tx.execute(
                "UPDATE chat_messages SET status = 'INCOMPLETE', stop_reason = 'process_interrupted' WHERE status = 'STREAMING'",
                [],
            )
            .map_err(map_sql_err)?;
            tx.execute(
                "UPDATE studio_artifacts SET status = 'PENDING', updated_at = ?1 WHERE status = 'RUNNING'",
                [lna_types::now_ms()],
            )
            .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
            Ok(())
        })
    }
}

fn apply_pragmas(conn: &Connection) -> LnaResult<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = -4096;
         PRAGMA temp_store = MEMORY;",
    )
    .map_err(map_sql_err)
}

fn run_migrations(conn: &Connection, db_path: &Path) -> LnaResult<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(map_sql_err)?;

    if version < 1 {
        if db_path.exists() && std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0) > 0 {
            let backup = db_path.with_extension("db.bak");
            let _ = std::fs::copy(db_path, backup);
        }
        conn.execute_batch("BEGIN IMMEDIATE;")
            .map_err(map_sql_err)?;
        if let Err(e) = conn.execute_batch(MIGRATION_0001) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(map_sql_err(e));
        }
        conn.pragma_update(None, "user_version", 1)
            .map_err(map_sql_err)?;
        conn.execute_batch("COMMIT;").map_err(map_sql_err)?;
        tracing::info!("Applied SQLite migration 0001_init.sql");
    }
    Ok(())
}

pub fn map_sql_err(err: rusqlite::Error) -> LnaError {
    let msg = err.to_string();
    if msg.contains("database or disk is full") {
        LnaError::new(ErrorCode::StorageFull, msg)
    } else if msg.contains("UNIQUE constraint") {
        LnaError::new(ErrorCode::Conflict, msg)
    } else {
        LnaError::internal(format!("SQLite error: {msg}"))
    }
}
