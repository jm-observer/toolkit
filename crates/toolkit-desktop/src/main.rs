#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod bridge;
mod browser;
mod config;
mod db;
mod ths;
mod ths_watcher;
mod uploader;
mod workspace;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use log::LevelFilter::Info;
use std::path::PathBuf;
use std::sync::Arc;
use uploader::UploaderState;

const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "toolkit";
const APP: &str = "toolkit-desktop";

#[derive(Parser, Debug, Clone)]
#[command(name = "toolkit-desktop", version)]
struct Cli {
    #[arg(long, env = "TOOLKIT_DESKTOP_WORKSPACE", global = true)]
    workspace: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    Run {
        #[arg(long, env = "TOOLKIT_DESKTOP_SERVER")]
        server: Option<String>,
        #[arg(long, env = "TOOLKIT_DESKTOP_TOKEN")]
        token: Option<String>,
    },
    Update {
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Clone)]
pub struct AppCtx {
    pub workspace: PathBuf,
    pub db: Arc<db::Db>,
    pub uploader: Arc<UploaderState>,
    pub ths: Arc<ths_watcher::ThsState>,
    /// 抖音登录用的 Chrome 子进程会话（headless_chrome / CDP）。
    pub douyin_browser: Arc<browser::Session>,
    /// 同花顺登录用的 Chrome 子进程会话。
    pub ths_browser: Arc<browser::Session>,
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
    let ths_state = Arc::new(ths_watcher::ThsState::default());
    // 持久 Chrome profile：登录态跨重启保留，profile 养熟后抖音 msToken 才会落库复用
    //（实测全新临时 profile 永远拿不到 msToken，详见 browser.rs 模块注释）。
    let login_profiles = workspace.join("login_profile");
    let douyin_browser = Arc::new(browser::Session::new(login_profiles.join("douyin")));
    let ths_browser = Arc::new(browser::Session::new(login_profiles.join("ths")));
    let ctx = AppCtx {
        workspace: workspace.clone(),
        db: db.clone(),
        uploader: uploader.clone(),
        ths: ths_state.clone(),
        douyin_browser: douyin_browser.clone(),
        ths_browser: ths_browser.clone(),
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
            cmd_open_ths_login,
            cmd_close_ths_login,
            cmd_ths_status,
            cmd_check_server_cookie,
            cmd_login_expiry,
            cmd_track_current_creator,
        ])
        .setup(move |app| {
            uploader::spawn(app.handle().clone(), ctx.clone());
            bridge::spawn(bridge::BridgeCtx {
                douyin: ctx.douyin_browser.clone(),
                ths: ctx.ths_browser.clone(),
                workspace: ctx.workspace.clone(),
            });
            ths_watcher::spawn(app.handle().clone(), ctx.clone());
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

// ============ 抖音登录窗（Chrome 子进程） ============

#[tauri::command]
async fn cmd_open_login(ctx: tauri::State<'_, AppCtx>) -> Result<(), String> {
    let session = ctx.douyin_browser.clone();
    tokio::task::spawn_blocking(move || session.open("https://www.douyin.com"))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[tauri::command]
async fn cmd_close_login(ctx: tauri::State<'_, AppCtx>) -> Result<(), String> {
    let session = ctx.douyin_browser.clone();
    tokio::task::spawn_blocking(move || session.close())
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?;
    Ok(())
}

// ============ 同花顺登录窗（Chrome 子进程） ============

#[tauri::command]
async fn cmd_open_ths_login(ctx: tauri::State<'_, AppCtx>) -> Result<(), String> {
    let session = ctx.ths_browser.clone();
    tokio::task::spawn_blocking(move || session.open(ths::LOGIN_URL))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[tauri::command]
async fn cmd_close_ths_login(ctx: tauri::State<'_, AppCtx>) -> Result<(), String> {
    let session = ctx.ths_browser.clone();
    tokio::task::spawn_blocking(move || session.close())
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?;
    Ok(())
}

#[tauri::command]
fn cmd_ths_status(ctx: tauri::State<'_, AppCtx>) -> ths::StatusReport {
    ths::status_report(&ctx.workspace)
}

// ============ 解析当前博主 ============

#[tauri::command]
async fn cmd_track_current_creator(
    ctx: tauri::State<'_, AppCtx>,
) -> Result<serde_json::Value, String> {
    let session = ctx.douyin_browser.clone();
    let url = tokio::task::spawn_blocking(move || session.current_url())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "没有打开抖音登录窗口或读 URL 失败".to_string())?;

    let settings = config::load(&workspace::config_path(&ctx.workspace));
    if !settings.is_configured() {
        return Err("Server base 未配置".to_string());
    }
    let base = settings.server_base.trim_end_matches('/');
    let endpoint = format!("{base}/api/web/douyin/creators");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .post(&endpoint)
        .json(&serde_json::json!({ "handle": url }));
    if let Some(tok) = settings.auth_token.as_deref().filter(|s| !s.is_empty()) {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("server {status}: {body}"));
    }
    Ok(body)
}

// ============ 登录 cookie 失效时间（CDP 拿） ============

#[tauri::command]
async fn cmd_login_expiry(ctx: tauri::State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    use chrono::TimeZone;
    let Some(tab) = ctx.douyin_browser.tab() else {
        return Ok(serde_json::json!({ "state": "no_window" }));
    };
    let cookies = tokio::task::spawn_blocking(move || tab.get_cookies())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    const CRITICAL: &[&str] = &["sessionid_ss", "ttwid", "sid_guard", "sid_tt"];
    let now = chrono::Utc::now().timestamp();
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut earliest: Option<i64> = None;
    for c in &cookies {
        if !CRITICAL.contains(&c.name.as_str()) {
            continue;
        }
        let ts = if c.expires > 0.0 {
            Some(c.expires as i64)
        } else {
            None
        };
        let iso = ts.and_then(|t| {
            chrono::Utc
                .timestamp_opt(t, 0)
                .single()
                .map(|d| d.with_timezone(&chrono::Local).to_rfc3339())
        });
        let remaining = ts.map(|t| t - now);
        if let Some(t) = ts {
            earliest = Some(earliest.map_or(t, |e| e.min(t)));
        }
        entries.push(serde_json::json!({
            "name": c.name,
            "expires_at": iso,
            "remaining_secs": remaining,
            "is_session": ts.is_none(),
        }));
    }
    let earliest_iso = earliest.and_then(|t| {
        chrono::Utc
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.with_timezone(&chrono::Local).to_rfc3339())
    });
    let earliest_remaining = earliest.map(|t| t - now);
    Ok(serde_json::json!({
        "state": "ok",
        "critical": entries,
        "earliest_expires_at": earliest_iso,
        "earliest_remaining_secs": earliest_remaining,
        "cookies_total": cookies.len(),
    }))
}

// ============ G10 server 探活 + cookie 状态 ============

#[tauri::command]
async fn cmd_check_server_cookie(
    ctx: tauri::State<'_, AppCtx>,
) -> Result<serde_json::Value, String> {
    let settings = config::load(&workspace::config_path(&ctx.workspace));
    if !settings.is_configured() {
        return Ok(serde_json::json!({ "state": "unconfigured" }));
    }
    let base = settings.server_base.trim_end_matches('/');
    let url = format!("{base}/api/web/douyin/cookie_status");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    if let Some(tok) = settings.auth_token.as_deref().filter(|s| !s.is_empty()) {
        req = req.bearer_auth(tok);
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(serde_json::json!({
                        "state": "parse_err", "error": e.to_string(),
                    }));
                }
            };
            if !status.is_success() {
                return Ok(serde_json::json!({
                    "state": "http_err", "status": status.as_u16(), "body": body,
                }));
            }
            Ok(serde_json::json!({ "state": "ok", "body": body }))
        }
        Err(e) => Ok(serde_json::json!({
            "state": "unreachable", "error": e.to_string(),
        })),
    }
}

#[tauri::command]
async fn cmd_inspect_cookies(ctx: tauri::State<'_, AppCtx>) -> Result<serde_json::Value, String> {
    let Some(tab) = ctx.douyin_browser.tab() else {
        return Ok(serde_json::json!({
            "state": "no_login_window",
            "hint": "请先点「抖音登录」",
        }));
    };
    let cookies = tokio::task::spawn_blocking(move || tab.get_cookies())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let names: Vec<&str> = cookies.iter().map(|c| c.name.as_str()).collect();
    let has_ms = names.contains(&"msToken");
    // 从网络请求 harvest 到的 msToken（cookie 里没有时的兜底来源）。
    let harvested = ctx.douyin_browser.harvested_ms_token();
    let ms_info = cookies.iter().find(|c| c.name == "msToken").map(|c| {
        serde_json::json!({
            "len": c.value.len(),
            "domain": c.domain,
            "path": c.path,
            "http_only": c.http_only,
            "secure": c.secure,
            "expires": c.expires,
        })
    });
    let all: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "len": c.value.len(),
                "domain": c.domain,
                "path": c.path,
                "http_only": c.http_only,
                "secure": c.secure,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "state": "ok",
        "count": cookies.len(),
        "has_ms_token": has_ms,
        "ms_token": ms_info,
        // harvest 兜底：cookie 没有但从外发请求抓到了，也算拿到 msToken。
        "ms_token_harvested": harvested.as_deref(),
        "has_ms_token_any": has_ms || harvested.is_some(),
        "names": names,
        "all": all,
    }))
}

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
