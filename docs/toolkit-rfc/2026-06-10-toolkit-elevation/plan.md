# RFC: toolkit-* 提级为统一工具中台（v2）

- 日期：2026-06-10（v2 同日修订）
- 状态：草案（待确认）
- **2026-06 后续修订（ASR 路线）**：本 RFC 第 §3 提出把 `streaming-speech/server/asr-server`
  （sherpa-onnx）整 crate 迁入 toolkit。该路线已**作废**——sherpa-onnx crate 经短暂落地
  后于 2026-06 被物理退役（crate 删除、`deploy/asr-tts` 仅保留 TTS、deploy-g10.ps1 移
  除对应 bin），改由 **streaming-speech 仓 FunASR `/transcribe` 端点**统一提供 ASR。
  下文 §3、§4 的 ASR 相关条目仅做历史记录，不再代表当前部署。
- v2 变更：按用户决策重排优先级——**Agent 接入与股票全部后置，先集中功能、打通两条业务流**：抖音知识管线、英语音频生产线。

## 0. 背景与定位

整体架构目标：wechat → Agent(zero) ⇄ llm(GB10) ⇄ **tools-server(toolkit-\*)** ⇄ english。toolkit-* 集合绝大多数基础能力（ASR、TTS、抖音、rag…），未来通过 MCP 增强 Agent 的专项能力。但 Agent 如何接入尚未想清楚，**现阶段不做 Agent 相关工作**，只做能力集中与流程打通；股票板块同样后置。

## 1. 两条目标业务流

### 流 A：抖音知识管线（→ RAG，未来供 Agent 专项能力）

```
获取 cookie → 解析作者主页 → 获取作品标签 → 下载作品 → ASR 转文本
   → [缺] LLM 整理文本 → 录入 RAG → [后置] MCP 暴露给 Agent
```

现状盘点（绝大部分已存在）：

| 环节 | 现状 | 位置 |
|---|---|---|
| 获取 cookie | ✅ toolkit-desktop 登录窗 + msToken harvest + 自动上传 G10 | crates/toolkit-desktop |
| 解析作者主页 | ✅ creator API + URL 分类 | toolkit-server /api/web/douyin/creator |
| 作品标签/筛选 | ✅ tags / filter 端点 | douyin crate |
| 下载作品 | ✅ DouyinDownload 长任务 | douyin crate |
| ASR 转文本 | ✅ process 任务已调 asr-server `from-source` 端点 | douyin/src/process.rs |
| **整理文本** | ❌ 缺失——ASR 原文直接进 knowledge md，无 LLM 清洗/分段/纠错 | — |
| 录入 RAG | ✅ kb_publish 任务 + rag ingest（扫 transcripts/*.md → sqlite-vec） | douyin + rag crate |
| MCP 暴露 | 后置 | — |

### 流 B：英语音频生产线（→ english 项目消费）

```
来源1：抖音管线产出的音频文本（精选片段）
来源2：[后置] Agent 按用户专题（如"数字"）生成文本；现阶段可手动/LLM 直接生成
   → TTS（CosyVoice2）生成音频 → english 项目录入（音频 + 文本句对）
```

现状：TTS（CosyVoice2，Python FastAPI）生产就绪但在 streaming-speech 仓库内；english 音频目前纯人工上传（multipart → workspace/audio/ + MySQL 元数据），**没有任何 TTS 消费能力**。这条线整体是新建。

### english 项目自身

按既有规划继续演进（用户能力水平定位等），属 english 仓库自己的 roadmap，中台只负责把「文本→音频」的供给侧打通。

## 2. 仓库形态

仓库整体**重定位改名为 `toolkit`**（不拆库）：workspace 本就以 toolkit-* 为主干，github-commit-info 只是其中一个工具 crate；改名保留历史，无下游 path/git 依赖，风险极低。配套补 CLAUDE.md。

## 3. 阶段规划

### Phase 0 — 正名与生态对齐（小）

1. 仓库改名 `toolkit`，补 CLAUDE.md。
2. 接入 trace-hub：custom-utils 升 0.15 + trace feature；两条管线都是多环节长任务，正是追踪的最大受益者（一条 trace 看完 下载→ASR→整理→入库 全链路）。

### Phase 1 — 收编语音能力（两条流的共同地基）

1. **ASR**：`streaming-speech/server/asr-server` 整 crate 迁入 toolkit workspace（已是独立 OpenAI 兼容 axum 服务），与 toolkit-server 同机部署；douyin process 的 asr_url 默认值指向本机。模型文件归入 `<workspace>/models/`。
2. **TTS**：CosyVoice2 保留 Python，纳入 G10 toolkit 的 compose/systemd 编排；toolkit-server 增加 `/api/web/audio/tts` 代理端点（统一入口、落任务记录、便于后续鉴权）。
3. streaming-speech 仓库改为消费中台 ASR/TTS（其部署脚本同步调整）。

### Phase 2 — 打通流 A：抖音管线补缺

1. **新增 `TextRefine`（LLM 整理文本）TaskKind**：输入 ASR 原文，调 GB10 vLLM（OpenAI 兼容）做纠错/去口语水词/分段/小结，输出整理稿；knowledge md 同时保留原文与整理稿两栏。
2. **管线编排**：现有各环节是独立任务，新增一个 `CreatorPipeline` 编排任务（或脚本级串联）：给定 unique_id + 标签筛选 → 自动 download → ASR → refine → kb_publish → rag ingest，进度统一上报。
3. 验收标准：选 1~2 个博主，端到端跑通，rag `POST /v1/search` 能检索到整理后的内容。

### Phase 3 — 打通流 B：英语音频生产线

1. **toolkit 侧**：新增 `AudioForge` 任务/端点——输入（句子文本列表 + voice_id + 语速等），逐句调 TTS 产出 wav，打包为「学习包草稿」（音频文件 + 句子清单 JSON）。
2. **素材来源 1（抖音）**：从流 A 的整理稿中筛选英语片段 → 进 AudioForge。
3. **素材来源 2（专题）**：现阶段用 LLM 直接按专题（如"数字""问路"）生成句子文本 → 进 AudioForge；Agent 介入方式后置。
4. **english 侧**：新增导入接口（沿用其 JSON-RPC 风格，如 `package.import`），接收学习包草稿，落入现有 audio 存储 + MySQL 流程；替代纯人工上传。
5. 验收标准：一个专题（如"数字"）从文本生成到 english 小程序里可播放，全程不经人工传文件。

### 后置（明确不在本轮）

- **Agent/MCP 接入**：mcp-server 网关方案保留为既定方向（TOML HTTP executor 包装 toolkit API），等接入方式想清楚后启动。
- **股票数据工具化**：tick-web/tdx-rust 包装全部后置。
- zero/zero-nova 不投入。
- english 的能力定位等功能按其自身规划走，不在中台范围。

## 4. 风险与注意

- TextRefine 依赖 GB10 vLLM 的稳定供给，需确认其常驻模型与吞吐；整理 prompt 需迭代，建议 knowledge md 保留原文以便重跑。
- asr-server 迁仓影响 streaming-speech 部署（release-server.ps1 / compose），需同步改。
- TTS 逐句生成英语短句的质量（CosyVoice2 英文表现）需先做小样本试听，不行再评估备选（如 kokoro/piper）。
- english 导入接口涉及其 MySQL schema，按其仓库 RFC 流程走，避免中台单方面定契约。
