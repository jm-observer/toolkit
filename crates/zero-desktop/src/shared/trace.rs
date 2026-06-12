//! Tauri command 入口的 trace 辅助封装。
//!
//! 设计目标：
//! - `TRACE_HUB_ENDPOINT` 未设时 **零成本**（`trace::enabled()` 为第一道门）。
//! - with_summary 的 JSON **不得**包含敏感字段（cookie 原文、token、音频 PCM）。
//! - RAII `CommandSpan` 确保 Drop 时自动 emit_end Ok；错误路径调 `.fail(msg)` 改为 Error。
//!
//! # 用法
//! ```ignore
//! pub async fn some_command(...) -> Result<(), String> {
//!     let mut span = CommandSpan::start("cmd_name", serde_json::json!({"key": "value"}));
//!     if let Err(e) = do_thing().await {
//!         return Err(span.fail(e.to_string()));
//!     }
//!     Ok(())
//! }
//! ```

use custom_utils::trace::{self, SpanScope, SpanStatus, TraceContext};

/// RAII span 包装器。Drop 时若未显式 fail，则 emit_end Ok。
pub struct CommandSpan {
    /// 仅在 trace 启用时 Some；`fail()` / Drop 通过 `Option::take()` 消耗（避免重复 emit）。
    inner: Option<SpanScope>,
}

impl CommandSpan {
    /// 创建并 emit_start。若 trace 未启用则返回 no-op 包装（inner = None）。
    pub fn start(kind: &'static str, summary: serde_json::Value) -> Self {
        let inner = trace::enabled().then(|| {
            let ctx = TraceContext::root();
            let scope = SpanScope::new(ctx, kind).with_summary(summary);
            scope.emit_start();
            scope
        });
        Self { inner }
    }

    /// 标记错误并返回 `msg`（方便在 `return Err(span.fail(e.to_string()))` 单行写法）。
    /// 调用后 Drop 不会再次 emit。
    pub fn fail(&mut self, msg: String) -> String {
        if let Some(scope) = self.inner.take() {
            scope.emit_end(Some(msg.clone()), SpanStatus::Error(msg.clone()), None);
        }
        msg
    }
}

impl Drop for CommandSpan {
    fn drop(&mut self) {
        // inner 被 fail() 消耗后为 None，无副作用。
        if let Some(scope) = self.inner.take() {
            scope.emit_end(None, SpanStatus::Ok, None);
        }
    }
}
