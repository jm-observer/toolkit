//! 复核 / 修订 prompt 模板（中文，随 crate 编译）。见 RFC §4「Prompt 模板」。

use serde::{Deserialize, Serialize};

/// 复核模式，仅影响 prompt 措辞。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMode {
    Design,
    Implementation,
}

/// 两个模板共用的「遇岔路用 ASK_USER」约束（见 RFC §4 / §10.3）。
const ASK_USER_CLAUSE: &str = "\n\n若遇到需要我方做选择的岔路（例如方案 A 还是 B、改动范围是 A 还是 B），\
不要自行假设。请只输出一行、以 `ASK_USER: ` 开头、后接一段合法 JSON，然后停止等我答复，例如：\
`ASK_USER: {\"question\": \"实现登录用哪种方案？\", \"options\": [\"方案A：JWT 无状态\", \"方案B：服务端 session\"]}`。\
无明确候选项时 options 可省（只给 question）。该行不要包含 JSON 之外的任何文字。";

/// Codex 复核 prompt。`round` 为当前轮次（从 1 起）。
pub fn render_codex_prompt(target_label: &str, mode: ReviewMode, round: u32) -> String {
    let scope = match mode {
        ReviewMode::Design => "只关注事实/逻辑/前后一致性/可行性错误，不纠结措辞。",
        ReviewMode::Implementation => {
            "只关注实现是否符合设计、有无逻辑/边界/正确性错误，不纠结风格。"
        }
    };
    let mut p = format!(
        "请以严格审阅者身份复核{target_label}。{scope}\
逐条列出发现的问题（无问题写\"无\"）。最后另起一行只输出结论：\
无明显错误输出 `VERDICT: PASS`，否则 `VERDICT: NEEDS_WORK`。"
    );
    if round > 1 {
        p.push_str(&format!(
            "（这是第 {round} 轮，对方已按你上轮意见修订，请重新复核。）"
        ));
    }
    p.push_str(ASK_USER_CLAUSE);
    p
}

/// Claude 修订 prompt：把 Codex 的复核意见原文转交。
pub fn render_claude_prompt(target_label: &str, codex_review: &str) -> String {
    let mut p = format!(
        "Codex 对{target_label}的复核意见如下：\n---\n{codex_review}\n---\n\
请据此修订{target_label}，只改确有问题处，并在回复末尾用一句话概述本轮改动。"
    );
    p.push_str(ASK_USER_CLAUSE);
    p
}

/// 由 target_path 生成默认 label（缺省 target_label 时用）。
pub fn default_label(target_path: &str) -> String {
    format!("目标 {target_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_prompt_first_round_no_revision_hint() {
        let p = render_codex_prompt("设计文档 docs/foo.md", ReviewMode::Design, 1);
        assert!(p.contains("设计文档 docs/foo.md"));
        assert!(p.contains("VERDICT: PASS"));
        assert!(!p.contains("这是第"));
        assert!(p.contains("ASK_USER: "));
    }

    #[test]
    fn codex_prompt_later_round_has_revision_hint() {
        let p = render_codex_prompt("docs/foo.md", ReviewMode::Implementation, 3);
        assert!(p.contains("第 3 轮"));
        assert!(p.contains("符合设计"));
    }

    #[test]
    fn claude_prompt_embeds_review() {
        let p = render_claude_prompt("docs/foo.md", "问题1：xxx");
        assert!(p.contains("问题1：xxx"));
        assert!(p.contains("只改确有问题处"));
        assert!(p.contains("ASK_USER: "));
    }

    #[test]
    fn default_label_from_path() {
        assert_eq!(default_label("docs/foo.md"), "目标 docs/foo.md");
    }
}
