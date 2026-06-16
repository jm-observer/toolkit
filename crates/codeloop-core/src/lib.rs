//! codeloop 协议核心（provider 无关、无框架依赖）：复核 prompt 模板、回复解析
//! （VERDICT / ASK_USER）、三方仓库一致性校验。
//!
//! 由 `toolkit-server`（HTTP/任务引擎宿主）与 `zero-desktop`（桌面内嵌宿主）共享，
//! 确保两端复核行为（verdict 协议 / ASK_USER 协议 / prompt 措辞 / 越界校验）严格一致。
//! 设计见 `docs/toolkit-rfc/2026-06-15-cross-session-review-loop/plan.md`。

pub mod parse;
pub mod prompt;
pub mod validate;
