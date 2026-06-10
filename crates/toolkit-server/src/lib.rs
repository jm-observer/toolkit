//! toolkit-server：axum 服务，装配 toolkit-core / toolkit-tasks + 业务模块（Plan 2+）。

pub mod audioforge;
pub mod config;
#[path = "douyin/mod.rs"]
pub mod douyin_mod;
pub mod routes;
pub mod state;
mod static_assets;

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

pub use config::Config;
pub use state::AppState;

/// workspace 根：优先 `TOOLKIT_WORKSPACE` 环境变量；未设置时回退到
/// `$HOME/.config/toolkit-server`（Windows 走 `%USERPROFILE%`）。
/// 与 `LinuxService` 安装期 `{workspace}` 模板默认对齐。
pub fn workspace_dir() -> Result<PathBuf> {
    if let Some(ws) = std::env::var_os("TOOLKIT_WORKSPACE") {
        return Ok(PathBuf::from(ws));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("TOOLKIT_WORKSPACE 与 HOME/USERPROFILE 均未设置，无法定位 workspace")?;
    Ok(PathBuf::from(home).join(".config").join("toolkit-server"))
}

/// 启动服务，阻塞直至 Ctrl+C。
pub async fn run(cfg: Config) -> Result<()> {
    let web_dir = cfg.web_dir.clone();
    let state = bootstrap(&cfg)?;
    serve_with_web(cfg.bind, state, &web_dir).await
}

/// 仅做装配（pool/migrate/registry/recovery），不监听 socket。供测试复用。
pub fn bootstrap(cfg: &Config) -> Result<AppState> {
    std::fs::create_dir_all(&cfg.workspace)
        .with_context(|| format!("create workspace {}", cfg.workspace.display()))?;
    let db_path = cfg.workspace.join("toolkit.db");
    let pool = toolkit_core::open_pool(&db_path)?;
    toolkit_core::migrate(&pool)?;

    let mut registry = toolkit_tasks::Registry::new();
    registry.register::<toolkit_tasks::EchoTask>();
    douyin_mod::kinds::register_all(&mut registry);
    audioforge::register_all(&mut registry);

    let recovered = toolkit_tasks::recover_interrupted(&pool)?;
    if recovered > 0 {
        log::info!("recovered {recovered} interrupted task(s) from prior run");
    }

    Ok(AppState {
        pool,
        registry: Arc::new(registry),
        db_path,
        workspace: cfg.workspace.clone(),
    })
}

/// 起 axum 服务（无 Web 静态目录）。供测试用——总是走内嵌最小 dashboard。
pub async fn serve(bind: SocketAddr, state: AppState) -> Result<()> {
    serve_with_web(bind, state, std::path::Path::new("/__nonexistent__")).await
}

/// 起 axum 服务并按 web_dir 是否存在决定 / 路由形态。
pub async fn serve_with_web(
    bind: SocketAddr,
    state: AppState,
    web_dir: &std::path::Path,
) -> Result<()> {
    let app = build_router(state, web_dir);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    log::info!("toolkit-server listening on {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("axum serve")
}

/// 装配 Router。`web_dir` 存在 → 静态托管；否则内嵌最小 HTML。
pub fn build_router(state: AppState, web_dir: &std::path::Path) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let mut router = axum::Router::new()
        .nest("/api/web", routes::web::router())
        .nest(
            "/api/web/audio",
            routes::audio::router().merge(audioforge::routes::router()),
        )
        .nest("/api/web/douyin", douyin_mod::routes::router())
        .nest("/api/agent", routes::agent::router())
        .nest("/api/browser", routes::browser::router());

    if web_dir.exists() {
        log::info!("serving static web/ from {}", web_dir.display());
        router = router.fallback_service(ServeDir::new(web_dir));
    } else {
        log::info!(
            "web_dir {} not present; falling back to embedded dashboard",
            web_dir.display()
        );
        router = router
            .route("/", axum::routing::get(static_assets::dashboard))
            .route("/app.js", axum::routing::get(static_assets::app_js))
            .route("/style.css", axum::routing::get(static_assets::style_css));
    }

    router.layer(cors).with_state(state)
}

/// 起一个本地随机端口供测试用。返回 (listener, addr)。
pub async fn bind_ephemeral() -> Result<(tokio::net::TcpListener, SocketAddr)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

/// 用于测试：把现成 listener + state 跑起来（无静态 web 目录，走嵌入式 HTML）。
pub async fn serve_with_listener(listener: tokio::net::TcpListener, state: AppState) -> Result<()> {
    let router = build_router(state, std::path::Path::new("/__nonexistent__"));
    axum::serve(listener, router).await?;
    Ok(())
}

/// helper：构造 Config 给测试用
pub fn test_config(workspace: PathBuf) -> Config {
    Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        workspace,
        web_dir: PathBuf::from("/__nonexistent__"),
    }
}
