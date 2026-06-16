//! Symphonia 0.6 解码包装：打开文件 → 探测容器/编解码 → 逐包解码为**交织 f32 帧**。
//!
//! 对外暴露 [`Decoder`]：`open` 建立解码器并报告源 [`AudioFormat`]；`next_frames` 拉下一批
//! 交织 f32 样本（解码线程调用，写入 rtrb）；`seek` 跳到目标秒并清解码器内部状态。
//!
//! 设计取向：保持「拉模型」——引擎解码线程主动 `next_frames`，把帧塞进无锁环形缓冲，
//! 输出回调只消费缓冲。symphonia 0.6 的音频 API（`GenericAudioBufferRef::copy_to_vec_interleaved`）
//! 直接给我们交织 f32，无需自己做 planar→interleaved。

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

use super::types::AudioFormat;

/// 单文件解码器，封装 format reader + audio decoder + 选中音轨。
pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    /// 时基（timestamp → 秒），用于把包的 pts 转成播放位置。
    time_base: Option<symphonia::core::units::TimeBase>,
    /// 源音频格式（采样率/声道/位深）。
    fmt: AudioFormat,
    /// 交织 f32 临时缓冲，复用以减少分配（仍在解码线程，非实时回调，分配可接受）。
    scratch: Vec<f32>,
}

/// 一批解码结果：交织 f32 帧 + 该批起始时间戳对应的秒数（用于进度基准）。
pub struct DecodedChunk {
    pub samples: Vec<f32>,
    /// 这批数据起点的播放位置（秒）。引擎当前以输出帧计数算位置，此字段保留供诊断/未来对齐。
    #[allow(dead_code)]
    pub timestamp_secs: f64,
}

impl Decoder {
    /// 打开音频文件，探测容器与编解码器，准备好默认音轨的解码器。
    ///
    /// `.opus` 等 symphonia 0.6 未含解码器的格式会在此返回错误（上层转 `music_error` 优雅提示）。
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("打开音频文件失败: {}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .with_context(|| {
                format!(
                    "探测音频格式失败（可能是不支持的编码，如 .opus）: {}",
                    path.display()
                )
            })?;

        let track = format
            .default_track(TrackType::Audio)
            .or_else(|| format.tracks().iter().find(|t| t.codec_params.is_some()))
            .ok_or_else(|| anyhow!("文件中找不到可解码的音轨: {}", path.display()))?;
        let track_id = track.id;
        let time_base = track.time_base;

        let codec_params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| anyhow!("音轨缺少音频编解码参数: {}", path.display()))?
            .clone();

        let sample_rate = codec_params.sample_rate.unwrap_or(44_100);
        let channels = codec_params
            .channels
            .as_ref()
            .map(|c| c.count() as u16)
            .filter(|&c| c > 0)
            .unwrap_or(2);
        let bits = codec_params.bits_per_sample.unwrap_or(16);

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
            .with_context(|| format!("不支持的音频编解码器: {}", path.display()))?;

        Ok(Self {
            format,
            decoder,
            track_id,
            time_base,
            fmt: AudioFormat {
                sample_rate,
                channels,
                bits,
            },
            scratch: Vec::new(),
        })
    }

    /// 源音频格式（采样率/声道/位深）。
    pub fn format(&self) -> AudioFormat {
        self.fmt
    }

    /// 把一个时间戳（timebase 单位）换算成秒。
    fn ts_to_secs(&self, ts: Timestamp) -> f64 {
        match self.time_base.and_then(|tb| tb.calc_time(ts)) {
            Some(time) => time.as_secs_f64(),
            None => ts.get().max(0) as f64 / self.fmt.sample_rate.max(1) as f64,
        }
    }

    /// 拉取下一批交织 f32 帧。返回：
    /// - `Ok(Some(chunk))`：解出一批数据。
    /// - `Ok(None)`：到达文件末尾（end-of-stream）。
    /// - `Err(_)`：解码错误（解码方丢包可被上层选择跳过；此处直接上抛）。
    pub fn next_frames(&mut self) -> Result<Option<DecodedChunk>> {
        loop {
            let packet = match self.format.next_packet()? {
                Some(p) => p,
                None => return Ok(None),
            };
            // 跳过非目标音轨的包。
            if packet.track_id != self.track_id {
                continue;
            }
            let ts_secs = self.ts_to_secs(packet.pts);

            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let frames = buf.frames();
                    if frames == 0 {
                        continue;
                    }
                    self.scratch.clear();
                    copy_interleaved_f32(&buf, &mut self.scratch);
                    let samples = std::mem::take(&mut self.scratch);
                    return Ok(Some(DecodedChunk {
                        samples,
                        timestamp_secs: ts_secs,
                    }));
                }
                // 解码偶发错误（损坏帧）跳过，继续下一包；I/O / 不可恢复错误上抛。
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// 跳到目标秒，重置解码器内部状态。返回实际定位到的秒（symphonia 精确 seek 落在目标前）。
    pub fn seek(&mut self, secs: f64) -> Result<f64> {
        let time = Time::from_micros((secs.max(0.0) * 1_000_000.0) as i64);
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(self.track_id),
            },
        )?;
        self.decoder.reset();
        Ok(self.ts_to_secs(seeked.actual_ts))
    }
}

/// 把 symphonia 的 `GenericAudioBufferRef`（任意源样本类型）转成交织 f32 推入 `out`。
fn copy_interleaved_f32(buf: &GenericAudioBufferRef<'_>, out: &mut Vec<f32>) {
    buf.copy_to_vec_interleaved(out);
}
