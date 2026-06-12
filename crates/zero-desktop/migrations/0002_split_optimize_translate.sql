ALTER TABLE asr_raw_records ADD COLUMN optimize_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE asr_raw_records ADD COLUMN translate_status TEXT NOT NULL DEFAULT 'blocked';

UPDATE asr_raw_records
SET optimize_status = CASE
    WHEN opt_status = 'done' THEN 'success'
    WHEN opt_status = 'failed' THEN 'failed'
    WHEN opt_status = 'running' THEN 'running'
    WHEN opt_status = 'skipped' THEN 'failed'
    ELSE 'pending'
END;

UPDATE asr_raw_records
SET translate_status = CASE
    WHEN opt_status = 'done' THEN 'success'
    ELSE 'blocked'
END;

CREATE TABLE asr_llm_results_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    text_optimized TEXT,
    text_english TEXT,
    optimize_error TEXT,
    translate_error TEXT,
    optimize_started_at TEXT,
    optimize_finished_at TEXT,
    translate_started_at TEXT,
    translate_finished_at TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id),
    UNIQUE(session_id, revision)
);

INSERT INTO asr_llm_results_new (id, session_id, revision, text_optimized, text_english, created_at)
SELECT id, session_id, revision, text_optimized, text_english, created_at
FROM asr_llm_results;

DROP TABLE asr_llm_results;
ALTER TABLE asr_llm_results_new RENAME TO asr_llm_results;
