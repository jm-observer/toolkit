//! toolkit-server：axum 服务，装配 toolkit-core / toolkit-tasks + 业务模块（Plan 2+）。

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

pub use config::Config;
pub use state::AppState;

/// 启动服务，阻塞直至 Ctrl+C。
pub async fn run(cfg: Config) -> Result<()> {
    let state = bootstrap(&cfg)?;
    serve(cfg.bind, state).await
}

/// 仅做装配（pool/migrate/registry/recovery），不监听 socket。供测试复用。
pub fn bootstrap(cfg: &Config) -> Result<AppState> {
    std::fs::create_dir_all(&cfg.data_dir)
        .with_context(|| format!("create data_dir {}", cfg.data_dir.display()))?;
    let db_path = cfg.data_dir.join("toolkit.db");
    let pool = toolkit_core::open_pool(&db_path)?;
    toolkit_core::migrate(&pool)?;

    let mut registry = toolkit_tasks::Registry::new();
    registry.register::<toolkit_tasks::EchoTask>();
    douyin_mod::kinds::register_all(&mut registry);

    let recovered = toolkit_tasks::recover_interrupted(&pool)?;
    if recovered > 0 {
        log::info!("recovered {recovered} interrupted task(s) from prior run");
    }

    Ok(AppState {
        pool,
        registry: Arc::new(registry),
        db_path,
        data_dir: cfg.data_dir.clone(),
    })
}

/// 起 axum 服务，阻塞直至 Ctrl+C。供测试覆盖时改为传入 listener。
pub async fn serve(bind: SocketAddr, state: AppState) -> Result<()> {
    let app = build_router(state);
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

/// 装配 Router——测试可直接调起一个 TcpListener + axum::serve。
pub fn build_router(state: AppState) -> axum::Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    axum::Router::new()
        .nest("/api/web", routes::web::router())
        .nest("/api/web/douyin", douyin_mod::routes::router())
        .nest("/api/agent", routes::agent::router())
        .nest("/api/browser", routes::browser::router())
        .route("/", axum::routing::get(static_assets::dashboard))
        .layer(cors)
        .with_state(state)
}

/// 起一个本地随机端口供测试用。返回 (listener, addr)。
pub async fn bind_ephemeral() -> Result<(tokio::net::TcpListener, SocketAddr)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

/// 用于测试：把现成 listener + state 跑起来。
pub async fn serve_with_listener(listener: tokio::net::TcpListener, state: AppState) -> Result<()> {
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

/// helper：构造 Config 给测试用
pub fn test_config(data_dir: PathBuf) -> Config {
    Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir,
    }
}
