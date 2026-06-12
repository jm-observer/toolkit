-- Add discard judgment fields to asr_raw_records
ALTER TABLE asr_raw_records ADD COLUMN is_discarded BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE asr_raw_records ADD COLUMN discard_reason TEXT NULL;
ALTER TABLE asr_raw_records ADD COLUMN discard_source TEXT NULL;
ALTER TABLE asr_raw_records ADD COLUMN discard_confidence REAL NULL;
ALTER TABLE asr_raw_records ADD COLUMN quality_check_status TEXT NOT NULL DEFAULT 'pending';