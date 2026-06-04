use toolkit_core::{migrate, open_pool};

#[test]
fn migrate_creates_all_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("toolkit.db");
    let pool = open_pool(&db_path).unwrap();
    migrate(&pool).unwrap();

    let conn = pool.get().unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for needed in [
        "browser_sessions",
        "cookies",
        "creators",
        "meta",
        "tasks",
        "works",
    ] {
        assert!(rows.iter().any(|n| n == needed), "missing table {needed}");
    }

    let v: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "1");
}

#[test]
fn migrate_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("toolkit.db");
    let pool = open_pool(&db_path).unwrap();
    migrate(&pool).unwrap();
    migrate(&pool).unwrap();
    migrate(&pool).unwrap();
}
