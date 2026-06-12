CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    ended_at TEXT NULL,
    sample_rate INTEGER NOT NULL DEFAULT 16000,
    channel_count INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS asr_raw_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    start_sec REAL NOT NULL,
    end_sec REAL NOT NULL,
    wall_start TEXT NOT NULL,
    wall_end TEXT NOT NULL,
    text_raw TEXT NOT NULL,
    opt_status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id),
    UNIQUE(session_id, revision)
);

CREATE INDEX IF NOT EXISTS idx_asr_raw_records_session_time ON asr_raw_records(session_id, start_sec);

CREATE TABLE IF NOT EXISTS asr_llm_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    text_optimized TEXT NOT NULL,
    text_english TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id),
    UNIQUE(session_id, revision)
);

CREATE TABLE IF NOT EXISTS correction_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    target TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 100,
    updated_at TEXT NOT NULL,
    UNIQUE(source, target)
);

CREATE TABLE IF NOT EXISTS correction_rule_versions (
    version INTEGER PRIMARY KEY,
    checksum TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
