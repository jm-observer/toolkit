use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use github_commit_info::run;
use log::LevelFilter::Info;

const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "github-commit-info";

#[derive(Parser, Debug)]
#[command(
    name = "github-commit-info",
    version,
    about = "获取GitHub仓库指定时间范围内的commit信息",
    version,
    long_about = None
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, help = "GitHub仓库URL (如: https://github.com/golang/go)")]
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

fn main() {
    let _ = custom_utils::logger::logger_feature(
        "github-commit-info",
        "debug,reqwest=info",
        Info,
        false,
    )
    .build();

    let args = Args::parse();

    if let Some(Command::Update { force }) = args.command {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        match rt.block_on(
            UpdateConfig::new(REPO_OWNER, REPO_NAME, env!("CARGO_PKG_VERSION"))
                .bin_name(REPO_NAME)
                .force(force)
                .execute(),
        ) {
            Ok(outcome) => {
                log::info!("update: {outcome:?}");
                return;
            }
            Err(e) => {
                eprintln!("自更新失败: {}", e);
                std::process::exit(1);
            }
        }
    }

    let url = args.url.unwrap_or_else(|| {
        eprintln!("错误: 缺少 --url 参数");
        std::process::exit(1);
    });

    let _ = std::env::var("GITHUB_TOKEN").expect("请设置 GITHUB_TOKEN 环境变量");

    let start_date = args.start_date.unwrap_or_else(|| {
        let yesterday = chrono::Utc::now() - chrono::Duration::days(1);
        yesterday.format("%Y-%m-%d").to_string()
    });

    let days = args.days.unwrap_or(1);

    if let Err(e) = run(&url, args.branch.as_deref(), &start_date, days) {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}
