//! AudioForge（Phase 3，流 B）：文本句子清单 → 逐句 TTS → 打包「学习包草稿」。
//!
//! - [`forge`]：`audio_forge` TaskKind（逐句调上游 TTS、落 wav、写 manifest）。
//! - [`manifest`]：学习包 manifest 数据模型 + workspace 路径解析。
//! - [`tts`]：任务内的上游 TTS 客户端（带重试 / trace 子 span）。
//! - [`wav`]：极简 WAV 头解析（推算时长）。
//! - [`routes`]：`POST /api/web/audio/forge` 提交 + `GET .../forge/{id}/{file}` 下载。
//!
//! 产物供 english `package.import` 拉取消费，全程零人工传文件。

pub mod forge;
pub mod manifest;
pub mod routes;
pub mod tts;
pub mod wav;

use toolkit_tasks::Registry;

/// 注册 AudioForge 的 TaskKind。
pub fn register_all(reg: &mut Registry) {
    reg.register::<forge::AudioForge>();
}
