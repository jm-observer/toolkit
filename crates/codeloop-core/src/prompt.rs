//! 复核 / 修订 prompt 模板（中文）。见 RFC §4「Prompt 模板」。
//!
//! 文案做成**带占位符的模板字符串**，可被上层（toolkit-server 的 `llm` 可配提示词目录）从
//! toolkit.db 覆盖；缺省用本文件内置的 [`DEFAULT_CODEX_TEMPLATE`] / [`DEFAULT_CLAUDE_TEMPLATE`]。
//! 渲染时把动态值（label / 仓库定位 / 复核意见 / 轮次提示 / 复核口径）填入占位符。
//!
//! 注意：codeloop 走 Codex/Claude **CLI 会话**通道，本模板只是发给会话的指令文案——纳入「可配
//! 提示词」仅为统一管理文案，与 HTTP 大模型通道无关。

use serde::{Deserialize, Serialize};

/// 模板语义版本。改内置模板文案时同步 bump。
pub const TEMPLATE_VERSION: &str = "v1";

/// Codex 复核模板支持的占位符（供控制台提示）。
pub const CODEX_PLACEHOLDERS: &[&str] = &[
    "{LABEL}",
    "{SCOPE}",
    "{ROUND_HINT}",
    "{REPO_ROOT}",
    "{REPO_REL}",
    "{ABS}",
];
/// Claude 修订模板支持的占位符。
pub const CLAUDE_PLACEHOLDERS: &[&str] =
    &["{LABEL}", "{REVIEW}", "{REPO_ROOT}", "{REPO_REL}", "{ABS}"];

/// 复核口径（design）。
const DESIGN_SCOPE: &str = "只关注事实/逻辑/前后一致性/可行性错误，不纠结措辞。";
/// 复核口径（implementation）。
const IMPL_SCOPE: &str = "只关注实现是否符合设计、有无逻辑/边界/正确性错误，不纠结风格。";

/// Codex 复核内置模板。占位符见 [`CODEX_PLACEHOLDERS`]。
pub const DEFAULT_CODEX_TEMPLATE: &str = "请以严格审阅者身份复核{LABEL}。{SCOPE}\
逐条列出发现的问题（无问题写\"无\"）。最后另起一行只输出结论：\
无明显错误输出 `VERDICT: PASS`，否则 `VERDICT: NEEDS_WORK`。{ROUND_HINT}\
\n\n复核/修订对象明确为：工作树根 `{REPO_ROOT}` 下的 `{REPO_REL}`（绝对路径 `{ABS}`）。\
请只针对该文件，按上述绝对路径定位，不要改动其他文件。\
\n\n若遇到需要我方做选择的岔路（例如方案 A 还是 B、改动范围是 A 还是 B），不要自行假设。\
请只输出一行、以 `ASK_USER: ` 开头、后接一段合法 JSON，然后停止等我答复，例如：\
`ASK_USER: {\"question\": \"实现登录用哪种方案？\", \"options\": [\"方案A：JWT 无状态\", \"方案B：服务端 session\"]}`。\
无明确候选项时 options 可省（只给 question）。该行不要包含 JSON 之外的任何文字。";

/// Claude 修订内置模板。占位符见 [`CLAUDE_PLACEHOLDERS`]。
pub const DEFAULT_CLAUDE_TEMPLATE: &str = "Codex 对{LABEL}的复核意见如下：\n---\n{REVIEW}\n---\n\
请据此修订，只改确有问题处，并在回复末尾用一句话概述本轮改动。\
\n\n复核/修订对象明确为：工作树根 `{REPO_ROOT}` 下的 `{REPO_REL}`（绝对路径 `{ABS}`）。\
请只针对该文件，按上述绝对路径定位，不要改动其他文件。\
\n\n若遇到需要我方做选择的岔路（例如方案 A 还是 B、改动范围是 A 还是 B），不要自行假设。\
请只输出一行、以 `ASK_USER: ` 开头、后接一段合法 JSON，然后停止等我答复，例如：\
`ASK_USER: {\"question\": \"实现登录用哪种方案？\", \"options\": [\"方案A：JWT 无状态\", \"方案B：服务端 session\"]}`。\
无明确候选项时 options 可省（只给 question）。该行不要包含 JSON 之外的任何文字。";

/// 复核模式，仅影响 prompt 措辞（复核口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMode {
    Design,
    Implementation,
}

/// 复核 / 修订对象的精确定位：人类 label + 仓库根 + 仓库相对路径 + 绝对路径。
///
/// 把仓库根与绝对/相对路径显式填进 prompt，避免会话在子目录启动时 agent 按子目录相对路径
/// 误解 target（见三方校验 §4.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSpec {
    /// 人类可读 label（默认用仓库相对路径）。
    pub label: String,
    /// 工作树根绝对路径（已去 `\\?\` 前缀，适合展示）。
    pub repo_root: String,
    /// 相对仓库根的路径（正斜杠）。
    pub repo_rel: String,
    /// target 绝对路径（已去 `\\?\` 前缀）。
    pub abs: String,
}

/// 把 target 的定位四占位符填入模板（label + 仓库根/相对/绝对路径）。
fn fill_locator(template: &str, target: &TargetSpec) -> String {
    template
        .replace("{REPO_ROOT}", &target.repo_root)
        .replace("{REPO_REL}", &target.repo_rel)
        .replace("{ABS}", &target.abs)
        .replace("{LABEL}", &target.label)
}

/// 用给定模板渲染 Codex 复核 prompt。`round` 为当前轮次（从 1 起）。
pub fn render_codex_prompt(
    template: &str,
    target: &TargetSpec,
    mode: ReviewMode,
    round: u32,
) -> String {
    let scope = match mode {
        ReviewMode::Design => DESIGN_SCOPE,
        ReviewMode::Implementation => IMPL_SCOPE,
    };
    let round_hint = if round > 1 {
        format!("（这是第 {round} 轮，对方已按你上轮意见修订，请重新复核。）")
    } else {
        String::new()
    };
    fill_locator(template, target)
        .replace("{SCOPE}", scope)
        .replace("{ROUND_HINT}", &round_hint)
}

/// 用给定模板渲染 Claude 修订 prompt：把 Codex 的复核意见原文填入。
pub fn render_claude_prompt(template: &str, target: &TargetSpec, codex_review: &str) -> String {
    fill_locator(template, target).replace("{REVIEW}", codex_review)
}

/// 由 target_path 生成默认 label（缺省 target_label 时用）。
pub fn default_label(target_path: &str) -> String {
    format!("目标 {target_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(label: &str) -> TargetSpec {
        TargetSpec {
            label: label.to_string(),
            repo_root: "/repo".to_string(),
            repo_rel: "docs/foo.md".to_string(),
            abs: "/repo/docs/foo.md".to_string(),
        }
    }

    #[test]
    fn codex_prompt_first_round_no_revision_hint() {
        let p = render_codex_prompt(
            DEFAULT_CODEX_TEMPLATE,
            &spec("设计文档 docs/foo.md"),
            ReviewMode::Design,
            1,
        );
        assert!(p.contains("设计文档 docs/foo.md"));
        assert!(p.contains("VERDICT: PASS"));
        assert!(!p.contains("这是第"));
        assert!(p.contains("ASK_USER: "));
        // 精确定位句包含绝对路径与仓库根。
        assert!(p.contains("/repo/docs/foo.md"));
        assert!(p.contains("工作树根"));
        // 占位符必须全部被替换（ASK_USER 示例 JSON 里的 `{` 属正常内容，故只校验具体占位符）。
        for ph in CODEX_PLACEHOLDERS {
            assert!(!p.contains(ph), "占位符 {ph} 未被替换");
        }
    }

    #[test]
    fn codex_prompt_later_round_has_revision_hint() {
        let p = render_codex_prompt(
            DEFAULT_CODEX_TEMPLATE,
            &spec("docs/foo.md"),
            ReviewMode::Implementation,
            3,
        );
        assert!(p.contains("第 3 轮"));
        assert!(p.contains("符合设计"));
    }

    #[test]
    fn claude_prompt_embeds_review() {
        let p = render_claude_prompt(DEFAULT_CLAUDE_TEMPLATE, &spec("docs/foo.md"), "问题1：xxx");
        assert!(p.contains("问题1：xxx"));
        assert!(p.contains("只改确有问题处"));
        assert!(p.contains("ASK_USER: "));
        assert!(p.contains("/repo/docs/foo.md"));
        for ph in CLAUDE_PLACEHOLDERS {
            assert!(!p.contains(ph), "占位符 {ph} 未被替换");
        }
    }

    #[test]
    fn default_label_from_path() {
        assert_eq!(default_label("docs/foo.md"), "目标 docs/foo.md");
    }
}
