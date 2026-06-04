# RFC：Web 控制台 + Chrome 扩展（2026-06-04，Plan 3+4）

> 把 Plan 1+2 已经接好的所有 HTTP endpoint 暴露成可操作 UI，让人能在浏览器里跑完整条抖音线（cookie → 解析博主 → 同步作品 → 看标签筛选 → 下载/转写/录入）；同时给 Chrome 装一个最小扩展，用户登录抖音时自动把 cookie 推给服务端。

## 状态

- 创建：2026-06-04
- 状态：已完成（2026-06-04）

## 设计关键

### 一、Web 控制台（Plan 3）

**形态决策**：无构建步骤的纯 ES module + 单 HTML，**不上 Svelte/Vite**。理由：
- 本仓不该引入 Node 工具链（github-commit-info 是 Rust 仓）
- "工具集，先把流程走通再优化"——一页控制面板能让所有 endpoint 都跑起来
- 后期需要更复杂 UI 时再升级（YAGNI）

**布局**：`web/index.html` + `web/app.js` + `web/style.css`，一个长滚动页，按操作分 sections。

**Sections（按抖音流程顺序）**：
1. Cookie 状态 — 显示 `/api/web/douyin/cookie_status` 结果 + raw header 文本框 + 写入按钮
2. 解析博主 — 输入 handle → `GET /api/web/douyin/creator`
3. 同步作品（异步） — 输入 handle + max_pages → `POST /api/web/douyin/sync_works`，跳到任务区
4. 看标签 — 输入 unique_id → `GET /api/web/douyin/tags`
5. 筛作品 — 输入 unique_id + tags + match → `GET /api/web/douyin/filter`
6. 下载（异步） — textarea aweme_ids → `POST /api/web/douyin/download`
7. 转写（异步） — textarea aweme_ids + vad → `POST /api/web/douyin/transcribe`
8. 录入（同步） — unique_id + only_ids → `POST /api/web/douyin/kb_publish`
9. 任务列表 — `GET /api/web/tasks`，3 秒自动刷新

**服务器集成**：toolkit-server 把 `/` 换成 `tower-http::services::ServeDir(web/)`，fallback index.html 实现 SPA 路由。`web/` 不存在时仍走原嵌入式最小 HTML（开发时编译产物可独立部署）。

实际启动时，web 路径相对当前工作目录解析。CLI 加 `--web-dir` 覆盖，default `./web`。

### 二、Chrome 扩展（Plan 4）

**形态决策**：manifest v3 + 单 `background.js`，本地未压缩加载。不打包、不上架。

**功能（最小子集）**：
- 启动时 POST `/api/browser/hello`（带 session_id 持久化到 chrome.storage）
- `chrome.tabs.onUpdated` 监听抖音域 URL → POST `/api/browser/url`
- `chrome.cookies.onChanged` 限定 `.douyin.com` cookie 变化 → debounce 1s → POST `/api/browser/cookie`（raw_header 由扩展拼）
- 不做本地按钮 / popup / URL 模式识别（Plan 4+ 再加，先把 cookie 自动同步跑通）

**服务器地址配置**：扩展 background.js 顶部硬编 `const SERVER = "http://127.0.0.1:8788"`，用户自己改。后续可加 popup options。

**安装文档**：`extension/README.md` 写好开发者模式加载步骤。

### 三、不在本 RFC 范围

- Web 上加"内嵌浏览器"页面（用 iframe 显示抖音）——抖音禁 iframe；这条路走不通，方案就是"用户日常 Chrome + 扩展"，本 RFC 已是终态
- 复杂的 UI 框架升级（按需后续 RFC）
- Agent namespace（Plan 5）

## 完成判据

- Web 端能完整跑通："cookie → 解析博主 → sync_works task → tags → filter → download/transcribe/kb_publish"全部从 UI 点
- 启动 toolkit-server 后访问 `http://127.0.0.1:8788/` 看到控制台
- Chrome 装扩展、登录抖音、observed g10 toolkit.db 里 `cookies` 表有数据（端到端验证由用户做）
- 现有测试不被破坏
