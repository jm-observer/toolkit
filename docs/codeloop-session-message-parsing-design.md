# Codeloop 会话消息解析补全（设计文档）

> 适用范围：`crates/agent-session/src/store.rs` 的会话消息解析
> （`store.messages()` / `codex_message_of` / `claude_content_to_text`）。
> 消费方：zero-desktop 复核循环对话面板（`codeloop_session_messages` Tauri 命令，
> `crates/zero-desktop/src/modules/codeloop/mod.rs`）与 toolkit-server 同名能力。
> 状态：设计待定稿。**只动解析层，不改 Tauri 命令签名、不改 `SessionMessage` 契约**。

## 1. 背景与动机

复核循环跑起来后，桌面端对话面板会**漏掉部分消息**——Codex 与 Claude 两边都有，
但成因不同、严重度不同。

对话面板的读链路是纯解析、只读磁盘：

```
codeloop_session_messages(provider, session_id, after)
  → Store::messages()                       // store.rs:111
    → 逐行 codex_message_of() / claude_message_of()
```

排查（对照本机真实 `~/.codex/sessions/**/rollout-*.jsonl` 与
`~/.claude/projects/**/*.jsonl`）定位到：解析器只认了**一种事件族 / 一部分 content block 类型**，
而真实落盘用了解析器没覆盖的结构来记录“后续轮次”的消息。

## 2. 根因（实测）

### 2.1 Codex —— 主因：resume 轮次的用户 prompt 全丢

`codex exec resume` 的**每一轮用户 prompt 只写成 `response_item`**，而不是 `event_msg`：

```json
{"type":"response_item","payload":{"type":"message","role":"user",
 "content":[{"type":"input_text","text":"请以严格审阅者身份复核 …"}]}}
```

但 `codex_message_of`（store.rs:477）**只解析 `type=="event_msg"` 且 payload 为
`user_message` / `agent_message` 的行**，对 `response_item` 一律返回 `None`。

实测一个 4 轮（4 个 `task_started`）的 rollout：只有 **1** 条 `event_msg/user_message`
（首轮种子 prompt，恰好被双写），第 2–4 轮用户 prompt 仅存在于 `response_item/user`，
桌面端**一条都看不到**。

附带：Codex 的 `reasoning`（思考）、`function_call` / `function_call_output`（工具调用）
也完全不显示——而 Claude 侧会显示 `[thinking]` / `[tool_use]` / `[tool_result]`，两边体验不对称。

> 助记：首轮能看到，是因为种子 prompt 被同时写成了 `event_msg/user_message`；resume 轮次
> 没有这次双写，所以“漏”了。

### 2.2 Claude —— 次因：`image` block 未处理

`claude_content_to_text`（store.rs:613）的 `match` 只覆盖
`text` / `thinking` / `tool_use` / `tool_result`。全量扫描本机 Claude 会话，content
里还有 `image` block 落在 `_ => {}` 被吃掉。当一条消息**只含 image**（贴图/截图）时，
渲染正文为空 → 被 store.rs:672 的 `text.trim().is_empty()` 判空整条丢弃；混合消息则丢失图标记。

Claude 的真实用户 prompt（含 resume 轮次）落盘是 `type:"user"` + **字符串 content**，
这条本就被正确解析——所以 Claude 不像 Codex 那样成片丢用户消息。

## 3. 目标 / 非目标

**目标**
- G1（必做）：Codex resume 轮次的用户 prompt 不再丢失，逐轮 user/assistant 完整呈现。
- G2：Codex 的思考 / 工具调用与 Claude 对齐（渲染为 `[thinking]` / `[tool_use]` / `[tool_result]`，
  真实内容入 `detail` 可展开）。
- G3：Claude `image` block 渲染为 `[image]` 标记，避免纯图消息被判空丢弃。
- G4：兼容旧的纯 `event_msg` rollout 与现有 fixture / 单测，不回归。

**非目标**
- 不改 Tauri 命令 / HTTP 契约、不改 `SessionMessage` 字段。
- 不渲染图片二进制本身（仅标记 `[image]`，不把 base64 塞进 `detail`）。
- 不引入跨页有状态游标；解析仍按“行片 → 消息”无副作用。

## 4. 关键约束：增量游标 + 双写去重

`messages(after)` 是**增量**接口（`after` = 已读行数游标，返回新增消息 + 新游标），
复核循环运行期被前端反复轮询。难点在于 Codex 同一轮会**同时**写两族事件：

| 事件族 | user 首轮种子 | user resume 轮 | assistant 回复 |
|---|---|---|---|
| `event_msg`（`user_message`/`agent_message`） | ✅ | ❌ | ✅ |
| `response_item`（`message` role=user/assistant） | ✅ | ✅ | ✅ |

- 若**只读 `event_msg`**（现状）→ 丢 resume 用户轮。
- 若在现状基础上**叠加 `response_item`** → assistant 与首轮 user **重复**（双写都计入）。

所以必须**择一权威源**。`response_item/message` 是完整的逐轮记录（user + assistant 每轮都有），
选它为权威源；`event_msg` 仅作旧 rollout 的回退。

## 5. 方案

### 5.1 Codex：权威源切到 `response_item`，按行片判模式

`messages()` 已持有 `lines[start..]` 整片，在转换前对该片做一次**模式判定**：

```text
若该行片包含任一 response_item/message(role ∈ {user, assistant})
  → 模式 = response_item（新源）
否则
  → 模式 = event_msg（旧源，保留 codex_message_of 现逻辑）
```

为什么按“行片”判定是安全的：Codex 每轮 **`response_item` 与 `event_msg` 同轮成对追加**，
任何含新轮次的增量页都会带上该轮的 `response_item` → 该页判为新源；纯旧 rollout 无
`response_item` → 全程旧源。fixture（仅 `event_msg`）→ 旧源，测试不回归。

> 注：增量页之间模式可不同（理论上一页旧、下一页新），但同一**轮次**的所有行总在同一页里
> 成对出现，不会把一轮拆成两种源 → 无重复、无割裂。

新源 `codex_response_item_to_msg(payload)` 规则：

| payload.type | role | 处理 |
|---|---|---|
| `message` | `developer` | **跳过**（系统/权限说明） |
| `message` | `user` | 取 `content[].input_text` 拼正文；正文以 `<environment_context>` 开头 → **跳过**（每轮注入的环境块，非真实输入） |
| `message` | `assistant` | 取 `content[].output_text` 拼正文 → role=assistant |
| `reasoning`（G2） | — | 正文 `[thinking]`，`summary`/`content` 文本入 `detail` |
| `function_call`（G2） | — | 正文 `[tool_use: <name>]`，`arguments` 入 `detail` |
| `function_call_output`（G2） | — | 正文 `[tool_result]`，`output` 入 `detail` |
| 其它 | — | 跳过 |

正文 `trim` 后为空仍丢弃（与现有行为一致）。

### 5.2 Claude：补 `image` 分支

`claude_content_to_text` 的 `match` 增加：

```rust
Some("image") => parts.push("[image]".to_string()),
```

不入 `detail`（不展开 base64）。这样纯图消息正文非空（`[image]`），不再被判空丢弃；
混合消息保留图标记。

### 5.3 渲染契约（不变）

仍复用 `SessionMessage { role, text, detail?, timestamp }`：正文里 thinking/tool/image
是标记，真实内容（思考正文 / 入参 / 返回体）汇入 `detail` 供前端折叠展开。前端无需改动。

## 6. 改动清单

- `crates/agent-session/src/store.rs`
  - `messages()`：加模式判定，Codex 分支按模式选 `codex_response_item_to_msg` 或现有 `codex_message_of`。
  - 新增 `codex_response_item_to_msg`（含 developer / environment_context 过滤、G2 的 reasoning/function_call 渲染）。
  - `claude_content_to_text`：加 `image` 分支。
- 测试 / fixture
  - 新增一个含 `response_item`（developer + environment_context + 多轮 user/assistant +
    reasoning/function_call）的 Codex fixture，断言：resume 用户轮可见、环境块/developer 不可见、
    无重复、思考/工具有 `detail`。
  - 保留现有纯 `event_msg` fixture，断言旧源回退不回归。
  - Claude fixture 加一条纯 `image` user 消息，断言渲染为 `[image]` 不被丢。

## 7. 验收

- 跑一轮真实 codeloop（≥2 轮往复），桌面对话面板：Codex 侧每轮用户 prompt + 回复齐全，
  思考/工具调用可见可展开；Claude 侧贴图消息显示 `[image]`。
- `cargo test -p agent-session` 全绿（新老 fixture 并存）。
- 增量轮询不产生重复消息（游标连续翻页，消息无重出）。

## 8. 风险与回退

- **Codex schema 再漂移**：`response_item` 字段名若再变，新源取不到正文 → 该消息按空丢弃
  （不崩、不重复），可经旧源回退兜底部分场景；坏行本就逐行跳过。
- **模式误判**：仅在“一页内既无 response_item 又确有新轮次”时才会漏，按上文“同轮成对追加”
  论证此情形不出现于真实 Codex 输出；最坏退化为现状（不会更差）。
- 纯解析层改动，**不触碰驱动 / CLI / 任务引擎**，回退即还原 `store.rs`。
