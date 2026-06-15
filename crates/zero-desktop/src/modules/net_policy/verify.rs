//! 出口与泄漏验证（§14.9 的 UI 化最小版）。
//!
//! 一键执行：当前公网出口 IP、DNS 是否被 hijack（fake-ip）、是否能解析。
//! 注：完整 fail-closed / VP-08~12 的取证须本地控制台 + 抓包（见报告 §0.8.3），
//! 此处只做 UI 侧"看一眼当前状态"的轻量验证。

use super::engine::CONTROLLER;
use super::win::run_ps;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct VerifyCase {
    pub id: String,
    pub name: String,
    /// passed / failed / unknown
    pub status: String,
    pub observed: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub mihomo_running: bool,
    pub cases: Vec<VerifyCase>,
}

fn case(id: &str, name: &str, status: &str, observed: String) -> VerifyCase {
    VerifyCase {
        id: id.into(),
        name: name.into(),
        status: status.into(),
        observed,
    }
}

/// 执行轻量验证用例。`secret` 用于鉴权查询 mihomo 外部控制器。
pub fn run(secret: &str) -> Result<VerifyReport> {
    let mihomo_running = super::engine::running(secret);
    let mut cases = Vec::new();

    // VP-01 轻量版：当前公网出口 IP（期望为 WG 海外出口）。
    match run_ps("try{ (Invoke-RestMethod https://api.ipify.org -TimeoutSec 10).Trim() }catch{ 'FAIL:'+$_.Exception.Message }")
    {
        Ok(ip) => {
            let ip = ip.trim().to_string();
            let status = if ip.starts_with("FAIL") || ip.is_empty() {
                "failed"
            } else {
                "passed"
            };
            cases.push(case("exit-ip", "当前公网出口 IP", status, ip));
        }
        Err(e) => cases.push(case("exit-ip", "当前公网出口 IP", "failed", format!("{e:#}"))),
    }

    // dns-hijack 轻量版：显式查公网 DNS，期望返回 fake-ip 198.18.x（说明被 TUN 劫持）。
    match run_ps(
        "try{ ((Resolve-DnsName example.com -Type A -Server 8.8.8.8 -DnsOnly -EA Stop | Where-Object Type -eq A).IPAddress -join ',') }catch{ 'FAIL:'+$_.Exception.Message }",
    ) {
        Ok(ans) => {
            let ans = ans.trim().to_string();
            let status = if ans.contains("198.18.") {
                "passed"
            } else if ans.starts_with("FAIL") {
                "failed"
            } else {
                "unknown"
            };
            cases.push(case("dns-hijack", "DNS 劫持(fake-ip)", status, ans));
        }
        Err(e) => cases.push(case("dns-hijack", "DNS 劫持(fake-ip)", "failed", format!("{e:#}"))),
    }

    // 引擎在线（外部控制器可达）。
    cases.push(case(
        "engine",
        "mihomo 控制器",
        if mihomo_running { "passed" } else { "failed" },
        format!("http://{CONTROLLER}/version"),
    ));

    Ok(VerifyReport {
        mihomo_running,
        cases,
    })
}
