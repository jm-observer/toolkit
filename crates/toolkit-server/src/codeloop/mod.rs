//! 跨会话复核循环（cross_review）：编排 Codex↔Claude 复核↔修订往复。见
//! `docs/toolkit-rfc/2026-06-15-cross-session-review-loop/plan.md`。

pub mod io;
pub mod kind;
pub mod parse;
pub mod prompt;
pub mod validate;

pub use kind::CrossReviewTask;

use toolkit_tasks::Registry;

/// 注册 codeloop 相关 kind。
pub fn register_all(reg: &mut Registry) {
    reg.register::<CrossReviewTask>();
}
