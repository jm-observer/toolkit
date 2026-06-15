//! Speech 录音清洗（Plan 4）。
//!
//! 把本地录音经 **toolkit-server 代理**（`POST /api/web/audio/clean`）清洗后，**并列**保存为
//! cleaned variant（`<stem>.cleaned.wav`），**绝不覆盖原录音**。桌面端不直连 GB10 :8097，
//! 统一走 toolkit-server :8788 代理（与 TTS 走 `/api/web/audio/tts` 同一先例）。
//!
//! `toolkit_base` 由前端传入（zero-desktop 现无内置 toolkit-server 地址配置，避免在本 Plan
//! 擅自新增设置项）。设计见 docs/2026-06-14-audio-cleanup/audio-cleanup-plan-4.md。

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::Serialize;
use tracing::info;

const CLEAN_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Serialize)]
pub struct CleanedRecording {
    /// cleaned variant 的落盘路径（与原录音并列，原录音保留不动）。
    pub cleaned_path: String,
    pub stages: Vec<String>,
    pub in_lufs: f32,
    pub out_lufs: f32,
}

/// 由原录音路径派生 cleaned variant 的并列路径：`<stem>.cleaned.wav`（始终是同目录兄弟文件，
/// 且必不等于原路径——保证不覆盖）。输出固定 wav。
fn cleaned_variant_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let name = format!("{stem}.cleaned.wav");
    match input.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    }
}

fn header_f32(resp: &reqwest::Response, name: &str) -> f32 {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(f32::NAN)
}

/// 清洗一段本地录音并并列落盘。`toolkit_base` 如 `http://127.0.0.1:8788`。
#[tauri::command]
pub async fn speech_clean_recording(
    toolkit_base: String,
    input_path: String,
    denoise: Option<bool>,
    pause: Option<String>,
) -> Result<CleanedRecording, String> {
    let input = PathBuf::from(&input_path);
    let bytes = tokio::fs::read(&input)
        .await
        .map_err(|e| format!("读录音文件失败 {}: {e}", input.display()))?;
    let file_name = input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("recording.wav")
        .to_string();

    let denoise = denoise.unwrap_or(true);
    let pause = pause.unwrap_or_else(|| "duck".to_string());

    let part = Part::bytes(bytes)
        .file_name(file_name)
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let form = Form::new()
        .part("audio", part)
        .text("denoise", if denoise { "1" } else { "0" })
        .text("pause", pause);

    let base = toolkit_base.trim_end_matches('/');
    let url = format!("{base}/api/web/audio/clean");
    let client = reqwest::Client::builder()
        .timeout(CLEAN_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("清洗请求失败: {e}"))?;

    let status = resp.status();
    match status.as_u16() {
        503 => return Err("清洗服务未配置（toolkit-server 缺 CLEAN_BASE_URL）".to_string()),
        502 => return Err("清洗服务不可达（上游 audio-cleanup 无响应）".to_string()),
        c if !(200..300).contains(&c) => {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "清洗失败 {status}: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        _ => {}
    }

    let stages = resp
        .headers()
        .get("X-Cleanup-Stages")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let in_lufs = header_f32(&resp, "X-Cleanup-In-LUFS");
    let out_lufs = header_f32(&resp, "X-Cleanup-Out-LUFS");
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("读清洗结果失败: {e}"))?;

    let out_path = cleaned_variant_path(&input);
    // 安全闸：绝不覆盖原录音。
    if out_path == input {
        return Err("派生的 cleaned 路径与原录音相同，已中止以防覆盖".to_string());
    }
    tokio::fs::write(&out_path, &body)
        .await
        .map_err(|e| format!("落盘 cleaned 失败 {}: {e}", out_path.display()))?;
    info!(
        target: "speech",
        "[clean] {} -> {} ({} bytes)", input.display(), out_path.display(), body.len()
    );

    Ok(CleanedRecording {
        cleaned_path: out_path.to_string_lossy().to_string(),
        stages,
        in_lufs,
        out_lufs,
    })
}

// 注：前端「清洗录音」按钮 + 并列展示 + toolkit-server base 的来源（新设置项 or 复用现有
// 配置）属前端接线，未在本后端命令内实现——见 docs/2026-06-14-audio-cleanup/audio-cleanup-plan-4.md。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_is_sibling_and_never_equals_input() {
        let input = Path::new("/data/rec/abc.wav");
        let out = cleaned_variant_path(input);
        assert_eq!(out.as_path(), Path::new("/data/rec/abc.cleaned.wav"));
        assert_ne!(out.as_path(), input);
    }

    #[test]
    fn variant_handles_no_extension() {
        let out = cleaned_variant_path(Path::new("rec"));
        assert_eq!(out.as_path(), Path::new("rec.cleaned.wav"));
    }

    #[test]
    fn variant_handles_mp4_input() {
        let input = Path::new("/x/clip.mp4");
        let out = cleaned_variant_path(input);
        assert_eq!(out.as_path(), Path::new("/x/clip.cleaned.wav"));
        assert_ne!(out.as_path(), input);
    }
}
