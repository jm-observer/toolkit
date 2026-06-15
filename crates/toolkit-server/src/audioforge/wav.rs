//! 极简 WAV 头解析：从 PCM WAV bytes 推算时长（秒）。
//!
//! 只为「打包学习包草稿」时给每句填 `duration` 字段，**不做完整音频解码**——
//! 读 RIFF/WAVE 容器里的 `fmt ` 与 `data` chunk：
//!   duration = data_chunk_bytes / (sample_rate * channels * bits_per_sample/8)
//!
//! 解析失败（非 WAV / 头缺失）返回 `None`，调用方把 duration 记为 null，不阻塞打包。

/// 从 WAV bytes 解析时长（秒，保留 3 位）。非法 / 非 WAV 返回 None。
pub fn wav_duration_secs(bytes: &[u8]) -> Option<f64> {
    // 最小长度：12(RIFF 头) + 8(chunk 头) 。
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12usize;
    let mut byte_rate: Option<u32> = None;
    let mut data_len: Option<u32> = None;

    // 顺序扫描 chunk：拿 fmt 的 byte_rate + data 的长度即可算时长。
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;

        if id == b"fmt " && body + 16 <= bytes.len() {
            // byte_rate 位于 fmt body 偏移 8（= sample_rate * channels * bits/8）。
            byte_rate = Some(u32::from_le_bytes([
                bytes[body + 8],
                bytes[body + 9],
                bytes[body + 10],
                bytes[body + 11],
            ]));
        } else if id == b"data" {
            // data 声明的 size 可能超过实际剩余（流式写入未回填），取两者较小。
            let avail = (bytes.len() - body) as u32;
            data_len = Some(size.min(avail as usize) as u32);
            // data 通常是最后一个有用 chunk，可提前结束。
            break;
        }

        // chunk 按偶数字节对齐。
        pos = body + size + (size & 1);
    }

    match (byte_rate, data_len) {
        (Some(br), Some(dl)) if br > 0 => {
            let secs = dl as f64 / br as f64;
            Some((secs * 1000.0).round() / 1000.0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小合法 PCM WAV：16-bit 单声道，给定采样率与样本数。
    fn make_wav(sample_rate: u32, samples: u32) -> Vec<u8> {
        let channels: u16 = 1;
        let bits: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * (bits / 8) as u32;
        let data_len = samples * channels as u32 * (bits / 8) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&byte_rate.to_le_bytes());
        v.extend_from_slice(&(channels * bits / 8).to_le_bytes()); // block align
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data_len as usize));
        v
    }

    #[test]
    fn one_second_16k_mono() {
        // 16000 样本 @ 16kHz = 1.0s。
        let w = make_wav(16000, 16000);
        assert_eq!(wav_duration_secs(&w), Some(1.0));
    }

    #[test]
    fn half_second_24k_mono() {
        let w = make_wav(24000, 12000);
        assert_eq!(wav_duration_secs(&w), Some(0.5));
    }

    #[test]
    fn non_wav_is_none() {
        assert_eq!(wav_duration_secs(b"not a wav at all...."), None);
        assert_eq!(wav_duration_secs(&[]), None);
    }
}
