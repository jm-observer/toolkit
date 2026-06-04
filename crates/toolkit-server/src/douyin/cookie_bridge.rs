//! 把扩展推过来的 Cookie 同步写到 `<workspace>/douyin/cookies.json`，让 douyin crate 直接读用。
//!
//! 调用方：`/api/browser/cookie` 端点处理完 SQLite 写入后，再调本函数；失败仅警告。

use crate::douyin_mod::paths::DouyinPaths;
use anyhow::Result;
use std::path::Path;

/// 把 raw header 写到 douyin 期望的 cookies.json。
/// 内部调 `douyin::run_set_cookie` 复用其格式约定（v1 wrapper + 字段校验）。
pub async fn write_from_raw_header(workspace: &Path, raw_header: &str) -> Result<serde_json::Value> {
    let paths = DouyinPaths::new(workspace);
    paths.ensure_dirs()?;
    let v = douyin::run_set_cookie(&paths.cookie_file, raw_header).await?;
    Ok(v)
}
