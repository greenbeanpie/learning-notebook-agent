-- migration: 0003_source_guide_and_index.sql
-- Adds per-source AI guide (summary + key topic chips) and hierarchical AI semantic index tree.

ALTER TABLE sources ADD COLUMN guide_summary_md TEXT;
ALTER TABLE sources ADD COLUMN guide_topics_json TEXT;
ALTER TABLE sources ADD COLUMN ai_indexed INTEGER NOT NULL DEFAULT 0 CHECK (ai_indexed IN (0,1));

ALTER TABLE source_chunks ADD COLUMN node_id TEXT;
CREATE INDEX IF NOT EXISTS idx_chunks_source_node ON source_chunks(source_id, node_id);

CREATE TABLE IF NOT EXISTS source_index_nodes (
    id            TEXT PRIMARY KEY,
    source_id     TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    parent_id     TEXT,
    ordinal       INTEGER NOT NULL,
    level         INTEGER NOT NULL DEFAULT 1,
    title         TEXT NOT NULL,
    summary       TEXT NOT NULL DEFAULT '',
    keywords_json TEXT NOT NULL DEFAULT '[]',
    page_start    INTEGER,
    page_end      INTEGER,
    char_start    INTEGER,
    char_end      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_index_nodes_source ON source_index_nodes(source_id, ordinal);
