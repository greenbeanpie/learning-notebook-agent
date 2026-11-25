-- Images attached to a chat message (JSON array of lna_types::Attachment).
ALTER TABLE chat_messages ADD COLUMN attachments_json TEXT;
