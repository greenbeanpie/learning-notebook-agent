ALTER TABLE studio_artifacts ADD COLUMN error_log TEXT;

CREATE TABLE IF NOT EXISTS artifact_thoughts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id TEXT NOT NULL REFERENCES studio_artifacts(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_artifact_thoughts_artifact
    ON artifact_thoughts(artifact_id, seq);
