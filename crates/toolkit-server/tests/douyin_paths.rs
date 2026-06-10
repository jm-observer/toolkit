use toolkit_server::douyin_mod::paths::DouyinPaths;

#[test]
fn paths_layout_matches_design() {
    let p = DouyinPaths::new(std::path::Path::new("/tmp/toolkit"));
    assert_eq!(
        p.cookie_file,
        std::path::PathBuf::from("/tmp/toolkit/douyin/cookies.json")
    );
    assert_eq!(
        p.task_dir,
        std::path::PathBuf::from("/tmp/toolkit/douyin/tasks")
    );
    assert_eq!(
        p.out_dir,
        std::path::PathBuf::from("/tmp/toolkit/downloads/douyin")
    );
    assert_eq!(
        p.transcript_dir,
        std::path::PathBuf::from("/tmp/toolkit/douyin/transcripts")
    );
    assert_eq!(
        p.refined_dir,
        std::path::PathBuf::from("/tmp/toolkit/douyin/refined")
    );
    assert_eq!(
        p.works_dir,
        std::path::PathBuf::from("/tmp/toolkit/douyin/works")
    );
    assert_eq!(
        p.knowledge_dir,
        std::path::PathBuf::from("/tmp/toolkit/knowledge/douyin")
    );
}

#[test]
fn ensure_dirs_creates_everything() {
    let dir = tempfile::tempdir().unwrap();
    let p = DouyinPaths::new(dir.path());
    p.ensure_dirs().unwrap();
    assert!(p.task_dir.exists());
    assert!(p.out_dir.exists());
    assert!(p.transcript_dir.exists());
    assert!(p.refined_dir.exists());
    assert!(p.works_dir.exists());
    assert!(p.knowledge_dir.exists());
    assert!(p.cookie_file.parent().unwrap().exists());
}
