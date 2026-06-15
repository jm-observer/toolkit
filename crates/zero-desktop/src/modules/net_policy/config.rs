//! net-policy 配置与规则模型 + 持久化。
//!
//! 落盘布局（见 docs/net-policy-validation-report.md §14.8）：
//! `{workspace}/net-policy/{settings.json, rules.json}`。
//! 安全约定：WireGuard 私钥不入 `rules.json`；存在 `settings.json` 的 `wg`
//! 段，后续可迁到 Windows Credential Manager（首版直存，已在文档标注）。

use super::valid;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 出口路由：默认走 WireGuard（海外），仅显式配置的走本地直连。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// 本地直连（绕过隧道）。
    Direct,
    /// 走 WireGuard 出口（默认）。
    #[default]
    Wg,
}

impl Route {
    /// 映射到 mihomo 规则的 outbound 名。
    pub fn outbound(self) -> &'static str {
        match self {
            Route::Direct => "DIRECT",
            Route::Wg => "wg-out",
        }
    }
}

/// 单条分流规则。`kind` 决定匹配维度，`value` 是匹配值，`route` 是命中后的出口。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Rule {
    /// 进程完整路径（`PROCESS-PATH`，反斜杠，大小写不敏感）。
    ProcessPath { value: String, route: Route },
    /// 进程名（`PROCESS-NAME`，仅 exe 文件名）。
    ProcessName { value: String, route: Route },
    /// 域名后缀（`DOMAIN-SUFFIX`）。
    DomainSuffix { value: String, route: Route },
    /// IP/CIDR（`IP-CIDR`）。
    IpCidr { value: String, route: Route },
}

impl Rule {
    /// 渲染为一行 mihomo rule。
    pub fn to_mihomo_line(&self) -> String {
        match self {
            Rule::ProcessPath { value, route } => {
                format!("  - PROCESS-PATH,{value},{}", route.outbound())
            }
            Rule::ProcessName { value, route } => {
                format!("  - PROCESS-NAME,{value},{}", route.outbound())
            }
            Rule::DomainSuffix { value, route } => {
                format!("  - DOMAIN-SUFFIX,{value},{}", route.outbound())
            }
            Rule::IpCidr { value, route } => {
                format!("  - IP-CIDR,{value},{},no-resolve", route.outbound())
            }
        }
    }

    /// 校验规则值（防注入 + 格式，P1-3）。
    pub fn validate(&self) -> Result<()> {
        match self {
            Rule::ProcessPath { value, .. } => valid::process_path(value),
            Rule::ProcessName { value, .. } => valid::process_name(value),
            Rule::DomainSuffix { value, .. } => valid::domain(value),
            Rule::IpCidr { value, .. } => valid::ip_or_cidr(value),
        }
    }
}

/// 程序组：用户选一个主程序，系统观察其子进程并允许确认加入同组（§14.4）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramGroup {
    pub id: String,
    pub name: String,
    pub root_paths: Vec<String>,
    #[serde(default)]
    pub known_children: Vec<ProcessRef>,
    #[serde(default)]
    pub route: Route,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProcessRef {
    ProcessPath(String),
    ProcessName(String),
}

/// WireGuard outbound（mihomo userspace WG）。`server` 必须是 IP（避免鸡生蛋解析）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WgConfig {
    pub server: String,
    pub port: u16,
    /// 隧道内本机地址（如 10.66.66.5）。
    pub ip: String,
    pub private_key: String,
    pub public_key: String,
    #[serde(default)]
    pub pre_shared_key: String,
    #[serde(default = "default_mtu")]
    pub mtu: u32,
}

fn default_mtu() -> u32 {
    1420
}

impl WgConfig {
    /// 校验 WG 配置（格式 + 防注入）。
    pub fn validate(&self) -> Result<()> {
        valid::ip(&self.server).context("WG server 必须是合法 IP（不能是域名）")?;
        if self.port == 0 {
            anyhow::bail!("WG 端口非法");
        }
        valid::ip_or_cidr(&self.ip).context("WG 隧道内地址非法")?;
        valid::wg_key(&self.private_key).context("WG 私钥非法")?;
        valid::wg_key(&self.public_key).context("WG 公钥非法")?;
        if !self.pre_shared_key.is_empty() {
            valid::wg_key(&self.pre_shared_key).context("WG 预共享密钥非法")?;
        }
        Ok(())
    }
}

/// net-policy 设置（与 rules 分文件存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetPolicySettings {
    pub wg: WgConfig,
    /// mihomo 上游 DNS bootstrap（必为 UDP IP）。§0.8.1 实测：走物理，kill-switch 必放行。
    #[serde(default = "default_dns_bootstrap")]
    pub dns_bootstrap: Vec<String>,
    /// 局域网保留段（防火墙白名单 + TUN route-exclude）。
    #[serde(default = "default_lan_ranges")]
    pub lan_ranges: Vec<String>,
    /// 是否启用防火墙 kill-switch（fail-closed）。**默认开启**——"未知流量必须海外 /
    /// fail-closed 不可妥协"是核心约束（P0-1）；关闭即"不受保护预览"模式。
    #[serde(default = "default_true")]
    pub killswitch_enabled: bool,
    /// 首版默认阻断 IPv6 公网（§0.8 / VP-12）。
    #[serde(default = "default_true")]
    pub block_ipv6: bool,
}

fn default_dns_bootstrap() -> Vec<String> {
    vec!["223.5.5.5".into(), "119.29.29.29".into()]
}

fn default_lan_ranges() -> Vec<String> {
    vec![
        "192.168.0.0/16".into(),
        "10.0.0.0/8".into(),
        "172.16.0.0/12".into(),
        "169.254.0.0/16".into(),
    ]
}

fn default_true() -> bool {
    true
}

impl Default for NetPolicySettings {
    fn default() -> Self {
        Self {
            wg: WgConfig::default(),
            dns_bootstrap: default_dns_bootstrap(),
            lan_ranges: default_lan_ranges(),
            killswitch_enabled: true,
            block_ipv6: true,
        }
    }
}

impl NetPolicySettings {
    /// 校验设置（WG + DNS bootstrap + LAN 段，防注入 P1-3）。
    pub fn validate(&self) -> Result<()> {
        self.wg.validate()?;
        if self.dns_bootstrap.is_empty() {
            anyhow::bail!("DNS bootstrap 不能为空（mihomo 上游解析需要）");
        }
        for d in &self.dns_bootstrap {
            valid::ip(d).context("DNS bootstrap 必须是 UDP IP")?;
        }
        for l in &self.lan_ranges {
            valid::ip_or_cidr(l).context("LAN 段非法")?;
        }
        Ok(())
    }
}

/// 规则集合（与 settings 分文件存）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleSet {
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub groups: Vec<ProgramGroup>,
}

impl RuleSet {
    /// 展开为 mihomo rules（程序组先展开成 PROCESS-* 规则，再追加普通规则，最后 MATCH,wg-out）。
    pub fn to_mihomo_rules(&self) -> Vec<String> {
        let mut lines = Vec::new();
        // 内网/保留地址直连——用显式 IP-CIDR 而非 GEOIP,private，避免依赖 geoip 数据库
        // （0.228 实测：fresh 机器若无 geoip.metadb，mihomo 会去 GitHub 下载，国内慢/失败）。
        for cidr in [
            "127.0.0.0/8",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "224.0.0.0/4",
        ] {
            lines.push(format!("  - IP-CIDR,{cidr},DIRECT,no-resolve"));
        }
        // 程序组展开
        for g in &self.groups {
            for p in &g.root_paths {
                lines.push(format!("  - PROCESS-PATH,{p},{}", g.route.outbound()));
            }
            for c in &g.known_children {
                match c {
                    ProcessRef::ProcessPath(v) => {
                        lines.push(format!("  - PROCESS-PATH,{v},{}", g.route.outbound()))
                    }
                    ProcessRef::ProcessName(v) => {
                        lines.push(format!("  - PROCESS-NAME,{v},{}", g.route.outbound()))
                    }
                }
            }
        }
        // 普通规则
        for r in &self.rules {
            lines.push(r.to_mihomo_line());
        }
        // fail-closed 核心：未知流量走 WG
        lines.push("  - MATCH,wg-out".to_string());
        lines
    }

    /// 校验全部规则（任一非法即整体拒绝，P1-3）。
    pub fn validate(&self) -> Result<()> {
        for (i, r) in self.rules.iter().enumerate() {
            r.validate().with_context(|| format!("规则 #{i} 非法"))?;
        }
        for g in &self.groups {
            for p in &g.root_paths {
                valid::process_path(p).with_context(|| format!("程序组 {} 路径非法", g.name))?;
            }
            for c in &g.known_children {
                match c {
                    ProcessRef::ProcessPath(v) => valid::process_path(v)?,
                    ProcessRef::ProcessName(v) => valid::process_name(v)?,
                }
            }
        }
        Ok(())
    }
}

/// net-policy workspace 子目录。
pub fn net_policy_dir(workspace: &Path) -> PathBuf {
    workspace.join("net-policy")
}

fn settings_path(workspace: &Path) -> PathBuf {
    net_policy_dir(workspace).join("settings.json")
}

fn rules_path(workspace: &Path) -> PathBuf {
    net_policy_dir(workspace).join("rules.json")
}

/// 生成的 mihomo 配置路径。
pub fn mihomo_config_path(workspace: &Path) -> PathBuf {
    net_policy_dir(workspace)
        .join("generated")
        .join("config.yaml")
}

/// kill-switch 状态快照路径（Remove 时按原值恢复 DefaultOutboundAction）。
pub fn killswitch_state_path(workspace: &Path) -> PathBuf {
    net_policy_dir(workspace).join("killswitch-state.json")
}

pub fn load_settings(workspace: &Path) -> NetPolicySettings {
    let p = settings_path(workspace);
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(workspace: &Path, s: &NetPolicySettings) -> Result<()> {
    let p = settings_path(workspace);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(s).context("serialize settings")?;
    std::fs::write(&p, json).with_context(|| format!("write {}", p.display()))
}

pub fn load_rules(workspace: &Path) -> RuleSet {
    let p = rules_path(workspace);
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_rules(workspace: &Path, r: &RuleSet) -> Result<()> {
    let p = rules_path(workspace);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(r).context("serialize rules")?;
    std::fs::write(&p, json).with_context(|| format!("write {}", p.display()))
}
