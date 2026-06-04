# 数据模型（toolkit Plan 1）

> SQLite schema 完整 DDL。所有表落在 `toolkit.db`，与现有 `crates/douyin` 的状态文件分开（douyin crate 自有 cookies.json / works 缓存，此处不复用，由 Plan 2 业务装配层做迁移/桥接）。

## 设计原则

- **字符串 PK**：`unique_id`（抖音号）/ `aweme_id` / `task_id` 都是抖音原值或可读 ID，便于人工排查
- **JSON 列**用 TEXT 存 serde 序列化结果；不依赖 SQLite json1 函数（兼容 bundled 模式简化部署）
- **时间统一 UTC ISO8601 字符串**（与现有 douyin crate / chrono 风格一致）
- **轻外键**：声明但不开 SQLite 外键约束（`PRAGMA foreign_keys=OFF`，避免迁移摩擦）；引用关系靠业务层维护

## Schema

```sql
-- ============================================================
-- creators: 已收藏的博主
-- ============================================================
CREATE TABLE IF NOT EXISTS creators (
    unique_id     TEXT PRIMARY KEY,            -- 抖音号（人可读）
    sec_uid       TEXT NOT NULL UNIQUE,        -- 抖音 API 主键
    nickname      TEXT NOT NULL,
    avatar_url    TEXT,
    signature     TEXT,
    follower_count INTEGER,
    aweme_count   INTEGER,                     -- 抖音返回的总作品数（权威）
    verified      INTEGER NOT NULL DEFAULT 0,  -- 0/1
    raw           TEXT NOT NULL,               -- resolve_user 原始 JSON（保留全部字段以备扩展）
    added_at      TEXT NOT NULL,               -- ISO8601 UTC
    last_synced_at TEXT                        -- 最近一次 works_sync 完成时间
);

CREATE INDEX IF NOT EXISTS idx_creators_sec_uid ON creators(sec_uid);
CREATE INDEX IF NOT EXISTS idx_creators_added_at ON creators(added_at);

-- ============================================================
-- works: 作品元数据 + 各阶段状态
-- ============================================================
CREATE TABLE IF NOT EXISTS works (
    aweme_id          TEXT PRIMARY KEY,
    unique_id         TEXT NOT NULL,           -- → creators.unique_id
    desc_text         TEXT NOT NULL DEFAULT '', -- 作品描述（抖音叫 desc）
    tags              TEXT NOT NULL DEFAULT '[]', -- JSON array of string
    create_time       TEXT NOT NULL,           -- 作品发布时间 ISO8601
    cover_url         TEXT,
    video_url         TEXT,                    -- 无水印源（可能过期需重取）
    duration_ms       INTEGER,
    statistics        TEXT NOT NULL DEFAULT '{}', -- JSON: digg/comment/share/collect
    raw               TEXT NOT NULL,           -- 完整作品 JSON
    -- 流程状态
    downloaded_path   TEXT,                    -- 已下载本地路径（NULL = 未下载）
    downloaded_at     TEXT,
    transcribed       INTEGER NOT NULL DEFAULT 0, -- 0=未转写 1=已转写 -1=失败
    transcript_path   TEXT,                    -- 转写文本（含字幕带时间戳）路径
    transcribed_at    TEXT,
    kb_published_mode TEXT,                    -- NULL / 'desc_only' / 'full'
    kb_published_at   TEXT,
    -- 抓取来源记录
    discovered_at     TEXT NOT NULL            -- 第一次出现在 works_sync 的时间
);

CREATE INDEX IF NOT EXISTS idx_works_unique_id ON works(unique_id);
CREATE INDEX IF NOT EXISTS idx_works_create_time ON works(unique_id, create_time DESC);
CREATE INDEX IF NOT EXISTS idx_works_downloaded ON works(unique_id, downloaded_at);
CREATE INDEX IF NOT EXISTS idx_works_kb_published ON works(unique_id, kb_published_mode);

-- ============================================================
-- tasks: 异步任务统一表
-- ============================================================
CREATE TABLE IF NOT EXISTS tasks (
    task_id      TEXT PRIMARY KEY,             -- 如 'tk_<random14>'，对人可读但带前缀避免和外部 ID 混
    kind         TEXT NOT NULL,                -- 'douyin_download' / 'douyin_transcribe' / 'douyin_kb_publish' / ...
    state        TEXT NOT NULL,                -- 'queued' / 'running' / 'succeeded' / 'failed' / 'cancelled' / 'interrupted'
    input        TEXT NOT NULL,                -- JSON serialize 的 TaskKind::Input
    output       TEXT,                         -- JSON serialize 的 TaskKind::Output（仅 succeeded/partial 有）
    error        TEXT,                         -- 失败原因字符串
    progress     TEXT NOT NULL DEFAULT '{}',   -- JSON：{done, total, current_item, ...} 由 task 自报
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT,
    -- callback（可选）：完成时 POST 此 URL（payload = task 完整记录）
    callback_url TEXT,
    callback_delivered_at TEXT                 -- 已投递时间（null = 未投或不需投）
);

CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);
CREATE INDEX IF NOT EXISTS idx_tasks_kind_state ON tasks(kind, state);
CREATE INDEX IF NOT EXISTS idx_tasks_created_at ON tasks(created_at DESC);

-- ============================================================
-- cookies: douyin 登录态（单行）
-- ============================================================
CREATE TABLE IF NOT EXISTS cookies (
    id                INTEGER PRIMARY KEY CHECK (id = 1),  -- 强制单行
    raw               TEXT NOT NULL,           -- Cookie 头原文
    parsed            TEXT NOT NULL,           -- JSON: {name: value, ...}
    captured_at       TEXT NOT NULL,
    last_validated_at TEXT,                    -- 最近一次 douyin API 调用成功的时间
    status            TEXT NOT NULL            -- 'unknown' / 'valid' / 'expired'
);

-- ============================================================
-- browser_sessions: 扩展握手记录（轻量，用于看哪台浏览器在线）
-- ============================================================
CREATE TABLE IF NOT EXISTS browser_sessions (
    session_id   TEXT PRIMARY KEY,             -- 扩展生成的 UUID，安装后持久
    user_agent   TEXT,
    first_seen   TEXT NOT NULL,
    last_seen    TEXT NOT NULL,
    current_url  TEXT                          -- 最近一次推送的 URL（仅用于 UI 展示）
);

CREATE INDEX IF NOT EXISTS idx_browser_sessions_last_seen ON browser_sessions(last_seen DESC);
```

## 迁移机制

- 沿用现有 `crates/rag` 风格：`migrations.rs` 用 `rusqlite` 在启动时按版本号顺序执行
- 单版本号常量 `SCHEMA_VERSION = 1`，存 `meta(key='schema_version', value)` 表
- Plan 1 只引入 v1（即上述全部）；后续表/列变更走 v2/v3，**不**用第三方迁移库

## 几个不固化的微决策（实现时再选）

- `tasks.progress` 列存 JSON vs 拆 `progress_done` / `progress_total` 单列？倾向 JSON，灵活——不同 kind 的进度形态差异大
- `works.tags` 存 JSON array vs 单独 `work_tags(aweme_id, tag)` 关联表？倾向 JSON——查询主路径是"列博主+筛标签"，本仓 SQLite 用 `LIKE '%"<tag>"%'` 也足够；多博主跨表统计标签后期需要时再加关联表
- 命名约定：`unique_id` 沿用抖音术语而非 `creator_id`，避免与 zero 仓概念漂移
