use crate::modules::{cookie::CookieState, english::EnglishState, speech::SpeechState};
use std::path::PathBuf;
use std::sync::Arc;

/// 顶层应用状态，持有 workspace 路径和三个模块的状态。
#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub workspace: PathBuf,
    pub english: Arc<EnglishState>,
    pub speech: Arc<SpeechState>,
    pub cookie: Arc<CookieState>,
}

impl AppState {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            english: Arc::new(EnglishState::default()),
            speech: Arc::new(SpeechState::default()),
            cookie: Arc::new(CookieState::default()),
        }
    }
}
