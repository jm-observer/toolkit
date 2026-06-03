//! `rag` 二进制入口。
//!
//! 子命令：
//! - `rag ingest`  扫 douyin 知识包 → 向量库
//! - `rag search`  语义检索，stdout 一行 JSON
//! - `rag serve`   起 axum HTTP 服务
//!
//! `ingest` / `search` 遵循 douyin 生态约定：结果以一行紧凑 JSON 输出到 stdout，
//! 业务失败输出 `{"error":..,"error_kind":..}`，进程退出码恒为 0。

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::LevelFilter::Info;
use serde_json::{json, Value};

use rag::types::SearchQuery;
use rag::{build_service, ingest::ingest_douyin_knowledge, resolve_workspace, serve, RagConfig};

#[derive(Parser, Debug)]
#[command(name = "rag", version, about = "douyin 知识库语义检索服务", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 全量扫描 douyin 知识包并 ingest 进向量库。
    Ingest {
        /// RAG 配置 JSON 路径（绝对）。
        #[arg(long)]
        config: PathBuf,
        /// workspace 根（缺省 ZERO_WORKSPACE 或 $HOME/.config/zero）。
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value = "douyin")]
        namespace: String,
    },
    /// 语义检索。
    Search {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long)]
        query: String,
        #[arg(long, default_value = "douyin")]
        namespace: String,
        #[arg(long, default_value_t = 5)]
        top_k: usize,
    },
    /// 起 HTTP 服务（daemon）。
    Serve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:8788")]
        bind: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = custom_utils::logger::logger_feature("rag", "info,reqwest=warn", Info, false).build();
    let args = Args::parse();
    match args.command {
        Command::Ingest {
            config,
            workspace,
            namespace,
        } => {
            emit(do_ingest(config, workspace, namespace).await, "ingest");
        }
        Command::Search {
            config,
            workspace,
            query,
            namespace,
            top_k,
        } => {
            emit(
                do_search(config, workspace, query, namespace, top_k).await,
                "search",
            );
        }
        Command::Serve {
            config,
            workspace,
            bind,
        } => {
            // serve 是长跑路径，不走 JSON stdout 契约；错误正常向上传播。
            return do_serve(config, workspace, bind).await;
        }
    }
    Ok(())
}

/// 把命令结果按契约打到 stdout（一行紧凑 JSON）。
fn emit(result: Result<Value>, kind: &str) {
    let out = match result {
        Ok(v) => v,
        Err(e) => json!({ "error": e.to_string(), "error_kind": kind }),
    };
    println!("{out}");
}

async fn do_ingest(
    config: PathBuf,
    workspace: Option<PathBuf>,
    namespace: String,
) -> Result<Value> {
    let ws = resolve_workspace(workspace.as_deref())?;
    let cfg = RagConfig::load(&config).await?;
    let service = build_service(&cfg, &ws).await?;
    let stats = ingest_douyin_knowledge(&service, &ws, &namespace).await?;
    Ok(json!({
        "ingested": stats.ingested,
        "skipped": stats.skipped,
        "failed": stats.failed,
    }))
}

async fn do_search(
    config: PathBuf,
    workspace: Option<PathBuf>,
    query: String,
    namespace: String,
    top_k: usize,
) -> Result<Value> {
    let ws = resolve_workspace(workspace.as_deref())?;
    let cfg = RagConfig::load(&config).await?;
    let service = build_service(&cfg, &ws).await?;
    let hits = service
        .search(SearchQuery {
            namespace,
            query,
            top_k,
        })
        .await?;
    Ok(json!({ "hits": hits }))
}

async fn do_serve(config: PathBuf, workspace: Option<PathBuf>, bind: String) -> Result<()> {
    let ws = resolve_workspace(workspace.as_deref())?;
    let cfg = RagConfig::load(&config).await?;
    let service = build_service(&cfg, &ws).await?;
    serve::run(service, ws, &bind).await
}
