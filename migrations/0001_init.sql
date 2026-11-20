-- migration: 0001_init.sql
-- Local Notebook Agent SQLite Schema (WAL + FTS5 + sqlite-vec)
-- Note: Video modality is intentionally excluded in V1.

CREATE TABLE IF NOT EXISTS notebooks (
    id                  TEXT PRIMARY KEY,
    title               TEXT NOT NULL,
    emoji               TEXT NOT NULL DEFAULT '📚',
    summary_md          TEXT,
    summary_fingerprint TEXT,
    suggested_questions_json TEXT,
    title_user_edited   INTEGER NOT NULL DEFAULT 0 CHECK (title_user_edited IN (0,1)),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sources (
    id              TEXT PRIMARY KEY,
    notebook_id     TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('pdf','md','txt','web','note')),
    origin_uri      TEXT,
    blob_path       TEXT,
    content_hash    TEXT NOT NULL,
    byte_size       INTEGER NOT NULL DEFAULT 0,
    page_count      INTEGER,
    token_count     INTEGER NOT NULL DEFAULT 0,
    language        TEXT,
    is_selected     INTEGER NOT NULL DEFAULT 1 CHECK (is_selected IN (0,1)),
    status          TEXT NOT NULL DEFAULT 'QUEUED'
                    CHECK (status IN ('QUEUED','PARSING','CHUNKING','EMBEDDING','READY','FAILED')),
    error_code      TEXT,
    error_detail    TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (notebook_id, content_hash)
);
CREATE INDEX IF NOT EXISTS idx_sources_notebook ON sources(notebook_id, created_at);

CREATE TABLE IF NOT EXISTS source_chunks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    notebook_id     TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    chunk_index     INTEGER NOT NULL,
    section_path    TEXT,
    page_start      INTEGER,
    page_end        INTEGER,
    char_start      INTEGER NOT NULL,
    char_end        INTEGER NOT NULL,
    content         TEXT NOT NULL,
    token_count     INTEGER NOT NULL,
    UNIQUE (source_id, chunk_index)
);
CREATE INDEX IF NOT EXISTS idx_chunks_notebook ON source_chunks(notebook_id);

-- 写入内容为 jieba 分词后、以空格连接的文本；原文从 source_chunks 读取
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    tokens,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS trg_chunks_fts_delete AFTER DELETE ON source_chunks BEGIN
    DELETE FROM chunks_fts WHERE rowid = old.id;
END;

CREATE TABLE IF NOT EXISTS source_digests (
    source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    section_key     TEXT NOT NULL,
    content_hash    TEXT NOT NULL,
    language        TEXT NOT NULL,
    digest_md       TEXT NOT NULL,
    chunk_ids_json  TEXT NOT NULL,
    token_count     INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (source_id, section_key, language)
);

CREATE TABLE IF NOT EXISTS model_profiles (
    id              TEXT PRIMARY KEY,
    display_name    TEXT NOT NULL,
    protocol        TEXT NOT NULL
                    CHECK (protocol IN ('chat_completion','responses','anthropic','gemini')),
    base_url        TEXT NOT NULL,
    model_id        TEXT NOT NULL,
    keychain_ref    TEXT,
    show_thinking   INTEGER NOT NULL DEFAULT 1 CHECK (show_thinking IN (0,1)),
    effort          TEXT,
    context_window  INTEGER NOT NULL DEFAULT 32000,
    max_output_tokens INTEGER NOT NULL DEFAULT 8000,
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    extra_headers_json TEXT NOT NULL DEFAULT '{}',
    token_ratio     REAL NOT NULL DEFAULT 1.0,
    input_price_per_m  REAL,
    output_price_per_m REAL,
    is_active       INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0,1)),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_active_profile ON model_profiles(is_active) WHERE is_active = 1;

CREATE TABLE IF NOT EXISTS embedding_models (
    id                  TEXT PRIMARY KEY,
    provider            TEXT NOT NULL CHECK (provider IN ('local_onnx','remote')),
    model_name          TEXT NOT NULL,
    dimension           INTEGER NOT NULL CHECK (dimension > 0),
    query_prefix        TEXT NOT NULL DEFAULT '',
    passage_prefix      TEXT NOT NULL DEFAULT '',
    vec_table           TEXT NOT NULL UNIQUE,
    endpoint_profile_id TEXT REFERENCES model_profiles(id) ON DELETE SET NULL,
    is_active           INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0,1)),
    created_at          INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_active_embedding ON embedding_models(is_active) WHERE is_active = 1;

CREATE TABLE IF NOT EXISTS chunk_embedding_state (
    chunk_id           INTEGER NOT NULL REFERENCES source_chunks(id) ON DELETE CASCADE,
    embedding_model_id TEXT NOT NULL REFERENCES embedding_models(id) ON DELETE CASCADE,
    PRIMARY KEY (chunk_id, embedding_model_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS chat_messages (
    id                       TEXT PRIMARY KEY,
    notebook_id              TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    role                     TEXT NOT NULL CHECK (role IN ('user','assistant')),
    status                   TEXT NOT NULL DEFAULT 'COMPLETE'
                             CHECK (status IN ('STREAMING','COMPLETE','INCOMPLETE','FAILED','CANCELLED')),
    content                  TEXT NOT NULL DEFAULT '',
    thoughts_json            TEXT,
    provider_state_json      TEXT,
    selected_source_ids_json TEXT,
    followups_json           TEXT,
    profile_id               TEXT REFERENCES model_profiles(id) ON DELETE SET NULL,
    prompt_version           TEXT,
    stop_reason              TEXT,
    error_code               TEXT,
    feedback                 INTEGER CHECK (feedback IN (-1,1)),
    created_at               INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_notebook ON chat_messages(notebook_id, created_at, id);

CREATE TABLE IF NOT EXISTS message_citations (
    message_id    TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
    citation_no   INTEGER NOT NULL,
    chunk_id      INTEGER REFERENCES source_chunks(id) ON DELETE SET NULL,
    source_id     TEXT REFERENCES sources(id) ON DELETE SET NULL,
    source_title  TEXT NOT NULL,
    page_start    INTEGER,
    page_end      INTEGER,
    section_path  TEXT,
    quote         TEXT NOT NULL,
    PRIMARY KEY (message_id, citation_no)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS conversation_summaries (
    notebook_id        TEXT PRIMARY KEY REFERENCES notebooks(id) ON DELETE CASCADE,
    through_message_id TEXT NOT NULL,
    summary            TEXT NOT NULL,
    token_count        INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS notes (
    id                  TEXT PRIMARY KEY,
    notebook_id         TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    title               TEXT NOT NULL,
    body_md             TEXT NOT NULL DEFAULT '',
    origin              TEXT NOT NULL DEFAULT 'manual'
                        CHECK (origin IN ('manual','chat','summary')),
    origin_message_id   TEXT REFERENCES chat_messages(id) ON DELETE SET NULL,
    citations_json      TEXT,
    converted_source_id TEXT REFERENCES sources(id) ON DELETE SET NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_notebook ON notes(notebook_id, updated_at);

CREATE TABLE IF NOT EXISTS studio_artifacts (
    id                      TEXT PRIMARY KEY,
    notebook_id             TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    artifact_type           TEXT NOT NULL CHECK (artifact_type IN
                            ('audio','slides','mindmap','report','flashcards','quiz','infographic')),
    title                   TEXT NOT NULL,
    status                  TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN
                            ('PENDING','RUNNING','DRAFT_REVIEW','READY','FAILED','CANCELLED')),
    stage                   TEXT,
    progress                REAL NOT NULL DEFAULT 0,
    config_json             TEXT NOT NULL,
    source_ids_json         TEXT NOT NULL,
    sources_fingerprint     TEXT NOT NULL,
    profile_id              TEXT REFERENCES model_profiles(id) ON DELETE SET NULL,
    schema_version          INTEGER NOT NULL DEFAULT 1,
    draft_json              TEXT,
    payload_json            TEXT,
    output_dir              TEXT,
    estimated_input_tokens  INTEGER,
    estimated_output_tokens INTEGER,
    error_code              TEXT,
    error_detail            TEXT,
    feedback                INTEGER CHECK (feedback IN (-1,1)),
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    finished_at             INTEGER
);
CREATE INDEX IF NOT EXISTS idx_artifacts_notebook ON studio_artifacts(notebook_id, created_at);

CREATE TABLE IF NOT EXISTS artifact_checkpoints (
    artifact_id TEXT NOT NULL REFERENCES studio_artifacts(id) ON DELETE CASCADE,
    step_key    TEXT NOT NULL,
    output_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (artifact_id, step_key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS artifact_progress (
    artifact_id TEXT PRIMARY KEY REFERENCES studio_artifacts(id) ON DELETE CASCADE,
    state_json  TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN
                 ('ingest_source','embed_source','refresh_summary','studio_generate')),
    notebook_id  TEXT REFERENCES notebooks(id) ON DELETE CASCADE,
    subject_id   TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'QUEUED'
                 CHECK (status IN ('QUEUED','RUNNING','DONE','FAILED','CANCELLED')),
    priority     INTEGER NOT NULL DEFAULT 100,
    attempt      INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    run_after    INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_jobs_claim ON jobs(status, priority, run_after, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_active_subject ON jobs(kind, subject_id)
    WHERE status IN ('QUEUED','RUNNING');

CREATE TABLE IF NOT EXISTS usage_events (
    id            TEXT PRIMARY KEY,
    notebook_id   TEXT REFERENCES notebooks(id) ON DELETE SET NULL,
    profile_id    TEXT REFERENCES model_profiles(id) ON DELETE SET NULL,
    purpose       TEXT NOT NULL,
    subject_id    TEXT,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    is_estimated  INTEGER NOT NULL DEFAULT 0 CHECK (is_estimated IN (0,1)),
    created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usage_notebook ON usage_events(notebook_id, created_at);

CREATE TABLE IF NOT EXISTS app_settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);
