# 语音指令通道设计（zero-desktop 唤醒词门控 → zero agent 经 WS 解析并调工具）

> 状态：**设计草案 v1**（仅设计，未动代码）。v0（在 toolkit 内做意图解析）已作废。
> 目标：把 zero-desktop 的实时语音识别变成 **zero agent 的又一个输入通道**。用户说出以唤醒词
> `zero` 开头的话，zero-desktop 把**优化后的文本**经 WebSocket 送进 zero；**意图解析与工具调用
> 全部由 zero（Router + nova tool-calling）完成**，与微信那条线对等。

---

## 1. 职责划分（本设计的核心）

| 仓库 | 职责 | 改动量 |
|---|---|---|
| **zero-desktop（本仓 `crates/zero-desktop`）** | ① 实时 ASR + 优化（已有）② **唤醒词门控**：判断哪条 segment 是"对 agent 说的" ③ 剥唤醒词后把**纯文本**经 WS 送进 zero ④ 接收 zero 回复并显示 | 新增一个 WS 客户端 + 门控逻辑 |
| **zero（`D:\git\zero`）** | ⑤ 经 `channel-websocket` 收文本 → 构造 `InboundMessage` ⑥ **Router LLM 决策 agent/session** ⑦ **nova bridge tool-calling 调 zero 自己的工具**（抖音/闹钟/英语/知识库…）⑧ 回复经 WS 推回 | **基本零改动**（仅新增一个独立 WS channel 配置；可选区分来源的提示词） |

**关键结论**：zero-desktop **不做 LLM 意图解析、不维护命令清单**。zero 本来就知道自己有哪些工具、怎么 tool-calling。语音文本进 zero 后与一条普通用户消息**无差别**。

> 对照 CLAUDE.md 的链路：`wechat → Agent(zero) ⇄ llm ⇄ tools-server`。本设计是再并一条
> `voice(zero-desktop) → Agent(zero) → …`，语音与微信对等。

---

## 2. 数据流总览

```
zero-desktop                                    │   zero (D:\git\zero)
────────────────────────────────────────────── │ ──────────────────────────────────
麦克风 → orchestrator(WS) → segment              │
   text_optimized（success）                     │
        │                                        │
        ▼                                        │
 ┌──────────────────────────┐                    │
 │ 唤醒词门控 (wake gate)     │                    │
 │  以 "zero" 变体开头?       │                    │
 │   否 → 普通听写，照旧       │                    │
 │   是 → 剥唤醒词，得指令文本 │                    │
 └──────────┬───────────────┘                    │
            │ "把刚才那段抖音整理一下"             │
            ▼                                     │
 ┌──────────────────────────┐   WS text frame   │  ┌─────────────────────────────┐
 │ zero WS 客户端            │ ─────────────────▶│  │ channel-websocket           │
 │ (新增)                    │                    │  │  → InboundMessage{text,...}  │
 └──────────┬───────────────┘                    │  └──────────────┬──────────────┘
            │                                     │                 ▼
            │                                     │      ┌─────────────────────┐
            │                                     │      │ Router (LLM 决策)    │
            │                                     │      │ agent / session      │
            │                                     │      └──────────┬──────────┘
            │                                     │                 ▼
            │                                     │      ┌─────────────────────┐
            │                                     │      │ nova ClawBridge      │
            │                                     │      │ tool-calling →       │
            │                                     │      │ zero 自己的工具       │
            │                                     │      └──────────┬──────────┘
            │            WS text / file / audio   │                 ▼
 ┌──────────▼───────────────┐ ◀──────────────────│      回复经 WS sink 推回
 │ 显示回复（toast/面板）     │                    │
 │ 可选：audio 信封 → 播报    │                    │
 └──────────────────────────┘                    │
```

---

## 3. zero-desktop 侧设计

### 3.1 唤醒词门控（wake gate）

仍然需要门控：zero-desktop 平时是听写工具，不能每句话都丢给 agent。门控决定"这句是不是对 agent 说的"。

- **触发源**：订阅现有 `segment_updated` 事件，取 **`text_optimized`（`optimize_status==="success"` 时）**——优化稿更干净，唤醒词更可能被纠正对；代价是有优化延迟。延迟敏感时可改用 `text_raw`。
- **匹配**：前缀**模糊匹配**唤醒词变体表 + 小编辑距离。ASR 对 `zero` 极不稳定，变体表起步：
  `["zero","Zero","泽罗","知乎","零","子萝"]`，可配。
- **命中后**：剥掉唤醒词前缀，剩余即指令文本；空则忽略。
- **不命中**：原样走现有听写流程，不送 zero。

> **为什么门控留在 zero-desktop 而非交给 zero 判断**：避免把每句听写都发去 zero（省 Router 调用、避免噪声）。门控只做**廉价的"是否要送"判断**，不做意图解析——意图解析是 zero 的事。
>
> **可选替代**：用一个 UI 开关/快捷键的"指令模式"取代唤醒词（按住说话才送 agent）。唤醒词更自然但误识率高；二者可并存。本设计以唤醒词为主，留开关为兜底。

### 3.2 zero WS 客户端（新增）

zero-desktop 已在用 WS（连 orchestrator），再加一个连 zero 的 WS 客户端是同构的。协议见 §5。

- **发送**：命中门控后，发一帧**纯文本**（zero 的 WS 接受裸文本帧，无需 JSON 包装）。
- **接收**：`onmessage` 先试 JSON 解析；解析失败即纯文本回复 → 显示。JSON 且 `type==="audio"/"file"` → 按附件处理（见 §5.3）。
- **连接**：长连接常驻；断线重连。地址可配（默认指向 zero 的语音 WS 端口，见 §4）。

### 3.3 回复显示

- **最简**：toast / 顶栏短提示显示 zero 的文本回复。
- **进阶**：在语音页加一个"指令对话"小面板，显示「我说的指令 → agent 回复」成对记录，便于看 agent 到底干了什么。
- **语音播报（可选）**：若 zero 回 `type==="audio"` 信封（zero 的 `send_voice`），可直接播 WAV。

---

## 4. 传输通道选型（专业判断）

zero 现有三类 channel：`channel-weixin` / `channel-websocket` / `channel-gateway`。

**选定 `channel-websocket`**，理由：
- 它本来就是 zero **桌面端的实时双向通道**（`crates/desktop-app` 即其客户端），既能送用户消息又能推回流式回复，正合"语音指令 + 看 agent 回复"。
- gateway 你已定位为**回调专用**，不拿来当用户消息入口。
- zero-desktop 已有 WS 基建，复用心智一致。

**关键约束 → 用独立 WS 实例**：zero 的 `channel-websocket` 是**单连接策略**（`lib.rs` 的 `ws_sink` 只保留最后一个连接，后连接覆盖前者回包通道）。若 zero-desktop 和 zero 自带 desktop-app 连**同一**端口会互相抢回包。

**因此建议**：在 zero 的 `config.toml` **新增一个独立的 `[[channel]] type="websocket"` 实例（单独端口、单独 name）** 专供语音客户端：

```toml
# zero/.zero/config.toml —— 新增，专供 zero-desktop 语音指令
[[channel]]
type = "websocket"
active = true
name = "voice-desktop"        # ← Router 可据此区分来源
role = "voice instruction bridge"
url = "ws://0.0.0.0:8101"     # 与 desktop-app(8100) 分开，避免单连接互抢
```

**附带好处**：`channel_id="voice-desktop"` 让 Router 能**区分"语音来源 vs 微信"**，给语音走更"指令式、简洁"的提示词（见 §6 可选项），无需改 `InboundMessage` 结构。

---

## 5. WS 协议契约（据 zero 源码核对，供客户端实现）

zero WS：`tokio-tungstenite`，**无 TLS / 无路径 / 无鉴权**，标准握手。URL = 上面配置的 `url`。

### 5.1 入站（zero-desktop → zero）

- **纯文本（本设计主用）**：直接发文本帧即可。zero 解析失败即降级为：
  `InboundMessage{ message_id=服务端UUID, channel_id="voice-desktop", sender_id="client", text=<原文>, timestamp_ms=now }`。
  **必填：仅正文字符串**。
- **切换会话（可选）**：`{"type":"switch_session","session_id":"<uuid>"}`。发一次后该连接后续消息都绑定此 session，不消耗一条消息。
- **附件（本设计暂不用）**：`{"type":"attachment","payload":{"kind","mime","name","caption?","data_base64"}}`。

### 5.2 会话语义

- 同一 WS 连接的多条消息**自动续接同一会话**；`sender_id` 当前硬编码 `"client"`。
- **指令场景的会话策略（需定，见 §8）**：
  - **常驻一个语音会话**（连接时发一次 `switch_session` 到固定/上次 session）→ agent 有上下文，"刚才那段""继续"这类指代能用。
  - **每条指令独立**（不发 switch，靠默认新建）→ 互不干扰，但无上下文。
  - 起步建议：**常驻一个语音会话**，更接近"助手"体验。

### 5.3 出站（zero → zero-desktop）

- **纯文本回复**：裸文本帧（无 JSON 包装）。本协议**不是 token-delta 流式**，回复以**完整文本帧**到达（可能多帧）。
- **音频信封**：`{"type":"audio","payload":{"mime","data_base64","name","message_id"}}`。
- **文件/图片信封**：`{"type":"file","payload":{"mime","kind","name","message_id","data_base64"}}`。
- 错误：`{"type":"error","payload":"<msg>"}`。

> 客户端收帧处理：`try JSON.parse` → 有 `type` 按信封；失败按纯文本。参考 zero 自带客户端
> `crates/desktop-app/src/ws.rs` 的 `WsProtocol` 枚举（`#[serde(tag="type",content="payload")]`）。

---

## 6. zero 侧改动（最小）

1. **配置**：新增上面 §4 的 `[[channel]] type="websocket"`（name=`voice-desktop`, 独立端口）。**仅配置，无代码**。
2. **（可选）来源区分提示词**：若希望语音走"指令式、简洁、少寒暄"的风格，Router 的 system prompt 里按 `channel_id=="voice-desktop"` 分支。属增强，非必需。
3. **工具**：**无需新增工具**——语音命中的诉求落到 zero 现有工具（抖音/闹钟/英语/知识库/导出…）。若发现语音常用而 zero 没有的能力，那是独立的"给 zero 加工具"任务，与本通道无关。

> 重申：**zero 不需要任何"反控 zero-desktop"的工具或回调通道**（已确认工具=zero 自己的能力）。

---

## 7. 用户反馈与可见性

- 命中门控 → zero-desktop 即时 toast「已发送指令：…」，给"正在处理"态。
- 收到 zero 回复 → 显示文本（toast 或指令面板）。
- 门控未命中但疑似（粗筛命中、用户其实想发）→ 可在面板给一条"刚才这句没当成指令（未以 zero 开头）"的弱提示，帮用户校准。
- 调试期：面板显示唤醒词命中率，便于调变体表。

---

## 8. 开放问题（落地前需定）

1. **门控触发源**：`text_optimized`（干净、有延迟）还是 `text_raw`（快、易错）？建议 optimized。
2. **唤醒词变体表**初始集合 + 是否做成可配 / 可在 UI 改。
3. **会话策略**：常驻一个语音会话 vs 每条独立？建议常驻（§5.2）。
4. **zero 语音 WS 端口**：定一个（草案 `ws://<g10或本机>:8101`），并明确 zero-desktop 连的是本机 zero 还是 G10 上的 zero。
5. **是否要来源区分提示词**（§6.2）：先不做也能跑通。
6. **唤醒词 vs 指令模式开关**：是否同时提供"按住说话"兜底。
7. **回复非流式**：本协议是整帧回复，zero-desktop 显示按"消息到达"处理即可；如需打字机效果需 zero 侧另支持（暂不做）。

---

## 9. MVP 切片建议（待批准后再做）

**zero 侧**：
1. `config.toml` 加一个 `voice-desktop` 的 websocket channel（独立端口）。启动确认能收文本、能回复。

**zero-desktop 侧**：
2. 新增 zero WS 客户端（连接/重连/收发），地址可配。
3. `useAppStore` 订阅里加 wake gate（前缀模糊匹配变体表），命中→剥唤醒词→WS 送文本。
4. 连接时发一次 `switch_session`（常驻语音会话）。
5. 收到回复 → toast / 指令面板显示。

**验收**：对着麦克风说「zero，你好」→ zero-desktop 门控命中 → zero 收到 `InboundMessage` → Router/Bridge 正常回复 → zero-desktop 显示回复。再说「zero，整理一下今天的抖音」验证真能触发 zero 的抖音工具。

> 先用**最安全的对话类指令**(打招呼/问答)跑通链路，再验证触发 zero 的有副作用工具。
