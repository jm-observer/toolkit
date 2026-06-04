# Toolkit 设计基线

> 在 github-commit-info 仓内逐步搭建一个**功能工具集**——本机使用者通过 Web 界面完成所有抖音相关操作（登录 / 浏览博主 / 选作品 / 下载 / 转写 / 知识库录入），同时把其中粗粒度入口暴露给 zero Agent 作为"手机接单台"调用对象。

## 1. 定位与范式

- **人能用的功能 = AI 能调的功能**——同一组后端函数，Web UI 和 Agent 是两套入口
- **粒度差异化**：Web 暴露细粒度（按具体作品 ID、按标签组合、按时间窗）；Agent 暴露粗粒度 submit/status 对（详见 zero 仓 [docs/design/agent-positioning.md](../../zero/docs/design/agent-positioning.md)）
- **服务部署在 g10**，用户从本机 Chrome 访问 Web UI，扩展把当前 tab URL/Cookie 推给 g10
- **持久状态单一事实来源**：creator/work/task/cookie/kb 都落 SQLite，Web 与 Agent 看同一份

## 2. 部署拓扑

```
[本机 Windows]                          [g10 192.168.0.68]
日常 Chrome                              toolkit-server (axum binary)
 ├─ 抖音 tab（已登录）                    ├─ 业务装配层（拼接 douyin / rag / tasks）
 ├─ toolkit Web UI(http://g10:PORT/)     ├─ SQLite（toolkit.db）
 └─ Chrome 扩展                          ├─ 文件存储（下载/中间产物）
     ├─ URL 推送 (WS)        ──────→     └─ 调 douyin/rag crate（已有）
     └─ Cookie 推送 (WS)
```

**Cookie 流转**：用户在本机 Chrome 正常使用抖音 → 扩展抓 `document.cookie` → WS 推 g10 → g10 落库 → 所有 douyin HTTP 调用从 g10 用此 cookie。已验证：现有 douyin CLI 就在 g10 上跑这套，cookie 跨 IP 不被风控。

## 3. 仓内 Workspace 增量

**保留不动**：`crates/douyin`（CLI + 业务库，zero 继续按现状用）、`crates/rag`（向量检索服务）、`crates/github-commit-info`、`crates/hf-watcher`。

**新增 crate**：

| Crate | 职责 | 备注 |
|---|---|---|
| `toolkit-core` | 共享领域类型 + SQLite schema/迁移 | Creator / Work / Task / Cookie / KbEntry |
| `toolkit-tasks` | 通用任务引擎 | submit/status/retry/persist，泛化自现有 list_works_task/process 模式 |
| `toolkit-server` | axum HTTP 服务（binary） | 装配 douyin/rag/tasks + 提供 REST API + 静态前端 |
| `toolkit-browser` | 扩展握手 + URL/Cookie 事件总线 | WS endpoint，握手 + 健康检查 |

**不新建 toolkit-douyin / toolkit-kb**：douyin 业务函数已在 `crates/douyin` 里；KB 录入靠组合 douyin + rag 完成，逻辑薄，放 toolkit-server 内即可。

**新增非 crate 资源**：

| 路径 | 内容 |
|---|---|
| `web/` | Svelte 前端源（构建产物嵌入 toolkit-server 静态） |
| `extension/` | Chrome 扩展（manifest v3 + background.js） |
| `data/`（g10 上运行时） | toolkit.db / 下载文件 / 中间产物（gitignored） |

## 4. 数据模型（首版）

| 表 | 关键字段 |
|---|---|
| `creators` | `unique_id PK / sec_uid / nickname / aweme_count / signature / added_at / last_synced_at` |
| `works` | `aweme_id PK / unique_id FK / desc / tags JSON / create_time / cover_url / video_url / downloaded_path / transcribed / kb_published_mode` |
| `tasks` | `task_id PK / kind(download/transcribe/kb_publish/...) / state / progress / result JSON / created_at` |
| `cookies` | 单行：`raw / parsed JSON / captured_at / last_validated_at / status` |
| `browser_sessions` | 扩展握手记录：`session_id / last_seen_at / current_url` |

`tasks.kind` 用枚举字符串，引擎本身对种类无感；新模块加一种 kind 即可注册。

## 5. 模块边界与 API 分层

```
                  ┌──────────────────────────────────────────┐
                  │  toolkit-server (axum binary)            │
                  │  ┌────────────────┐  ┌────────────────┐  │
                  │  │ REST API: Web  │  │ REST API: Agent│  │
                  │  │ /api/web/...   │  │ /api/agent/... │  │
                  │  │ (细粒度全集)   │  │ (粗粒度子集)   │  │
                  │  └───────┬────────┘  └────────┬───────┘  │
                  │          └────────┬───────────┘          │
                  │                   ▼                      │
                  │            业务装配层（async fn）        │
                  └─┬────────────┬─────────────┬─────────────┘
                    ▼            ▼             ▼
              toolkit-core  toolkit-tasks  douyin / rag (已有)
```

- **两个 API namespace** 共用同一组业务函数，区别在路由暴露范围（粗 vs 细）
- Agent namespace 鉴权与 Web namespace 不同（前者后续可加 token，后者本机访问默认放行）

## 6. 抖音模块细分（首版功能清单）

人形态 / Agent 形态对照详见 [docs/toolkit-douyin-functions.md]（待写）。当前已和用户对齐：
- Cookie 抓取、博主搜索/添加 → Web 独占
- 下载 / 转写 / 录入 → Web 全功能，Agent 粗粒度子集（按标签/最近 N/全部）
- Agent 不暴露搜索/添加博主——遇到库外博主反问"晚上去 Web 加"，可走 `todo_add` 备忘
- `creator_search` 函数不实现，让用户用抖音自家搜索

## 7. 任务引擎契约（toolkit-tasks）

```rust
pub trait TaskKind {
    type Input: Serialize + DeserializeOwned;
    type Output: Serialize + DeserializeOwned;
    const KIND: &'static str;
    async fn run(input: Self::Input, ctx: TaskCtx) -> Result<Self::Output>;
}

pub async fn submit<K: TaskKind>(input: K::Input) -> Result<TaskId>;
pub async fn status(id: TaskId) -> Result<TaskStatus>;
```

- 任务在 tokio task 中跑，状态写 SQLite，进程重启从 DB 恢复未终态任务
- 完成时可触发 callback（推 zero 等订阅方）
- 第一版三个 kind：`DouyinDownload` / `DouyinTranscribe` / `DouyinKbPublish`

## 8. 浏览器扩展契约

- manifest v3，background service worker 一个文件
- 监听 `chrome.tabs.onUpdated`：当 URL 命中抖音域名时把 `{tab_id, url, title}` 推 g10 WS
- 监听 cookie 变化（`chrome.cookies.onChanged` 限定 .douyin.com）：变化后把全量 douyin cookies 推 g10
- 接收 g10 推回的"操作建议"（如"当前页可收藏"），可显示徽章或 popup

## 9. 决策固化

| 决策 | 选择 | 备选与否决理由 |
|---|---|---|
| 集成方式 | 进 github-commit-info 仓 | 现有 douyin/rag crate 已是基础，独立项目反而漂移 |
| 前端 | Svelte SPA + axum 静态托管 | HTMX 状态密集页拧巴；Leptos 生态弱 |
| 内嵌浏览器 | Chrome 扩展 + 用户日常浏览器 | CDP 启独立 Chrome 体验割裂；Tauri WebView 登录态脆 |
| 后端形态 | 单 axum binary，同进程异步任务 | 多进程 worker 单用户不必要 |
| 数据库 | SQLite（rusqlite，沿用 workspace 依赖） | 单机工具不需要 Postgres |
| Agent 接入 | HTTP 调用，非同进程 | 两仓独立演进 |

## 10. 后续计划

写在 `docs/toolkit-rfc/` 子目录或独立 RFC 文档（待定）：

1. **Plan 1**：toolkit-core schema + toolkit-tasks 引擎 + 最小 toolkit-server
2. **Plan 2**：把现有 douyin crate 的下载/转写/录入函数包装成 task kind，HTTP endpoint 暴露
3. **Plan 3**：Web UI 框架 + creator/works 列表页 + 筛选
4. **Plan 4**：Chrome 扩展 + URL/Cookie 推送 + "收藏当前博主"按钮
5. **Plan 5**：Agent namespace 路由 + zero 侧工具切换到 HTTP 调用 toolkit
6. **Plan 6**：替换 zero 侧 douyin/douyin-ingest Skill，砍 prompt 行数至定位文档要求形态

Plan 1–4 让 Web 单端跑通；Plan 5–6 替换 Agent 入口。
