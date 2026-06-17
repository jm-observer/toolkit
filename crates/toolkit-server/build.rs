use std::path::Path;
use std::process::Command;

/// 编译时取 git 短哈希，emit 给程序（`env!`/`option_env!("GIT_COMMIT")`）。
/// 注意：build.rs 跑在 host 上（交叉编译到 aarch64 时也是如此），用 host 的 git 即可。
/// git 失败 / 非 git 环境一律兜底为 "unknown"，绝不 panic。
fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());

    // 让 commit 变了能重编：仓库根 .git/HEAD（crate 在 crates/toolkit-server/ 下，
    // 仓库根 = ../../）。路径取不到就算了，不因此失败。
    let head = Path::new(&manifest_dir).join("../../.git/HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
    }

    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT={hash}");
}
