#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod bridge;
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
    pub ths: Arc<ths_watcher::ThsState>,
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
    let ths = Arc::new(ths_watcher::ThsState::default());
    let ctx = AppCtx {
        workspace: workspace.clone(),
        db: db.clone(),
        uploader: uploader.clone(),
        ths: ths.clone(),
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
                app: app.handle().clone(),
                uploader: ctx.uploader.clone(),
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

#[tauri::command]
async fn cmd_close_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    Ok(())
}

/// 打开/前置同花顺登录窗口。重复调用幂等。watcher tick 拿到关键 cookie 后落盘到
/// `<workspace>/ths/cookies.json`（与 stock-trade/ths 项目兼容格式）。
#[tauri::command]
async fn cmd_open_ths_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("ths-login") {
        let _ = w.set_focus();
        return Ok(());
    }
    let target: url::Url = ths::LOGIN_URL
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    WebviewWindowBuilder::new(&app, "ths-login", WebviewUrl::External(target))
        .title("同花顺登录 - toolkit-desktop")
        .inner_size(1180.0, 820.0)
        .center()
        .build()
        .map_err(|e| format!("create ths login window: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn cmd_close_ths_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("ths-login") {
        let _ = w.close();
    }
    Ok(())
}

#[tauri::command]
fn cmd_ths_status(ctx: tauri::State<'_, AppCtx>) -> ths::StatusReport {
    ths::status_report(&ctx.workspace)
}

/// 读 login 窗当前 URL → POST G10 `/api/web/douyin/creators` 解析 + upsert 到博主库。
/// desktop 唯一的业务"动作"，因为博主上下文（用户当前看的是谁）只在登录窗里。
/// 解析结果由 G10 落库，web 端去拉列表展示，desktop 不存。
#[tauri::command]
async fn cmd_track_current_creator(
    ctx: tauri::State<'_, AppCtx>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let login = app
        .get_webview_window("login")
        .ok_or_else(|| "没有打开抖音登录窗口".to_string())?;
    let url = login.url().map_err(|e| format!("读 login URL: {e}"))?.to_string();

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

/// 解析 login 窗里关键 cookie 的失效时间（持久 cookie 含 expires，session cookie 标记为 null）。
/// 关键 cookie 名：sessionid_ss / ttwid / sid_guard / sid_tt。返回最早过期时间 + 各字段明细。
#[tauri::command]
async fn cmd_login_expiry(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    use chrono::TimeZone;
    let Some(login) = app.get_webview_window("login") else {
        return Ok(serde_json::json!({ "state": "no_window" }));
    };
    let target: url::Url = "https://www.douyin.com"
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    let cookies = login.cookies_for_url(target).map_err(|e| e.to_string())?;

    const CRITICAL: &[&str] = &["sessionid_ss", "ttwid", "sid_guard", "sid_tt"];
    let now = chrono::Utc::now().timestamp();
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut earliest: Option<i64> = None;
    for c in &cookies {
        if !CRITICAL.contains(&c.name()) {
            continue;
        }
        let ts = match c.expires() {
            Some(tauri::webview::cookie::Expiration::DateTime(dt)) => Some(dt.unix_timestamp()),
            _ => None,
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
            "name": c.name(),
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

// 业务下沉：所有抖音 / 同花顺业务操作走 G10 web UI。
// desktop 不再代理业务调用，仅提供本机上下文（登录窗 URL / cookie / msToken / ths）
// 给 G10 web 通过 127.0.0.1:28788 bridge 拉取。
//
// 例外：cookie_status — 桌面端在「打开抖音登录」前需要先问一下 G10 cookie 是否还活，
// 给用户「可能不必重登」的判断依据，所以走 desktop 后端代理一次。

/// 拉 `<server>/api/web/douyin/cookie_status`，给桌面端做「是否需要重登」预判用。
/// 4s 超时；不抛业务错，返回结构化 {state, ...}。
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
