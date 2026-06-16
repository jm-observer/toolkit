//! 公共大模型层：连接配置解析 + 可配提示词的「内置默认 + DB 覆盖」目录 + HTTP 路由。
//!
//! - **连接配置解析**（[`resolve_config`]）：DB（`llm_config` 表）优先，缺失回退环境变量
//!   （`LLM_BASE_URL` / `LLM_MODEL` / `LLM_API_KEY`）。两者都没有 → 明确报错。
//! - **提示词目录**：各功能在 [`builtins`] 注册内置默认（name + 语义版本 + 默认文本）。运行时
//!   解析（[`resolve_prompt`]）DB 行优先，缺失用内置默认；控制台改了就写 DB 覆盖、删 DB 行即
//!   「恢复内置默认」。
//!
//! 各功能（douyin 整理、对话总结、codeloop 文案…）都经此层取配置/提示词，不再各自读 env /
//! `include_str!`。

pub mod routes;

use anyhow::{anyhow, Context, Result};
use toolkit_core::llm_store;
use toolkit_core::SqlitePool;
use toolkit_llm::{prompt_hash, LlmClient, LlmConfig};

// ---- 内置提示词名字（其他模块引用，避免裸字符串散落）----
pub const NAME_DOUYIN_REFINE: &str = "douyin_refine";
pub const NAME_CHAT_SUMMARY: &str = "chat_summary";
pub const NAME_CODELOOP_CODEX_REVIEW: &str = "codeloop_codex_review";
pub const NAME_CODELOOP_CLAUDE_REVISION: &str = "codeloop_claude_revision";

/// 对话总结内置 prompt。`{CONVERSATION}` 占位符在调用时替换为粘贴的会话文本。
pub const CHAT_SUMMARY_PROMPT: &str =
    "你是会话总结助手。请阅读下面的对话内容，输出简洁的中文总结：\
先用一句话概述主题，再用要点列出关键结论 / 决定 / 待办（无则省略对应小节）。\
只输出总结本身，不要复述原文，不要编造对话中没有的信息。\n\n对话内容：\n{CONVERSATION}";

/// 一条内置提示词定义（编译期默认）。
pub struct BuiltinPrompt {
    pub name: &'static str,
    /// 人类可读说明（控制台展示）。
    pub description: &'static str,
    /// 语义版本（与功能自身的 PROMPT_VERSION 对齐）。
    pub version: &'static str,
    /// 占位符列表（仅用于控制台提示，例如 `{TRANSCRIPT}` / `{CONVERSATION}`）。
    pub placeholders: &'static [&'static str],
    /// 编译期默认文本。
    pub default_text: &'static str,
}

/// 全部内置提示词目录。新增可配提示词在此登记一行即可被控制台列出 / 编辑 / 重置。
pub fn builtins() -> Vec<BuiltinPrompt> {
    vec![
        BuiltinPrompt {
            name: NAME_DOUYIN_REFINE,
            description: "抖音 ASR 原文整理（纠错/去口语水词/分段/小结）",
            version: douyin::refine::PROMPT_VERSION,
            placeholders: &["{TRANSCRIPT}"],
            default_text: douyin::refine::REFINE_PROMPT,
        },
        BuiltinPrompt {
            name: NAME_CHAT_SUMMARY,
            description: "对话总结：粘贴会话文本 → 输出要点总结",
            version: "v1",
            placeholders: &["{CONVERSATION}"],
            default_text: CHAT_SUMMARY_PROMPT,
        },
        BuiltinPrompt {
            name: NAME_CODELOOP_CODEX_REVIEW,
            description: "codeloop 复核方（Codex）指令模板（走 CLI 会话，非 HTTP LLM）",
            version: codeloop_core::prompt::TEMPLATE_VERSION,
            placeholders: codeloop_core::prompt::CODEX_PLACEHOLDERS,
            default_text: codeloop_core::prompt::DEFAULT_CODEX_TEMPLATE,
        },
        BuiltinPrompt {
            name: NAME_CODELOOP_CLAUDE_REVISION,
            description: "codeloop 修订方（Claude）指令模板（走 CLI 会话，非 HTTP LLM）",
            version: codeloop_core::prompt::TEMPLATE_VERSION,
            placeholders: codeloop_core::prompt::CLAUDE_PLACEHOLDERS,
            default_text: codeloop_core::prompt::DEFAULT_CLAUDE_TEMPLATE,
        },
    ]
}

/// 查内置默认（按名字）。
pub fn builtin(name: &str) -> Option<BuiltinPrompt> {
    builtins().into_iter().find(|b| b.name == name)
}

/// 配置来源标记。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    Db,
    Env,
    None,
}

impl ConfigSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigSource::Db => "db",
            ConfigSource::Env => "env",
            ConfigSource::None => "none",
        }
    }
}

/// 解析有效连接配置：DB 优先（base_url + model 非空才算有效），否则环境变量。
pub fn resolve_config(pool: &SqlitePool) -> Result<LlmConfig> {
    if let Some(c) = llm_store::get_config(pool)? {
        if !c.base_url.trim().is_empty() && !c.model.trim().is_empty() {
            return Ok(LlmConfig::new(c.base_url, c.model, c.api_key));
        }
    }
    LlmConfig::from_env()
        .context("未配置大模型：请在控制台填写地址/模型，或设置 LLM_BASE_URL/LLM_MODEL 环境变量")
}

/// 仅判断来源（不报错）。
pub fn config_source(pool: &SqlitePool) -> Result<ConfigSource> {
    if let Some(c) = llm_store::get_config(pool)? {
        if !c.base_url.trim().is_empty() && !c.model.trim().is_empty() {
            return Ok(ConfigSource::Db);
        }
    }
    Ok(match LlmConfig::from_env() {
        Ok(_) => ConfigSource::Env,
        Err(_) => ConfigSource::None,
    })
}

/// 装配可用的 LLM 客户端（解析配置 → LlmClient）。
pub fn resolve_client(pool: &SqlitePool) -> Result<LlmClient> {
    LlmClient::new(resolve_config(pool)?)
}

/// 解析有效提示词文本：DB 覆盖优先，否则内置默认；都没有则报错（未知 name）。
pub fn resolve_prompt(pool: &SqlitePool, name: &str) -> Result<String> {
    if let Some(p) = llm_store::get_prompt(pool, name)? {
        return Ok(p.text);
    }
    builtin(name)
        .map(|b| b.default_text.to_string())
        .ok_or_else(|| anyhow!("未知提示词 {name}（既无 DB 覆盖也无内置默认）"))
}

/// 解析提示词的版本号（DB 覆盖优先，否则内置版本）。落产物元信息用。
pub fn resolve_prompt_version(pool: &SqlitePool, name: &str) -> Result<String> {
    if let Some(p) = llm_store::get_prompt(pool, name)? {
        return Ok(p.version);
    }
    builtin(name)
        .map(|b| b.version.to_string())
        .ok_or_else(|| anyhow!("未知提示词 {name}"))
}

/// 当前生效提示词的短哈希（落产物元信息用，配合 version 溯源）。
pub fn resolve_prompt_hash(pool: &SqlitePool, name: &str) -> Result<String> {
    Ok(prompt_hash(&resolve_prompt(pool, name)?))
}
