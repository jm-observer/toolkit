use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoCopyMode {
    Off,
    #[default]
    English,
    OptimizedZh,
}

/// Default auto-copy "stitch window" (ms).
pub const DEFAULT_MERGE_WINDOW_MS: u64 = 3000;

/// Upper bound for the configurable stitch window (ms).
pub const MAX_MERGE_WINDOW_MS: u64 = 60_000;

fn default_merge_window_ms() -> u64 {
    DEFAULT_MERGE_WINDOW_MS
}

fn default_notify_sound() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmSettings {
    #[serde(default)]
    pub auto_copy_mode: AutoCopyMode,
    #[serde(default = "default_merge_window_ms")]
    pub merge_window_ms: u64,
    #[serde(default)]
    pub want_secondary: bool,
    #[serde(default = "default_notify_sound")]
    pub notify_sound: bool,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            auto_copy_mode: AutoCopyMode::default(),
            merge_window_ms: DEFAULT_MERGE_WINDOW_MS,
            want_secondary: false,
            notify_sound: true,
        }
    }
}
