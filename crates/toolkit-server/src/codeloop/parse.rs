//! 复核回复解析：VERDICT 行 + ASK_USER 结构化标记。纯函数，便于单测。

use serde::{Deserialize, Serialize};

/// Codex 复核结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// 无明显错误，终止循环。
    Pass,
    /// 上方有问题清单，继续修订（含解析不到时的保守兜底）。
    NeedsWork,
}

/// 一个待用户拍板的结构化问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUser {
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
}

/// 解析 VERDICT：取**最后一次**出现的 `VERDICT: PASS|NEEDS_WORK`。
///
/// 解析不到 → 保守视为 `NeedsWork`（由调用方据「连续 N 轮解析失败」判 AbortedParse）。
/// 返回 `None` 表示根本没有 VERDICT 行（用于上层统计连续解析失败）。
pub fn parse_verdict(reply: &str) -> Option<Verdict> {
    let mut found = None;
    for line in reply.lines() {
        let t = line.trim();
        // 前缀大小写不敏感（容 agent 偶尔写 `Verdict:` / `verdict:`）。
        const PFX: usize = "VERDICT:".len();
        if t.len() < PFX || !t.is_char_boundary(PFX) || !t[..PFX].eq_ignore_ascii_case("VERDICT:") {
            continue;
        }
        let rest = &t[PFX..];
        match rest.trim().to_ascii_uppercase().as_str() {
            "PASS" => found = Some(Verdict::Pass),
            "NEEDS_WORK" => found = Some(Verdict::NeedsWork),
            _ => {}
        }
    }
    found
}

/// 解析 ASK_USER：取**第一行**以 `ASK_USER:` 开头者，后接一段 JSON。
///
/// JSON 解析失败兜底：把 `ASK_USER:` 之后整段当纯文本 question（options 空）。
/// 无 ASK_USER 行返回 `None`。
pub fn parse_ask_user(reply: &str) -> Option<AskUser> {
    for line in reply.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("ASK_USER:") else {
            continue;
        };
        let payload = rest.trim();
        if let Ok(parsed) = serde_json::from_str::<AskUser>(payload) {
            return Some(parsed);
        }
        // 兜底：整段当纯文本问题。
        return Some(AskUser {
            question: payload.to_string(),
            options: Vec::new(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_pass() {
        assert_eq!(
            parse_verdict("无问题。\nVERDICT: PASS"),
            Some(Verdict::Pass)
        );
    }

    #[test]
    fn verdict_needs_work_case_insensitive() {
        assert_eq!(
            parse_verdict("有问题\nverdict: needs_work"),
            Some(Verdict::NeedsWork)
        );
    }

    #[test]
    fn verdict_takes_last_occurrence() {
        let reply = "VERDICT: NEEDS_WORK\n继续\nVERDICT: PASS";
        assert_eq!(parse_verdict(reply), Some(Verdict::Pass));
    }

    #[test]
    fn verdict_none_when_absent() {
        assert_eq!(parse_verdict("没有结论行"), None);
    }

    #[test]
    fn ask_user_structured() {
        let reply = r#"ASK_USER: {"question": "用哪种方案？", "options": ["A", "B"]}"#;
        let q = parse_ask_user(reply).unwrap();
        assert_eq!(q.question, "用哪种方案？");
        assert_eq!(q.options, vec!["A", "B"]);
    }

    #[test]
    fn ask_user_no_options() {
        let reply = r#"ASK_USER: {"question": "要不要砍掉模块 X？"}"#;
        let q = parse_ask_user(reply).unwrap();
        assert_eq!(q.question, "要不要砍掉模块 X？");
        assert!(q.options.is_empty());
    }

    #[test]
    fn ask_user_fallback_to_plain_text() {
        let reply = "ASK_USER: 这不是合法 JSON 的问题";
        let q = parse_ask_user(reply).unwrap();
        assert_eq!(q.question, "这不是合法 JSON 的问题");
        assert!(q.options.is_empty());
    }

    #[test]
    fn ask_user_none_when_absent() {
        assert!(parse_ask_user("普通回复\nVERDICT: PASS").is_none());
    }
}
