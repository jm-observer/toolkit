use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use custom_utils::updater::UpdateConfig;
use log::LevelFilter::Info;
use std::path::PathBuf;

// 自更新指向承载本工具集的同一 GitHub 仓库（与 github-commit-info 同仓）。
const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "github-commit-info";
const BIN_NAME: &str = "douyin";

#[derive(Parser, Debug)]
#[command(name = "douyin", version, about = "zero 的抖音工具集", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// cookie 自检：字段完整性 + 登录态实测。
    CookieStatus {
        /// cookie 文件路径（缺省取 $ZERO_WORKSPACE/douyin/cookies.json）。
        #[arg(long)]
        cookie_file: Option<PathBuf>,
    },
    /// 写入 cookies.json（接受浏览器 Cookie 头串或 JSON 对象）。
    SetCookie {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        /// cookie 原文：`k=v; k=v` 或 `{"k":"v"}`。
        #[arg(long)]
        raw: String,
    },
    /// 按昵称/抖音号搜博主（v2 已知被风控，多返回 anti_bot 引导用主页 URL）。
    SearchUser {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        keyword: String,
        #[arg(long, default_value_t = 15)]
        count: i64,
    },
    /// URL / 短链 / sec_uid → 博主资料（含 aweme_count）。
    ResolveUser {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        input: String,
    },
    /// 列博主作品元数据（含 throttled / pages_fetched 信号）。
    ListWorks {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        input: String,
        #[arg(long, default_value_t = 60)]
        max_pages: usize,
    },
    /// 异步入队下载，立即返回 task_id。
    DownloadSubmit {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// 逗号分隔的 aweme_id。
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
    },
    /// 查下载任务进度。
    DownloadStatus {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 重启一个下载任务（重 spawn worker，已下载文件靠幂等跳过）。
    DownloadRetry {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 扫描并重启心跳超时（stale）的 running 下载任务。
    DownloadReap {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 600)]
        stale_secs: i64,
    },
    /// 请求取消一个下载任务（worker 处理下一条前转 cancelled）。
    DownloadCancel {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 内部：后台下载 worker（由 download-submit spawn，勿手动调）。
    #[command(hide = true)]
    DownloadWorker {
        #[arg(long)]
        task_dir: PathBuf,
        #[arg(long)]
        task_id: String,
    },
    /// 异步入队列博主作品，立即返回 task_id。
    ListWorksSubmit {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        input: String,
        #[arg(long, default_value_t = 60)]
        max_pages: usize,
        /// 回调寻址 handle（从主 Agent prompt 头部 `[Delivery]` 行原样取）。
        /// worker 跑完时携带此 handle POST gateway 触发第二轮 LLM 周期。
        /// 缺失则 worker 跑完只落 status，不发回调（CLI 手测场景）。
        #[arg(long)]
        delivery_handle: Option<String>,
        /// E2E 测试用 zero session_id（从主 Agent prompt 头部 `[Session]` 行原样取）。
        /// worker POST callback 时回填到 payload，sps correlate 据此关联 sub-agent 调用。
        /// 生产环境可不传，不传不影响功能。
        #[arg(long)]
        session_id: Option<String>,
    },
    /// 查列博主作品任务进度。
    ListWorksStatus {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 重启一个列作品任务（重 spawn worker，整任务重头翻页）。
    ListWorksRetry {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 扫描并重启心跳超时（stale）的 running 列作品任务。
    ListWorksReap {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 600)]
        stale_secs: i64,
    },
    /// 请求取消一个列作品任务（worker 翻下一页前转 cancelled）。
    ListWorksCancel {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 跨三类任务列出精简摘要（task_id/kind/state/时间），可按 --state 过滤。
    ListTasks {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        /// 可选状态过滤：queued/running/succeeded/partial/failed/cancelled。
        #[arg(long)]
        state: Option<String>,
    },
    /// 聚合某博主已拉取作品的话题标签 + 计数（Plan 5 标签预筛）。
    ListTags {
        #[arg(long)]
        works_dir: Option<PathBuf>,
        #[arg(long)]
        unique_id: String,
    },
    /// 按标签筛选已拉取作品，返回匹配 aweme_ids（Plan 5 标签预筛）。
    FilterWorks {
        #[arg(long)]
        works_dir: Option<PathBuf>,
        #[arg(long)]
        unique_id: String,
        /// 逗号分隔的标签名（不含 #）。
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// 匹配模式：all=同时含全部（默认），any=含任一。
        #[arg(long, default_value = "all")]
        r#match: String,
    },
    /// 把缓存里的作品逐条机械写入知识包目录（Plan 5 逐条录入，有转写缓存则回填）。
    PublishKnowledge {
        #[arg(long)]
        works_dir: Option<PathBuf>,
        #[arg(long)]
        knowledge_dir: Option<PathBuf>,
        #[arg(long)]
        transcript_dir: Option<PathBuf>,
        #[arg(long)]
        unique_id: String,
        /// 可选：仅录入这些 aweme_id（逗号分隔），用于标签筛选后录入子集。
        #[arg(long, value_delimiter = ',')]
        only_ids: Vec<String>,
    },
    /// 异步入队「下载 mp4 + ASR 识别」合并任务，立即返回 task_id（Plan 5 阶段1）。
    ProcessSubmit {
        #[arg(long)]
        cookie_file: Option<PathBuf>,
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        #[arg(long)]
        transcript_dir: Option<PathBuf>,
        /// 逗号分隔的 aweme_id。
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
        /// asr-server from-source 端点 URL。
        #[arg(
            long,
            default_value = "http://127.0.0.1:8091/v1/audio/transcriptions/from-source"
        )]
        asr_url: String,
        /// 写入 transcript 缓存的 asr_model 标记（仅记录用）。
        #[arg(long, default_value = "sense-voice")]
        asr_model: String,
        /// 是否启用 VAD 切段以产出字幕时间轴（默认 true）。
        #[arg(long, default_value_t = true)]
        vad: bool,
        /// 回调寻址 handle（从主 Agent prompt 头部 `[Delivery]` 行取）。
        #[arg(long)]
        delivery_handle: Option<String>,
        /// 博主 unique_id（从 §D 输入里的 `unique_id=...` 原样取）。
        /// worker POST callback 时回填到 payload，供 §E 精确回填知识包。
        #[arg(long)]
        unique_id: Option<String>,
        /// zero session_id（从主 Agent prompt 头部 `[Session]` 行原样取）。
        /// worker POST callback 时回填到 payload，sps correlate 据此关联 sub-agent 调用。
        #[arg(long)]
        session_id: Option<String>,
    },
    /// 查「下载+ASR」任务进度。
    ProcessStatus {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 重启一个「下载+ASR」任务（重 spawn worker，已完成 item 靠幂等自动跳过）。
    ProcessRetry {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 扫描并重启心跳超时（stale）的 running「下载+ASR」任务。
    ProcessReap {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        /// 心跳超时阈值（秒）：running 任务心跳距今超过此值即判 stale 并重启。
        #[arg(long, default_value_t = 600)]
        stale_secs: i64,
    },
    /// 请求取消一个「下载+ASR」任务（worker 处理下一条前转 cancelled）。
    ProcessCancel {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 内部：后台 process worker（由 process-submit spawn，勿手动调）。
    #[command(hide = true)]
    ProcessWorker {
        #[arg(long)]
        task_dir: PathBuf,
        #[arg(long)]
        task_id: String,
    },
    /// 内部：后台 list-works worker（由 list-works-submit spawn，勿手动调）。
    #[command(hide = true)]
    ListWorksWorker {
        #[arg(long)]
        task_dir: PathBuf,
        #[arg(long)]
        task_id: String,
    },
    /// 补发未送达的持久 callback（pending 且到期的重投一次）。无 daemon 时由定时调用触发。
    CallbackFlush {
        #[arg(long)]
        task_dir: Option<PathBuf>,
    },
    /// 查某任务的事件时间线（append-only event log）。
    Events {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        #[arg(long)]
        task_id: String,
    },
    /// 启动常驻 daemon + 本机 HTTP API（自动 reap/flush + 跨三类任务查询/控制）。
    Serve {
        #[arg(long)]
        task_dir: Option<PathBuf>,
        /// 监听地址（默认仅本机）。
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
        /// 后台维护轮询间隔（秒）。
        #[arg(long, default_value_t = 60)]
        tick_secs: u64,
        /// stale 判定阈值（秒）：running 任务心跳超过即重启。
        #[arg(long, default_value_t = 600)]
        stale_secs: i64,
    },
    /// 从 GitHub Release 自更新当前可执行文件。
    Update {
        #[arg(short, long, help = "即使版本未升级也强制更新")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ =
        custom_utils::logger::logger_feature("douyin", "info,reqwest=warn", Info, false).build();

    let args = Args::parse();

    // worker 与 update 是特殊路径：不走 JSON stdout 契约。
    match &args.command {
        Command::DownloadWorker { task_dir, task_id } => {
            douyin::download::run_worker(task_dir, task_id).await?;
            return Ok(());
        }
        Command::ListWorksWorker { task_dir, task_id } => {
            douyin::list_works_task::run_worker(task_dir, task_id).await?;
            return Ok(());
        }
        Command::ProcessWorker { task_dir, task_id } => {
            douyin::process::run_worker(task_dir, task_id).await?;
            return Ok(());
        }
        Command::Serve {
            task_dir,
            bind,
            tick_secs,
            stale_secs,
        } => {
            douyin::serve::run(
                douyin::resolve_task_dir(task_dir.clone())?,
                bind.clone(),
                *tick_secs,
                *stale_secs,
            )
            .await?;
            return Ok(());
        }
        Command::Update { force } => {
            let outcome = UpdateConfig::new(REPO_OWNER, REPO_NAME, env!("CARGO_PKG_VERSION"))
                .bin_name(BIN_NAME)
                .force(*force)
                .execute()
                .await
                .context("自更新失败")?;
            log::info!("update: {outcome:?}");
            return Ok(());
        }
        _ => {}
    }

    let value = match args.command {
        Command::CookieStatus { cookie_file } => {
            douyin::run_cookie_status(&douyin::resolve_cookie_file(cookie_file)?).await?
        }
        Command::SetCookie { cookie_file, raw } => {
            douyin::run_set_cookie(&douyin::resolve_cookie_file(cookie_file)?, &raw).await?
        }
        Command::SearchUser {
            cookie_file,
            keyword,
            count,
        } => {
            douyin::run_search_user(&douyin::resolve_cookie_file(cookie_file)?, &keyword, count)
                .await?
        }
        Command::ResolveUser { cookie_file, input } => {
            douyin::run_resolve_user(&douyin::resolve_cookie_file(cookie_file)?, &input).await?
        }
        Command::ListWorks {
            cookie_file,
            input,
            max_pages,
        } => {
            douyin::run_list_works(
                &douyin::resolve_cookie_file(cookie_file)?,
                &input,
                max_pages,
            )
            .await?
        }
        // ===== 长任务命令：透传 daemon（store 在 daemon 侧，--task-dir 不再生效）=====
        Command::DownloadSubmit {
            cookie_file,
            out_dir,
            ids,
            ..
        } => {
            douyin::client::submit(
                "download",
                serde_json::json!({
                    "cookie_file": douyin::resolve_cookie_file(cookie_file)?.to_string_lossy(),
                    "out_dir": douyin::resolve_out_dir(out_dir)?.to_string_lossy(),
                    "ids": ids,
                }),
            )
            .await
        }
        Command::DownloadStatus { task_id, .. } => {
            douyin::client::get(&format!("/v1/tasks/{task_id}")).await
        }
        Command::DownloadRetry { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/retry"), None).await
        }
        Command::DownloadReap { .. } => douyin::client::post("/v1/maintenance/run", None).await,
        Command::DownloadCancel { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/cancel"), None).await
        }
        Command::ListWorksSubmit {
            cookie_file,
            input,
            max_pages,
            delivery_handle,
            session_id,
            ..
        } => {
            let mut params = serde_json::json!({
                "cookie_file": douyin::resolve_cookie_file(cookie_file)?.to_string_lossy(),
                "input": input,
                "max_pages": max_pages,
            });
            if let Some(dh) = delivery_handle {
                params["delivery_handle"] = serde_json::json!(dh);
            }
            if let Some(sid) = session_id {
                params["session_id"] = serde_json::json!(sid);
            }
            douyin::client::submit("list-works", params).await
        }
        Command::ListWorksStatus { task_id, .. } => {
            douyin::client::get(&format!("/v1/tasks/{task_id}")).await
        }
        Command::ListWorksRetry { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/retry"), None).await
        }
        Command::ListWorksReap { .. } => douyin::client::post("/v1/maintenance/run", None).await,
        Command::ListWorksCancel { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/cancel"), None).await
        }
        Command::ListTasks { state, .. } => {
            let path = match state {
                Some(s) => format!("/v1/tasks?state={s}"),
                None => "/v1/tasks".to_string(),
            };
            douyin::client::get(&path).await
        }
        Command::ListTags {
            works_dir,
            unique_id,
        } => douyin::run_list_tags(&douyin::resolve_works_dir(works_dir)?, &unique_id)?,
        Command::FilterWorks {
            works_dir,
            unique_id,
            tags,
            r#match,
        } => douyin::run_filter_works(
            &douyin::resolve_works_dir(works_dir)?,
            &unique_id,
            &tags,
            r#match != "any",
        )?,
        Command::PublishKnowledge {
            works_dir,
            knowledge_dir,
            transcript_dir,
            unique_id,
            only_ids,
        } => douyin::run_publish_knowledge(
            &douyin::resolve_works_dir(works_dir)?,
            &douyin::resolve_knowledge_dir(knowledge_dir)?,
            &douyin::resolve_transcript_dir(transcript_dir)?,
            &unique_id,
            &only_ids,
        )?,
        Command::ProcessSubmit {
            cookie_file,
            out_dir,
            transcript_dir,
            ids,
            asr_url,
            asr_model,
            vad,
            delivery_handle,
            unique_id,
            session_id,
            ..
        } => {
            let mut params = serde_json::json!({
                "cookie_file": douyin::resolve_cookie_file(cookie_file)?.to_string_lossy(),
                "out_dir": douyin::resolve_out_dir(out_dir)?.to_string_lossy(),
                "transcript_dir": douyin::resolve_transcript_dir(transcript_dir)?.to_string_lossy(),
                "ids": ids,
                "asr_url": asr_url,
                "asr_model": asr_model,
                "vad": vad,
            });
            if let Some(dh) = delivery_handle {
                params["delivery_handle"] = serde_json::json!(dh);
            }
            if let Some(u) = unique_id {
                params["unique_id"] = serde_json::json!(u);
            }
            if let Some(sid) = session_id {
                params["session_id"] = serde_json::json!(sid);
            }
            douyin::client::submit("process", params).await
        }
        Command::ProcessStatus { task_id, .. } => {
            douyin::client::get(&format!("/v1/tasks/{task_id}")).await
        }
        Command::CallbackFlush { .. } => douyin::client::post("/v1/callbacks/flush", None).await,
        Command::Events { task_id, .. } => {
            douyin::client::get(&format!("/v1/tasks/{task_id}/events")).await
        }
        Command::ProcessRetry { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/retry"), None).await
        }
        Command::ProcessReap { .. } => douyin::client::post("/v1/maintenance/run", None).await,
        Command::ProcessCancel { task_id, .. } => {
            douyin::client::post(&format!("/v1/tasks/{task_id}/cancel"), None).await
        }
        Command::DownloadWorker { .. }
        | Command::ListWorksWorker { .. }
        | Command::ProcessWorker { .. }
        | Command::Serve { .. }
        | Command::Update { .. } => {
            unreachable!("已在上面处理")
        }
    };

    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
