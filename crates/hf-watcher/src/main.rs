use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use hf_watcher::{run_model_card, run_trending};
use log::LevelFilter::Info;
use std::path::PathBuf;

// 自更新指向承载本工具集的同一 GitHub 仓库（toolkit，与其他工具同仓）。
const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "toolkit";
const BIN_NAME: &str = "hf-watcher";

#[derive(Parser, Debug)]
#[command(
    name = "hf-watcher",
    version,
    about = "zero 的 HuggingFace 趋势监听工具集",
    long_about = None
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 取某 pipeline_tag 的 trending top-N，与上次快照对比，输出新进榜 / 出新版本的模型。
    Trending {
        /// HuggingFace pipeline_tag，如 text-to-speech / automatic-speech-recognition /
        /// image-text-to-text / any-to-any。
        #[arg(long)]
        pipeline_tag: String,

        /// 取榜数量，默认 20。
        #[arg(long, default_value_t = 20)]
        top_n: usize,

        /// 快照目录的**绝对路径**，必填。本工具不做任何回退/默认值——目录归属由
        /// 调用方（zero agent）按 config.toml 配置决定。
        #[arg(long)]
        snapshot_dir: PathBuf,

        /// 只对比、不回写快照（试跑用）。
        #[arg(long, default_value_t = false)]
        no_write: bool,
    },

    /// 取单模型的 README 原文 + meta（参数量 / likes / tags 等）。
    ModelCard {
        /// 模型 id，形如 owner/repo。
        #[arg(long)]
        model_id: String,

        /// README 字节预算，超出按字符边界截断，默认 30000。
        #[arg(long, default_value_t = 30000)]
        max_bytes: usize,
    },

    /// 从 GitHub Release 自更新当前可执行文件。
    Update {
        #[arg(short, long, help = "即使版本未升级也强制更新")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // 日志初始化：dev 输出到控制台；prod（部署）写 {home}/log/hf-watcher，
    // 不污染 stdout 的 JSON 输出契约。与 github-commit-info 一致。
    let _ = custom_utils::logger::logger_feature("hf-watcher", "info,reqwest=warn", Info, false)
        .build();

    let args = Args::parse();

    if let Command::Update { force } = args.command {
        let outcome = UpdateConfig::new(REPO_OWNER, REPO_NAME, env!("CARGO_PKG_VERSION"))
            .bin_name(BIN_NAME)
            .force(force)
            .execute()
            .await
            .context("自更新失败")?;
        log::info!("update: {outcome:?}");
        return Ok(());
    }

    let value = match args.command {
        Command::Trending {
            pipeline_tag,
            top_n,
            snapshot_dir,
            no_write,
        } => run_trending(&pipeline_tag, top_n, &snapshot_dir, !no_write).await?,
        Command::ModelCard {
            model_id,
            max_bytes,
        } => run_model_card(&model_id, max_bytes).await?,
        Command::Update { .. } => unreachable!("Update 已在上面处理"),
    };

    // 紧凑 JSON 到 stdout，供 zero 工具层解析；应用日志一律走 logger（prod 落文件）。
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
