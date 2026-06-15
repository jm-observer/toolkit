# 跨会话复核循环（Cross-Session Review Loop）设计

> 状态：设计草案，待评审。
> 创建：2026-06-15。
> 定位：toolkit 中台新增一个长任务 kind——把一对**已存在**的 Codex / Claude Code 会话关联起来，
> 自动驱动「复核 ↔ 修订」往复，替代当前人工复制粘贴。后续向 zero 开放粗粒度 `/api/agent` 子集。

## 1. 背景与目标

用户已有的工作流：在 Claude Code 里写设计文档 / 实现代码，再手动把内容复制到 Codex 让它复核，
把 Codex 的意见复制回 Claude Code 改，如此往复。痛点是**全程手动复制粘贴**。

目标场景（三合一，本质同一个循环）：

| 场景 | Claude 角色 | Codex 角色 | 终止 |
|---|---|---|---|
| 1 设计文档复核 | 产出/修订设计文档 | 复核设计文档，给问题清单 | Codex 判定无明显错误 |
| 2 实现复核 | 实现设计方案 | 复核本次实现 | Codex 判定通过 |
| 3 自动化 | （同上） | （同上） | 用关联会话替代手工复制粘贴 |

## 2. 已验证的事实与硬约束（来自 2026-06-15 真机验证）

底层机械动作已用 Python 探针（zero 仓 `scripts/probe_sessions.py`）+ 真机 round-trip 验证：

- **读会话状态**：两边会话都是本机磁盘上的明文 JSONL，可外部解析：
  - Codex：`~/.codex/session_index.jsonl`（清单：`{id, thread_name, updated_at}`）+
    `~/.codex/sessions/<年>/<月>/<日>/rollout-*-<id>.jsonl`（事件流）。
  - Claude：`~/.claude/projects/<编码 cwd>/<sessionId>.jsonl`（事件流）。
- **判断状态**：
  - Codex 看 `event_msg` 的 `task_started` vs `task_complete` 配对；`task_complete` 直接带
    `last_agent_message`（最近回复，权威）。
  - Claude 看最后一条 `assistant` 的 `message.stop_reason`：`end_turn`=结束、`tool_use`=工具循环中、
    末条是 `user` 则=处理中。
  - 「正在生成」需叠加「文件 mtime 是否仍在刷新」兜底。
- **主动发消息**（已跑通，各消耗真实额度）：
  - Codex：`codex exec resume <id> "<prompt>"`（**不挑当前目录**，给 UUID 即可续）。
  - Claude：`claude -p "<prompt>" --resume <id>`（**必须在该会话的原始 cwd 下执行**，否则
    `No conversation found`；`-p` 阻塞到本轮完成并把回复打到 stdout）。

**硬约束（直接决定设计边界）**：

1. **桌面端非实时**：实测桌面客户端要**重启/重开**才能读到外部写入，不会 live watch。
2. **单写者**：一个会话同一时刻假设只有一个写入者。循环运行期间用户**不得**在桌面端操作这两个会话，
   否则并发写会让会话分叉。→ 任务说明里强约束「跑循环时别碰这两个会话，跑完重开桌面端看结果」。
3. **会话存储与 CLI 登录态都在本机 Windows**：`~/.codex`、`~/.claude` 和 codex/claude 的登录都在
   用户本机，**不在 g10**。→ 见 §8 部署待定项。

## 3. 架构定位

复核循环 = 一个**长任务**，完美契合现有 `toolkit-tasks` 引擎（`TaskKind` trait + submit 即 spawn +
状态机 + `tasks` 表持久化 + `report_progress` + trace）。无需新引擎。

新增：

| 新增项 | 形态 | 职责 |
|---|---|---|
| `crates/agent-session` | 新 lib crate | provider 无关的「外部编码 Agent 会话」观测 + 驱动：读 Codex/Claude 会话存储、解析状态/回复、CLI 子进程发消息、等待空闲。把探针逻辑从 Python 移植为 Rust。 |
| `CrossReviewTask`（`const KIND = "cross_review"`） | toolkit-server 内新模块 `src/codeloop/` | 编排循环。复用 `agent-session` 的读/写能力。 |
| HTTP 路由 | `toolkit-server` | 见 §4.3 端点清单。复用现有 `/api/web/tasks/{task_id}`；后续 `/api/agent/codeloop/*` 粗粒度。 |
| Web 双栏视图 | **沿用现有嵌入式静态页**（`crates/toolkit-server/web/` + `include_str!`，参照已有 `hub.html/js/css`）新增 `codeloop.html/js/css` | 左 Claude / 右 Codex 双栏滚动对话流 + 顶部循环状态条。**不引入 Svelte**。见 §9。 |

依赖方向：`agent-session`（独立，依赖 tokio/serde/serde_json/anyhow；解析 Codex/Claude JSONL 需 serde_json，
驱动子进程用 tokio process）← `toolkit-server`（装配为 TaskKind）。
不污染 `toolkit-core`（会话存储不是 toolkit 自己的领域数据，不进 `toolkit.db`）。

### `crates/agent-session` 模块

```
crates/agent-session/src/
├── lib.rs       # Provider 枚举、SessionRef、SessionSnapshot、TurnResult
├── store.rs     # 读存储：list / locate / parse_events / snapshot（移植 probe_sessions.py）
├── driver.rs    # 发消息：send(provider, &SessionRef, prompt) -> TurnResult
└── watch.rs     # wait_for_idle(&SessionRef)：轮询会话文件直到本轮完成 + mtime 稳定
```

关键类型（草案）：

```rust
pub enum Provider { Codex, Claude }

pub struct SessionRef {
    pub provider: Provider,
    pub session_id: String,
    /// 会话原始工作目录。两边都解析/记录（见下）：
    /// - Claude：来自 jsonl 事件 `cwd`；resume 必须在此目录下 spawn。
    /// - Codex：来自 rollout `session_meta.cwd` / `turn_context.cwd`（已实测存在）；
    ///   `exec resume` 虽不挑当前目录，但用于 §4.1 的仓库一致性校验。
    pub cwd: PathBuf,
}

pub enum SessionStatus { Idle, Generating, Processing, Unknown }

pub struct SessionSnapshot {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub latest_reply: String,
    pub updated_at: String,
}

pub struct TurnResult {
    pub reply_text: String,   // 自然语言回复（跳过纯 tool_use）
    pub raw_tail: String,     // 末段原始输出，排障用
}

pub async fn send(s: &SessionRef, prompt: &str) -> anyhow::Result<TurnResult>;
pub async fn snapshot(s: &SessionRef) -> anyhow::Result<SessionSnapshot>;
pub async fn wait_for_idle(s: &SessionRef, timeout: Duration) -> anyhow::Result<()>;
```

`send` 实现：用 `tokio::process::Command` 起 CLI 子进程。命令形态（参数位置已对 `--help` 核实，
**组合调用需 Plan 3 实测固化**）：

- **Codex**：`-s/--sandbox`、`-C/--cd` 在 `codex exec` 顶层，`resume` 子命令**不接受** `--sandbox`；
  `--json` 两层都有；审批策略经 `-c` 配置覆盖。固化形态：
  ```
  codex exec -s workspace-write -c approval_policy="never" --cd <repo> resume --json <id> <prompt>
  ```
  **（2026-06-15 真机固化）** stdout 为事件 JSONL：`thread.started`/`turn.started`/`item.completed`{item}/
  `turn.completed`{usage}。回复 = 末个 `item.completed` 且 `item.type=="agent_message"` 的 `item.text`
  （退化兼容旧 `task_complete.last_agent_message`）。Windows 下 stdout 可能混 GBK 噪声行 → lossy 解码后跳过。
  详见 `docs/runbook-codeloop-e2e.md §5`。
- **Claude**：`claude -p <prompt> --resume <id> --permission-mode acceptEdits`，`Command::current_dir(cwd)`，
  取 stdout 文本（**真机固化**：干净 UTF-8 纯文本）。（可选 `--output-format stream-json` 拿结构化事件；MVP 先用纯文本。）

## 4. CrossReviewTask 契约

```rust
#[derive(Deserialize)]
struct CrossReviewInput {
    // DTO 两边对称：前端可只传 { session_id }，server 从会话存储解析各自 cwd 补全并按 §4.1 校验；
    // 也允许前端显式带 cwd（则校验是否与解析值一致）。
    claude: SessionRefDto,          // { session_id, cwd?: String }
    codex:  SessionRefDto,          // { session_id, cwd?: String }
    target_path: String,            // 文件/目录路径，用于 §4.1 校验 + 读写定位，如 "docs/foo.md"
    #[serde(default)] target_label: Option<String>,  // prompt 展示用描述，如 "设计文档 docs/foo.md"；
                                                      // 省略则由 server 用 target_path 生成默认 label
    mode: ReviewMode,               // Design | Implementation（仅影响 prompt 措辞）
    #[serde(default = "default_max_rounds")] max_rounds: u32,   // 默认 5，硬上限
    #[serde(default)] wait_for_claude_idle: bool,              // 先等当前 Claude 轮完成再开跑
    #[serde(default)] notify_callback: Option<String>,         // 提醒回调 URL（推 zero）；空则仅 UI 提醒
}

#[derive(Serialize)]
struct CrossReviewOutput {
    rounds: Vec<RoundRecord>,       // 每轮：codex_verdict / codex_review / claude_revision
    final_verdict: FinalVerdict,    // Pass | MaxRounds | AbortedTimeout | AbortedParse
    total_rounds: u32,
}
```

**与现有任务状态机的映射（明确语义，对齐 `runner.rs` 的 succeeded/failed/interrupted）**：

- 所有**业务终态**（`Pass` / `MaxRounds` / `AbortedTimeout` / `AbortedParse`）都让 `run` 返回
  `Ok(CrossReviewOutput)` → 任务 `state = succeeded`，业务结果看 `output.final_verdict`。
  「Aborted」是业务终态，**不**等于任务 failed。
- 只有**基础设施错误**（CLI 未找到、子进程 spawn 失败、DB 写失败等）才 `Err(..)` →
  任务 `state = failed`。
- `interrupted` 仍由引擎在 server 重启时产生（见 §10.3 重启说明），非本任务主动设置。
- UI / callback 据此双层读：`task.state` 看基础设施健康，`final_verdict` 看复核业务结论。

### 4.1 提交校验（拒绝跑错仓）

`codex exec resume` 不挑当前目录 ≠ Codex 会话一定在目标仓库里。提交时**强制三方一致**，否则拒绝启动：

1. 解析 Claude 会话 cwd（jsonl `cwd`）与 Codex 会话 cwd（rollout `session_meta.cwd` / `turn_context.cwd`）。
2. **分别从 Claude.cwd、Codex.cwd、`target_path` 各自向上找 `.git`，求出三者的 repo root**
   （会话常从子目录如 `docs/`、`crates/toolkit-server/` 启动，cwd 本就不等于 repo 根，比 cwd 没意义）。
3. `canonicalize` 后要求 **三个 repo root 相等**，且 `target_path` 落在该 root 内。
4. 不一致 → 返回 400，提示三方 cwd 与解析出的 repo root，不提交任务。

> 比的是「是否同一棵工作树」，而非「是否同一个目录」。这样 Codex「复核仓库里的文件」和
> Claude「修订同一文件」才真正落在同一棵树上，又不会误杀从子目录启动的会话。

### 循环流程（`run`）

```
if wait_for_claude_idle: agent_session::wait_for_idle(claude, 10min)   // 场景1「Claude 完成回复后」
for n in 1..=max_rounds:
    1. review = send_and_resolve(codex, render_codex_prompt(target_label, mode, n))  // 含 ASK_USER 挂起处理(§10)
    2. verdict = parse_verdict(review.reply_text)                       // 解析 VERDICT 行
    3. report_progress({ round:n, phase:"reviewed", verdict })
    4. if verdict == PASS: final = Pass; break
    5. revision = send_and_resolve(claude, render_claude_prompt(target_label, review))  // Claude 据意见修订
    6. report_progress({ round:n, phase:"revised" })
    record round(n, verdict, review, revision)
final = final.unwrap_or(MaxRounds)
notify(done, final)                                                    // 完成提醒(§10)
```

`send_and_resolve` = 发一轮 → 若回复含 `ASK_USER:` 则挂起等用户回答（§10）→ 把答案发回同一会话 →
重读回复 → 直到不再 ASK_USER，返回最终回复。

**关键点**：Codex/Claude 共用同一个仓库工作目录，设计文档/代码是磁盘上的文件。编排器**只在两个会话间
搬运「复核意见文本」**，不搬运文件内容——Codex 直接读仓库里的 `target_path` 文件，Claude 直接改它。这正是
「无需复制粘贴」的实现。

### Verdict 协议

Codex prompt 末尾强制要求输出独立一行结论。解析（正则取最后一次出现）：

```
VERDICT: PASS         → 无明显错误，终止
VERDICT: NEEDS_WORK   → 上方有问题清单，继续修订
ASK_USER: {json}      → 需用户拍板（结构化问题+选项），挂起循环（§10.3），与 VERDICT 互斥优先处理
解析不到               → 保守视为 NEEDS_WORK，raw 记入 round 排障；连续 2 轮解析失败 → AbortedParse
```

### Prompt 模板（中文，随 crate 编译，可 bump 版本）

- **Codex 复核**（design）：
  > 请以严格审阅者身份复核{target_label}。只关注事实/逻辑/前后一致性/可行性错误，不纠结措辞。
  > 逐条列出发现的问题（无问题写"无"）。最后另起一行只输出结论：无明显错误输出
  > `VERDICT: PASS`，否则 `VERDICT: NEEDS_WORK`。
  > （n>1 追加：这是第 n 轮，对方已按你上轮意见修订，请重新复核。）
- **Claude 修订**：
  > Codex 对{target_label}的复核意见如下：\n---\n{codex_review}\n---\n
  > 请据此修订{target_label}，只改确有问题处，并在回复末尾用一句话概述本轮改动。

两个模板都追加**统一约束**（实现「中途要用户拍板」的关键，与 §10.3 的结构化协议对齐）：
  > 若遇到需要我方做选择的岔路（例如方案 A 还是 B、改动范围是 A 还是 B），**不要自行假设**。
  > 请只输出一行、以 `ASK_USER: ` 开头、后接**一段合法 JSON**，然后停止等我答复，例如：
  > `ASK_USER: {"question": "实现登录用哪种方案？", "options": ["方案A：JWT 无状态", "方案B：服务端 session"]}`
  > 无明确候选项时 `options` 可省略（只给 `question`）。该行不要包含 JSON 之外的任何文字。

### 4.3 HTTP 端点清单

| 方法 路径 | 用途 |
|---|---|
| `POST /api/web/codeloop/submit` | 提交一对会话 + `target_path`(+可选 `target_label`)，启动 cross_review 任务，返回 `task_id` |
| `GET /api/web/tasks/{task_id}` | 复用现有任务状态路由（实为 `/tasks/{task_id}`，见 `web.rs:24`）：拿 round/phase/verdict/最终结果 |
| `GET /api/web/codeloop/sessions` | 列出本机 Codex/Claude 会话清单（供前端挑选配对，复用 store::list） |
| `GET /api/web/codeloop/session/{provider}/{id}/messages?after={cursor}` | 增量取某会话消息（append-only JSONL，cursor=已读行数/字节偏移），供双栏视图轮询 |
| `POST /api/web/codeloop/{task_id}/answer` | 回答挂起循环：`{seq, text}` 写入 `codeloop_io`，任务轮询取走并 resume 提问方会话（见 §10.3） |

> 读消息只需 session_id（store 按 id 全盘 glob 定位文件）；cwd 仅 `send`(resume) 时才需要。

## 5. 全自动 watch 语义

用户选「全自动 watch」（非单步）。落地为：

- 提交任务一次，引擎自动跑完整个 ping-pong 到收敛（PASS）或 `max_rounds`，**中途不需要人工点继续**。
- `wait_for_claude_idle=true` 时，先 watch Claude 会话文件等当前（用户驱动的）轮次 `end_turn` 完成，
  再开第一轮 Codex 复核——对应场景 1「**当 Claude 完成回复后**」自动接管。
- 循环内部每轮 `send` 走 `-p`/`exec` 阻塞模式，子进程返回即本轮完成，`watch` 仅作完成性兜底校验。

## 6. Plan 拆分

| Plan | 内容 | 依赖 |
|---|---|---|
| Plan 1 | `crates/agent-session`：store 读取 + status/snapshot + **解析两边 cwd**（Claude jsonl `cwd`、Codex `session_meta/turn_context.cwd`），单测用真机会话样本 | 无 |
| Plan 2 | HTTP `GET /codeloop/sessions` + `/session/{provider}/{id}/messages` + **双栏视图页面**（§9，改 `codeloop.html/js/css` 走嵌入静态）。**先于 loop 落地** | Plan 1 |
| Plan 3 | `agent-session::driver::send`（Codex/Claude 子进程，**实测固化 §3 命令形态**）+ `watch::wait_for_idle` | Plan 1 |
| Plan 4 | `toolkit-core` 的 `DDL_V1` 追加 `codeloop_io` 表（`IF NOT EXISTS`，不 bump 版本，见 §10.3）；`CrossReviewTask` kind + verdict/ASK_USER(结构化) 解析 + prompt 模板 + `send_and_resolve`(DB 轮询握手) + notify 自投递，注册进 `bootstrap()` | Plan 3 |
| Plan 5 | HTTP `POST /codeloop/submit`（含 §4.1 三方 cwd 校验）+ `POST /codeloop/{id}/answer`（写 `codeloop_io`）；双栏视图叠加状态条 + 模拟选项弹窗；runbook 端到端 | Plan 2,4 |
| Plan 6 | `/api/agent/codeloop/*` 粗粒度子集 + zero 侧工具接线 + zero 接收 notify 推微信（后续，跨仓） | Plan 5 |

Plan 1–2 即可交付**双栏只读观测视图**（你最想先要的）；Plan 3–5 补上自动循环；Plan 6 才涉及 zero。

## 7. 风险

- **CLI 格式漂移**：codex `--json` 仍带 experimental 基因、claude jsonl 是内部格式 → 解析层容错 +
  版本探测，坏行跳过。
- **并发写**：见 §2 约束 2，文档强约束 + 任务启动时可 snapshot 两会话 mtime，运行中若被外部改动检测告警。
- **烧 token / 失控**：`max_rounds` 硬上限；每轮记录 token（Codex 输出带用量）；解析失败兜底中止。
- **Claude `-p` / Codex `exec` 会真改文件**：implementation 模式正需要；design 模式也可能动文件。
  非交互权限配置见 §10.5；MVP 默认允许（目标就是让它改 `target_path` 文件），但限本机可信仓库。
- **耗时**：每轮 2 次 LLM 调用，分钟级；纯异步任务，状态可轮询，不阻塞 server。

## 8. 决策固化（2026-06-15 与用户对齐）

| 决策 | 选择 |
|---|---|
| 跑在哪台机器 | **本机 Windows**（会话存储 + codex/claude 登录都在本机）。需本机跑一个 toolkit-server 实例，不复用 g10 实例 |
| 实现节奏 | **先定设计文档**；实现按既定跨仓边界默认归用户，待本稿评审后再议 |
| 双栏视图刷新 | **先轮询**（1–2s 增量 cursor）；量起来再考虑 SSE/WS |
| 会话配对来源 | 提交请求显式带 session_id（Claude 带 cwd）+ `GET /codeloop/sessions` 供前端挑选（Plan 2） |
| 中途用户输入 | `ASK_USER`（**结构化问题+选项**）挂起 → **UI 模拟弹窗点选** → `POST /answer`；唤醒走 **DB 握手 + 任务轮询**（非 AppState sender，因 `TaskKind::run` 拿不到 AppState）（§10.3） |
| 提醒 | 完成 / 需输入两类，**任务内 reqwest 自投递**到 `notify_callback`（通用 `callback_url` 只存不投）+ 本机 UI（§10.4） |
| 仓库一致性 | 提交时分别向上解析三者 **repo root** 并比对相等（非比 cwd，容子目录启动）、`target_path` 须在 root 内，否则拒绝（§4.1） |
| target 字段 | 拆 `target_path`（校验+读写定位）与 `target_label`（prompt 展示，可省由路径生成）（§4） |
| 终态语义 | 业务终态(Pass/MaxRounds/Aborted*)→`Ok`→任务 succeeded；仅基础设施错→`Err`→failed（§4 映射） |

剩余开放项：

- ASK_USER 挂起态**跨 server 重启**会失效（引擎标 interrupted 不自动重跑）。MVP 接受并在 runbook 注明；
  若要做可恢复挂起，需把「待答问题」也持久化进 DB 并支持重启后重连——是否纳入后续 Plan？

## 9. 双栏实时视图（Web UI）

关联建立后，打开一个页面实时滚动展示整个复核过程。沿用 toolkit **现有的嵌入式静态页方案**
（`crates/toolkit-server/web/` 下手写 HTML/JS/CSS + `include_str!`，参照已有 `hub.html`；
**不引入 Svelte**），新增一个 `codeloop.html` + `codeloop.js` + `codeloop.css`。

### 布局

```
┌──────────────────────────────────────────────────────────────┐
│  状态条： [配对: claudeTitle ⇄ codexTitle]   轮次 2/5          │
│           Codex 判定: NEEDS_WORK   循环状态: running ●          │
├───────────────────────────────┬──────────────────────────────┤
│  Claude Code  (左)            │  Codex  (右)                  │
│  ┌──────────────────────────┐ │ ┌──────────────────────────┐ │
│  │ [user]   …               │ │ │ [user]   复核请求…        │ │
│  │ [assistant] 修订说明…     │ │ │ [assistant] 问题清单…     │ │
│  │ ……（自动滚动到底）        │ │ │ VERDICT: NEEDS_WORK       │ │
│  └──────────────────────────┘ │ └──────────────────────────┘ │
└───────────────────────────────┴──────────────────────────────┘
```

- **两栏各是一个会话的对话流**：role 区分气泡（user / assistant），纯 `tool_use` 折叠为
  「🔧 工具调用」可展开，`thinking` 默认折叠。新消息到达自动滚到底（用户上滚时暂停自动滚）。
- **顶部状态条**：配对双方标题、当前轮次 `n/max`、最近 Codex `VERDICT`、循环运行状态
  （running/idle/done/aborted）。
- **配对入口**：进页面先用 `GET /codeloop/sessions` 列两边会话，各选一个（Claude 那个带上 cwd）建立
  关联；可只观测（不启动 loop），也可点「启动复核循环」触发 `POST /codeloop/submit`。
- **模拟弹窗（中途用户输入）**：轮询到 `phase==awaiting_input` 时，弹出模态框显示 agent 的问题，
  把 `options` 渲染成可点按钮（+ 自由文本兜底）；用户点选即 `POST /answer {seq, text}`，弹窗关闭、循环继续。

### 数据流

```
页面每 1–2s（完整路径，与 §4.3 一致）：
  ├─ GET /api/web/codeloop/session/claude/{id}/messages?after={claudeCursor}  → 追加左栏
  ├─ GET /api/web/codeloop/session/codex/{id}/messages?after={codexCursor}    → 追加右栏
  └─ GET /api/web/tasks/{taskId}（若已启动 loop）                             → 刷新状态条
```

会话文件 append-only，`after` 用「已读行数」做增量游标，每次只回新增消息，前端追加不重拉。
这套读取对会话是**只读旁路**，与会话自身的写入者（CLI / 桌面端）互不干扰——这也是本视图能
「实时」而桌面端做不到的原因：我们主动 tail 文件，桌面端不 watch。

### 与桌面端的关系

此双栏视图 = toolkit 自己的实时观测面板，**不是**替代 Codex/Claude 桌面端，而是把「一对关联会话的
往复过程」聚合到一屏。桌面端该开还开（用于人工介入），但跑自动循环时按 §2 约束别在桌面端碰这两会话。

## 10. 用户输入挂起与提醒

### 10.1 问题

headless 驱动（`-p` / `codex exec`）下，agent 不能弹窗等人。但实际中途常需用户拍板：「实现方案 A
还是 B」「这次改动范围是 A 还是 B」。若放任 agent 自行假设，往复就跑歪了。

### 10.2 两类"需要人"要分开处理

| 类型 | 例子 | 处理 |
|---|---|---|
| **工具权限审批** | 是否允许执行某 shell / 写某文件 | 启动时配非交互权限，**不**走 ASK_USER（见 §10.5） |
| **决策岔路** | 方案 A/B、范围 A/B、要不要砍某模块 | 走 `ASK_USER` 协议挂起，交用户拍板 |

### 10.3 ASK_USER 协议（决策岔路）：结构化选项 + 模拟弹窗

**结构化标记**（让 UI 能渲染选项弹窗，而非纯文本框）。prompt 约定 agent 遇岔路输出一行：

```
ASK_USER: {"question": "实现登录用哪种方案？", "options": ["方案A：JWT 无状态", "方案B：服务端 session"]}
```

`options` 可省（无候选项时退化为自由文本问题）。解析失败兜底：把整行问题当纯文本问。

**唤醒机制 —— DB 握手 + 任务轮询（对齐现有接口，不需要 AppState 注入）**：

> 现实约束：`TaskKind::run(input, ctx)` 只拿 `TaskCtx{task_id, pool, data_dir}`（见 `kind.rs:18`），
> 拿不到 AppState，所以**不能**用「AppState 里的 `HashMap<task_id, Sender>`」唤醒。改用任务体轮询
> DB——这正是现有抖音 kind「每 2s 轮询下游状态」的同款模式，零引擎改动。

新增表（**只把 `CREATE TABLE IF NOT EXISTS codeloop_io` 追加进 `toolkit-core/schema.rs` 的 `DDL_V1`**）。
迁移行为已核 `migrations.rs`：`migrate()` 每次启动都 `execute_batch(DDL_V1)`（幂等），新老 DB 下次启动
都会建出此表——**因此不需要、也不应 bump `SCHEMA_VERSION`**：`migrate` 只在 `meta` 无 `schema_version`
时写入，bump 并不会更新已有 DB 的 `meta`（除非另补迁移逻辑 + 迁移测试）。本次纯加表无需走那条路。

```sql
codeloop_io(
  task_id TEXT, seq INTEGER,           -- 同一任务多次提问按 seq 递增
  asked_by TEXT,                       -- 'codex' | 'claude'，答案只发回提问方
  question_json TEXT,                  -- {question, options}
  answer_text TEXT,                    -- NULL=待答；非 NULL=已答
  created_at TEXT, answered_at TEXT,
  PRIMARY KEY(task_id, seq)
)
```

`send_and_resolve` 流程：

```
loop:
  r = send(session, prompt)
  if r.reply 含 ASK_USER:
     q = parse_ask_user(r.reply)                              // {question, options?}
     seq = insert codeloop_io(task_id, asked_by=session, question_json=q, answer=NULL)
     report_progress({phase:"awaiting_input", seq, question:q}); notify(awaiting_input, q)
     answer = poll codeloop_io.answer_text WHERE task_id,seq 每 2s 直到非 NULL 或超时   // 不阻塞 runtime
     prompt = "用户答复：" + answer                            // 下一轮发回同一会话
     continue
  else:
     return r
```

- `POST /codeloop/{id}/answer { seq, text }` 只是 `UPDATE codeloop_io SET answer_text=?,answered_at=?`，
  任务下一次轮询即取到——HTTP 侧无需触达运行中的任务句柄。
- **超时**：轮询设上限（默认 30min）→ `final_verdict = AbortedTimeout`（业务终态，任务 succeeded）。
- **重启**：问题已落 DB，但引擎在 server 重启时把 `running` 标 `interrupted`（不自动重跑），
  挂起任务不会自动续。MVP 接受，runbook 注明「别在挂起时重启 server」。（可恢复挂起列为后续开放项。）
- **答案归谁**：`asked_by` 记录提问方，答案只 resume 回那个会话。

**UI 模拟弹窗**：双栏视图轮询到 `phase==awaiting_input` 时，弹出模态框展示 `question` +
把 `options` 渲染成可点按钮（额外留一个自由文本输入兜底）；用户点选 → `POST /answer {seq, text}`。

### 10.4 提醒动作

触发条件两个，统一一个 `notify(kind, payload)`：

| kind | 时机 |
|---|---|
| `awaiting_input` | 出现 ASK_USER 挂起，需用户拍板 |
| `done` | 循环终止（Pass / MaxRounds / AbortedTimeout / AbortedParse），任务完成 |

**投递方式**：现有通用 `submit` 虽收 `callback_url`，但 `runner.rs` **没有任何投递实现**（只存不投），
且通用 callback 只能「完成时触发」，无法在中途 `awaiting_input` 触发。故本任务**自己投递**：

- `CrossReviewTask` 在 `awaiting_input` / `done` 两个时机用 `reqwest` 直接 `POST` 到 Input 的
  `notify_callback`（codeloop 私有字段，不依赖通用 `callback_url`）：
  `{ task_id, kind, title, question?, verdict? }`；由 zero 推用户手机微信。
- **本机 UI**：双栏视图状态条变色 + 角标 + 可选提示音；`awaiting_input` 时弹模拟选项弹窗（§10.3）。
- callback 失败不影响任务（best-effort，记日志）。
- （若将来 `runner` 补上通用 callback 投递，`done` 事件可迁回走它；`awaiting_input` 仍需任务自投。）

### 10.5 非交互权限配置（避免卡在工具审批）

驱动子进程时配非交互权限，让循环不被「工具审批」类提示卡住（决策岔路仍走 ASK_USER）。参数位置
已对 `--help` 核实（见 §3 `send` 固化形态）：

- Codex：`codex exec -s workspace-write -c approval_policy="never" ...`（`-s/--sandbox` 在 exec 顶层，
  `resume` 子命令不接受；审批策略走 `-c` 配置覆盖，非独立 flag）。
- Claude：`claude -p ... --permission-mode acceptEdits`（允许改文件）。

> ⚠️ 安全取舍：这等于让循环在该仓自由改文件。本 MVP 跑本机、目标就是让 Claude 改 `target_path` 文件，
> 可接受；但**不要**对不信任的仓库或开 `danger-full-access` / `bypassPermissions` 跑。runbook 注明。
