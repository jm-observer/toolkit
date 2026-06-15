use anyhow::{Context, Result};
use rusqlite::Connection;

pub(crate) fn run_migrations(conn: &Connection) -> Result<()> {
    let sql_0001 = include_str!("../../../../migrations/0001_init.sql");
    conn.execute_batch(sql_0001)
        .context("failed to run sqlite migration 0001_init.sql")?;
    if !column_exists(conn, "asr_raw_records", "optimize_status")? {
        let sql_0002 = include_str!("../../../../migrations/0002_split_optimize_translate.sql");
        conn.execute_batch(sql_0002)
            .context("failed to run sqlite migration 0002_split_optimize_translate.sql")?;
    }
    if !column_exists(conn, "asr_raw_records", "segment_id")? {
        let sql_0003 = include_str!("../../../../migrations/0003_add_segment_id.sql");
        conn.execute_batch(sql_0003)
            .context("failed to run sqlite migration 0003_add_segment_id.sql")?;
    }
    if !column_exists(conn, "asr_raw_records", "is_discarded")? {
        let sql_0004 = include_str!("../../../../migrations/0004_add_discard_fields.sql");
        conn.execute_batch(sql_0004)
            .context("failed to run sqlite migration 0004_add_discard_fields.sql")?;
    }
    // Backfill legacy rows introduced with default segment_id=0.
    conn.execute_batch("UPDATE asr_raw_records SET segment_id = id WHERE segment_id = 0;")
        .context("failed to backfill legacy segment_id")?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_asr_raw_records_session_seg
         ON asr_raw_records(session_id, segment_id);",
    )
    .context("failed to ensure unique index idx_asr_raw_records_session_seg")?;
    // 0005：标注样本表（CREATE TABLE IF NOT EXISTS，幂等，无条件执行）。
    let sql_0005 = include_str!("../../../../migrations/0005_speech_samples.sql");
    conn.execute_batch(sql_0005)
        .context("failed to run sqlite migration 0005_speech_samples.sql")?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to prepare table info query for {table}"))?;
    let mut rows = stmt
        .query([])
        .with_context(|| format!("failed to query table info for {table}"))?;
    while let Some(row) = rows.next().context("failed to read table info row")? {
        let name: String = row.get(1).context("failed to get table column name")?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::speech::db::repository::{self, NewSample};

    #[test]
    fn migrations_create_speech_samples_and_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // 表存在且 0005 列齐全。
        assert!(column_exists(&conn, "speech_samples", "audio_status").unwrap());
        assert!(column_exists(&conn, "speech_samples", "hotword_sync").unwrap());

        // 插入 → 列出 → 读回。
        let id = repository::insert_sample(
            &conn,
            &NewSample {
                segment_id: 42,
                session_id: Some("sess-1".into()),
                label: "hotword".into(),
                text_raw: "旧菜盒子".into(),
                text_optimized: None,
                text_english: None,
                text_secondary: None,
                correction: Some("韭菜盒子".into()),
                note: None,
                audio_status: "skipped".into(),
                marked_at: "2026-06-15 10:00:00".into(),
            },
        )
        .unwrap();
        repository::update_sample_audio(&conn, id, Some("/x/1.wav"), "saved").unwrap();
        repository::update_sample_hotword_sync(&conn, id, "added").unwrap();

        let rows = repository::list_samples(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.segment_id, 42);
        assert_eq!(r.audio_status, "saved");
        assert_eq!(r.audio_path.as_deref(), Some("/x/1.wav"));
        assert_eq!(r.hotword_sync.as_deref(), Some("added"));

        let one = repository::get_sample(&conn, id).unwrap().unwrap();
        assert_eq!(one.label, "hotword");
        assert_eq!(one.correction.as_deref(), Some("韭菜盒子"));
    }
}
