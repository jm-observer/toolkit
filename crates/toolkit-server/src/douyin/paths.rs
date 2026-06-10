//! 从 toolkit workspace 解析 douyin crate 所需的各项目录/文件路径。
//!
//! 与 douyin 库自身的 `resolve_*` 函数语义对齐，但**不依赖** `ZERO_WORKSPACE` 环境变量。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DouyinPaths {
    pub cookie_file: PathBuf,
    pub task_dir: PathBuf,
    pub out_dir: PathBuf,
    pub transcript_dir: PathBuf,
    pub refined_dir: PathBuf,
    pub works_dir: PathBuf,
    pub knowledge_dir: PathBuf,
}

impl DouyinPaths {
    pub fn new(workspace: &Path) -> Self {
        let douyin = workspace.join("douyin");
        Self {
            cookie_file: douyin.join("cookies.json"),
            task_dir: douyin.join("tasks"),
            out_dir: workspace.join("downloads").join("douyin"),
            transcript_dir: douyin.join("transcripts"),
            refined_dir: douyin.join("refined"),
            works_dir: douyin.join("works"),
            knowledge_dir: workspace.join("knowledge").join("douyin"),
        }
    }

    /// 启动 / 任务执行前确保目录存在。cookie_file 的父目录也建好。
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            &self.task_dir,
            &self.out_dir,
            &self.transcript_dir,
            &self.refined_dir,
            &self.works_dir,
            &self.knowledge_dir,
        ] {
            std::fs::create_dir_all(d)
                .with_context(|| format!("create_dir_all {}", d.display()))?;
        }
        if let Some(parent) = self.cookie_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
        Ok(())
    }
}
