#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app_state;
mod modules;
mod shared;

use anyhow::{Context, Result};
use app_state::AppState;
use clap::{Parser, Subcommand};
use log::LevelFilter::Info;
use std::path::PathBuf;

const REPO_OWNER: &str = "jm-observer";
const REPO_NAME: &str = "toolkit";
const APP: &str = "zero-desktop";

#[derive(Parser, Debug, Clone)]
#[command(name = "zero-desktop", version)]
struct Cli {
    #[arg(long, env = "ZERO_DESKTOP_WORKSPACE", global = true)]
    workspace: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// 启动图形界面（默认）。
    Run,
    /// 自更新。
    Update {
        #[arg(short, long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    let _ = custom_utils::logger::logger_feature(APP, "info,reqwest=warn", Info, false).build();

    let cli = Cli::parse();

    let workspace = resolve_workspace(&cli.workspace)?;
    log::info!("workspace = {}", workspace.display());

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_gui(workspace),
        Command::Update { force } => shared::update::run_update(REPO_OWNER, REPO_NAME, APP, force),
    }
}

fn resolve_workspace(arg: &Option<String>) -> Result<PathBuf> {
    let path = match arg {
        Some(p) => PathBuf::from(p),
        None => dirs::data_local_dir()
            .context("cannot determine data_local_dir")?
            .join("zero-desktop"),
    };
    Ok(path)
}

fn run_gui(workspace: PathBuf) -> Result<()> {
    shared::workspace::ensure_workspace(&workspace)?;

    let state = AppState::new(workspace);

    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            modules::english::english_ping,
            modules::speech::speech_ping,
            modules::cookie::cookie_ping,
        ])
        .setup(move |app| {
            modules::english::setup(app.handle(), state.english.clone())
                .context("english::setup")?;
            modules::speech::setup(app.handle(), state.speech.clone()).context("speech::setup")?;
            modules::cookie::setup(app.handle(), state.cookie.clone()).context("cookie::setup")?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("run tauri");
    Ok(())
}
