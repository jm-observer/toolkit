use rusqlite::params;
use toolkit_core::{migrate, now_iso8601, open_pool};
use toolkit_tasks::recover_interrupted;

#[test]
fn recovery_marks_residual_tasks_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_pool(&dir.path().join("t.db")).unwrap();
    migrate(&pool).unwrap();

    {
        let conn = pool.get().unwrap();
        for (id, state) in [
            ("tk_running01____", "running"),
            ("tk_queued002____", "queued"),
            ("tk_done003______", "succeeded"),
        ] {
            conn.execute(
                "INSERT INTO tasks(task_id, kind, state, input, progress, created_at)
                 VALUES (?1, 'echo', ?2, '{}', '{}', ?3)",
                params![id, state, now_iso8601()],
            )
            .unwrap();
        }
    }

    let n = recover_interrupted(&pool).unwrap();
    assert_eq!(n, 2);

    let conn = pool.get().unwrap();
    let s: String = conn
        .query_row(
            "SELECT state FROM tasks WHERE task_id='tk_running01____'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(s, "interrupted");
    let s: String = conn
        .query_row(
            "SELECT state FROM tasks WHERE task_id='tk_done003______'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(s, "succeeded");
}
