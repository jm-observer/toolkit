//! §4.1 三方一致性校验：Claude.cwd / Codex.cwd / target_path 各自向上找 `.git` 求 repo
//! root，要求三者相等且 target 落在 root 内。拒绝跑错仓。

use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

/// 从 `start` 向上逐级找含 `.git` 的目录（工作树根）。找不到返回 `None`。
///
/// 纯文件系统查找，不依赖 git 进程。`.git` 可以是目录（普通仓）或文件（worktree / submodule）。
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// 校验结果：三方共同的 repo root，以及 canonical 化后的 target 绝对路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub repo_root: PathBuf,
    pub target_abs: PathBuf,
}

/// 三方一致性校验。
///
/// - `claude_cwd` / `codex_cwd`：两会话解析出的工作目录。
/// - `target_path`：可为相对（相对 repo root）或绝对路径。
///
/// 成功返回共同 repo root + target 绝对路径；任一不一致 / 找不到 `.git` / target 越界 → `Err`
/// （消息含三方实际路径，供 400 提示）。
pub fn validate_three_way(
    claude_cwd: &Path,
    codex_cwd: &Path,
    target_path: &str,
) -> Result<Validated> {
    let claude_root = find_repo_root(claude_cwd)
        .ok_or_else(|| anyhow!("Claude cwd 向上未找到 .git：{}", claude_cwd.display()))?;
    let codex_root = find_repo_root(codex_cwd)
        .ok_or_else(|| anyhow!("Codex cwd 向上未找到 .git：{}", codex_cwd.display()))?;

    // target 可能是相对路径（相对 claude_root 解析）或绝对路径。
    let target_raw = {
        let p = Path::new(target_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            claude_root.join(p)
        }
    };
    let target_root = find_repo_root(&target_raw)
        .ok_or_else(|| anyhow!("target_path 向上未找到 .git：{}", target_raw.display()))?;

    // 三方 repo root 必须相等（canonicalize 抹平 .. / 大小写 / 符号链接差异）。
    let cr = canon(&claude_root);
    let dr = canon(&codex_root);
    let tr = canon(&target_root);
    if cr != dr || cr != tr {
        bail!(
            "三方不在同一工作树：Claude repo root={}，Codex repo root={}，target repo root={}（claude cwd={}，codex cwd={}，target={}）",
            claude_root.display(),
            codex_root.display(),
            target_root.display(),
            claude_cwd.display(),
            codex_cwd.display(),
            target_path,
        );
    }

    // target 必须落在 root 内。
    let target_abs = canon(&target_raw);
    if !target_abs.starts_with(&cr) {
        bail!(
            "target_path 不在 repo root 内：target={}，root={}",
            target_abs.display(),
            cr.display(),
        );
    }

    Ok(Validated {
        repo_root: cr,
        target_abs,
    })
}

/// canonicalize 兜底：路径不存在时退回逻辑归一（避免对未创建的 target 文件报错）。
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 造一个临时 git 工作树：root/.git + root/docs/foo.md + root/crates/x/。
    fn make_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("crates/x")).unwrap();
        fs::write(root.join("docs/foo.md"), "doc").unwrap();
        tmp
    }

    #[test]
    fn find_root_walks_up() {
        let tmp = make_repo();
        let root = canon(tmp.path());
        let from_sub = find_repo_root(&tmp.path().join("crates/x")).unwrap();
        assert_eq!(canon(&from_sub), root);
    }

    #[test]
    fn find_root_none_when_no_git() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_repo_root(tmp.path()).is_none());
    }

    #[test]
    fn three_way_ok_from_subdirs() {
        let tmp = make_repo();
        let root = tmp.path();
        // Claude 从 docs 启动，Codex 从 crates/x 启动，target 相对路径。
        let v =
            validate_three_way(&root.join("docs"), &root.join("crates/x"), "docs/foo.md").unwrap();
        assert_eq!(v.repo_root, canon(root));
        assert_eq!(v.target_abs, canon(&root.join("docs/foo.md")));
    }

    #[test]
    fn three_way_rejects_different_trees() {
        let a = make_repo();
        let b = make_repo();
        let err = validate_three_way(
            &a.path().join("docs"),
            &b.path().join("crates/x"),
            "docs/foo.md",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("同一工作树"));
    }

    #[test]
    fn three_way_rejects_no_git() {
        let tmp = tempfile::tempdir().unwrap();
        let err = validate_three_way(tmp.path(), tmp.path(), "foo.md").unwrap_err();
        assert!(format!("{err}").contains(".git"));
    }
}
