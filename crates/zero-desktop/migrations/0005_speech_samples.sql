-- 标注样本采集：把 segment 卡片标注为训练/纠错样本，存档音频 + 元信息。
CREATE TABLE IF NOT EXISTS speech_samples (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  segment_id     INTEGER NOT NULL,
  session_id     TEXT,
  label          TEXT NOT NULL,      -- asr_wrong | hotword | bad_optimize | ok | other
  text_raw       TEXT NOT NULL,
  text_optimized TEXT,
  text_english   TEXT,
  text_secondary TEXT,
  correction     TEXT,
  note           TEXT,
  audio_path     TEXT,
  audio_status   TEXT NOT NULL,      -- saved | expired | fetch_failed | skipped
  hotword_sync   TEXT,              -- hotword 标签专属: added | exists | failed | null
  marked_at      TEXT NOT NULL
);
