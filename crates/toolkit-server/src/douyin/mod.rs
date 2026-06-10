//! 抖音业务接线层。
//!
//! 把现有 `douyin` crate 的库函数接到 toolkit 的两条入口：
//! - **TaskKind 包装**：3 个长任务（download / transcribe / list_works）跑在 toolkit-tasks 引擎里，
//!   状态统一在 SQLite tasks 表观察。
//! - **HTTP 路由**：`/api/web/douyin/*` 暴露同步查询（creator / works / tags / filter）+ 长任务提交。
//!
//! 路径解析见 [paths]；Cookie 桥接见 [cookie_bridge]；TaskKind 实现见 [kinds]。

pub mod cookie_bridge;
pub mod kinds;
pub mod paths;
pub mod pipeline;
pub mod refine;
pub mod routes;
