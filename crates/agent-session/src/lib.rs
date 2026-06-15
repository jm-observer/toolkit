//! agent-session：provider 无关的「外部编码 Agent 会话」只读观测库。
//!
//! 把 zero 仓 `scripts/probe_sessions.py` 探针的会话存储解析逻辑移植为 Rust：
//! 读取本机磁盘上 Codex / Claude Code 的明文 JSONL 会话存储，解析状态 / 消息 / cwd。
//!
//! 读存储（`store`）只读、不调用任何模型。驱动子进程发消息（`driver`）会真实调用
//! codex / claude CLI 消耗额度；轮询等待（`watch`）只读文件状态。

pub mod driver;
pub mod store;
pub mod watch;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 会话所属的编码 Agent provider。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    /// URL / CLI 友好的小写标识。
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
        }
    }

    /// 从字符串解析（`codex` / `claude`，大小写不敏感）。
    pub fn parse(s: &str) -> Option<Provider> {
        match s.to_ascii_lowercase().as_str() {
            "codex" => Some(Provider::Codex),
            "claude" => Some(Provider::Claude),
            _ => None,
        }
    }
}

/// 定位一个具体会话所需的最小信息：provider + 会话 id + 原始工作目录。
///
/// 两边都带 `cwd`：
/// - Claude：来自 jsonl 事件的 `cwd` 字段；resume 必须在此目录下 spawn。
/// - Codex：来自 rollout 的 `session_meta.cwd` / `turn_context.cwd`；
///   `exec resume` 虽不挑当前目录，但用于仓库一致性校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub provider: Provider,
    pub session_id: String,
    pub cwd: PathBuf,
}

/// 会话当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// 本轮已结束（Codex 无未闭合 turn / Claude 末条 assistant 是 end_turn）。
    Idle,
    /// 正在生成（Codex 有未闭合 turn / Claude 末条 assistant 是 tool_use）。
    Generating,
    /// 已收到用户输入但尚无回复（Claude 末条是 user）。
    Processing,
    /// 无法判断（空会话 / 未知 stop_reason / 文件缺失）。
    Unknown,
}

/// 会话清单 / 快照的轻量摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub provider: Provider,
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub updated_at: String,
}

/// 单会话快照：摘要信息 + 最近一条自然语言回复 + cwd。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub provider: Provider,
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub latest_reply: String,
    pub updated_at: String,
    pub cwd: PathBuf,
}

/// 会话消息流中的一条消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// `user` / `assistant`。
    pub role: String,
    /// 已渲染的正文（user/assistant 文本；纯 tool_use 标记为 `[tool_use: name]`，thinking 标记为 `[thinking]`）。
    pub text: String,
    pub timestamp: String,
}

/// 一次 `messages` 增量查询的结果：从 `after` 行号起的新消息 + 新游标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagesPage {
    pub messages: Vec<SessionMessage>,
    /// 新游标 = 已读到的 JSONL 行数（含本次返回，供下次 `after` 传入）。
    pub cursor: usize,
}

/// 一轮驱动的结果（驱动属后续 Plan，类型先定义）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnResult {
    /// 自然语言回复（跳过纯 tool_use）。
    pub reply_text: String,
    /// 末段原始输出，排障用。
    pub raw_tail: String,
}
