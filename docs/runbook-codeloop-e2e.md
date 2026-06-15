# Codeloop 跨会话复核循环 — 本机端到端 runbook

> 适用：在**本机 Windows**起一个 toolkit-server 实例，用双栏视图选一对 Codex / Claude
> 会话，启动自动复核循环并在挂起时应答。设计见
> `docs/toolkit-rfc/2026-06-15-cross-session-review-loop/plan.md`。

## 0. 前置

- 本机已装并登录 `codex` 与 `claude` CLI，且二者在 PATH 中。
- 会话存储在本机用户 home：`~/.codex`、`~/.claude`（不在 g10）。
- 待复核的 Codex / Claude 会话**同属一棵 git 工作树**（§4.1 校验，否则 400）。

## 1. 起本机 server 实例

```powershell
# 单独的本机 workspace，别复用 g10 实例
$env:TOOLKIT_WORKSPACE = "$env:USERPROFILE\.config\toolkit-server"
cargo run -p toolkit-server
# 默认监听见 config；浏览器打开 http://127.0.0.1:<port>/codeloop
```

会话存储自动从 `~/.codex` / `~/.claude` 读取（`Store::from_env`）。

## 2. 选会话 + 启动循环

1. 顶部两个下拉分别选 Claude 会话与 Codex 会话（`↻` 刷新清单）。
2. 填 `target_path`（相对工作树根，如 `docs/foo.md`；也可绝对路径）。
3. 选 `mode`（design / implementation）、`max_rounds`（默认 5）。
4. 需要先等当前 Claude 轮次结束再接管时，勾「等 Claude 空闲」。
5. 点「启动复核循环」→ `POST /api/web/codeloop/submit`。
   - 服务端先做三方 repo root 一致性校验，不一致返回 400（提示三方实际路径）。
   - 通过则起 `cross_review` 任务，状态条显示轮次 / phase / 最近 VERDICT。

## 3. 应答挂起（ASK_USER）

循环中 agent 遇决策岔路会输出 `ASK_USER: {json}` 并挂起。视图轮询到
`phase==awaiting_input` 时弹模拟弹窗：点选项按钮或自由输入 → `POST /api/web/codeloop/{task_id}/answer`
写 `codeloop_io`，任务下次轮询取走答案 resume 提问方会话。

## 4. 约束（务必遵守）

- **跑循环时别在桌面端碰这两个会话**（单写者约束，§2），跑完重开桌面端看结果。
- **别在挂起（awaiting_input）时重启 server**：重启会把 running 任务标 `interrupted`，
  挂起不自动续（MVP 限制，§10.3）。
- 非交互权限：循环让 codex/claude 在该仓自由改文件（codex `-s workspace-write` +
  `approval_policy="never"`；claude `--permission-mode acceptEdits`）。**仅对本机可信仓库跑**，
  不要开 `danger-full-access` / `bypassPermissions`。

## 5. ✅ CLI 命令真机固化结论（2026-06-15 实跑核实）

driver 的两条命令已在本机各实跑一次（codex-cli 0.130.0 / claude 2.1.170，Windows）确认。
`agent-session/src/driver.rs` 的 `parse_codex_stdout` 已据此修正（见下「重要修正」）。

### Codex（已验证）

```powershell
codex exec -s workspace-write -c approval_policy="never" --cd <repo_root> resume --json <codex_session_id> "<prompt>"
```

- 结果：exit 0，能 resume、不卡审批，stdout 为事件 JSONL。
- **实测 `--json` 事件 schema**（与 rollout 文件格式不同！）：
  - `{"type":"thread.started","thread_id":...}`
  - `{"type":"turn.started"}`
  - `{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"..."}}` ← **回复在此**
  - `{"type":"turn.completed","usage":{input_tokens,output_tokens,...}}`
- **解析（已固化）**：取末个 `item.completed` 且 `item.type=="agent_message"` 的 `item.text`；
  退化兼容旧 `task_complete.last_agent_message` / `agent_message.message`。
- ⚠️ **Windows stdout 编码坑（实测）**：stdout 会混入非 UTF-8（GBK）噪声行，例如 codex 收尾时的
  `成功: 已终止 PID xxxxx 的进程`（taskkill 输出，含 `0xb3` 字节）。`run_capture` 用
  `from_utf8_lossy` 兜底 → 噪声行变替换字符 → 作为非 JSON 行被解析器跳过。**不要**用严格 UTF-8
  整体解码 stdout。

### Claude（已验证，须在会话原始 cwd 下执行）

```powershell
# 在该 Claude 会话的原始 cwd 下：
claude -p "<prompt>" --resume <claude_session_id> --permission-mode acceptEdits
```

- 结果：exit 0，原始 cwd 下能 resume，`-p` 阻塞到完成，stdout 为**干净 UTF-8 纯文本**（实测 `OK\n`）。
- 解析（已固化）：取 stdout 纯文本 trim。若改 `--output-format stream-json` 需同步改 `parse_claude_stdout`。
- 注意：`-p` 无管道输入时 stderr 会有「no stdin data received in 3s, proceeding」warning，不影响结果；
  driver spawn 不接 stdin（如需可显式 `< NUL`）。

### 重要修正

固化实测发现 driver 原 `parse_codex_stdout` 假设的 `task_complete.last_agent_message` **不出现在
`--json` stdout**（那是 rollout 文件字段）。已改为优先解析 `item.completed.item`，并补真机序列回归单测
（`parse_codex_real_json_schema` / `parse_codex_item_completed_wins_over_legacy`）。

### 验后回归（建议）

可再跑一次最小真机循环（小 target、`max_rounds=1~2`）验证：状态条轮次推进、VERDICT 解析、
PASS / MaxRounds 终态、ASK_USER 弹窗与应答 resume。本次仅固化了单轮 send/解析，未跑完整循环。
