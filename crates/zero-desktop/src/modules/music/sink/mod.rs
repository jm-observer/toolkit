//! 音频输出抽象 [`AudioSink`] + 两个实现的选择器。
//!
//! sink 接收**交织 f32** 帧（解码线程经 rtrb 传来），按平台/协商把样本送达设备：
//! - Windows：首选 [`wasapi::WasapiExclusiveSink`]（独占 + event-driven，bit-perfect），
//!   协商失败回退 [`cpal::CpalSink`]（共享）。
//! - 非 Windows：直接 [`cpal::CpalSink`]（CoreAudio/ALSA 共享）。
//!
//! sink 内部各自起一条**输出线程**：从一个 [`rtrb::Consumer<f32>`] 拉数据写设备，回调/写循环内
//! **零分配、零锁**（只 `pop`/`read_chunk` 与原子读）。本 trait 只暴露生命周期控制；样本通道在
//! 构造时交给 sink。

use anyhow::Result;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use super::types::{ActualFormat, AudioFormat, OutputMode};

pub mod cpal;
#[cfg(windows)]
pub mod wasapi;

/// sink 与引擎共享的实时计数器（输出回调累加已消费帧数 → 引擎据此算播放位置）。
/// 用 `AtomicU32` 存「自本曲起已输出的帧数」；引擎每曲重置。
pub type FramesPlayed = Arc<AtomicU32>;

/// 软件音量（0..=1，定点放大 1<<16）。独占 bit-perfect 时引擎置 1.0 旁路（输出回调仍乘，但 ×1 无损）。
pub type VolumeQ16 = Arc<AtomicU32>;

/// 音频输出后端：拿到源格式后协商设备，起输出线程/回调消费 rtrb 中的交织 f32。
///
/// 注意：cpal 的 `Stream` 在部分平台 `!Send`，故本 trait **不要求 `Send`**——
/// `SinkHandle` 自始至终只在引擎专用线程上构造与持有，从不跨线程移动。
pub trait AudioSink {
    /// 实际协商生效的格式（采样率/位深/独占/是否重采样），构造后即固定，供引擎回报 UI。
    /// （引擎当前从 [`SinkHandle::format`] 直接取，此方法保留为 §5.4 trait 完整契约。）
    #[allow(dead_code)]
    fn actual_format(&self) -> ActualFormat;

    /// 暂停输出（设备保持打开，停止消费缓冲）。
    fn pause(&mut self);

    /// 恢复输出。
    fn resume(&mut self);
}

/// 一个已建好的输出链路：sink + 把解码帧写进去的生产端。
pub struct SinkHandle {
    pub sink: Box<dyn AudioSink>,
    /// 解码线程把交织 f32 写这里（实时安全 SPSC）。
    pub producer: rtrb::Producer<f32>,
    /// sink 实际生效格式。
    pub format: ActualFormat,
}

/// 按平台与源格式建立输出链路。`buffer_frames` 是 rtrb 容量（帧），取较大值吸收解码抖动。
///
/// `mode=Auto`：Windows 优先尝试独占，失败回退共享 cpal。`mode=Shared`：跳过独占，直接走
/// 共享 cpal（设备率 + 引擎重采样；兼容独占 44.1k 异常的设备）。非 Windows 恒走 cpal。
pub fn build_sink(
    fmt: AudioFormat,
    frames_played: FramesPlayed,
    volume: VolumeQ16,
    buffer_frames: usize,
    mode: OutputMode,
) -> Result<SinkHandle> {
    let capacity = buffer_frames * fmt.channels.max(1) as usize;

    #[cfg(windows)]
    if mode == OutputMode::Auto {
        match wasapi::WasapiExclusiveSink::try_build(
            fmt,
            frames_played.clone(),
            volume.clone(),
            capacity,
        ) {
            Ok(handle) => return Ok(handle),
            Err(e) => {
                tracing::warn!(target: "music", "WASAPI 独占协商失败，回退共享模式: {e:#}");
            }
        }
    }

    cpal::CpalSink::build(fmt, frames_played, volume, capacity)
}
