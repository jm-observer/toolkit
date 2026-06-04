//! toolkit-tasks：通用任务引擎。
//!
//! - 任务 kind 通过 `TaskKind` trait 注册；输入/输出走 serde_json
//! - 提交即 spawn tokio task；状态持久化到 SQLite（toolkit-core schema 的 `tasks` 表）
//! - 进程启动时把 queued/running 旧任务标记为 interrupted（不自动重跑）

pub mod api;
pub mod echo;
pub mod kind;
pub mod runner;
pub mod store;

pub use api::{list_tasks, status, submit, TaskListFilter, TaskStatusDto};
pub use echo::{EchoInput, EchoOutput, EchoTask};
pub use kind::{Registry, TaskCtx, TaskKind};
pub use runner::recover_interrupted;
