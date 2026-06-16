//! Windows PowerShell 调用助手。
//!
//! 通过 stdin 把脚本喂给 `powershell -Command -`，避开 SSH→cmd→PS 多层转义坑
//! （验证阶段的血泪教训，见 docs/net-policy-validation-report.md §0.2.1）。
//! 脚本顶部强制 UTF-8 输出编码，stdout 以 UTF-8 读回。

use anyhow::{bail, Context, Result};

/// 在 Windows 上执行一段 PowerShell 脚本，返回 stdout（UTF-8）。
/// 非 Windows 平台返回错误（net-policy 仅承诺 Windows，见设计 §14.0）。
#[cfg(windows)]
pub fn run_ps(script: &str) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let wrapped = format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8\n$ErrorActionPreference='Stop'\n{script}"
    );

    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        "-",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    crate::shared::proc::hide_console(&mut cmd); // 不弹控制台窗口
    let mut child = cmd.spawn().context("spawn powershell")?;

    child
        .stdin
        .take()
        .context("powershell stdin")?
        .write_all(wrapped.as_bytes())
        .context("write powershell script")?;

    let out = child.wait_with_output().context("wait powershell")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("powershell failed ({}): {}", out.status, stderr.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(windows))]
pub fn run_ps(_script: &str) -> Result<String> {
    bail!("net-policy 仅支持 Windows（当前非 Windows 平台）")
}

/// 当前是否为 Windows（命令层用来给出明确错误而非静默失败）。
pub fn is_windows() -> bool {
    cfg!(windows)
}
