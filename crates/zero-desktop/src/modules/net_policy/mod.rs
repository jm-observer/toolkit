//! net-policy 模块：未知流量默认走 WireGuard 海外出口，仅显式配置的程序/域名/IP
//! 走本地直连；防火墙 kill-switch 提供 fail-closed 兜底。
//!
//! 设计见 docs/unified-desktop-shell-design.md §14；落地依据的真机验证见
//! docs/net-policy-validation-report.md（§0.x 实测结论）。**仅 Windows**。
//!
//! 所有 Tauri command 名称以 `net_policy_` 开头。

mod config;
mod connections;
mod engine;
mod firewall;
mod process_watch;
mod valid;
mod verify;
mod win;

use crate::app_state::AppState;
use anyhow::{bail, Context, Result};
use config::{NetPolicySettings, Rule, RuleSet};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

/// net-policy 模块状态。
pub struct NetPolicyState {
    pub workspace: PathBuf,
    rt: Mutex<Runtime>,
}

#[derive(Default)]
struct Runtime {
    /// 是否已 apply（mihomo + 可选 kill-switch）。
    applied: bool,
    mihomo_pid: Option<u32>,
    /// 本次 apply 生成的 external-controller secret（鉴权 mihomo API，P0-1）。
    secret: Option<String>,
}

/// 新防火墙白名单模型（`Program=mihomo.exe`，§0.10.1）是否已在新模型下重跑 VP-08/09/10 通过。
/// 在重新真机验证通过前为 `false`——`protected` 仅算"实验保护"，前端须如实标注（P0-2）。
const FIREWALL_MODEL_VALIDATED: bool = false;

impl NetPolicyState {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            rt: Mutex::new(Runtime::default()),
        }
    }
}

/// 初始化：确保 workspace 子目录存在。首版**不自动 apply**（安全：避免开机即改全局防火墙）。
///
/// **P1（secret 恢复）**：上次 apply 的 mihomo + kill-switch 可能在应用重启后仍存活（防火墙规则
/// 跨重启持久、mihomo 若注册为服务/计划任务也可能仍在）。此时从生成的 `config.yaml` 解析出
/// controller secret，并在确认 mihomo 仍可鉴权访问时恢复运行态——否则重启后 `secret` 丢失会导致
/// `get_status`/`emergency_stop` 用空 secret 鉴权失败、无法管理或停掉旧实例（防火墙被卡死）。
pub fn setup(_app: &tauri::AppHandle, state: Arc<NetPolicyState>) -> Result<()> {
    let dir = config::net_policy_dir(&state.workspace);
    std::fs::create_dir_all(dir.join("generated")).ok();
    if win::is_windows() {
        if let Some(secret) = config::read_generated_secret(&state.workspace) {
            // 仅在旧实例确实仍在（且 secret 有效可鉴权）时恢复，避免把陈旧配置误判为已应用。
            if engine::running(&secret) {
                let mut rt = state.rt.lock().unwrap();
                rt.applied = true;
                rt.secret = Some(secret);
                // pid 跨进程重启不可知；graceful_stop 在 pid=None 时回退按二进制名停。
                rt.mihomo_pid = None;
            }
        }
    }
    Ok(())
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

/// 离线生成产物（供 CLI `net-policy-gen` 预览 / 真机验证用，不执行任何副作用）。
/// 返回 `(mihomo_config_yaml, firewall_apply_script)`，输入取自 workspace 的 settings/rules。
pub fn gen_artifacts(workspace: &std::path::Path) -> Result<(String, String)> {
    let settings = config::load_settings(workspace);
    let rules = config::load_rules(workspace);
    let cfg = engine::generate_config(&settings, &rules, "<runtime-secret>");
    let fw =
        firewall::build_apply_script(workspace, &settings, &rules, &engine::mihomo_bin(workspace))?;
    Ok((cfg, fw))
}

// ============ 状态 ============

#[derive(Debug, Serialize)]
pub struct NetPolicyStatus {
    pub platform_supported: bool,
    pub wg_configured: bool,
    pub killswitch_enabled: bool,
    pub applied: bool,
    pub mihomo_running: bool,
    /// mihomo TUN（Meta 适配器）是否已起栈并 Up（P2）。controller 可达但 TUN 未起栈时为 false——
    /// 用于区分"kill-switch 已阻断但隧道未连通"（fail-closed 仍成立，但应用无法联网）与真正连通。
    /// 真实出口可达性 / DNS 劫持等更重的探测放在按需的 `net_policy_verify`（避免每次轮询打网络）。
    pub tun_ready: bool,
    /// 是否处于"受保护"状态：kill-switch 启用 + 防火墙默认出站已 Block + mihomo 在跑 **且 TUN 已起栈**。
    /// 若 false 且 applied=true，前端据 firewall.active 区分"不受保护预览"与"已阻断但未连通"（P0-1/P2）。
    pub protected: bool,
    /// 当前防火墙白名单模型是否已真机验证（P0-2）。false=实验保护，不能宣称 fail-closed。
    pub protection_validated: bool,
    pub firewall: Option<firewall::FirewallStatus>,
}

#[tauri::command]
pub fn net_policy_get_status(state: State<'_, AppState>) -> Result<NetPolicyStatus, String> {
    let np = &state.net_policy;
    let settings = config::load_settings(&np.workspace);
    let (applied, secret) = {
        let rt = np.rt.lock().unwrap();
        (rt.applied, rt.secret.clone())
    };
    let firewall = if win::is_windows() {
        firewall::status().ok()
    } else {
        None
    };
    let mihomo_running = win::is_windows() && engine::running(secret.as_deref().unwrap_or(""));
    // TUN 起栈检查只在 controller 可达时才有意义（也省掉一次无谓的 PS 调用）。
    let tun_ready = mihomo_running && engine::tun_up();
    let protected = settings.killswitch_enabled
        && firewall.as_ref().map(|f| f.active).unwrap_or(false)
        && mihomo_running
        && tun_ready;
    Ok(NetPolicyStatus {
        platform_supported: win::is_windows(),
        wg_configured: engine::validate(&settings).is_ok(),
        killswitch_enabled: settings.killswitch_enabled,
        applied,
        mihomo_running,
        tun_ready,
        protected,
        protection_validated: FIREWALL_MODEL_VALIDATED,
        firewall,
    })
}

/// 活跃连接快照（P0-1，设计 §3.1/§5）：代理运行中 mihomo 的 external-controller `/connections`，
/// 复用 runtime 里的 controller secret 做 Bearer 鉴权。驱动全景图双分支聚合与「当前活跃连接」活数据。
/// mihomo 未跑 / secret 缺失 / 控制器不可达 → 返回**空快照**（非错误），供 3s 快轮询平滑降级。
#[tauri::command]
pub async fn net_policy_connections(
    state: State<'_, AppState>,
) -> Result<connections::ConnectionsSnapshot, String> {
    if !win::is_windows() {
        // 非 Windows：net-policy 不可用，返回空快照而非错误（与失败语义一致，便于前端统一处理）。
        return Ok(connections::ConnectionsSnapshot::empty_snapshot());
    }
    let secret = {
        let rt = state.net_policy.rt.lock().unwrap();
        rt.secret.clone().unwrap_or_default()
    };
    Ok(connections::fetch(&secret).await)
}

// ============ 设置（含 WG 配置） ============

#[tauri::command]
pub fn net_policy_get_settings(state: State<'_, AppState>) -> NetPolicySettings {
    config::load_settings(&state.net_policy.workspace)
}

#[tauri::command]
pub fn net_policy_save_settings(
    state: State<'_, AppState>,
    settings: NetPolicySettings,
) -> Result<(), String> {
    // 校验后再存（P1-3：拒绝非法/可注入的 WG/DNS/LAN 值）。
    settings.validate().map_err(err)?;
    config::save_settings(&state.net_policy.workspace, &settings).map_err(err)
}

/// 解析用户选择的 WireGuard `.conf` 文本，返回填好的 WG 出口配置（**不落盘**）。
/// 前端读取文件内容后调用，拿到结果合并进当前设置，由用户确认后再走
/// `net_policy_save_settings` 校验保存（Endpoint 为域名等问题在保存时报错）。
#[tauri::command]
pub fn net_policy_parse_wg_conf(content: String) -> Result<config::WgConfig, String> {
    config::WgConfig::from_wg_quick(&content).map_err(err)
}

// ============ 规则 ============

#[tauri::command]
pub fn net_policy_list_rules(state: State<'_, AppState>) -> RuleSet {
    config::load_rules(&state.net_policy.workspace)
}

/// 追加一条规则并持久化（新增；编辑由前端整集替换或先删后加）。
#[tauri::command]
pub fn net_policy_save_rule(state: State<'_, AppState>, rule: Rule) -> Result<RuleSet, String> {
    rule.validate().map_err(err)?; // P1-3：校验规则值，拒绝注入
    let ws = &state.net_policy.workspace;
    let mut rs = config::load_rules(ws);
    rs.rules.push(rule);
    config::save_rules(ws, &rs).map_err(err)?;
    Ok(rs)
}

/// 删除第 `index` 条普通规则。
#[tauri::command]
pub fn net_policy_delete_rule(state: State<'_, AppState>, index: usize) -> Result<RuleSet, String> {
    let ws = &state.net_policy.workspace;
    let mut rs = config::load_rules(ws);
    if index >= rs.rules.len() {
        return Err(format!("规则下标越界：{index}"));
    }
    rs.rules.remove(index);
    config::save_rules(ws, &rs).map_err(err)?;
    Ok(rs)
}

// ============ 进程候选 ============

#[tauri::command]
pub async fn net_policy_list_process_candidates(
) -> Result<Vec<process_watch::ProcessCandidate>, String> {
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    tokio::task::spawn_blocking(process_watch::list_candidates)
        .await
        .map_err(err)?
        .map_err(err)
}

// ============ 应用进度事件（Phase 2，设计 §3.3） ============

/// `net_policy_apply` 逐阶段进度事件的频道名。前端 `listen('net-policy://apply-progress')` 订阅。
pub const APPLY_PROGRESS_EVENT: &str = "net-policy://apply-progress";

/// apply 的 6 个阶段（与设计 §3.3 stepper 对齐，索引从 0 起）。
const APPLY_STEPS: [&str; 6] = [
    "校验配置",
    "装防火墙基线",
    "启动引擎",
    "等待 TUN 起栈",
    "补 TUN 白名单",
    "验证连通",
];

/// 单步进度。`status` ∈ {running, ok, fail}；`detail` 为可选补充（如 TUN 轮询 N/14、错误原文）。
#[derive(Debug, Clone, Serialize)]
pub struct ApplyProgress {
    /// 步索引（0..6），对应 `APPLY_STEPS`。
    pub step: usize,
    /// 步名（冗余给前端，省得对索引表）。
    pub name: String,
    /// running / ok / fail。
    pub status: String,
    /// 可选补充信息（进度、错误原文等）。
    pub detail: Option<String>,
}

/// 发一条进度事件（emit 失败不影响主流程——进度仅为可观测性）。
fn emit_progress(app: &tauri::AppHandle, step: usize, status: &str, detail: Option<String>) {
    use tauri::Emitter;
    let _ = app.emit(
        APPLY_PROGRESS_EVENT,
        ApplyProgress {
            step,
            name: APPLY_STEPS.get(step).copied().unwrap_or("").to_string(),
            status: status.to_string(),
            detail,
        },
    );
}

// ============ 应用 / 急停 ============

/// 应用策略：校验 → 生成配置 → 启动 mihomo → 应用 kill-switch（默认开启）。
/// **事务化（P0-2）**：任一步失败按安全顺序回滚（先停引擎再撤防火墙），不留半应用状态。
/// **受控 reapply（P0-1）**：已应用过则先用旧 secret/pid 优雅停旧引擎再起新引擎，旧 kill-switch
/// 在交换窗口内保持生效（fail-closed 不破），杜绝"重复 apply → 旧 mihomo 仍在跑/占端口、新引擎
/// 起不来、防火墙却被撤掉"。kill-switch 关闭时为"不受保护预览"，status.protected=false（P0-1）。
/// `rt` 在每个中间/失败点都被更新为与机器实际状态一致（避免重启/急停拿到陈旧 pid/secret）。
#[tauri::command]
pub async fn net_policy_apply(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<NetPolicyStatus, String> {
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    let np = state.net_policy.clone();
    let ws = np.workspace.clone();
    let settings = config::load_settings(&ws);
    let rules = config::load_rules(&ws);

    // 步 0：校验配置（进副作用前先校验全部输入，P1-3）。
    emit_progress(&app, 0, "running", None);
    if let Err(e) = settings.validate().and_then(|_| rules.validate()) {
        emit_progress(&app, 0, "fail", Some(err(&e)));
        return Err(err(e));
    }
    emit_progress(&app, 0, "ok", None);

    let killswitch = settings.killswitch_enabled;
    let mihomo_bin = engine::mihomo_bin(&ws);
    let secret = engine::gen_secret(); // P0-1：每次 apply 随机 controller secret
    let app_bg = app.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let app = app_bg;
        // 当前已应用状态快照——受控 reapply 的依据（P0-1）。
        let (was_applied, old_pid, old_secret) = {
            let rt = np.rt.lock().unwrap();
            (
                rt.applied,
                rt.mihomo_pid,
                rt.secret.clone().unwrap_or_default(),
            )
        };

        // 受控 reapply：先用旧 secret/pid 优雅停旧引擎。旧 kill-switch 此刻仍生效 → 交换窗口
        // fail-closed 不破。停旧失败则中止（旧实例 + 旧 kill-switch 保留，仍受保护），不动配置/不起新引擎。
        if was_applied {
            engine::graceful_stop(old_pid, &old_secret).context(
                "受控 reapply：优雅停旧 mihomo 失败（保留原 kill-switch 与旧实例，未改配置）",
            )?;
            // 旧引擎已停；kill-switch 仍在（applied 维持 true）。清掉失效的旧 pid/secret。
            let mut rt = np.rt.lock().unwrap();
            rt.mihomo_pid = None;
            rt.secret = None;
        }

        // 安全回滚（P0-2，与 emergency_stop 同序）：有新 pid 时**先优雅停引擎**，确认停下后**才**
        // 撤防火墙；停不掉则**保留防火墙**维持 fail-closed，并把 rt 记成"仍受保护"以便后续 emergency_stop。
        let rollback = |new_pid: Option<u32>| -> Result<()> {
            if let Some(p) = new_pid {
                if let Err(e) = engine::graceful_stop(Some(p), &secret) {
                    let mut rt = np.rt.lock().unwrap();
                    rt.applied = true;
                    rt.mihomo_pid = Some(p);
                    rt.secret = Some(secret.clone());
                    return Err(e)
                        .context("回滚：停 mihomo 失败，kill-switch 保持生效以维持 fail-closed");
                }
            }
            if killswitch {
                let _ = firewall::remove(&ws);
            }
            let mut rt = np.rt.lock().unwrap();
            rt.applied = false;
            rt.mihomo_pid = None;
            rt.secret = None;
            Ok(())
        };

        // 步 1（阶段 A，P1-2）：先建 fail-closed（不依赖 Meta 的白名单 + 默认 Block），再起 mihomo。
        emit_progress(&app, 1, "running", None);
        if killswitch {
            if let Err(e) = firewall::apply_base(&ws, &settings, &mihomo_bin) {
                rollback(None)?; // 无新引擎在跑，仅撤可能残留的 kill-switch 回基线。
                emit_progress(&app, 1, "fail", Some(err(&e)));
                return Err(e.context("apply kill-switch（阶段A）"));
            }
        }
        if let Err(e) = engine::write_config(&ws, &settings, &rules, &secret) {
            rollback(None)?;
            emit_progress(&app, 1, "fail", Some(err(&e)));
            return Err(e.context("write mihomo config"));
        }
        emit_progress(
            &app,
            1,
            "ok",
            Some(if killswitch {
                "kill-switch 基线已就位".into()
            } else {
                "不受保护预览（未装 kill-switch）".into()
            }),
        );

        // 步 2：启动 mihomo 引擎。
        emit_progress(&app, 2, "running", None);
        let pid = match engine::start(&ws) {
            Ok(p) => p,
            Err(e) => {
                rollback(None)?;
                emit_progress(&app, 2, "fail", Some(err(&e)));
                return Err(e.context("start mihomo"));
            }
        };
        // 新引擎已起：立刻把 pid/secret 落进 rt，确保即便后续失败，回滚/急停也能按 pid 安全停。
        {
            let mut rt = np.rt.lock().unwrap();
            rt.mihomo_pid = Some(pid);
            rt.secret = Some(secret.clone());
        }
        emit_progress(&app, 2, "ok", Some(format!("pid {pid}")));

        // 步 3：等待 TUN(Meta) 起栈（真·轮询，复用 graceful_stop 的 14×500ms 范式；诚实分步，
        // 替代旧的固定 sleep(6s)+单次控制器探测）。控制器可达且 Meta Up 才算就绪。
        emit_progress(&app, 3, "running", Some("0/14".into()));
        let mut tun_ready = false;
        for i in 0..14 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if engine::running(&secret) && engine::tun_up() {
                tun_ready = true;
                emit_progress(&app, 3, "running", Some(format!("{}/14 已起栈", i + 1)));
                break;
            }
            emit_progress(&app, 3, "running", Some(format!("{}/14", i + 1)));
        }
        if !tun_ready {
            // 控制器可达但 TUN 没起栈，或控制器都不可达——区分提示。
            let detail = if engine::running(&secret) {
                "控制器可达但 TUN(Meta) 未在超时内起栈"
            } else {
                "mihomo 外部控制器不可达"
            };
            rollback(Some(pid))?;
            emit_progress(&app, 3, "fail", Some(detail.into()));
            bail!("等待 TUN 起栈超时（{detail}），已回滚（检查 WG 配置 / 管理员权限）");
        }
        emit_progress(&app, 3, "ok", None);

        // 步 4（阶段 B）：Meta 已出现，补 KS-TUN 放行应用流量进隧道。
        emit_progress(&app, 4, "running", None);
        if killswitch {
            if let Err(e) = firewall::apply_tun(&ws) {
                rollback(Some(pid))?;
                emit_progress(&app, 4, "fail", Some(err(&e)));
                return Err(e.context("apply kill-switch（阶段B KS-TUN）"));
            }
        }
        emit_progress(&app, 4, "ok", None);

        // 步 5：验证连通（控制器可达 + TUN 起栈，已在步 3 确认；此处终确认并标终态）。
        emit_progress(&app, 5, "running", None);
        {
            let mut rt = np.rt.lock().unwrap();
            rt.applied = true;
            rt.mihomo_pid = Some(pid);
            rt.secret = Some(secret.clone());
        }
        emit_progress(&app, 5, "ok", Some("引擎在线 · TUN 已起栈".into()));
        Ok(())
    })
    .await
    .map_err(err)?
    .map_err(err)?;

    net_policy_get_status(state)
}

/// 紧急停止 / 撤销。**顺序（P1-2）**：先在 kill-switch 仍生效时优雅停引擎（API 关 TUN
/// 清理路由，§0.8.2bis），确认引擎停下后才撤防火墙——避免"防火墙先撤、引擎还在/卡住"的
/// 泄漏窗口。若引擎停不下来，防火墙保持生效（继续 fail-closed），返回错误。
#[tauri::command]
pub async fn net_policy_emergency_stop(
    state: State<'_, AppState>,
) -> Result<NetPolicyStatus, String> {
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    let np = state.net_policy.clone();
    let ws = np.workspace.clone();
    let (pid, secret) = {
        let rt = np.rt.lock().unwrap();
        (rt.mihomo_pid, rt.secret.clone().unwrap_or_default())
    };
    tokio::task::spawn_blocking(move || -> Result<()> {
        // 1) 先优雅停引擎（关 TUN + 确认 Meta 拆除后才杀，按 pid，P1-1/P2-2）；
        //    失败则不撤防火墙，保持 fail-closed。
        engine::graceful_stop(pid, &secret)
            .context("优雅停 mihomo 失败（防火墙保持生效以维持 fail-closed）")?;
        // 2) 引擎已停、路由已清，再撤防火墙恢复联网。
        firewall::remove(&ws).context("移除 kill-switch")?;
        Ok(())
    })
    .await
    .map_err(err)?
    .map_err(err)?;

    {
        let mut rt = np.rt.lock().unwrap();
        rt.applied = false;
        rt.mihomo_pid = None;
        rt.secret = None;
    }
    net_policy_get_status(state)
}

// ============ 验证 ============

#[tauri::command]
pub async fn net_policy_verify(state: State<'_, AppState>) -> Result<verify::VerifyReport, String> {
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    let secret = state
        .net_policy
        .rt
        .lock()
        .unwrap()
        .secret
        .clone()
        .unwrap_or_default();
    tokio::task::spawn_blocking(move || verify::run(&secret))
        .await
        .map_err(err)?
        .map_err(err)
}
