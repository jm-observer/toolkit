//! VAD 切段（Plan B）。
//!
//! 移植自 `src-tauri/src/commands/recording.rs`（瘦客户端重构前的 silero_vad
//! 循环逻辑），不直接共享 src-tauri 的代码——workspace 两 crate 故意独立。
//! 这里只保留「整段 PCM → 语音段」这一步，去掉麦克风/说话人门控等客户端逻辑。

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

pub const SAMPLE_RATE: i32 = 16000;
const WINDOW_SIZE: usize = 512;

/// 一段 VAD 切出的语音：时间戳（秒）+ 16k mono PCM。
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub samples: Vec<f32>,
}

/// 把整段 16k mono PCM 用 silero_vad 切成语音段。
///
/// `max_speech_duration` 设得足够大（不在 VAD 层硬切长段），长段的 30s 拆分
/// 交给上层 whisper 子切窗逻辑处理，从而 1 段连续语音仍返回 1 个 segment。
pub fn segment(model_path: &str, samples: &[f32]) -> anyhow::Result<Vec<Segment>> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model_path.to_string()),
            threshold: 0.5,
            min_silence_duration: 0.3,
            min_speech_duration: 0.2,
            window_size: WINDOW_SIZE as i32,
            max_speech_duration: 100.0,
        },
        sample_rate: SAMPLE_RATE,
        num_threads: 1,
        ..Default::default()
    };

    let vad = VoiceActivityDetector::create(&config, 120.0)
        .ok_or_else(|| anyhow::anyhow!("failed to create VAD (silero_vad model: {model_path})"))?;

    let mut out = Vec::new();
    let mut i = 0;
    while i + WINDOW_SIZE <= samples.len() {
        vad.accept_waveform(&samples[i..i + WINDOW_SIZE]);
        i += WINDOW_SIZE;
        drain(&vad, &mut out);
    }
    // 尾部不足一窗：补零凑满一窗喂进去，再 flush。
    if i < samples.len() {
        let mut tail = samples[i..].to_vec();
        tail.resize(WINDOW_SIZE, 0.0);
        vad.accept_waveform(&tail);
    }
    vad.flush();
    drain(&vad, &mut out);
    Ok(out)
}

fn drain(vad: &VoiceActivityDetector, out: &mut Vec<Segment>) {
    while let Some(seg) = vad.front() {
        let start = seg.start() as f64 / SAMPLE_RATE as f64;
        let samples = seg.samples().to_vec();
        let end = start + samples.len() as f64 / SAMPLE_RATE as f64;
        out.push(Segment {
            start,
            end,
            samples,
        });
        vad.pop();
    }
}
