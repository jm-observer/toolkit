use anyhow::{Context, Result};
use std::path::Path;

/// 幂等建立 workspace 目录树。
pub fn ensure_workspace(path: &Path) -> Result<()> {
    for sub in &["logs", "english", "speech", "cookie"] {
        let dir = path.join(sub);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create workspace subdir: {}", dir.display()))?;
    }
    Ok(())
}
