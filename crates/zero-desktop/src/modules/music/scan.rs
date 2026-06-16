//! 递归扫描音乐文件夹 + lofty 元数据/封面提取。
//!
//! 对外暴露 [`scan_dir`]：递归遍历给定目录，对每个支持的音频文件用 lofty 取
//! title/artist/album/duration，并把内嵌封面落盘到 `music/covers/<sha256>.<ext>`
//! （内容哈希命名，天然去重 + 幂等）。返回 [`Track`] 列表。
//!
//! 封面落 workspace（asset scope 已含 `$LOCALDATA/zero-desktop/**`），前端用
//! `convertFileSrc` 走 asset 协议显示图片——只有**音频**不走浏览器，图片照常走。

use std::path::{Path, PathBuf};

use anyhow::Result;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture};
use lofty::prelude::Accessor;
use lofty::probe::Probe;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::types::Track;

/// 支持扫描的音频扩展名（小写）。`.opus` 列入扫描（可见元数据），但播放时
/// symphonia 0.6 无 opus 解码器，会在播放阶段优雅报 `music_error`。
const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac"];

/// 判断路径是否是受支持的音频文件（按扩展名）。
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 递归收集目录下所有音频文件路径（深度优先，符号链接不跟随以避免环）。
fn collect_audio_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(target: "music", "读取目录失败 {}: {}", dir.display(), e);
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            collect_audio_files(&path, out);
        } else if file_type.is_file() && is_audio_file(&path) {
            out.push(path);
        }
    }
}

/// 把封面图片字节按内容哈希落盘到 `covers_dir`，返回落盘绝对路径。幂等（已存在则跳过写）。
fn dump_cover(pic: &Picture, covers_dir: &Path) -> Option<PathBuf> {
    let data = pic.data();
    if data.is_empty() {
        return None;
    }
    let ext = match pic.mime_type() {
        Some(MimeType::Png) => "png",
        Some(MimeType::Jpeg) => "jpg",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Bmp) => "bmp",
        Some(MimeType::Tiff) => "tiff",
        _ => "img",
    };
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hex::encode(hasher.finalize());
    let out = covers_dir.join(format!("{hash}.{ext}"));
    if !out.exists() {
        if let Err(e) = std::fs::write(&out, data) {
            warn!(target: "music", "写封面失败 {}: {}", out.display(), e);
            return None;
        }
    }
    Some(out)
}

/// 文件名 stem 作为标题兜底。
fn stem_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 读取单个文件的元数据，构造 [`Track`]。失败（无法解析标签）时退化为仅含路径/文件名的 Track。
fn read_track(path: &Path, covers_dir: &Path) -> Track {
    let tagged = Probe::open(path).ok().and_then(|p| p.read().ok());

    let (title, artist, album, cover_path) = match &tagged {
        Some(tf) => {
            let tag = tf.primary_tag().or_else(|| tf.first_tag());
            let title = tag
                .and_then(|t| t.title())
                .map(|c| c.to_string())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| stem_title(path));
            let artist = tag
                .and_then(|t| t.artist())
                .map(|c| c.to_string())
                .unwrap_or_default();
            let album = tag
                .and_then(|t| t.album())
                .map(|c| c.to_string())
                .unwrap_or_default();
            let cover = tag
                .and_then(|t| t.pictures().first())
                .and_then(|pic| dump_cover(pic, covers_dir))
                .map(|p| p.to_string_lossy().into_owned());
            (title, artist, album, cover)
        }
        None => (stem_title(path), String::new(), String::new(), None),
    };

    let duration_secs = tagged
        .as_ref()
        .map(|tf| tf.properties().duration().as_secs_f64())
        .unwrap_or(0.0);

    Track {
        path: path.to_string_lossy().into_owned(),
        title,
        artist,
        album,
        duration_secs,
        cover_path,
    }
}

/// 读取单文件元数据（供引擎懒补当前曲信息复用）。
pub fn read_track_pub(path: &Path, covers_dir: &Path) -> Track {
    read_track(path, covers_dir)
}

/// 递归扫描 `dir`，封面落 `covers_dir`，返回曲目列表（按路径排序，稳定顺序）。
pub fn scan_dir(dir: &Path, covers_dir: &Path) -> Result<Vec<Track>> {
    std::fs::create_dir_all(covers_dir).ok();
    let mut files = Vec::new();
    collect_audio_files(dir, &mut files);
    files.sort();
    debug!(target: "music", "扫描 {} 命中 {} 个音频文件", dir.display(), files.len());

    let tracks = files
        .iter()
        .map(|p| read_track(p, covers_dir))
        .collect::<Vec<_>>();
    Ok(tracks)
}
