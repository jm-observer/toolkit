//! Plan 5：逐条作品知识录入（标签聚合/筛选 + 知识包机械生成）。
//!
//! 设计要点（见 zero 仓 docs/2026-05-30-douyin-knowledge/plan-5-per-item-ingestion.md）：
//! - **完整性由代码保证**：`run_publish_knowledge` 遍历 works[] 一条写一条 md，列全 N 条由
//!   循环保证，不经任何 LLM 判断——这是 known-issues #2（漏列/省略）在结构上消失的根。
//! - **标签机械解析**：从 `desc` 抽 `#话题`（抖音把话题明文写进文案），零新依赖。
//! - **按 unique_id 稳定缓存**：worker 终态落 `works/<unique_id>.json`，与知识库目录键一致、
//!   跨 task 稳定；list_tags / filter_works / publish_knowledge 都基于它工作。
//!
//! ASR/字幕回填（has_transcript / 字幕段）依赖外部 ASR（streaming-speech），本模块预留
//! 字段与占位段，待 `process` 子命令落地后填充。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// 博主作品稳定缓存：worker 终态落盘，按 unique_id（缺失时退化用 sec_uid）命名。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorksCache {
    pub sec_uid: String,
    #[serde(default)]
    pub unique_id: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    pub aweme_count: i64,
    pub count: usize,
    pub throttled: bool,
    pub cached_at: String,
    /// 每项：aweme_id / desc / create_time / create_ym / tags[]。
    pub works: Vec<Value>,
}

fn cache_path(works_dir: &Path, id: &str) -> PathBuf {
    works_dir.join(format!("{id}.json"))
}

/// 原子写缓存（先 .tmp 再 rename，避免读到半截）。
pub fn write_cache(works_dir: &Path, id: &str, cache: &WorksCache) -> Result<()> {
    std::fs::create_dir_all(works_dir)
        .with_context(|| format!("建作品缓存目录 {}", works_dir.display()))?;
    let target = cache_path(works_dir, id);
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string(cache)?)
        .with_context(|| format!("写缓存临时文件 {}", tmp.display()))?;
    std::fs::rename(&tmp, &target).with_context(|| format!("替换作品缓存 {}", target.display()))?;
    Ok(())
}

fn read_cache(works_dir: &Path, id: &str) -> Result<Option<WorksCache>> {
    let p = cache_path(works_dir, id);
    if !p.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&p).with_context(|| format!("读作品缓存 {}", p.display()))?;
    Ok(Some(
        serde_json::from_str(&raw).context("解析作品缓存 JSON")?,
    ))
}

/// 从 desc 机械解析话题标签：收集每个 `#` 后到下一个空白 / `#` / `@` 前的子串，去重保序。
pub fn parse_tags(desc: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut chars = desc.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '#' {
            continue;
        }
        let mut tag = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_whitespace() || nc == '#' || nc == '@' {
                break;
            }
            tag.push(nc);
            chars.next();
        }
        let t = tag.trim();
        if !t.is_empty() && !tags.iter().any(|x| x == t) {
            tags.push(t.to_string());
        }
    }
    tags
}

/// 给一个 work item（含 desc）补 `tags` 字段；已有则不动。worker 与同步 list_works 共用，
/// 保证两条路径产出的 item 结构一致。
pub fn enrich_with_tags(item: &mut Value) {
    if item.get("tags").is_some() {
        return;
    }
    let desc = item.get("desc").and_then(|v| v.as_str()).unwrap_or("");
    let tags = parse_tags(desc);
    if let Some(obj) = item.as_object_mut() {
        obj.insert("tags".into(), json!(tags));
    }
}

/// 取一个 work 的标签：优先已有 tags 字段，否则现场从 desc 解析（兼容旧缓存）。
fn work_tags(w: &Value) -> Vec<String> {
    if let Some(arr) = w.get("tags").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
    }
    let desc = w.get("desc").and_then(|v| v.as_str()).unwrap_or("");
    parse_tags(desc)
}

fn not_listed(id: &str) -> Value {
    json!({
        "error": "该博主尚未列过作品（无缓存），请先 list_works_submit 拉取一次",
        "error_kind": "not_listed",
        "unique_id": id,
    })
}

/// `list_tags`：聚合某博主已拉取作品的话题标签 + 计数，按计数倒序。纯机械，不分析内容。
pub fn run_list_tags(works_dir: &Path, unique_id: &str) -> Result<Value> {
    let cache = match read_cache(works_dir, unique_id)? {
        Some(c) => c,
        None => return Ok(not_listed(unique_id)),
    };
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for w in &cache.works {
        for t in work_tags(w) {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    let mut tags: Vec<Value> = counts
        .into_iter()
        .map(|(name, count)| json!({ "name": name, "count": count }))
        .collect();
    // 计数倒序，同计数按名称稳定（BTreeMap 已按名称序进入）。
    tags.sort_by(|a, b| b["count"].as_i64().cmp(&a["count"].as_i64()));
    Ok(json!({
        "unique_id": unique_id,
        "nickname": cache.nickname,
        "total_works": cache.works.len(),
        "tags": tags,
    }))
}

/// `filter_works`：按标签筛选已拉取作品，返回匹配 aweme_ids。match_all=true 须同时含全部标签。
pub fn run_filter_works(
    works_dir: &Path,
    unique_id: &str,
    tags: &[String],
    match_all: bool,
) -> Result<Value> {
    let cache = match read_cache(works_dir, unique_id)? {
        Some(c) => c,
        None => return Ok(not_listed(unique_id)),
    };
    let want: Vec<String> = tags
        .iter()
        .map(|t| t.trim().trim_start_matches('#').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if want.is_empty() {
        return Ok(json!({ "error": "tags 为空", "error_kind": "invalid_input" }));
    }
    let mut matched: Vec<String> = Vec::new();
    for w in &cache.works {
        let wt = work_tags(w);
        let hit = if match_all {
            want.iter().all(|t| wt.iter().any(|x| x == t))
        } else {
            want.iter().any(|t| wt.iter().any(|x| x == t))
        };
        if hit {
            if let Some(id) = w.get("aweme_id").and_then(|v| v.as_str()) {
                matched.push(id.to_string());
            }
        }
    }
    Ok(json!({
        "unique_id": unique_id,
        "tags": want,
        "match": if match_all { "all" } else { "any" },
        "matched": matched.len(),
        "total": cache.works.len(),
        "aweme_ids": matched,
    }))
}

/// desc → 标题：去换行、截前 30 字符；空则「（无文案）」。
fn title_from_desc(desc: &str) -> String {
    let one_line = desc.replace(['\n', '\r'], " ");
    let trimmed = one_line.trim();
    if trimmed.is_empty() {
        return "（无文案）".to_string();
    }
    let truncated: String = trimmed.chars().take(30).collect();
    if trimmed.chars().count() > 30 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn render_transcript_md(
    aweme_id: &str,
    unique_id: &str,
    nickname: &str,
    create_ym: &str,
    tags: &[String],
    desc: &str,
    title: &str,
) -> String {
    let tags_yaml = tags
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "'")))
        .collect::<Vec<_>>()
        .join(", ");
    let desc_body = if desc.trim().is_empty() {
        "（博主未写文案）"
    } else {
        desc.trim()
    };
    format!(
        "---\n\
aweme_id: \"{aweme_id}\"\n\
unique_id: \"{unique_id}\"\n\
nickname: \"{nickname}\"\n\
create_ym: \"{create_ym}\"\n\
tags: [{tags_yaml}]\n\
has_transcript: false\n\
has_subtitle: false\n\
asr_model: null\n\
ingested_at: \"{date}\"\n\
---\n\n\
# {title}\n\n\
## 文案\n{desc_body}\n\n\
## 视频内容（ASR）\n（待转写）\n\n\
## 字幕（时间轴）\n（待转写）\n",
        date = today(),
    )
}

fn render_profile(cache: &WorksCache, unique_id: &str) -> String {
    let nickname = cache.nickname.as_deref().unwrap_or("（未知）");
    let throttle_note = if cache.throttled {
        format!(
            " ⚠️ 限流，未拉全（本次 {} / 共 {}）",
            cache.count, cache.aweme_count
        )
    } else {
        String::new()
    };
    format!(
        "---\n\
unique_id: \"{unique_id}\"\n\
sec_uid: \"{sec_uid}\"\n\
nickname: \"{nickname}\"\n\
aweme_count: {aweme_count}\n\
ingested_count: {count}\n\
throttled: {throttled}\n\
cached_at: \"{cached_at}\"\n\
---\n\n\
# 「{nickname}」博主资料\n\n\
- **抖音号**：{unique_id}\n\
- **总作品**：{aweme_count} 条\n\
- **本次录入**：{count} 条{throttle_note}\n\
- **拉取时间**：{cached_at}\n",
        sec_uid = cache.sec_uid,
        aweme_count = cache.aweme_count,
        count = cache.count,
        throttled = cache.throttled,
        cached_at = cache.cached_at,
    )
}

fn render_index(cache: &WorksCache, unique_id: &str, rows: &[String], filtered: bool) -> String {
    let nickname = cache.nickname.as_deref().unwrap_or("（未知）");
    let scope = if filtered {
        format!("按标签筛选录入 {} 条", rows.len())
    } else {
        format!("全部录入 {} 条", rows.len())
    };
    format!(
        "# 「{nickname}」作品知识索引\n\n\
> 数据来源：抖音 · 逐条机械录入（{scope}）。视频内容/字幕在各条目内随 ASR 就绪回填。\n\n\
- 抖音号：{unique_id}\n\
- 总作品：{aweme_count} 条\n\
- 本次录入：{count} 条\n\n\
## 作品清单（按时间倒序）\n\n\
{body}\n",
        aweme_count = cache.aweme_count,
        count = rows.len(),
        body = rows.join("\n"),
    )
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// `publish_knowledge`：把缓存里的作品逐条机械写入 `<knowledge_dir>/<unique_id>/`。
/// only_ids 非空时仅录入子集（标签筛选后用）。幂等：内容确定，重跑覆盖同名文件。
pub fn run_publish_knowledge(
    works_dir: &Path,
    knowledge_dir: &Path,
    unique_id: &str,
    only_ids: &[String],
) -> Result<Value> {
    let cache = match read_cache(works_dir, unique_id)? {
        Some(c) => c,
        None => return Ok(not_listed(unique_id)),
    };
    let root = knowledge_dir.join(unique_id);
    let transcripts = root.join("transcripts");
    std::fs::create_dir_all(&transcripts)
        .with_context(|| format!("建知识包目录 {}", transcripts.display()))?;

    let filter: Option<HashSet<&str>> = if only_ids.is_empty() {
        None
    } else {
        Some(only_ids.iter().map(String::as_str).collect())
    };

    let nickname = cache.nickname.as_deref().unwrap_or("");
    let mut written = 0usize;
    // 时间倒序排索引行（works 已按抓取顺序，create_ym 倒序更友好）。
    let mut rows: Vec<(String, String)> = Vec::new(); // (create_ym, row)
    for w in &cache.works {
        let id = w.get("aweme_id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if let Some(f) = &filter {
            if !f.contains(id) {
                continue;
            }
        }
        let desc = w.get("desc").and_then(|v| v.as_str()).unwrap_or("");
        let ym = w.get("create_ym").and_then(|v| v.as_str()).unwrap_or("");
        let tags = work_tags(w);
        let title = title_from_desc(desc);
        let md = render_transcript_md(id, unique_id, nickname, ym, &tags, desc, &title);
        std::fs::write(transcripts.join(format!("{id}.md")), md)
            .with_context(|| format!("写条目 {id}.md"))?;
        written += 1;
        rows.push((
            ym.to_string(),
            format!("- `{ym}` [{title}](transcripts/{id}.md) `{id}`"),
        ));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let index_rows: Vec<String> = rows.into_iter().map(|(_, r)| r).collect();

    std::fs::write(root.join("profile.md"), render_profile(&cache, unique_id))
        .context("写 profile.md")?;
    std::fs::write(
        root.join("index.md"),
        render_index(&cache, unique_id, &index_rows, filter.is_some()),
    )
    .context("写 index.md")?;

    Ok(json!({
        "unique_id": unique_id,
        "written": written,
        "with_transcript": 0,
        "with_subtitle": 0,
        "path": root.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tags_basic() {
        assert_eq!(
            parse_tags("教程来啦 #ComfyUI #SD绘画 大家学起来"),
            vec!["ComfyUI".to_string(), "SD绘画".to_string()]
        );
    }

    #[test]
    fn parse_tags_adjacent_and_at() {
        assert_eq!(
            parse_tags("#AI绘画#StableDiffusion@某人 #AI绘画"),
            vec!["AI绘画".to_string(), "StableDiffusion".to_string()]
        );
    }

    #[test]
    fn parse_tags_none() {
        assert!(parse_tags("纯文案没有标签").is_empty());
    }

    #[test]
    fn enrich_adds_tags_field() {
        let mut v = json!({"aweme_id": "1", "desc": "测试 #标签A #标签B"});
        enrich_with_tags(&mut v);
        assert_eq!(
            v.get("tags").and_then(|t| t.as_array()).map(|a| a.len()),
            Some(2)
        );
        // 已有 tags 不覆盖
        let mut v2 = json!({"desc": "#x", "tags": ["保留"]});
        enrich_with_tags(&mut v2);
        assert_eq!(v2["tags"], json!(["保留"]));
    }

    #[test]
    fn title_truncation() {
        assert_eq!(title_from_desc(""), "（无文案）");
        assert_eq!(title_from_desc("短标题"), "短标题");
        let long = "一".repeat(40);
        let t = title_from_desc(&long);
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().count(), 31); // 30 + …
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let id = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("douyin-kb-test-{}-{}", std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample_cache() -> WorksCache {
        WorksCache {
            sec_uid: "MS4wTEST".into(),
            unique_id: Some("82933463317".into()),
            nickname: Some("熊猫怪兽AI日记".into()),
            aweme_count: 3,
            count: 3,
            throttled: false,
            cached_at: "2026-05-31T00:00:00Z".into(),
            works: vec![
                json!({"aweme_id":"7a","desc":"入门 #ComfyUI #SD","create_ym":"2026-05","tags":["ComfyUI","SD"]}),
                json!({"aweme_id":"7b","desc":"进阶 #ComfyUI","create_ym":"2026-04","tags":["ComfyUI"]}),
                json!({"aweme_id":"7c","desc":"杂谈 #日常","create_ym":"2026-03","tags":["日常"]}),
            ],
        }
    }

    #[test]
    fn list_tags_counts_and_order() {
        let dir = tempdir();
        write_cache(&dir, "82933463317", &sample_cache()).unwrap();
        let v = run_list_tags(&dir, "82933463317").unwrap();
        assert_eq!(v["total_works"], 3);
        let tags = v["tags"].as_array().unwrap();
        // ComfyUI 计数 2，应排第一
        assert_eq!(tags[0]["name"], "ComfyUI");
        assert_eq!(tags[0]["count"], 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_tags_not_listed() {
        let dir = tempdir();
        let v = run_list_tags(&dir, "nope").unwrap();
        assert_eq!(v["error_kind"], "not_listed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filter_match_all_vs_any() {
        let dir = tempdir();
        write_cache(&dir, "82933463317", &sample_cache()).unwrap();
        let all =
            run_filter_works(&dir, "82933463317", &["ComfyUI".into(), "SD".into()], true).unwrap();
        assert_eq!(all["matched"], 1); // 仅 7a 同时含两者
        let any =
            run_filter_works(&dir, "82933463317", &["ComfyUI".into(), "SD".into()], false).unwrap();
        assert_eq!(any["matched"], 2); // 7a + 7b
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn publish_writes_all_items_mechanically() {
        let works = tempdir();
        let kb = tempdir();
        write_cache(&works, "82933463317", &sample_cache()).unwrap();
        let v = run_publish_knowledge(&works, &kb, "82933463317", &[]).unwrap();
        assert_eq!(v["written"], 3);
        let root = kb.join("82933463317");
        assert!(root.join("profile.md").exists());
        assert!(root.join("index.md").exists());
        assert!(root.join("transcripts/7a.md").exists());
        assert!(root.join("transcripts/7b.md").exists());
        assert!(root.join("transcripts/7c.md").exists());
        let md = std::fs::read_to_string(root.join("transcripts/7a.md")).unwrap();
        assert!(md.contains("has_transcript: false"));
        assert!(md.contains("## 字幕（时间轴）"));
        std::fs::remove_dir_all(&works).ok();
        std::fs::remove_dir_all(&kb).ok();
    }

    #[test]
    fn publish_only_ids_subset() {
        let works = tempdir();
        let kb = tempdir();
        write_cache(&works, "82933463317", &sample_cache()).unwrap();
        let v =
            run_publish_knowledge(&works, &kb, "82933463317", &["7a".into(), "7c".into()]).unwrap();
        assert_eq!(v["written"], 2);
        let root = kb.join("82933463317");
        assert!(root.join("transcripts/7a.md").exists());
        assert!(!root.join("transcripts/7b.md").exists());
        assert!(root.join("transcripts/7c.md").exists());
        std::fs::remove_dir_all(&works).ok();
        std::fs::remove_dir_all(&kb).ok();
    }
}
