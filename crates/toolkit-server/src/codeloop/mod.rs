//! 跨会话复核循环（cross_review）：编排 Codex↔Claude 复核↔修订往复。见
//! `docs/toolkit-rfc/2026-06-15-cross-session-review-loop/plan.md`。

pub mod io;
pub mod kind;

// 协议核心（prompt / parse / validate）已抽到 `codeloop-core` crate，与 zero-desktop 共享，
// 避免两端复核行为分叉。此处 re-export，保持 `super::{prompt,parse,validate}` 与
// `crate::codeloop::{validate,...}` 旧引用路径不变。
pub use codeloop_core::{parse, prompt, validate};

pub use kind::CrossReviewTask;

use toolkit_tasks::Registry;

/// 注册 codeloop 相关 kind。
pub fn register_all(reg: &mut Registry) {
    reg.register::<CrossReviewTask>();
}
