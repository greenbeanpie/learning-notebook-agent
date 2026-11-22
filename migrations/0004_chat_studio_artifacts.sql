-- Track whether a Studio artifact is shown in the right-side Studio panel or originated inside chat
ALTER TABLE studio_artifacts
    ADD COLUMN in_studio INTEGER NOT NULL DEFAULT 1 CHECK (in_studio IN (0, 1));

ALTER TABLE studio_artifacts
    ADD COLUMN origin_message_id TEXT REFERENCES chat_messages(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_artifacts_origin_message
    ON studio_artifacts(origin_message_id)
    WHERE origin_message_id IS NOT NULL;
