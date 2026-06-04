# RFC：toolkit 初始骨架（2026-06-04）

> 在 `github-commit-info` 仓内搭起 toolkit 工具集的最小可跑骨架。完成本 RFC 后，目标是：可在 g10 启动 `toolkit-server` binary、本机浏览器访问 dashboard 看到空表、Chrome 扩展握手成功、可以手动 submit/status 一个 dummy 任务跑完。

## 时间

- 创建：2026-06-04
- 最后更新：2026-06-04

## 项目现状

- 仓内已有 `crates/douyin`（业务函数全集 + CLI + 雏形 dashboard.html + serve.rs）、`crates/rag`（向量检索服务）
- 设计基线已落：[docs/toolkit-design.md](../../toolkit-design.md)
- 抖音业务函数已实现并稳定运行在 g10（zero 通过 CLI 调用）

## 整体目标

落地 [toolkit-design.md](../../toolkit-design.md) 第 9 节决策固化的形态。本 RFC 聚焦"骨架"，**不实现任何抖音业务路由**——业务装配是 Plan 2 的事。本 RFC 完成后所有抖音功能仍只能用现有 douyin CLI 跑。

## Plan 拆分

| Plan | 主题 | 依赖 | 状态 |
|---|---|---|---|
| 1 | toolkit-core schema + toolkit-tasks 引擎 + 最小 toolkit-server（含浏览器扩展 HTTP endpoint） | — | 已完成（2026-06-04） |
| — | （Plan 2-6 见 [toolkit-design.md §10](../../toolkit-design.md)，单独 RFC） | | |

本 RFC 范围内只有 Plan 1。Plan 2 起每个 Plan 单独开 RFC。

## 配套文档

- [data-model.md](data-model.md) — Plan 1 数据模型完整 DDL（索引 / 外键 / 类型）
- [extension-contract.md](extension-contract.md) — Chrome 扩展契约（manifest / HTTP 协议）。**注**：此文档为后续 Plan 4 的预先规范，落地不在 Plan 1 范围内；提前固化是为了让 Plan 1 的 server HTTP endpoint 直接按它实现，避免后期返工。
- [plan-1.md](plan-1.md) — Plan 1 子文档（任务目标、执行范围、Agent 步骤、完成条件）

## 风险与待定项

- **rusqlite 在 axum handler 里的连接管理**：用 `r2d2-sqlite` 连接池还是手工锁？倾向 r2d2-sqlite（workspace 依赖新增一项，需用户确认）
- **任务持久化的崩溃恢复语义**：进程崩了，`running` 状态的任务该自动重跑还是标记为 `interrupted`？倾向后者（避免重复副作用如重复下载），用户来 Web 手动重试。Plan 1 实现此策略
- **浏览器 endpoint 鉴权**：扩展请求时怎么证明自己是合法扩展（防止局域网内别的进程伪造）？Plan 1 简化为"任何来源都接受"，后续 Plan 加 token 机制
- Plan 1 不实现前端，dashboard 复用现有 `crates/douyin/src/dashboard.html`（仅最小验证服务端通；最终 SPA 是 Plan 3）
