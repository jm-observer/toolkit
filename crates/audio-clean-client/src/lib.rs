//! `audio-clean-client` —— streaming-speech 仓 audio-cleanup `/clean` 端点的 Rust 客户端。
//!
//! 端点契约权威源：`streaming-speech/docs/audio-cleanup-api.md`（实现见
//! `streaming-speech/server/audio-cleanup/`）。形状刻意照抄同仓的 `asr-client`：把
//! 上传 multipart / 解析响应 / 错误归类集中一处，端点契约变更时只改这里。
//!
//! 与 asr-client 的差异：`/clean` 响应体是**二进制音频**，清洗元数据放在响应头
//! （`X-Cleanup-Stages` / `X-Cleanup-In-LUFS` / `X-Cleanup-Out-LUFS`）。
//!
//! ## 用法
//!
//! ```no_run
//! use audio_clean_client::{AudioCleanClient, CleanOpts, PauseMode};
//! # async fn _x() -> anyhow::Result<()> {
//! let client = AudioCleanClient::new("http://127.0.0.1:8097");
//! // 带 BGM 视频 → 去乐人声给 ASR：分离 + 关删停顿 + 16k 输出
//! let opts = CleanOpts { separate: true, pause: PauseMode::Off, sr: 16000, ..Default::default() };
//! let out = client.clean_path("clip.mp4", opts).await?;
//! tokio::fs::write("vocals.wav", &out.bytes).await?;
//! println!("stages={:?} in={} out={}", out.stages, out.in_lufs, out.out_lufs);
//! # Ok(()) }
//! ```

use anyhow::{bail, Context, Result};
use reqwest::multipart::{Form, Part};
use std::path::Path;
use std::time::Duration;

/// 默认 base URL：同机 audio-cleanup 容器经 `127.0.0.1:8097:8097` 暴露。
/// `streaming-speech/server/audio-cleanup/compose.cleanup.yaml` 决定。
pub const DEFAULT_BASE: &str = "http://127.0.0.1:8097";

/// 读盘 + 上传 + 清洗的总超时。开 Demucs 的整段清洗可能数十秒到数分钟，串行排队 ——
/// 10 分钟兜底，与服务端 `PROCESS_TIMEOUT_SEC` 对齐。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// 删停顿模式。`Default` 为 `Duck`（压低不删，保留节奏）—— 与服务端默认对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseMode {
    /// 删除非语音段（会改变时长）。
    Drop,
    /// 压低非语音段增益（保留节奏，默认）。
    #[default]
    Duck,
    /// 不处理停顿。
    Off,
}

impl PauseMode {
    fn as_str(self) -> &'static str {
        match self {
            PauseMode::Drop => "drop",
            PauseMode::Duck => "duck",
            PauseMode::Off => "off",
        }
    }
}

/// 降噪强度档位。`Default` 为 `Balanced`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// 温和，宁留底噪（TTS 素材 / ASR 预处理）。
    Gentle,
    /// 均衡（默认）。
    #[default]
    Balanced,
    /// 激进，底噪极重时。
    Aggressive,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Gentle => "gentle",
            Level::Balanced => "balanced",
            Level::Aggressive => "aggressive",
        }
    }
}

/// 输出编码格式。`Default` 为 `Wav`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioFormat {
    /// PCM16 WAV（默认）。
    #[default]
    Wav,
    Mp3,
    Flac,
}

impl AudioFormat {
    fn as_str(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Flac => "flac",
        }
    }
}

/// 单次 `/clean` 请求参数。镜像服务端 multipart 字段；`Default` 与服务端默认对齐
/// （降噪开、压低停顿、48k、目标 -16 LUFS）。
#[derive(Debug, Clone)]
pub struct CleanOpts {
    /// `true` → 人声分离（Demucs，慢）。
    pub separate: bool,
    /// `true` → DeepFilterNet 降噪+去混响（固定 48k 内部处理）。
    pub denoise: bool,
    /// 停顿处理：删 / 压低 / 不动。
    pub pause: PauseMode,
    /// 降噪强度档位。
    pub level: Level,
    /// 目标响度 LUFS；`None` 关闭归一化。
    pub loudness: Option<f32>,
    /// 输出采样率（仅末端生效，不影响 DF 的 48k）。支持 16000 / 24000 / 48000。
    pub sr: u32,
    /// 输出编码格式。
    pub format: AudioFormat,
}

impl Default for CleanOpts {
    fn default() -> Self {
        Self {
            separate: false,
            denoise: true,
            pause: PauseMode::Duck,
            level: Level::Balanced,
            loudness: Some(-16.0),
            sr: 48000,
            format: AudioFormat::Wav,
        }
    }
}

/// `/clean` 成功响应：清洗后音频字节 + 从响应头解析的元数据。
#[derive(Debug, Clone)]
pub struct CleanedAudio {
    /// 清洗后的音频文件字节（按 `CleanOpts::format` 编码）。
    pub bytes: Vec<u8>,
    /// 实际执行过的 stage 列表（`X-Cleanup-Stages`）。
    pub stages: Vec<String>,
    /// 输入响度 LUFS（`X-Cleanup-In-LUFS`）；缺失/不可解析为 `NaN`。
    pub in_lufs: f32,
    /// 输出响度 LUFS（`X-Cleanup-Out-LUFS`）；缺失/不可解析为 `NaN`。
    pub out_lufs: f32,
}

/// audio-cleanup `/clean` 客户端。`Clone` 廉价（内部是 `reqwest::Client` 的 Arc 句柄）。
#[derive(Debug, Clone)]
pub struct AudioCleanClient {
    http: reqwest::Client,
    clean_base_url: String,
}

impl AudioCleanClient {
    /// 用 base URL（如 `http://127.0.0.1:8097`，**不含** `/clean`）构造客户端。
    /// 内部 `reqwest::Client` 用默认 timeout（600 s）。
    pub fn new(clean_base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("reqwest client build");
        Self::with_client(http, clean_base_url)
    }

    /// 复用调用方已有的 `reqwest::Client`（自定义代理 / 连接池 / TLS 时使用）。
    ///
    /// **防错**：对入参 trim 掉尾随 `/` 与误带的 `/clean` 段（沿用 asr-client
    /// `strip_suffix` 容错先例），保证即便调用方误传 `.../clean` 也不会拼成 `/clean/clean`。
    pub fn with_client(http: reqwest::Client, clean_base_url: impl Into<String>) -> Self {
        let mut base = clean_base_url.into();
        trim_base(&mut base);
        Self {
            http,
            clean_base_url: base,
        }
    }

    /// 当前 base URL（去尾斜杠 / `/clean` 后的形态）。日志/诊断用。
    pub fn base(&self) -> &str {
        &self.clean_base_url
    }

    /// 用文件路径清洗。读盘 + 推 MIME 在内部做，调用方只关心 path/opts。
    pub async fn clean_path(
        &self,
        path: impl AsRef<Path>,
        opts: CleanOpts,
    ) -> Result<CleanedAudio> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("读清洗输入文件: {}", path.display()))?;
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.bin")
            .to_string();
        let mime = mime_from_path(path);
        self.clean_bytes(bytes, file_name, mime, opts).await
    }

    /// 直接用字节缓冲清洗。`file_name`/`mime` 仅作 multipart 元数据；服务端靠 ffmpeg 自识别。
    pub async fn clean_bytes(
        &self,
        bytes: Vec<u8>,
        file_name: impl Into<String>,
        mime: impl AsRef<str>,
        opts: CleanOpts,
    ) -> Result<CleanedAudio> {
        let part = Part::bytes(bytes)
            .file_name(file_name.into())
            .mime_str(mime.as_ref())
            .context("构造 multipart audio part")?;
        let mut form = Form::new()
            .part("audio", part)
            .text("separate", if opts.separate { "1" } else { "0" })
            .text("denoise", if opts.denoise { "1" } else { "0" })
            .text("pause", opts.pause.as_str())
            .text("level", opts.level.as_str())
            .text("sr", opts.sr.to_string())
            .text("format", opts.format.as_str());
        form = match opts.loudness {
            Some(lufs) => form.text("loudness", lufs.to_string()),
            None => form.text("loudness", "off"),
        };

        let url = format!("{}/clean", self.clean_base_url);
        let resp = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("调 audio-cleanup /clean ({url})"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "audio-cleanup /clean {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let stages = header_str(&resp, "X-Cleanup-Stages")
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default();
        let in_lufs = header_f32(&resp, "X-Cleanup-In-LUFS");
        let out_lufs = header_f32(&resp, "X-Cleanup-Out-LUFS");
        let bytes = resp
            .bytes()
            .await
            .context("读 /clean 响应音频 body")?
            .to_vec();
        Ok(CleanedAudio {
            bytes,
            stages,
            in_lufs,
            out_lufs,
        })
    }
}

/// trim 尾随 `/` 与误带的 `/clean` 段。幂等：`http://x:8097/clean/` → `http://x:8097`。
fn trim_base(base: &mut String) {
    loop {
        let before = base.len();
        while base.ends_with('/') {
            base.pop();
        }
        if let Some(stripped) = base.strip_suffix("/clean") {
            *base = stripped.to_string();
        }
        if base.len() == before {
            break;
        }
    }
}

fn header_str(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn header_f32(resp: &reqwest::Response, name: &str) -> f32 {
    header_str(resp, name)
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(f32::NAN)
}

fn mime_from_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4a") => "video/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("ogg" | "opus") => "audio/ogg",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_strips_trailing_slash() {
        assert_eq!(
            AudioCleanClient::new("http://x:8097/").base(),
            "http://x:8097"
        );
        assert_eq!(
            AudioCleanClient::new("http://x:8097///").base(),
            "http://x:8097"
        );
    }

    #[test]
    fn base_strips_misplaced_clean_suffix() {
        // 调用方误传 .../clean 不会拼成 /clean/clean。
        assert_eq!(
            AudioCleanClient::new("http://x:8097/clean").base(),
            "http://x:8097"
        );
        assert_eq!(
            AudioCleanClient::new("http://x:8097/clean/").base(),
            "http://x:8097"
        );
    }

    #[test]
    fn opts_default_matches_server() {
        let o = CleanOpts::default();
        assert!(o.denoise);
        assert!(!o.separate);
        assert_eq!(o.pause, PauseMode::Duck);
        assert_eq!(o.level, Level::Balanced);
        assert_eq!(o.sr, 48000);
        assert_eq!(o.loudness, Some(-16.0));
        assert_eq!(o.format, AudioFormat::Wav);
    }

    #[test]
    fn enum_strings() {
        assert_eq!(PauseMode::Drop.as_str(), "drop");
        assert_eq!(PauseMode::Off.as_str(), "off");
        assert_eq!(Level::Gentle.as_str(), "gentle");
        assert_eq!(AudioFormat::Flac.as_str(), "flac");
    }

    #[test]
    fn mime_lookup() {
        assert_eq!(mime_from_path(Path::new("a.MP4")), "video/mp4");
        assert_eq!(mime_from_path(Path::new("a.wav")), "audio/wav");
        assert_eq!(
            mime_from_path(Path::new("noext")),
            "application/octet-stream"
        );
    }
}
