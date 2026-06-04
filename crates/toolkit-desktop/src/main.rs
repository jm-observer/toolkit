#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod bridge;
mod config;
mod db;
mod uploader;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use log::LevelFilter::Info;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use uploader::UploaderState;

const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "github-commit-info";
const APP: &str = "toolkit-desktop";

#[derive(Parser, Debug, Clone)]
#[command(name = "toolkit-desktop", version)]
struct Cli {
    /// workspace 根目录，覆盖 `~/.config/toolkit-desktop` 默认。
    #[arg(long, env = "TOOLKIT_DESKTOP_WORKSPACE", global = true)]
    workspace: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// 启动 GUI（默认）。可通过 `--server` / `--token` 覆盖配置文件后写回 workspace。
    Run {
        /// G10 server base URL（含 scheme，不含路径）。
        #[arg(long, env = "TOOLKIT_DESKTOP_SERVER")]
        server: Option<String>,
        #[arg(long, env = "TOOLKIT_DESKTOP_TOKEN")]
        token: Option<String>,
    },
    /// 从 GitHub Release 自更新当前 exe（跨平台）。
    Update {
        #[arg(short, long, help = "即使版本未升级也强制更新")]
        force: bool,
    },
}

#[derive(Clone)]
pub struct AppCtx {
    pub workspace: PathBuf,
    pub db: Arc<db::Db>,
    pub uploader: Arc<UploaderState>,
}

fn main() -> Result<()> {
    let _ = custom_utils::logger::logger_feature(APP, "info,reqwest=warn", Info, false).build();
    let cli = Cli::parse();
    let workspace = workspace::resolve(&cli.workspace)?;
    log::info!("workspace = {}", workspace.display());

    match cli.command.unwrap_or(Command::Run {
        server: None,
        token: None,
    }) {
        Command::Run { server, token } => run_gui(workspace, server, token),
        Command::Update { force } => run_update(force),
    }
}

fn run_update(force: bool) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio rt")?;
    let outcome = rt.block_on(async {
        UpdateConfig::new(REPO_OWNER, REPO_NAME, env!("CARGO_PKG_VERSION"))
            .bin_name(APP)
            .force(force)
            .execute()
            .await
    })?;
    log::info!("update outcome: {outcome:?}");
    Ok(())
}

fn run_gui(workspace: PathBuf, server: Option<String>, token: Option<String>) -> Result<()> {
    // CLI 覆盖立刻写盘，后续 uploader / UI 一致读 workspace/config.json。
    let cfg_path = workspace::config_path(&workspace);
    let mut s = config::load(&cfg_path);
    let mut changed = false;
    if let Some(srv) = server.as_deref() {
        s.server_base = srv.to_string();
        changed = true;
    }
    if let Some(tok) = token.as_deref() {
        s.auth_token = Some(tok.to_string());
        changed = true;
    }
    if changed {
        config::save(&cfg_path, &s).context("save settings")?;
    }

    let db = Arc::new(db::Db::open(&workspace::db_path(&workspace)).context("open state.db")?);
    let uploader = Arc::new(UploaderState::default());
    let ctx = AppCtx {
        workspace: workspace.clone(),
        db: db.clone(),
        uploader: uploader.clone(),
    };

    tauri::Builder::default()
        .manage(ctx.clone())
        .invoke_handler(tauri::generate_handler![
            cmd_get_settings,
            cmd_save_settings,
            cmd_open_login,
            cmd_close_login,
            cmd_force_upload_now,
            cmd_recent_uploads,
            cmd_workspace_path,
            cmd_ping_server,
            cmd_inspect_cookies,
            cmd_capture_ms_token,
        ])
        .setup(move |app| {
            uploader::spawn(app.handle().clone(), ctx.clone());
            bridge::spawn(ctx.uploader.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("run tauri");
    Ok(())
}

#[tauri::command]
fn cmd_get_settings(ctx: tauri::State<'_, AppCtx>) -> config::Settings {
    config::load(&workspace::config_path(&ctx.workspace))
}

#[tauri::command]
fn cmd_save_settings(
    ctx: tauri::State<'_, AppCtx>,
    settings: config::Settings,
) -> Result<(), String> {
    config::save(&workspace::config_path(&ctx.workspace), &settings).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn cmd_workspace_path(ctx: tauri::State<'_, AppCtx>) -> String {
    ctx.workspace.to_string_lossy().to_string()
}

#[tauri::command]
fn cmd_recent_uploads(
    ctx: tauri::State<'_, AppCtx>,
    limit: Option<i64>,
) -> Result<Vec<db::UploadRow>, String> {
    ctx.db
        .recent_uploads(limit.unwrap_or(20))
        .map_err(|e| format!("{e:#}"))
}

const LOGIN_HOOK_JS: &str = include_str!("../ui/login_hook.js");

/// 打开/前置 douyin 登录窗口。重复调用幂等。
/// 注入 login_hook.js：hook 出站请求 URL 抠 msToken（抖音 SDK 已不写 cookie）。
#[tauri::command]
async fn cmd_open_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.set_focus();
        return Ok(());
    }
    let target: url::Url = "https://www.douyin.com"
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    WebviewWindowBuilder::new(&app, "login", WebviewUrl::External(target))
        .title("抖音登录 - toolkit-desktop")
        .inner_size(1280.0, 860.0)
        .center()
        .initialization_script(LOGIN_HOOK_JS)
        .build()
        .map_err(|e| format!("create login window: {e}"))?;
    Ok(())
}

/// login_hook.js 抓到 msToken 后回传，缓存到 UploaderState；下一 tick uploader 拼进 cookie 上传。
#[tauri::command]
async fn cmd_capture_ms_token(
    ctx: tauri::State<'_, AppCtx>,
    value: String,
) -> Result<(), String> {
    let trimmed = value.trim().to_string();
    if trimmed.len() < 16 {
        return Ok(());
    }
    let mut slot = ctx.uploader.ms_token.lock().await;
    let changed = slot.as_deref() != Some(trimmed.as_str());
    if changed {
        log::info!("captured new msToken (len={})", trimmed.len());
        *slot = Some(trimmed);
        // 让 uploader 下一 tick 必传一次新版本。
        *ctx.uploader.last_hash.lock().await = None;
    }
    Ok(())
}

#[tauri::command]
async fn cmd_close_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    Ok(())
}

/// 强制立刻把当前 cookies 上传一次（清掉 dedup hash，下次 tick 必然上传）。
/// 诊断：列出 login 窗口能看到的所有 douyin cookie（仅名字 + 长度，避免回显敏感值到 UI）。
#[tauri::command]
async fn cmd_inspect_cookies(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let Some(login) = app.get_webview_window("login") else {
        return Ok(serde_json::json!({
            "state": "no_login_window",
            "hint": "请先点「打开抖音登录」按钮，并保持窗口不关闭",
        }));
    };
    let url: url::Url = "https://www.douyin.com".parse().map_err(|e: url::ParseError| e.to_string())?;
    let cookies = login.cookies_for_url(url).map_err(|e| e.to_string())?;
    let summary: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name(),
                "len": c.value().len(),
                "domain": c.domain(),
                "path": c.path(),
                "http_only": c.http_only(),
                "secure": c.secure(),
            })
        })
        .collect();
    let names: Vec<&str> = cookies.iter().map(|c| c.name()).collect();
    let required = ["ttwid", "sessionid_ss"];
    let missing: Vec<&str> = required.iter().copied().filter(|k| !names.contains(k)).collect();
    let has_ms_token = names.contains(&"msToken");
    let session_variants: Vec<&str> = ["sessionid", "sessionid_ss", "sid_tt", "sid_guard", "passport_csrf_token"]
        .iter()
        .copied()
        .filter(|k| names.contains(k))
        .collect();
    Ok(serde_json::json!({
        "state": "ok",
        "count": cookies.len(),
        "missing_required": missing,
        "has_ms_token": has_ms_token,
        "ms_token_hint": if has_ms_token { serde_json::Value::Null } else {
            serde_json::Value::String("msToken 由抖音前端 JS 动态写入，浏览首页/视频后会出现；缺它不影响上传".into())
        },
        "session_variants_present": session_variants,
        "cookies": summary,
    }))
}

/// 探活 `<server>/api/web/health`，前端 pill 用。3s 超时；不抛错，返回结构化结果。
#[tauri::command]
async fn cmd_ping_server(ctx: tauri::State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    let settings = config::load(&workspace::config_path(&ctx.workspace));
    if !settings.is_configured() {
        return Ok(serde_json::json!({ "state": "unconfigured" }));
    }
    let base = settings.server_base.trim_end_matches('/');
    let url = format!("{base}/api/web/health");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let latency_ms = started.elapsed().as_millis() as u64;
            let body = resp.text().await.unwrap_or_default();
            let version = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(String::from));
            Ok(serde_json::json!({
                "state": if (200..300).contains(&status) { "ok" } else { "http_err" },
                "status": status,
                "latency_ms": latency_ms,
                "server_base": base,
                "server_version": version,
            }))
        }
        Err(e) => Ok(serde_json::json!({
            "state": "unreachable",
            "error": e.to_string(),
            "server_base": base,
        })),
    }
}

#[tauri::command]
async fn cmd_force_upload_now(
    app: tauri::AppHandle,
    ctx: tauri::State<'_, AppCtx>,
) -> Result<(), String> {
    *ctx.uploader.last_hash.lock().await = None;
    use tauri::Emitter;
    let _ = app.emit("uploader:status", serde_json::json!({"state": "forced"}));
    Ok(())
}
