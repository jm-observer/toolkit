//! 跨平台共享模式输出（cpal）。Win=WASAPI 共享 / mac=CoreAudio / Linux=ALSA。
//!
//! 作为 WASAPI 独占协商失败或非 Windows 平台的兜底。输出回调从 rtrb 拉交织 f32，
//! 回调内**零分配、零锁**：只 `pop` 样本、乘定点音量、累加已播帧原子量。
//!
//! cpal 共享模式跟随设备默认采样率；若设备默认率 ≠ 源采样率，本 sink 仍按设备率开流，
//! **重采样由上层引擎（rubato）在写入前完成**（引擎据 `actual_format().sample_rate` 决定是否重采样）。

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tracing::info;

use super::super::types::{ActualFormat, AudioFormat};
use super::{AudioSink, FramesPlayed, SinkHandle, VolumeQ16};

/// cpal 共享模式 sink，持有 `Stream`（drop 即停流）。
pub struct CpalSink {
    _stream: cpal::Stream,
    /// 实际生效格式（经 `actual_format()` 暴露，属 §5.4 trait 契约）。
    #[allow(dead_code)]
    actual: ActualFormat,
    paused: bool,
}

impl CpalSink {
    /// 建立共享输出流。`fmt` 为源格式；实际输出采样率取设备默认配置的采样率
    /// （若与源不同，`resampled=true`，引擎负责重采样到该率）。
    pub fn build(
        fmt: AudioFormat,
        frames_played: FramesPlayed,
        volume: VolumeQ16,
        capacity: usize,
    ) -> Result<SinkHandle> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("找不到默认音频输出设备"))?;

        let supported = device
            .default_output_config()
            .context("获取默认输出配置失败")?;
        let sample_format = supported.sample_format();
        let mut config: cpal::StreamConfig = supported.config();
        // 跟随源声道数（若设备支持）；多数情况下立体声。设备默认通道可能与源不同，
        // 引擎会按 actual 通道数交织，这里保持设备默认通道，引擎做声道适配。
        let dev_channels = config.channels as usize;
        let dev_rate = config.sample_rate.0;
        // 用尽量小的缓冲降低延迟；若设备不接受 Fixed 则回退默认。
        config.buffer_size = cpal::BufferSize::Default;

        let resampled = dev_rate != fmt.sample_rate;
        info!(
            target: "music",
            "cpal 共享输出: 设备率={dev_rate} 源率={} 通道={dev_channels} 格式={:?} 重采样={resampled}",
            fmt.sample_rate, sample_format
        );

        let (producer, mut consumer) =
            rtrb::RingBuffer::<f32>::new(capacity.max(dev_channels * 1024));

        let err_fn = |err| tracing::error!(target: "music", "cpal 输出流错误: {err}");
        let frames_atomic = frames_played;
        let vol_atomic = volume;
        let channels_for_cb = dev_channels.max(1);

        // 回调：拉交织 f32，乘音量写出；缺数据补静音（欠载）。零分配零锁。
        macro_rules! build_typed {
            ($t:ty, $convert:expr) => {{
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [$t], _: &cpal::OutputCallbackInfo| {
                            fill_callback::<$t>(
                                out,
                                &mut consumer,
                                &vol_atomic,
                                &frames_atomic,
                                channels_for_cb,
                                $convert,
                            );
                        },
                        err_fn,
                        None,
                    )
                    .context("建立 cpal 输出流失败")?
            }};
        }

        let stream = match sample_format {
            SampleFormat::F32 => build_typed!(f32, |s: f32| s),
            SampleFormat::I16 => {
                build_typed!(i16, |s: f32| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            }
            SampleFormat::U16 => build_typed!(u16, |s: f32| {
                ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
            }),
            other => return Err(anyhow!("设备不支持的样本格式: {other:?}")),
        };

        stream.play().context("启动 cpal 输出流失败")?;

        let actual = ActualFormat {
            sample_rate: dev_rate,
            bits: bits_for_format(sample_format),
            channels: dev_channels as u16,
            exclusive: false,
            resampled,
        };

        Ok(SinkHandle {
            sink: Box::new(CpalSink {
                _stream: stream,
                actual,
                paused: false,
            }),
            producer,
            format: actual,
        })
    }
}

impl AudioSink for CpalSink {
    fn actual_format(&self) -> ActualFormat {
        self.actual
    }

    fn pause(&mut self) {
        if !self.paused {
            let _ = self._stream.pause();
            self.paused = true;
        }
    }

    fn resume(&mut self) {
        if self.paused {
            let _ = self._stream.play();
            self.paused = false;
        }
    }
}

/// 设备样本格式对应的位深（回报 UI）。
fn bits_for_format(fmt: SampleFormat) -> u32 {
    match fmt {
        SampleFormat::F32 => 32,
        SampleFormat::I16 | SampleFormat::U16 => 16,
        _ => 32,
    }
}

/// 实时回调主体：从 rtrb 拉交织 f32 → 乘音量 → 转目标样本类型写 `out`；欠载补静音。
/// **零分配、零锁**：仅 `pop`、原子读写、算术。
#[inline]
fn fill_callback<T>(
    out: &mut [T],
    consumer: &mut rtrb::Consumer<f32>,
    volume: &AtomicU32,
    frames_played: &AtomicU32,
    channels: usize,
    convert: impl Fn(f32) -> T,
) where
    T: Copy,
{
    // 定点音量：volume 存为 (vol * 65536) 的整数。
    let vol = volume.load(Ordering::Relaxed) as f32 / 65536.0;
    let ch = channels.max(1);
    let n = out.len();
    let mut i = 0usize;
    let mut produced_frames = 0usize;
    // 按帧原子消费：缓冲不足一整帧时该帧写静音，避免在一帧的多声道中途欠载导致 L/R 永久错位。
    while i + ch <= n {
        if consumer.slots() >= ch {
            for _ in 0..ch {
                let s = consumer.pop().unwrap_or(0.0);
                out[i] = convert(s * vol);
                i += 1;
            }
            produced_frames += 1;
        } else {
            for _ in 0..ch {
                out[i] = convert(0.0);
                i += 1;
            }
        }
    }
    // 尾部不足一帧的零头（通常 out 帧对齐，不会触发）补静音。
    while i < n {
        out[i] = convert(0.0);
        i += 1;
    }
    if produced_frames > 0 {
        frames_played.fetch_add(produced_frames as u32, Ordering::Relaxed);
    }
}
