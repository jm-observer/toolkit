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
}

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
    let cfg = engine::generate_config(&settings, &rules);
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
    /// 是否处于"受保护"状态：kill-switch 启用且防火墙默认出站已 Block。
    /// 若 false 且 applied=true，则为**不受保护预览**模式（P0-1，前端须明确标注）。
    pub protected: bool,
    pub firewall: Option<firewall::FirewallStatus>,
}

#[tauri::command]
pub fn net_policy_get_status(state: State<'_, AppState>) -> Result<NetPolicyStatus, String> {
    let np = &state.net_policy;
    let settings = config::load_settings(&np.workspace);
    let (applied, _) = {
        let rt = np.rt.lock().unwrap();
        (rt.applied, rt.mihomo_pid)
    };
    let firewall = if win::is_windows() {
        firewall::status().ok()
    } else {
        None
    };
    let mihomo_running = win::is_windows() && engine::running();
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

    let pid = tokio::task::spawn_blocking(move || -> Result<u32> {
        engine::write_config(&ws, &settings, &rules).context("write mihomo config")?;
        let pid = engine::start(&ws).context("start mihomo")?;
        std::thread::sleep(std::time::Duration::from_secs(6));
        // 验证 mihomo 就绪；没起来 → 回滚。
        if !engine::running() {
            let _ = engine::graceful_stop(Some(pid));
            bail!("mihomo 启动后外部控制器不可达，已回滚（检查 WG 配置 / 管理员权限）");
        }
        if killswitch {
            if let Err(e) = firewall::apply(&ws, &settings, &rules, &mihomo_bin) {
                // 防火墙失败 → 回滚：撤防火墙快照 + 优雅停 mihomo，避免半应用 + 无保护。
                let _ = firewall::remove(&ws);
                let _ = engine::graceful_stop(Some(pid));
                return Err(e.context("应用 kill-switch 失败，已回滚 mihomo + 防火墙"));
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
    let pid = np.rt.lock().unwrap().mihomo_pid;
    tokio::task::spawn_blocking(move || -> Result<()> {
        // 1) 先优雅停引擎（按 pid，P2-2）；失败则不撤防火墙，保持 fail-closed。
        engine::graceful_stop(pid)
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
    }
    net_policy_get_status(state)
}

// ============ 验证 ============

#[tauri::command]
pub async fn net_policy_verify(state: State<'_, AppState>) -> Result<verify::VerifyReport, String> {
    let _ = &state; // 验证不依赖具体 state，但保持签名一致便于前端调用。
    if !win::is_windows() {
        return Err("net-policy 仅支持 Windows".into());
    }
    tokio::task::spawn_blocking(verify::run)
        .await
        .map_err(err)?
        .map_err(err)
}
