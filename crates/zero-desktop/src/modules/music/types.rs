//! Music 模块的对外类型（前端冻结契约）与内部音频格式类型。
//!
//! 所有对前端暴露的 struct 都是 `serde` serialize、snake_case 字段，命名/字段与
//! `docs/zero-desktop-music-design.md` §5.2 一致——前端 `tauri-client.ts` 据此对齐。

use serde::Serialize;

/// 单首曲目的元数据（扫描时由 lofty 提取，前端列表渲染用）。
#[derive(Debug, Clone, Serialize)]
pub struct Track {
    /// 音频文件的绝对路径（既是唯一标识，也是 `music_play_queue` 入参）。
    pub path: String,
    /// 标题（缺标签时回退文件名 stem）。
    pub title: String,
    /// 歌手（缺标签时空串）。
    pub artist: String,
    /// 专辑（缺标签时空串）。
    pub album: String,
    /// 时长（秒，浮点）；无法解析时为 0。
    pub duration_secs: f64,
    /// 内嵌封面落盘后的绝对路径（前端 `convertFileSrc` 走 asset 协议显示图片）；无封面为 `None`。
    pub cover_path: Option<String>,
}

/// 播放状态快照（`music_get_state` 返回 / 首屏与自愈拉取用）。
#[derive(Debug, Clone, Serialize)]
pub struct PlaybackState {
    /// `"playing" | "paused" | "stopped"`。
    pub status: String,
    /// 当前曲在队列中的下标；无曲为 `-1`。
    pub index: i64,
    /// 当前曲元数据；无曲为 `None`。
    pub track: Option<Track>,
    /// 当前播放位置（秒）。
    pub position_secs: f64,
    /// 当前曲时长（秒）。
    pub duration_secs: f64,
    /// 软件音量 0.0..=1.0（独占 bit-perfect 时为 1.0 旁路）。
    pub volume: f32,
    /// `"off" | "one" | "all"`。
    pub repeat: String,
    /// 随机播放是否开启。
    pub shuffle: bool,
}

/// 音频输出模式。
///
/// `Auto`=独占 bit-perfect 优先（协商失败回退共享）；`Shared`=强制共享模式 + 重采样
/// （兼容：部分设备锁 48k 时独占 44.1k 会加速/杂音，共享模式重采样到设备率可正常播放，
/// 且软件音量生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Auto,
    Shared,
}

impl OutputMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "shared" => OutputMode::Shared,
            _ => OutputMode::Auto,
        }
    }
}

/// 循环模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

impl RepeatMode {
    /// 从前端字符串解析（未知值回退 `Off`）。
    pub fn from_str(s: &str) -> Self {
        match s {
            "one" => RepeatMode::One,
            "all" => RepeatMode::All,
            _ => RepeatMode::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        }
    }
}

/// 播放状态枚举（内部用；序列化时转字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayStatus {
    Playing,
    Paused,
    Stopped,
}

impl PlayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayStatus::Playing => "playing",
            PlayStatus::Paused => "paused",
            PlayStatus::Stopped => "stopped",
        }
    }
}

/// 解码出的源音频格式（驱动 sink 协商）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// 采样率（Hz）。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 每样本有效位深（源 PCM 位深，回报 UI 用；输出统一以 f32 送出）。
    pub bits: u32,
}

/// sink 实际协商生效的格式（经 `music_format_changed` 回报前端，让用户看清是否真 bit-perfect）。
#[derive(Debug, Clone, Copy)]
pub struct ActualFormat {
    pub sample_rate: u32,
    pub bits: u32,
    /// sink 实际输出声道数（cpal 共享=设备声道；wasapi 独占=源声道）。引擎据此把源声道
    /// 适配到设备声道，避免按错误声道数消费缓冲导致播放加速/错位。
    pub channels: u16,
    /// 是否拿到 WASAPI 独占。
    pub exclusive: bool,
    /// 是否经过了 Rubato 重采样（设备不支持源采样率时为 true）。
    pub resampled: bool,
}
