ALTER TABLE asr_raw_records ADD COLUMN segment_id INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_asr_raw_records_session_seg ON asr_raw_records(session_id, segment_id);
