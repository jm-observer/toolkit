//! mihomo 引擎编排：从规则生成 mihomo 配置 + 进程生命周期。
//!
//! 落地的验证结论（docs/net-policy-validation-report.md）：
//! - TUN(gvisor) + WG userspace outbound，§9#1 实测无路由环，无需手动 route-exclude host route。
//! - `strict-route: true` + `dns-hijack: any:53`（§0.6 拦系统 DNS，含显式 8.8.8.8）。
//! - DNS 模式 A：上游 nameserver 走物理 bootstrap（§0.8.1），kill-switch 放行其 IP；
//!   零泄漏的 `respect-rules`（模式 B）未实测，默认不启用。
//! - 停 mihomo 必须优雅：先 API 关 TUN 再结束进程（§0.8.2bis，避免 wintun 路由残留断网）。

use super::config::{mihomo_config_path, net_policy_dir, NetPolicySettings, RuleSet};
use super::win::run_ps;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// mihomo 外部控制器端口（loopback）。
pub const CONTROLLER: &str = "127.0.0.1:9090";

/// 解析 mihomo 可执行文件路径：优先 `MIHOMO_BIN`，否则 workspace 内置目录。
pub fn mihomo_bin(workspace: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("MIHOMO_BIN") {
        return PathBuf::from(p);
    }
    net_policy_dir(workspace)
        .join("mihomo")
        .join("mihomo-windows-amd64.exe")
}

/// 生成随机 controller secret（hex，防同用户其他进程打 mihomo API 改规则，P0-1）。
pub fn gen_secret() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// 构造带 Authorization 头的 PS hashtable 片段（secret 空则无头）。
fn auth_header(secret: &str) -> String {
    if secret.is_empty() {
        "@{}".to_string()
    } else {
        format!("@{{ Authorization = 'Bearer {secret}' }}")
    }
}

/// 从规则集 + 设置生成 mihomo `config.yaml` 文本。`secret` 是 external-controller 鉴权口令。
pub fn generate_config(settings: &NetPolicySettings, rules: &RuleSet, secret: &str) -> String {
    let wg = &settings.wg;
    let dns_bootstrap = settings
        .dns_bootstrap
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let lan_exclude = settings
        .lan_ranges
        .iter()
        .map(|s| format!("    - {s}"))
        .collect::<Vec<_>>()
        .join("\n");
    let rule_lines = rules.to_mihomo_rules().join("\n");
    let ipv6 = !settings.block_ipv6; // block_ipv6=true → mihomo ipv6:false

    format!(
        r#"# 由 zero-desktop net_policy 模块生成，请勿手改（改规则用 UI / net_policy_apply）。
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
ipv6: {ipv6}
find-process-mode: always
external-controller: {CONTROLLER}
secret: "{secret}"

dns:
  enable: true
  listen: 127.0.0.1:1053
  ipv6: {ipv6}
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  fake-ip-filter:
    - '*.lan'
    - '*.local'
    - '+.msftconnecttest.com'
    - '+.msftncsi.com'
  # DNS 模式 A：上游 bootstrap 走物理（§0.8.1，kill-switch 放行其 IP）。
  default-nameserver: [{dns_bootstrap}]
  nameserver: [{dns_bootstrap}]
  direct-nameserver: [{dns_bootstrap}]
  fallback: []

tun:
  enable: true
  stack: gvisor
  dns-hijack:
    - any:53
    - tcp://any:53
  auto-route: true
  auto-detect-interface: true
  strict-route: true
  route-exclude-address:
{lan_exclude}

proxies:
  - name: wg-out
    type: wireguard
    server: {server}
    port: {port}
    ip: {ip}
    private-key: {priv}
    public-key: {pubk}
    pre-shared-key: {psk}
    udp: true
    mtu: {mtu}
    remote-dns-resolve: false

proxy-groups: []

rules:
{rule_lines}
"#,
        server = wg.server,
        port = wg.port,
        ip = wg.ip,
        priv = wg.private_key,
        pubk = wg.public_key,
        psk = wg.pre_shared_key,
        mtu = wg.mtu,
    )
}

/// 写出生成的 mihomo 配置文件，返回其路径。
pub fn write_config(
    workspace: &Path,
    settings: &NetPolicySettings,
    rules: &RuleSet,
    secret: &str,
) -> Result<PathBuf> {
    let path = mihomo_config_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, generate_config(settings, rules, secret))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// 校验设置（严格 IP/CIDR/key 校验 + 防注入，委托 `NetPolicySettings::validate`）。
pub fn validate(settings: &NetPolicySettings) -> Result<()> {
    settings.validate()
}

/// 启动 mihomo（以管理员/当前进程上下文 spawn 子进程，配置须已写）。返回 pid。
pub fn start(workspace: &Path) -> Result<u32> {
    let bin = mihomo_bin(workspace);
    if !bin.exists() {
        bail!(
            "mihomo 可执行文件不存在：{}（设 MIHOMO_BIN 或放到 net-policy/mihomo/）",
            bin.display()
        );
    }
    let dir = net_policy_dir(workspace);
    let child = std::process::Command::new(&bin)
        .arg("-d")
        .arg(&dir)
        .spawn()
        .with_context(|| format!("spawn mihomo: {}", bin.display()))?;
    Ok(child.id())
}

/// 优雅停 mihomo（§0.8.2bis + 审查 P1-1）：先 API 关 TUN，**轮询确认 Meta 适配器已拆除**
/// 再按 pid 结束进程。若 TUN 未在超时内拆除则 **bail（不强杀）**，让调用方保持防火墙生效、
/// 不进入"强杀残留路由"的泄漏/断网路径。pid 未知时回退按本模块二进制名。
pub fn graceful_stop(pid: Option<u32>, secret: &str) -> Result<()> {
    let h = auth_header(secret);
    let kill = match pid {
        Some(p) => format!(
            "Stop-Process -Id {p} -Force -ErrorAction SilentlyContinue; \
             if(Get-Process -Id {p} -ErrorAction SilentlyContinue){{ throw 'mihomo pid {p} 未能结束' }}"
        ),
        None => "Get-Process mihomo-windows-amd64 -ErrorAction SilentlyContinue | Stop-Process -Force"
            .to_string(),
    };
    run_ps(&format!(
        r#"$h={h}
try{{ Invoke-RestMethod 'http://{CONTROLLER}/configs' -Method PATCH -Headers $h -Body '{{"tun":{{"enable":false}}}}' -TimeoutSec 4 | Out-Null }}catch{{}}
$gone=$false
for($i=0;$i -lt 14;$i++){{ if(-not (Get-NetAdapter -Name Meta -ErrorAction SilentlyContinue)){{ $gone=$true; break }}; Start-Sleep -Milliseconds 500 }}
if(-not $gone){{ throw 'TUN(Meta) 未在超时内优雅拆除——拒绝强杀以避免 wintun 路由残留；防火墙保持生效（请重试或本地排查）' }}
{kill}
'OK'
"#
    ))?;
    Ok(())
}

/// mihomo 是否在运行（查外部控制器，带鉴权）。
pub fn running(secret: &str) -> bool {
    let h = auth_header(secret);
    run_ps(&format!(
        "$h={h}; try{{ Invoke-RestMethod 'http://{CONTROLLER}/version' -Headers $h -TimeoutSec 3 | Out-Null; 'yes' }}catch{{ 'no' }}"
    ))
    .map(|s| s.trim() == "yes")
    .unwrap_or(false)
}
