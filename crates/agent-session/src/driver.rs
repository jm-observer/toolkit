//! 驱动外部编码 Agent 会话：构造 CLI 命令 + spawn 子进程发一轮消息。
//!
//! 设计要点（见 plan.md §3 / 任务硬约束）：
//! - **命令构造是纯函数**（[`codex_argv`] / [`claude_argv`]），可脱离真进程单测。
//! - **输出解析是纯函数**（[`parse_codex_stdout`] / [`parse_claude_stdout`]），用 fixture 单测。
//! - 真正 spawn 的 [`send`] 只是把以上两步接到 `tokio::process::Command`，**不在单测里跑**
//!   （会真实消耗 codex / claude 额度）。
//!
//! 固化命令形态（已于 2026-06-15 真机实跑核实，见 runbook §5）：
//! - Codex：`codex exec -s workspace-write -c approval_policy="never" --cd <repo> resume --json <id> <prompt>`
//!   stdout 是事件 JSONL：`thread.started` / `turn.started` / `item.completed`{item} / `turn.completed`{usage}。
//!   回复 = 末个 `item.completed` 且 `item.type=="agent_message"` 的 `item.text`。
//!   ⚠️ Windows 下 stdout 可能混入非 UTF-8（GBK）噪声行（如 taskkill 的「成功: 已终止 PID…」）——
//!   `run_capture` 用 `from_utf8_lossy` 兜底，噪声行变替换字符后作为非 JSON 行跳过。
//! - Claude：`claude -p <prompt> --resume <id> --permission-mode acceptEdits`，
//!   `Command::current_dir(cwd)`，stdout 即纯文本回复（实测干净 UTF-8）。

use crate::{Provider, SessionRef, TurnResult};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// stdout 末段保留长度（排障用），避免把超长输出整段塞进 TurnResult。
const RAW_TAIL_MAX: usize = 4096;

/// 单轮 CLI 执行硬超时：codex / claude 网络卡死、等交互、子进程不退出时兜底，
/// 防止任务永久 running。超时后 kill 子进程并返回基础设施错（→ 任务 failed）。
/// 取较宽松值（含 agent 真实改文件的耗时），仅防真正的 hang。
const TURN_TIMEOUT: Duration = Duration::from_secs(1200);

/// 构造 Codex `exec resume` 的 argv（不含 `codex` 程序名本身）。
///
/// `repo_root` 用于 `--cd`：Codex `exec resume` 虽不挑当前目录，但传入工作树根更稳妥。
pub fn codex_argv(repo_root: &Path, session_id: &str, prompt: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        "-s".to_string(),
        "workspace-write".to_string(),
        "-c".to_string(),
        "approval_policy=\"never\"".to_string(),
        "--cd".to_string(),
        repo_root.to_string_lossy().to_string(),
        "resume".to_string(),
        "--json".to_string(),
        session_id.to_string(),
        prompt.to_string(),
    ]
}

/// 构造 Codex `exec`（**新建会话**，不带 `resume`）的 argv（不含 `codex` 程序名本身）。
///
/// 与 [`codex_argv`] 同形但省去 `resume <id>`：codex 会创建一个新会话，其 id 由首个
/// `thread.started` 事件的 `thread_id` 给出（见 [`parse_codex_thread_id`]）。
pub fn codex_create_argv(repo_root: &Path, prompt: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        "-s".to_string(),
        "workspace-write".to_string(),
        "-c".to_string(),
        "approval_policy=\"never\"".to_string(),
        "--cd".to_string(),
        repo_root.to_string_lossy().to_string(),
        "--json".to_string(),
        prompt.to_string(),
    ]
}

/// 从 codex `exec --json` stdout 解析新建会话 id：首个 `thread.started` 事件的 `thread_id`。
pub fn parse_codex_thread_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) == Some("thread.started") {
            if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// 构造 Claude `-p --resume` 的 argv（不含 `claude` 程序名本身）。
///
/// 注意：Claude 必须在会话原始 cwd 下 spawn（由调用方 `current_dir` 设置），argv 不含目录。
pub fn claude_argv(session_id: &str, prompt: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        prompt.to_string(),
        "--resume".to_string(),
        session_id.to_string(),
        "--permission-mode".to_string(),
        "acceptEdits".to_string(),
    ]
}

/// 解析 Codex `exec --json` 的 stdout（事件 JSONL）。
///
/// 优先按真机固化 schema：取末个 `item.completed` 且 `item.type=="agent_message"` 的 `item.text`。
/// 退化兼容旧/rollout 形态：`task_complete.last_agent_message`、`agent_message.message`。
/// 都没有时回复为空（调用方据 raw_tail 排障）。坏行 / 非 UTF-8 噪声行（lossy 后非 JSON）跳过。
pub fn parse_codex_stdout(stdout: &str) -> TurnResult {
    let mut last_item: Option<String> = None;
    let mut last_complete: Option<String> = None;
    let mut last_agent: Option<String> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        // 真机固化 schema：codex exec --json 顶层事件 item.completed{item}。
        if v.get("type").and_then(Value::as_str) == Some("item.completed") {
            if let Some(item) = v.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("agent_message") {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        last_item = Some(t.trim().to_string());
                    }
                }
            }
            continue;
        }
        // 退化：rollout / 旧形态（type 可能平铺顶层或在 payload 下）。
        let payload = v.get("payload").unwrap_or(&v);
        match payload.get("type").and_then(Value::as_str) {
            Some("task_complete") => {
                if let Some(m) = payload.get("last_agent_message").and_then(Value::as_str) {
                    last_complete = Some(m.trim().to_string());
                }
            }
            Some("agent_message") => {
                if let Some(m) = payload.get("message").and_then(Value::as_str) {
                    last_agent = Some(m.trim().to_string());
                }
            }
            _ => {}
        }
    }
    let reply_text = last_item
        .or(last_complete)
        .or(last_agent)
        .unwrap_or_default();
    TurnResult {
        reply_text,
        raw_tail: tail(stdout),
    }
}

/// 解析 Claude `-p` 的 stdout（纯文本回复）。
pub fn parse_claude_stdout(stdout: &str) -> TurnResult {
    TurnResult {
        reply_text: stdout.trim().to_string(),
        raw_tail: tail(stdout),
    }
}

fn tail(s: &str) -> String {
    if s.len() <= RAW_TAIL_MAX {
        return s.to_string();
    }
    // 退到最近的 char 边界，避免切断多字节字符。
    let mut start = s.len() - RAW_TAIL_MAX;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(truncated){}", &s[start..])
}

/// 发一轮消息到指定会话，阻塞至本轮完成，返回解析后的回复。
///
/// **会真实调用 codex / claude CLI 并消耗额度**——不要在单测中调用。
pub async fn send(s: &SessionRef, prompt: &str) -> Result<TurnResult> {
    match s.provider {
        Provider::Codex => {
            let argv = codex_argv(&s.cwd, &s.session_id, prompt);
            let stdout = run_capture("codex", &argv, None).await?;
            Ok(parse_codex_stdout(&stdout))
        }
        Provider::Claude => {
            let argv = claude_argv(&s.session_id, prompt);
            // Claude resume 必须在会话原始 cwd 下执行。
            let stdout = run_capture("claude", &argv, Some(&s.cwd)).await?;
            Ok(parse_claude_stdout(&stdout))
        }
    }
}

/// 新建一个 Codex 会话：在 `cwd` 下跑一轮 `codex exec`（无 resume），返回新会话 id。
///
/// **会真实调用 codex CLI 并消耗额度**——不要在单测中调用。`prompt` 是建立会话的种子提示词。
pub async fn create_codex_session(cwd: &Path, prompt: &str) -> Result<String> {
    let argv = codex_create_argv(cwd, prompt);
    let stdout = run_capture("codex", &argv, Some(cwd)).await?;
    parse_codex_thread_id(&stdout)
        .ok_or_else(|| anyhow!("codex 新建会话未返回 thread_id；stdout 末段：{}", tail(&stdout)))
}

/// 起子进程并捕获 stdout；非零退出码视为基础设施错误（`Err`）。
///
/// 带 [`TURN_TIMEOUT`] 硬超时：`stdin` 接 null（CLI 误等交互时立即 EOF），`kill_on_drop`
/// 保证超时丢弃 future 时子进程被杀，避免僵尸进程 / 任务永久 running。
async fn run_capture(program: &str, argv: &[String], cwd: Option<&Path>) -> Result<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn {program} 失败（CLI 是否已安装并在 PATH 中？）"))?;
    let output = match tokio::time::timeout(TURN_TIMEOUT, child.wait_with_output()).await {
        Ok(r) => r.with_context(|| format!("等待 {program} 子进程退出失败"))?,
        // 超时：child future 在此被丢弃，kill_on_drop 触发子进程终止。
        Err(_) => {
            return Err(anyhow!(
                "{program} 单轮执行超时（{TURN_TIMEOUT:?}），已终止子进程",
            ));
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "{program} 退出码 {:?}：{}",
            output.status.code(),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn codex_argv_shape() {
        let argv = codex_argv(
            &PathBuf::from("D:/git/repo"),
            "11111111-aaaa",
            "请复核 docs/plan.md",
        );
        assert_eq!(
            argv,
            vec![
                "exec",
                "-s",
                "workspace-write",
                "-c",
                "approval_policy=\"never\"",
                "--cd",
                "D:/git/repo",
                "resume",
                "--json",
                "11111111-aaaa",
                "请复核 docs/plan.md",
            ]
        );
    }

    #[test]
    fn claude_argv_shape() {
        let argv = claude_argv("sess-1", "据意见修订");
        assert_eq!(
            argv,
            vec![
                "-p",
                "据意见修订",
                "--resume",
                "sess-1",
                "--permission-mode",
                "acceptEdits",
            ]
        );
    }

    #[test]
    fn codex_create_argv_has_no_resume() {
        let argv = codex_create_argv(&PathBuf::from("D:/git/repo"), "建立会话");
        assert_eq!(
            argv,
            vec![
                "exec",
                "-s",
                "workspace-write",
                "-c",
                "approval_policy=\"never\"",
                "--cd",
                "D:/git/repo",
                "--json",
                "建立会话",
            ]
        );
        assert!(!argv.iter().any(|a| a == "resume"));
    }

    #[test]
    fn parse_codex_thread_id_from_started_event() {
        let stdout = concat!(
            r#"{"type":"thread.started","thread_id":"019eca83-aaaa"}"#,
            "\n",
            r#"{"type":"turn.started"}"#,
            "\n",
        );
        assert_eq!(
            parse_codex_thread_id(stdout),
            Some("019eca83-aaaa".to_string())
        );
    }

    #[test]
    fn parse_codex_thread_id_none_when_absent() {
        let stdout = r#"{"type":"turn.started"}"#;
        assert_eq!(parse_codex_thread_id(stdout), None);
    }

    #[test]
    fn parse_codex_real_json_schema() {
        // 2026-06-15 真机 `codex exec --json` 实测序列（含一行 lossy 后的非 JSON 噪声）。
        let stdout = concat!(
            r#"{"type":"thread.started","thread_id":"019eca83"}"#,
            "\n",
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK\nVERDICT: PASS"}}"#,
            "\n",
            "\u{fffd}\u{fffd}\u{fffd}: \u{fffd}\u{fffd}ֹ PID 14008\n", // GBK 噪声 lossy 后
            r#"{"type":"turn.completed","usage":{"output_tokens":3187}}"#,
            "\n",
        );
        let r = parse_codex_stdout(stdout);
        assert_eq!(r.reply_text, "OK\nVERDICT: PASS");
    }

    #[test]
    fn parse_codex_item_completed_wins_over_legacy() {
        // 同时出现新旧两种形态时，以新 schema 的 item.completed 为准。
        let stdout = concat!(
            r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"旧形态"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"新形态"}}"#,
            "\n",
        );
        assert_eq!(parse_codex_stdout(stdout).reply_text, "新形态");
    }

    #[test]
    fn parse_codex_prefers_task_complete() {
        let stdout = concat!(
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"中间回复"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"最终回复\nVERDICT: PASS"}}"#,
            "\n",
        );
        let r = parse_codex_stdout(stdout);
        assert_eq!(r.reply_text, "最终回复\nVERDICT: PASS");
    }

    #[test]
    fn parse_codex_falls_back_to_agent_message() {
        let stdout = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"只有 agent_message"}}"#;
        let r = parse_codex_stdout(stdout);
        assert_eq!(r.reply_text, "只有 agent_message");
    }

    #[test]
    fn parse_codex_skips_bad_lines_and_empty() {
        let stdout = concat!(
            "这是坏行\n",
            "\n",
            r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":" 带空白 "}}"#,
        );
        let r = parse_codex_stdout(stdout);
        assert_eq!(r.reply_text, "带空白");
    }

    #[test]
    fn parse_codex_handles_top_level_payload_shape() {
        // 某些版本把 type 平铺在顶层而非 payload。
        let stdout = r#"{"type":"task_complete","last_agent_message":"平铺形态"}"#;
        let r = parse_codex_stdout(stdout);
        assert_eq!(r.reply_text, "平铺形态");
    }

    #[test]
    fn parse_codex_empty_when_no_reply() {
        let r = parse_codex_stdout(r#"{"type":"event_msg","payload":{"type":"task_started"}}"#);
        assert!(r.reply_text.is_empty());
    }

    #[test]
    fn parse_claude_trims_text() {
        let r = parse_claude_stdout("  已完成修订。\nVERDICT 不在这里  \n");
        assert_eq!(r.reply_text, "已完成修订。\nVERDICT 不在这里");
    }

    #[test]
    fn raw_tail_truncates_long_output() {
        let big = "a".repeat(RAW_TAIL_MAX + 100);
        let r = parse_claude_stdout(&big);
        assert!(r.raw_tail.starts_with("…(truncated)"));
        assert!(r.raw_tail.len() < big.len());
    }
}
