-- Local Notebook Agent: SQLite WAL + FTS5 + sqlite-vec 本地存储 Schema
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS notebooks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    emoji TEXT DEFAULT '📚',
    summary TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,
    notebook_id TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    file_type TEXT NOT NULL,         -- 'pdf' | 'md' | 'txt' | 'web'
    file_path TEXT,
    is_selected INTEGER NOT NULL DEFAULT 1,
    content_hash TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS source_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    page_or_section TEXT,
    content TEXT NOT NULL,
    token_count INTEGER NOT NULL
);

-- BM25 全文检索虚拟表
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    content,
    content='source_chunks',
    content_rowid='id',
    tokenize='unicode61'
);

-- 向量检索虚拟表 (需加载 sqlite-vec 扩展; 以 bge-small-zh 512 维为例)
-- CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
--     chunk_id INTEGER PRIMARY KEY,
--     embedding FLOAT[512]
-- );

CREATE TABLE IF NOT EXISTS model_profiles (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    protocol TEXT NOT NULL,          -- 'chat_completion' | 'responses' | 'anthropic' | 'gemini'
    base_url TEXT NOT NULL,
    model_id TEXT NOT NULL,
    keychain_ref TEXT NOT NULL,      -- 仅保存 OS Keychain/DPAPI 引用键，不存明文 Key
    enable_thinking INTEGER NOT NULL DEFAULT 1,
    is_active INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY,
    notebook_id TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    role TEXT NOT NULL,              -- 'user' | 'assistant'
    thoughts_json TEXT,              -- 折叠推理步骤 JSON
    content TEXT NOT NULL,
    citations_json TEXT,             -- 引用切片 ID 映射
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS studio_artifacts (
    id TEXT PRIMARY KEY,
    notebook_id TEXT NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    artifact_type TEXT NOT NULL,     -- 'quiz' | 'mindmap' | 'report' | 'flashcards' | 'audio' | 'slides'
    title TEXT NOT NULL,
    status TEXT NOT NULL,            -- 'PENDING' | 'READY' | 'FAILED'
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
