use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use github_commit_info::run;
use log::LevelFilter::Info;

const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "toolkit";

#[derive(Parser, Debug)]
#[command(
    name = "github-commit-info",
    version,
    about = "获取GitHub仓库指定时间范围内的commit信息",
    long_about = None,
    subcommand_negates_reqs = true,
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        long,
        required = true,
        help = "GitHub仓库URL (如: https://github.com/golang/go)"
    )]
    url: Option<String>,

    #[arg(long, help = "分支名称 (如不指定则自动获取默认分支)")]
    branch: Option<String>,

    #[arg(long, help = "起始日期, 格式: yyyy-MM-dd (默认昨天)")]
    start_date: Option<String>,

    #[arg(long, help = "从起始日期开始的天数 (默认1)")]
    days: Option<i64>,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(about = "从 GitHub Release 自更新当前可执行文件")]
    Update {
        #[arg(short, long, help = "即使版本未升级也强制更新")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = custom_utils::logger::logger_feature(
        "github-commit-info",
        "debug,reqwest=info",
        Info,
        false,
    )
    .build();

    let args = Args::parse();

    if let Some(Command::Update { force }) = args.command {
        let outcome = UpdateConfig::new(REPO_OWNER, REPO_NAME, env!("CARGO_PKG_VERSION"))
            .bin_name("github-commit-info")
            .force(force)
            .execute()
            .await
            .context("自更新失败")?;
        log::info!("update: {outcome:?}");
        return Ok(());
    }

    if std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .is_none()
    {
        log::warn!("未设置 GITHUB_TOKEN，将匿名调用 GitHub API（限额 60 次/小时）");
    }

    // clap 已经用 subcommand_negates_reqs 保证 url 在非 update 路径下存在。
    let url = args.url.expect("clap 应已强制 --url 存在");

    let start_date = args.start_date.unwrap_or_else(|| {
        let yesterday = chrono::Utc::now() - chrono::Duration::days(1);
        yesterday.format("%Y-%m-%d").to_string()
    });

    let days = args.days.unwrap_or(1);

    run(&url, args.branch.as_deref(), &start_date, days).await
}
