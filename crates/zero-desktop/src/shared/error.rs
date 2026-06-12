/// 将 `anyhow::Error` 转为前端可用的字符串。
#[allow(dead_code)]
pub fn to_cmd_err(e: anyhow::Error) -> String {
    format!("{e:#}")
}
