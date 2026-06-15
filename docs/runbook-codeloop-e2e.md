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

## 5. ⚠️ 待用户真机固化的 CLI 命令（Plan 3 留待实跑确认）

driver 的子进程执行**本轮未实跑**（避免烧额度）。命令向量构造与输出解析已是纯函数并单测，
但下面两条 argv / 输出 schema **需用户在本机各实跑一次确认**，发现漂移后回 `agent-session`
的 `driver.rs`（`codex_argv` / `claude_argv` / `parse_codex_stdout` / `parse_claude_stdout`）校正：

### Codex（固化形态）

```powershell
codex exec -s workspace-write -c approval_policy="never" --cd <repo_root> resume --json <codex_session_id> "<prompt>"
```

- 验证点：能 resume 指定会话、不卡审批、stdout 是 JSONL。
- 解析点：取最后一个 `payload.type == "task_complete"` 的 `last_agent_message`；
  无则退化取末个 `agent_message.message`。确认字段路径与本机 codex 版本一致
  （部分版本可能把 `type` 平铺在顶层，解析已兼容两种形态）。

### Claude（固化形态，必须在会话原始 cwd 下执行）

```powershell
# 在该 Claude 会话的原始 cwd 下：
claude -p "<prompt>" --resume <claude_session_id> --permission-mode acceptEdits
```

- 验证点：在原始 cwd 下能 resume（换目录会 `No conversation found`）、`-p` 阻塞到本轮完成、
  回复打到 stdout。
- 解析点：MVP 取 stdout 纯文本 trim。若改用 `--output-format stream-json` 需同步改
  `parse_claude_stdout`。

### 验后回归

实跑确认无误后，跑一次最小真机循环（小 target、`max_rounds=1~2`）验证：状态条轮次推进、
VERDICT 解析、PASS / MaxRounds 终态、ASK_USER 弹窗与应答 resume。
