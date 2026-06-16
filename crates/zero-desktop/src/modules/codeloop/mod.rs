//! Codeloop 模块：Codex⇄Claude Code 跨会话复核循环的桌面内嵌实现。
//!
//! 采集层直接用 `agent-session`（读会话 / 起 CLI / 等空闲），协议层用 `codeloop-core`
//! （prompt 模板 / verdict 解析 / 三方校验）。循环跑在 zero-desktop 自己进程里，本机无需
//! 任何额外进程（不依赖 toolkit-server）。设计见
//! `docs/toolkit-rfc/2026-06-15-cross-session-review-loop/plan.md` 与本仓 plan。
//!
//! 与 toolkit-server 版（`crates/toolkit-server/src/codeloop/kind.rs`）的差异：
//! - 进度上报：`report_progress` 写 DB → 改 `app.emit("codeloop://progress")` 推前端 + 内存快照。
//! - ASK_USER 挂起：`codeloop_io` 表 + 2s 轮询 → 同进程 `oneshot` channel（拿得到 AppState，更干净）。
//! - 任务引擎：`impl TaskKind` → `tokio::spawn` 的后台任务，句柄存 `CodeloopState`。
//! - 通知回调（推微信）本期不做。

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_session::store::Store;
use agent_session::{driver, watch, MessagesPage, Provider, SessionRef, SessionSummary};
use anyhow::Result;
use codeloop_core::parse::{self, Verdict};
use codeloop_core::prompt::{self, ReviewMode, TargetSpec};
use codeloop_core::validate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::app_state::AppState;

/// 等待 Claude 当前轮空闲的超时（对应 wait_for_claude_idle）。
const CLAUDE_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// ASK_USER 挂起等待用户回答的上限。
const ANSWER_TIMEOUT: Duration = Duration::from_secs(1800);
/// 连续解析失败到此轮数 → AbortedParse。
const MAX_PARSE_FAILS: u32 = 2;

/// 进度事件名（前端 listen 它刷新状态条 / 触发 ASK_USER 弹窗）。
const EV_PROGRESS: &str = "codeloop_progress";

// ------------------------- 模块状态 -------------------------

/// Codeloop 模块状态：同一时刻只允许一个复核循环在跑。
#[derive(Default)]
pub struct CodeloopState {
    inner: Mutex<Option<RunningLoop>>,
}

/// 一个运行中（或刚结束）的循环。
struct RunningLoop {
    handle: JoinHandle<()>,
    /// 最近一次上报的进度快照（供 `codeloop_status` 兜底读取）。
    progress: Arc<Mutex<Value>>,
    /// ASK_USER 挂起态（非 None = 正等用户回答）。
    pending: Arc<Mutex<Option<Pending>>>,
}

/// 一个待用户回答的问题：seq + 唤醒循环的 oneshot 发送端。
struct Pending {
    seq: i64,
    answer_tx: oneshot::Sender<String>,
}

// ------------------------- 输入契约 -------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SessionRefDto {
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StartInput {
    pub claude: SessionRefDto,
    pub codex: SessionRefDto,
    pub target_path: String,
    #[serde(default)]
    pub target_label: Option<String>,
    pub mode: ReviewMode,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    #[serde(default)]
    pub wait_for_claude_idle: bool,
}

fn default_max_rounds() -> u32 {
    5
}

/// 业务终态（对齐 toolkit-server 版 FinalVerdict 语义）。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum FinalVerdict {
    Pass,
    MaxRounds,
    AbortedTimeout,
    AbortedParse,
}

// ------------------------- 循环上下文 -------------------------

/// 运行期上下文：解析好的两端 SessionRef + target 定位 + 配置 + 共享句柄。
struct LoopCtx {
    app: AppHandle,
    store: Store,
    claude: SessionRef,
    codex: SessionRef,
    target: TargetSpec,
    mode: ReviewMode,
    max_rounds: u32,
    wait_for_claude_idle: bool,
    progress: Arc<Mutex<Value>>,
    pending: Arc<Mutex<Option<Pending>>>,
    seq: Arc<AtomicI64>,
}

impl LoopCtx {
    /// 写进度快照 + emit 给前端。
    async fn report(&self, v: Value) {
        *self.progress.lock().await = v.clone();
        let _ = self.app.emit(EV_PROGRESS, v);
    }

    /// 终态收尾：上报 done。
    async fn finish(&self, final_verdict: FinalVerdict, total_rounds: u32) {
        self.report(json!({
            "phase": "done", "final_verdict": final_verdict, "total_rounds": total_rounds,
        }))
        .await;
    }
}

/// send_and_resolve 的结果：拿到回复，或等用户答超时。
enum Resolved {
    Reply(String),
    Timeout,
}

/// 发一轮 → 若含 ASK_USER 则挂起等用户答（同进程 oneshot）→ 把答案发回同一会话 →
/// 直到不再 ASK_USER。基础设施错（CLI 缺失 / spawn 失败）→ Err。
async fn send_and_resolve(
    ctx: &LoopCtx,
    session: &SessionRef,
    prompt_text: &str,
) -> Result<Resolved> {
    let mut current = prompt_text.to_string();
    loop {
        let turn = driver::send(session, &current).await?;
        let Some(q) = parse::parse_ask_user(&turn.reply_text) else {
            return Ok(Resolved::Reply(turn.reply_text));
        };

        // 挂起：建 channel、存 pending、emit awaiting_input。
        let seq = ctx.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel::<String>();
        *ctx.pending.lock().await = Some(Pending { seq, answer_tx: tx });
        ctx.report(json!({
            "phase": "awaiting_input",
            "seq": seq,
            "asked_by": session.provider.as_str(),
            "question": q,
        }))
        .await;

        // 等用户回答（带超时）。超时 / 发送端被丢 → 业务终态 Timeout。
        match tokio::time::timeout(ANSWER_TIMEOUT, rx).await {
            Ok(Ok(answer)) => current = format!("用户答复：{answer}"),
            _ => {
                *ctx.pending.lock().await = None;
                return Ok(Resolved::Timeout);
            }
        }
    }
}

/// 复核↔修订主循环。基础设施错 → Err（上层 emit error）；业务终态正常收尾。
async fn drive(ctx: &LoopCtx) -> Result<()> {
    if ctx.wait_for_claude_idle {
        if let Err(e) = watch::wait_for_idle(&ctx.store, &ctx.claude, CLAUDE_IDLE_TIMEOUT).await {
            log::warn!("wait_for_claude_idle 超时/失败，按 AbortedTimeout 处理: {e:#}");
            ctx.finish(FinalVerdict::AbortedTimeout, 0).await;
            return Ok(());
        }
    }

    let mut consecutive_parse_fail = 0u32;
    for n in 1..=ctx.max_rounds {
        // 1. Codex 复核（含 ASK_USER 挂起）。
        let codex_prompt =
            prompt::render_codex_prompt(prompt::DEFAULT_CODEX_TEMPLATE, &ctx.target, ctx.mode, n);
        let review = match send_and_resolve(ctx, &ctx.codex, &codex_prompt).await? {
            Resolved::Reply(r) => r,
            Resolved::Timeout => {
                ctx.finish(FinalVerdict::AbortedTimeout, n - 1).await;
                return Ok(());
            }
        };

        // 2. 解析 VERDICT。
        let verdict = match parse::parse_verdict(&review) {
            Some(v) => {
                consecutive_parse_fail = 0;
                v
            }
            None => {
                consecutive_parse_fail += 1;
                if consecutive_parse_fail >= MAX_PARSE_FAILS {
                    ctx.report(json!({
                        "round": n, "phase": "reviewed", "verdict": "parse_failed",
                        "consecutive_parse_fail": consecutive_parse_fail,
                    }))
                    .await;
                    ctx.finish(FinalVerdict::AbortedParse, n - 1).await;
                    return Ok(());
                }
                Verdict::NeedsWork
            }
        };
        ctx.report(json!({ "round": n, "phase": "reviewed", "verdict": verdict }))
            .await;

        // 3. PASS → 终止。
        if verdict == Verdict::Pass {
            ctx.finish(FinalVerdict::Pass, n).await;
            return Ok(());
        }

        // 4. Claude 据意见修订（含 ASK_USER 挂起）。
        let claude_prompt =
            prompt::render_claude_prompt(prompt::DEFAULT_CLAUDE_TEMPLATE, &ctx.target, &review);
        match send_and_resolve(ctx, &ctx.claude, &claude_prompt).await? {
            Resolved::Reply(_) => {}
            Resolved::Timeout => {
                ctx.finish(FinalVerdict::AbortedTimeout, n).await;
                return Ok(());
            }
        }
        ctx.report(json!({ "round": n, "phase": "revised" })).await;
    }

    // 跑满未 PASS。
    ctx.finish(FinalVerdict::MaxRounds, ctx.max_rounds).await;
    Ok(())
}

/// 循环顶层：跑 drive，基础设施错时 emit error；收尾清 pending。
async fn run_loop(ctx: LoopCtx) {
    if let Err(e) = drive(&ctx).await {
        ctx.report(json!({ "phase": "error", "error": format!("{e:#}") }))
            .await;
    }
    *ctx.pending.lock().await = None;
}

/// 把 DTO 解析成 SessionRef：cwd 缺省时从会话存储 snapshot 补全。
fn resolve_ref(store: &Store, provider: Provider, dto: &SessionRefDto) -> Result<SessionRef> {
    let cwd = match &dto.cwd {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => store.snapshot(provider, &dto.session_id)?.cwd,
    };
    Ok(SessionRef {
        provider,
        session_id: dto.session_id.clone(),
        cwd,
    })
}

// ------------------------- Tauri 命令 -------------------------

/// 列出本机 Codex / Claude 会话清单（供前端配对挑选）。
#[tauri::command]
pub async fn codeloop_list_sessions(limit: Option<usize>) -> Result<Vec<SessionSummary>, String> {
    let store = Store::from_env()
        .map_err(|e| format!("定位会话存储失败（~/.codex / ~/.claude）：{e:#}"))?;
    store
        .list(limit.unwrap_or(30))
        .map_err(|e| format!("{e:#}"))
}

/// 新建 Codex 会话的种子提示词（仅用于建立会话；真正的复核任务由循环后续发起）。
const NEW_CODEX_SEED: &str =
    "你好。这是一个用于跨会话复核的新会话，已就绪。请回复「已就绪」，等待后续复核任务。";

/// 新建一个 Codex 会话：复用所选 Claude 会话的 cwd（同一仓库），用默认种子提示词跑一轮
/// `codex exec` 建会话，返回新会话 id（前端据此选中 + 刷新清单）。**消耗 codex 额度**。
#[tauri::command]
pub async fn codeloop_new_codex_session(claude_session_id: String) -> Result<String, String> {
    let store = Store::from_env().map_err(|e| format!("定位会话存储失败：{e:#}"))?;
    let snap = store
        .snapshot(Provider::Claude, &claude_session_id)
        .map_err(|e| format!("读取所选 Claude 会话的仓库目录失败：{e:#}"))?;
    driver::create_codex_session(&snap.cwd, NEW_CODEX_SEED)
        .await
        .map_err(|e| format!("新建 Codex 会话失败（codex CLI 是否在 PATH？）：{e:#}"))
}

/// 增量取某会话消息（cursor = 已读行数）。
#[tauri::command]
pub async fn codeloop_session_messages(
    provider: String,
    session_id: String,
    after: usize,
) -> Result<MessagesPage, String> {
    let p =
        Provider::parse(&provider).ok_or_else(|| "provider 必须是 codex 或 claude".to_string())?;
    let store = Store::from_env().map_err(|e| format!("定位会话存储失败：{e:#}"))?;
    store
        .messages(p, &session_id, after)
        .map_err(|e| format!("{e:#}"))
}

/// 启动一对会话的复核循环。三方一致性校验通过后 spawn 后台循环。
#[tauri::command]
pub async fn codeloop_start(
    app: AppHandle,
    state: State<'_, AppState>,
    input: StartInput,
) -> Result<(), String> {
    let cs = &state.codeloop;

    // 单写者：已有未结束的循环则拒。
    {
        let guard = cs.inner.lock().await;
        if let Some(rl) = guard.as_ref() {
            if !rl.handle.is_finished() {
                return Err("已有复核循环在运行，请先停止再启动".into());
            }
        }
    }

    let store = Store::from_env()
        .map_err(|e| format!("定位会话存储失败（~/.codex / ~/.claude）：{e:#}"))?;
    let claude = resolve_ref(&store, Provider::Claude, &input.claude)
        .map_err(|e| format!("解析 Claude 会话失败：{e:#}"))?;
    let mut codex = resolve_ref(&store, Provider::Codex, &input.codex)
        .map_err(|e| format!("解析 Codex 会话失败：{e:#}"))?;

    // 三方仓库一致性校验（拒绝跑错仓）。
    let validated = validate::validate_three_way(&claude.cwd, &codex.cwd, &input.target_path)
        .map_err(|e| format!("{e:#}"))?;

    let repo_root = validate::display_path(&validated.repo_root);
    let target_abs = validate::display_path(&validated.target_abs);
    let repo_rel = validated
        .target_abs
        .strip_prefix(&validated.repo_root)
        .unwrap_or(&validated.target_abs)
        .to_string_lossy()
        .replace('\\', "/");

    // Codex `exec resume` 的 --cd 用工作树根，消除子目录相对路径歧义；Claude resume 保持原 cwd。
    codex.cwd = repo_root.clone();

    let label = input
        .target_label
        .unwrap_or_else(|| prompt::default_label(&repo_rel));
    let target = TargetSpec {
        label,
        repo_root: repo_root.to_string_lossy().to_string(),
        repo_rel,
        abs: target_abs.to_string_lossy().to_string(),
    };

    let progress = Arc::new(Mutex::new(json!({ "phase": "starting" })));
    let pending = Arc::new(Mutex::new(None));
    let seq = Arc::new(AtomicI64::new(0));

    let ctx = LoopCtx {
        app: app.clone(),
        store,
        claude,
        codex,
        target,
        mode: input.mode,
        max_rounds: input.max_rounds.max(1),
        wait_for_claude_idle: input.wait_for_claude_idle,
        progress: progress.clone(),
        pending: pending.clone(),
        seq,
    };

    let handle = tokio::spawn(run_loop(ctx));
    *cs.inner.lock().await = Some(RunningLoop {
        handle,
        progress,
        pending,
    });
    Ok(())
}

/// 当前循环状态快照：`{ running, progress }`。
#[tauri::command]
pub async fn codeloop_status(state: State<'_, AppState>) -> Result<Value, String> {
    let guard = state.codeloop.inner.lock().await;
    match guard.as_ref() {
        Some(rl) => {
            let running = !rl.handle.is_finished();
            let progress = rl.progress.lock().await.clone();
            Ok(json!({ "running": running, "progress": progress }))
        }
        None => Ok(json!({ "running": false, "progress": Value::Null })),
    }
}

/// 回答挂起的 ASK_USER：唤醒循环。
#[tauri::command]
pub async fn codeloop_answer(
    state: State<'_, AppState>,
    seq: i64,
    text: String,
) -> Result<(), String> {
    let guard = state.codeloop.inner.lock().await;
    let Some(rl) = guard.as_ref() else {
        return Err("没有运行中的复核循环".into());
    };
    let mut pending = rl.pending.lock().await;
    match pending.take() {
        Some(p) if p.seq == seq => p
            .answer_tx
            .send(text)
            .map_err(|_| "循环已不在等待该回答".to_string()),
        Some(other) => {
            // seq 不匹配，放回。
            *pending = Some(other);
            Err("seq 与当前待答问题不匹配".into())
        }
        None => Err("当前没有待回答的问题".into()),
    }
}

/// 停止当前循环（abort 后台任务，清状态）。
#[tauri::command]
pub async fn codeloop_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.codeloop.inner.lock().await;
    if let Some(rl) = guard.take() {
        rl.handle.abort();
    }
    Ok(())
}
