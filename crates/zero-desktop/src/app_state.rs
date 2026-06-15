use crate::modules::{
    cookie::CookieState, english::EnglishState, net_policy::NetPolicyState, speech::SpeechState,
};
use crate::shared::workspace::speech_db_path;
use std::path::PathBuf;
use std::sync::Arc;

/// 顶层应用状态，持有 workspace 路径和各模块的状态。
#[derive(Clone)]
pub struct AppState {
    pub workspace: PathBuf,
    pub english: Arc<EnglishState>,
    pub speech: Arc<SpeechState>,
    pub cookie: Arc<CookieState>,
    pub net_policy: Arc<NetPolicyState>,
}

impl AppState {
    pub fn new(workspace: PathBuf) -> anyhow::Result<Self> {
        let cookie = Arc::new(CookieState::new(workspace.clone())?);
        let speech = SpeechState::new(&speech_db_path(&workspace))?;
        let net_policy = Arc::new(NetPolicyState::new(workspace.clone()));
        Ok(Self {
            workspace,
            english: Arc::new(EnglishState::default()),
            speech,
            cookie,
            net_policy,
        })
    }
}
