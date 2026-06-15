//! 输入校验：所有进 PowerShell / mihomo YAML 的值必须先过这里。
//!
//! 防注入（审查 P1-3）：IP/CIDR/域名/进程名严格白名单字符；拒绝换行、逗号、引号、
//! 反引号、`$`、`;`、`|` 等会改写 PS 命令或 YAML 规则行的字符。

use anyhow::{bail, Result};
use std::net::{Ipv4Addr, Ipv6Addr};

fn no_meta(s: &str) -> Result<()> {
    if s.is_empty() {
        bail!("空值");
    }
    if s.chars()
        .any(|c| c.is_control() || "\n\r,;|&`$'\"<>(){}".contains(c))
    {
        bail!("含非法字符：{s:?}");
    }
    Ok(())
}

/// 校验 IPv4 或 IPv6 地址。
pub fn ip(s: &str) -> Result<()> {
    no_meta(s)?;
    if s.parse::<Ipv4Addr>().is_ok() || s.parse::<Ipv6Addr>().is_ok() {
        Ok(())
    } else {
        bail!("非法 IP：{s}")
    }
}

/// 校验 CIDR（`ip/prefix`）。
pub fn cidr(s: &str) -> Result<()> {
    no_meta(s)?;
    let (addr, prefix) = s
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("非法 CIDR（缺 /）：{s}"))?;
    let p: u32 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("非法前缀：{s}"))?;
    if addr.parse::<Ipv4Addr>().is_ok() {
        if p > 32 {
            bail!("IPv4 前缀越界：{s}");
        }
    } else if addr.parse::<Ipv6Addr>().is_ok() {
        if p > 128 {
            bail!("IPv6 前缀越界：{s}");
        }
    } else {
        bail!("非法 CIDR 地址：{s}");
    }
    Ok(())
}

/// IP 或 CIDR 都接受。
pub fn ip_or_cidr(s: &str) -> Result<()> {
    if s.contains('/') {
        cidr(s)
    } else {
        ip(s)
    }
}

/// 校验域名（DOMAIN-SUFFIX 用）。
pub fn domain(s: &str) -> Result<()> {
    no_meta(s)?;
    if s.len() > 253 {
        bail!("域名过长：{s}");
    }
    let ok = s.split('.').all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    if !ok {
        bail!("非法域名：{s}");
    }
    Ok(())
}

/// 校验进程名（如 `steam.exe`）。
pub fn process_name(s: &str) -> Result<()> {
    no_meta(s)?;
    if s.contains('/') || s.contains('\\') {
        bail!("进程名不应含路径分隔符：{s}");
    }
    if s.len() > 260 {
        bail!("进程名过长");
    }
    Ok(())
}

/// 校验进程完整路径（Windows 路径，允许空格/反斜杠/盘符，但拒绝注入元字符与换行）。
pub fn process_path(s: &str) -> Result<()> {
    no_meta(s)?; // no_meta 已拒绝换行/逗号/引号等
    if s.len() > 260 {
        bail!("路径过长");
    }
    Ok(())
}

/// base64 密钥粗校验（WireGuard key 是 44 字符 base64）。
pub fn wg_key(s: &str) -> Result<()> {
    no_meta(s)?;
    if s.len() < 40
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
    {
        bail!("非法 WireGuard 密钥");
    }
    Ok(())
}
