//! 业务领域结构。仅本 RFC Plan 1 需要的字段。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creator {
    pub unique_id: String,
    pub sec_uid: String,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub signature: Option<String>,
    pub follower_count: Option<i64>,
    pub aweme_count: Option<i64>,
    pub verified: bool,
    pub added_at: String,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub progress: serde_json::Value,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub callback_url: Option<String>,
    pub callback_delivered_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieRow {
    pub raw: String,
    pub parsed: serde_json::Value,
    pub captured_at: String,
    pub last_validated_at: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSession {
    pub session_id: String,
    pub user_agent: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub current_url: Option<String>,
}
