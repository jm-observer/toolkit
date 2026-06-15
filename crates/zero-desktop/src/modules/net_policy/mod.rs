//! net-policy 模块：未知流量默认走 WireGuard 海外出口，仅显式配置的程序/域名/IP
//! 走本地直连；防火墙 kill-switch 提供 fail-closed 兜底。
//!
//! 设计见 docs/unified-desktop-shell-design.md §14；落地依据的真机验证见
//! docs/net-policy-validation-report.md（§0.x 实测结论）。**仅 Windows**。
//!
//! 所有 Tauri command 名称以 `net_policy_` 开头。

mod config;
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
pub fn setup(_app: &tauri::AppHandle, state: Arc<NetPolicyState>) -> Result<()> {
    let dir = config::net_policy_dir(&state.workspace);
    std::fs::create_dir_all(dir.join("generated")).ok();
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
    /// 是否处于"受保护"状态：kill-switch 启用且防火墙默认出站已 Block 且 mihomo 在跑。
    /// 若 false 且 applied=true，则为**不受保护预览**模式（P0-1，前端须明确标注）。
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
    let protected = settings.killswitch_enabled
        && firewall.as_ref().map(|f| f.active).unwrap_or(false)
        && mihomo_running;
    Ok(NetPolicyStatus {
        platform_supported: win::is_windows(),
        wg_configured: engine::validate(&settings).is_ok(),
        killswitch_enabled: settings.killswitch_enabled,
        applied,
        mihomo_running,
        protected,
        protection_validated: FIREWALL_MODEL_VALIDATED,
        firewall,
    })
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

// ============ 应用 / 急停 ============

/// 应用策略：校验 → 生成配置 → 启动 mihomo → 应用 kill-switch（默认开启）。
/// **事务化（P0-2）**：任一步失败回滚已起的 mihomo + 已改的防火墙，不留半应用状态。
/// kill-switch 关闭时为"不受保护预览"模式，status.protected=false（P0-1）。
#[tauri::command]
pub async fn net_policy_apply(state: State<'_, AppState>) -> Result<NetPolicyStatus, String> {
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    let np = state.net_policy.clone();
    let ws = np.workspace.clone();
    let settings = config::load_settings(&ws);
    let rules = config::load_rules(&ws);

    // 进副作用前先校验全部输入（P1-3）。
    settings.validate().map_err(err)?;
    rules.validate().map_err(err)?;
    let killswitch = settings.killswitch_enabled;
    let mihomo_bin = engine::mihomo_bin(&ws);
    let secret = engine::gen_secret(); // P0-1：每次 apply 随机 controller secret
    let secret_for_run = secret.clone();

    let pid = tokio::task::spawn_blocking(move || -> Result<u32> {
        // 阶段 A（P1-2）：先建 fail-closed（不依赖 Meta 的白名单 + 默认 Block），再起 mihomo。
        if killswitch {
            firewall::apply_base(&ws, &settings, &mihomo_bin)
                .context("apply kill-switch（阶段A）")?;
        }
        // 启动 mihomo（其物理出站已被 KS-mihomo 放行；TUN 起栈）。
        let rollback = |pid: Option<u32>| {
            if killswitch {
                let _ = firewall::remove(&ws);
            }
            if let Some(p) = pid {
                let _ = engine::graceful_stop(Some(p), &secret);
            }
        };
        if let Err(e) = engine::write_config(&ws, &settings, &rules, &secret) {
            rollback(None);
            return Err(e.context("write mihomo config"));
        }
        let pid = match engine::start(&ws) {
            Ok(p) => p,
            Err(e) => {
                rollback(None);
                return Err(e.context("start mihomo"));
            }
        };
        std::thread::sleep(std::time::Duration::from_secs(6));
        if !engine::running(&secret) {
            rollback(Some(pid));
            bail!("mihomo 启动后外部控制器不可达，已回滚（检查 WG 配置 / 管理员权限）");
        }
        // 阶段 B：Meta 已出现，补 KS-TUN 放行应用流量进隧道。
        if killswitch {
            if let Err(e) = firewall::apply_tun(&ws) {
                rollback(Some(pid));
                return Err(e.context("apply kill-switch（阶段B KS-TUN）"));
            }
        }
        Ok(pid)
    })
    .await
    .map_err(err)?
    .map_err(err)?;

    {
        let mut rt = np.rt.lock().unwrap();
        rt.applied = true;
        rt.mihomo_pid = Some(pid);
        rt.secret = Some(secret_for_run);
    }
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
