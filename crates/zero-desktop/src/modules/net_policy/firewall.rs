//! Windows 防火墙 kill-switch（§0.4/§0.9 验证过的"默认 Block + 白名单"模型）。
//!
//! 模型：`Set-NetFirewallProfile -DefaultOutboundAction Block` + 纯 Allow 白名单
//! （v1 的「Allow + Block 兜底」在 New-NetFirewallRule 层不工作，§0.2②实测 Block 永远赢 Allow）。
//!
//! 白名单（v2.2 重设计，修复审查 P1-1：原 RemoteAddress 白名单使域名/程序 DIRECT 在
//! kill-switch 下被拦死）：
//! - **R-mihomo（Program=mihomo.exe）**：放行 mihomo 进程出物理网卡的全部流量
//!   —— 覆盖 WG 握手、上游 DNS（§0.8.1）、**以及 DIRECT 命中后 mihomo 替程序/域名拨号**。
//!   fail-closed 不破：mihomo 崩溃 → 进程没了 → 此规则不匹配任何流量 → 物理全 Block。
//! - R-TUN(InterfaceAlias=Meta)：应用流量进 TUN。
//! - R-LO(127.0.0.0/8) / R-LAN(物理 NIC 的 LAN 段，非 mihomo 的本机互访)。
//! - R-IPv6Block（block_ipv6 时）：显式 Block 2000::/3，真正阻断 IPv6 公网（修 P2-1）。
//!
//! 移除（-Remove）按状态文件还原原 DefaultOutboundAction（实测原值 NotConfigured，不可盲设 Allow）。

use super::config::{killswitch_state_path, NetPolicySettings, RuleSet};
use super::valid;
use super::win::run_ps;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

const GROUP: &str = "NetPolicy-KillSwitch";
/// mihomo TUN 适配器名（gvisor wintun，固定 "Meta"）。
const TUN_ALIAS: &str = "Meta";

#[derive(Debug, Serialize)]
pub struct FirewallStatus {
    pub default_outbound: String,
    pub rule_count: u32,
    pub active: bool,
}

fn ps_squote(s: &str) -> String {
    s.replace('\'', "''")
}

/// 应用 kill-switch：快照原值 → 建白名单 → 设默认 Block。
pub fn apply(
    workspace: &Path,
    settings: &NetPolicySettings,
    rules: &RuleSet,
    mihomo_bin: &Path,
) -> Result<()> {
    run_ps(&build_apply_script(workspace, settings, rules, mihomo_bin)?)?;
    Ok(())
}

/// 构造 apply 用的 PowerShell 脚本（纯函数 + 输入校验，便于 CLI 预览/测试，不执行）。
pub fn build_apply_script(
    workspace: &Path,
    settings: &NetPolicySettings,
    _rules: &RuleSet,
    mihomo_bin: &Path,
) -> Result<String> {
    // 防注入：所有进 PS 的值再校验一遍（apply 前 settings.validate 已校验，这里防御性重复）。
    valid::ip(&settings.wg.server)?;
    for l in &settings.lan_ranges {
        valid::ip_or_cidr(l)?;
    }

    let state = killswitch_state_path(workspace);
    let state_s = ps_squote(&state.to_string_lossy());
    let state_dir = state
        .parent()
        .map(|p| ps_squote(&p.to_string_lossy()))
        .unwrap_or_default();
    let lan = settings.lan_ranges.join(",");
    let mihomo = ps_squote(&mihomo_bin.to_string_lossy());

    let mut rules_ps = String::new();
    // R-mihomo：放行 mihomo 进程出物理网卡（覆盖 WG 握手 / 上游 DNS / DIRECT 拨号）。
    rules_ps.push_str(&format!(
        "New-NetFirewallRule -Group $G -Name 'KS-mihomo' -DisplayName 'KS mihomo egress' -Direction Outbound -Action Allow -Program '{mihomo}' -InterfaceAlias $eth -Enabled True | Out-Null\n"
    ));
    rules_ps.push_str(&format!(
        "New-NetFirewallRule -Group $G -Name 'KS-TUN' -DisplayName 'KS TUN' -Direction Outbound -Action Allow -InterfaceAlias '{TUN_ALIAS}' -Enabled True | Out-Null\n"
    ));
    rules_ps.push_str(
        "New-NetFirewallRule -Group $G -Name 'KS-LO' -DisplayName 'KS Loopback v4' -Direction Outbound -Action Allow -RemoteAddress 127.0.0.0/8 -Enabled True | Out-Null\n",
    );
    rules_ps.push_str(&format!(
        "New-NetFirewallRule -Group $G -Name 'KS-LAN' -DisplayName 'KS LAN' -Direction Outbound -Action Allow -RemoteAddress {lan} -InterfaceAlias $eth -Enabled True | Out-Null\n"
    ));
    if settings.block_ipv6 {
        // 显式阻断 IPv6 公网（真正落实"阻断 IPv6"，不止 mihomo ipv6:false）。
        rules_ps.push_str(
            "New-NetFirewallRule -Group $G -Name 'KS-IPv6Block' -DisplayName 'KS block IPv6 public' -Direction Outbound -Action Block -RemoteAddress 2000::/3 -Enabled True | Out-Null\n",
        );
    }

    let script = format!(
        r#"$G='{GROUP}'
$state='{state_s}'
if(-not (Test-Path $state)){{
  $snap=[ordered]@{{}}
  foreach($p in 'Domain','Private','Public'){{ $snap[$p]=(Get-NetFirewallProfile -Profile $p).DefaultOutboundAction.ToString() }}
  New-Item -ItemType Directory -Path '{state_dir}' -Force | Out-Null
  $snap | ConvertTo-Json | Set-Content -Path $state -Encoding UTF8
}}
$eth=@(Get-NetAdapter -Physical | Where-Object {{ $_.Status -eq 'Up' }} | Select-Object -ExpandProperty Name)
if($eth.Count -eq 0){{ throw '没有处于 Up 的物理网卡' }}
Get-NetFirewallRule -Group $G -ErrorAction SilentlyContinue | Remove-NetFirewallRule
{rules_ps}Set-NetFirewallProfile -Profile Domain,Private,Public -DefaultOutboundAction Block
'OK'
"#
    );
    Ok(script)
}

/// 移除 kill-switch：删白名单 + 按状态文件还原 DefaultOutboundAction。
pub fn remove(workspace: &Path) -> Result<()> {
    let state = killswitch_state_path(workspace);
    let state_s = ps_squote(&state.to_string_lossy());
    let script = format!(
        r#"$G='{GROUP}'
$state='{state_s}'
Get-NetFirewallRule -Group $G -ErrorAction SilentlyContinue | Remove-NetFirewallRule
if(Test-Path $state){{
  $s=Get-Content $state -Raw | ConvertFrom-Json
  foreach($p in 'Domain','Private','Public'){{ if($s.$p){{ Set-NetFirewallProfile -Profile $p -DefaultOutboundAction $s.$p }} }}
  Remove-Item $state -Force
}}else{{
  Set-NetFirewallProfile -Profile Domain,Private,Public -DefaultOutboundAction NotConfigured
}}
'OK'
"#
    );
    run_ps(&script)?;
    Ok(())
}

/// 查询 kill-switch 当前状态。
pub fn status() -> Result<FirewallStatus> {
    let out = run_ps(&format!(
        r#"$o=(Get-NetFirewallProfile -Profile Domain).DefaultOutboundAction
$c=(Get-NetFirewallRule -Group '{GROUP}' -ErrorAction SilentlyContinue | Measure-Object).Count
"$o|$c"
"#
    ))?;
    let line = out.trim();
    let mut parts = line.split('|');
    let default_outbound = parts.next().unwrap_or("Unknown").trim().to_string();
    let rule_count: u32 = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    Ok(FirewallStatus {
        active: default_outbound.eq_ignore_ascii_case("Block"),
        default_outbound,
        rule_count,
    })
}
