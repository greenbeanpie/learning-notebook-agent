-- migration: 0002_extended_source_kinds.sql
-- Relax sources.kind CHECK constraint to support extended document, image, audio, and video formats.

CREATE TABLE sources_new (
    id              TEXT PRIMARY KEY,
    notebook_id     TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    kind            TEXT NOT NULL,
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

INSERT INTO sources_new (
    id, notebook_id, title, kind, origin_uri, blob_path, content_hash,
    byte_size, page_count, token_count, language, is_selected, status,
    error_code, error_detail, created_at, updated_at
)
SELECT
    id, notebook_id, title, kind, origin_uri, blob_path, content_hash,
    byte_size, page_count, token_count, language, is_selected, status,
    error_code, error_detail, created_at, updated_at
FROM sources;

DROP TABLE sources;
ALTER TABLE sources_new RENAME TO sources;
CREATE INDEX IF NOT EXISTS idx_sources_notebook ON sources(notebook_id, created_at);
