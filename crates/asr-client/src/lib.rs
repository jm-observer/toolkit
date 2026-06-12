//! `asr-client` —— streaming-speech 仓 FunASR `/transcribe` 端点的 Rust 客户端。
//!
//! 端点契约权威源：`streaming-speech/docs/asr-transcribe-api.md`。toolkit 仓任何
//! 消费 ASR 的 crate（当前是 `douyin`，未来可能是英语流水线 / web 控制台等）
//! 都应通过本 crate 调用 FunASR，而不是自行拼 multipart——把上传/解析/错误归类
//! 集中在一处，端点契约变更时只改这里。
//!
//! ## 用法
//!
//! ```no_run
//! use asr_client::{AsrClient, TranscribeOpts};
//! # async fn _x() -> anyhow::Result<()> {
//! let client = AsrClient::new("http://127.0.0.1:9101");
//! let bytes = tokio::fs::read("clip.mp4").await?;
//! let out = client
//!     .transcribe_bytes(bytes, "clip.mp4", "video/mp4", TranscribeOpts::default())
//!     .await?;
//! println!("model={} segs={} text={}", out.model, out.segments.len(), out.text);
//! # Ok(()) }
//! ```
//!
//! `transcribe_path` 是上面的便捷封装,直接传文件路径,内部读盘 + 推 MIME。

use anyhow::{bail, Context, Result};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// 默认 base URL：同机 FunASR 容器经 `127.0.0.1:9101:9101` 端口映射对外暴露。
/// `streaming-speech/server/compose.yaml` 与 `server/asr/app.py` 共同决定。
pub const DEFAULT_BASE: &str = "http://127.0.0.1:9101";

/// 文件读盘 + 上传 + 推理的总超时。FunASR 单机串行,1 分钟音频通常 3-8 s,
/// 但叠加排队可能更长 —— 5 分钟兜底,远超正常值。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// 单次 `/transcribe` 请求的可选参数。`Default` 是「VAD 切段」—— 与服务端默认对齐。
#[derive(Debug, Clone, Serialize)]
pub struct TranscribeOpts {
    /// `true` → 服务端跑 FSMN-VAD,返回带时间戳的 segments。
    /// `false` → 全段一锤识别,`segments` 为空。
    pub vad: bool,
}

impl Default for TranscribeOpts {
    fn default() -> Self {
        Self { vad: true }
    }
}

/// `/transcribe` 成功响应的强类型映射。字段语义见
/// `streaming-speech/docs/asr-transcribe-api.md`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transcription {
    /// 全文（`vad=true` 时是各段用 `\n` 拼接；`vad=false` 时是单次识别原文）。
    pub text: String,
    /// VAD 切段结果。`vad=false` **恒为空数组**（服务端契约）。
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,
    /// 实际产出本次结果的模型名（`paraformer` / `sensevoice` / `whisper-turbo` /
    /// `whisper-large-v3`）。落档 `asr_model` 字段时应使用此值。
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptionSegment {
    /// 段在原音频中的开始秒数。
    pub t_start: f64,
    /// 段结束秒数。
    pub t_end: f64,
    /// 段文本（已剥离 SenseVoice 等模型的 `<|lang|>` meta token）。
    pub text: String,
}

/// FunASR `/transcribe` 客户端。`Clone` 廉价（内部是 `reqwest::Client` 的 Arc 句柄）。
#[derive(Debug, Clone)]
pub struct AsrClient {
    http: reqwest::Client,
    base: String,
}

impl AsrClient {
    /// 用 base URL（如 `http://127.0.0.1:9101`，不带尾斜杠）构造一个新客户端。
    /// 内部 `reqwest::Client` 用默认 timeout（300 s）。
    pub fn new(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("reqwest client build");
        Self::with_client(http, base)
    }

    /// 复用调用方已有的 `reqwest::Client`（自定义代理 / 连接池 / TLS 配置时使用）。
    pub fn with_client(http: reqwest::Client, base: impl Into<String>) -> Self {
        let mut base = base.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self { http, base }
    }

    /// 当前 base URL（去尾斜杠后的形态）。日志/诊断用。
    pub fn base(&self) -> &str {
        &self.base
    }

    /// 用文件路径转写。读盘 + 推 MIME 在内部做，调用方只关心 path/opts。
    /// MIME 用 `.mp4` → `video/mp4`、`.mp3` → `audio/mpeg`、其它 → `application/octet-stream`
    /// 作为粗糙兜底；服务端走 ffmpeg 自识别容器，MIME 仅作日志参考。
    pub async fn transcribe_path(
        &self,
        path: impl AsRef<Path>,
        opts: TranscribeOpts,
    ) -> Result<Transcription> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("读 ASR 输入文件: {}", path.display()))?;
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.bin")
            .to_string();
        let mime = mime_from_path(path);
        self.transcribe_bytes(bytes, file_name, mime, opts).await
    }

    /// 直接用字节缓冲转写。`file_name` 和 `mime` 仅作 multipart 元数据 +
    /// 服务端日志；服务端真正解码靠 ffmpeg 自识别。
    pub async fn transcribe_bytes(
        &self,
        bytes: Vec<u8>,
        file_name: impl Into<String>,
        mime: impl AsRef<str>,
        opts: TranscribeOpts,
    ) -> Result<Transcription> {
        let part = Part::bytes(bytes)
            .file_name(file_name.into())
            .mime_str(mime.as_ref())
            .context("构造 multipart audio part")?;
        let form = Form::new()
            .part("audio", part)
            .text("vad", if opts.vad { "1" } else { "0" });

        let url = format!("{}/transcribe", self.base);
        let resp = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("调 FunASR /transcribe ({url})"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .context("读 /transcribe 响应 body")?;
        if !status.is_success() {
            bail!(
                "FunASR /transcribe {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let parsed: Transcription = serde_json::from_str(&body).with_context(|| {
            format!(
                "解析 /transcribe 响应失败,前 200 字符: {}",
                body.chars().take(200).collect::<String>()
            )
        })?;
        Ok(parsed)
    }
}

fn mime_from_path(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()).map(str::to_ascii_lowercase).as_deref() {
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
    fn transcription_parses_with_segments() {
        let v: Transcription = serde_json::from_str(
            r#"{"text":"a\nb","model":"paraformer",
                "segments":[{"t_start":0.0,"t_end":1.0,"text":"a"},
                            {"t_start":1.2,"t_end":2.0,"text":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(v.model, "paraformer");
        assert_eq!(v.segments.len(), 2);
        assert_eq!(v.segments[1].t_start, 1.2);
        assert_eq!(v.text, "a\nb");
    }

    #[test]
    fn transcription_parses_without_segments() {
        let v: Transcription =
            serde_json::from_str(r#"{"text":"hello","model":"sensevoice"}"#).unwrap();
        assert!(v.segments.is_empty());
        assert_eq!(v.model, "sensevoice");
    }

    #[test]
    fn base_strips_trailing_slash() {
        let c = AsrClient::new("http://x:9101/");
        assert_eq!(c.base(), "http://x:9101");
        let c = AsrClient::new("http://x:9101///");
        assert_eq!(c.base(), "http://x:9101");
    }

    #[test]
    fn mime_lookup() {
        assert_eq!(mime_from_path(Path::new("a.MP4")), "video/mp4");
        assert_eq!(mime_from_path(Path::new("a.wav")), "audio/wav");
        assert_eq!(mime_from_path(Path::new("a.unknown")), "application/octet-stream");
        assert_eq!(mime_from_path(Path::new("noext")), "application/octet-stream");
    }
}
