//! `llm_config` / `llm_prompts` 表读写：公共大模型连接配置 + 可配提示词的持久层。
//!
//! 本模块只做**通用存取**，不持有任何内置默认值——「DB 缺失时回退到哪个编译期默认」由各
//! 功能层 / toolkit-server 的内置目录决定（见 toolkit-server `llm` 模块）。解析顺序约定：
//! - 连接配置：DB 行存在用 DB，否则 env（`LlmConfig::from_env`）。
//! - 提示词：DB 行存在用 DB，否则用功能层编译期默认。

use crate::{now_iso8601, SqlitePool};
use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 持久化的 LLM 连接配置（单行 id=1）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredLlmConfig {
    pub base_url: String,
    pub model: String,
    /// 可选 Bearer token；为空表示无鉴权。
    #[serde(default)]
    pub api_key: Option<String>,
}

/// 持久化的提示词条目。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredPrompt {
    pub name: String,
    pub text: String,
    pub version: String,
    pub hash: String,
    /// 覆盖时记录的内置基线哈希（供控制台判断「是否已偏离内置默认」）。
    #[serde(default)]
    pub builtin_hash: Option<String>,
    pub updated_at: String,
}

/// 读连接配置（无行 = None）。
pub fn get_config(pool: &SqlitePool) -> Result<Option<StoredLlmConfig>> {
    let conn = pool.get()?;
    let row = conn
        .query_row(
            "SELECT base_url, model, api_key FROM llm_config WHERE id = 1",
            [],
            |r| {
                Ok(StoredLlmConfig {
                    base_url: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    model: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    api_key: r.get::<_, Option<String>>(2)?,
                })
            },
        )
        .optional()
        .context("read llm_config")?;
    Ok(row)
}

/// 写连接配置（upsert 单行）。空 api_key 归一为 NULL。
pub fn set_config(pool: &SqlitePool, cfg: &StoredLlmConfig) -> Result<()> {
    let conn = pool.get()?;
    let api_key = cfg.api_key.as_deref().filter(|s| !s.trim().is_empty());
    conn.execute(
        "INSERT INTO llm_config(id, base_url, model, api_key, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             base_url = excluded.base_url,
             model = excluded.model,
             api_key = excluded.api_key,
             updated_at = excluded.updated_at",
        params![cfg.base_url, cfg.model, api_key, now_iso8601()],
    )
    .context("upsert llm_config")?;
    Ok(())
}

/// 读单条提示词（无行 = None）。
pub fn get_prompt(pool: &SqlitePool, name: &str) -> Result<Option<StoredPrompt>> {
    let conn = pool.get()?;
    let row = conn
        .query_row(
            "SELECT name, text, version, hash, builtin_hash, updated_at
             FROM llm_prompts WHERE name = ?1",
            params![name],
            row_to_prompt,
        )
        .optional()
        .context("read llm_prompt")?;
    Ok(row)
}

/// 列出全部已保存提示词（按名字排序）。
pub fn list_prompts(pool: &SqlitePool) -> Result<Vec<StoredPrompt>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT name, text, version, hash, builtin_hash, updated_at
         FROM llm_prompts ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], row_to_prompt)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("list llm_prompts")?;
    Ok(rows)
}

/// upsert 一条提示词。
pub fn set_prompt(
    pool: &SqlitePool,
    name: &str,
    text: &str,
    version: &str,
    hash: &str,
    builtin_hash: Option<&str>,
) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO llm_prompts(name, text, version, hash, builtin_hash, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET
             text = excluded.text,
             version = excluded.version,
             hash = excluded.hash,
             builtin_hash = excluded.builtin_hash,
             updated_at = excluded.updated_at",
        params![name, text, version, hash, builtin_hash, now_iso8601()],
    )
    .context("upsert llm_prompt")?;
    Ok(())
}

/// 删除一条提示词（控制台「重置为内置默认」即删 DB 行）。返回受影响行数。
pub fn delete_prompt(pool: &SqlitePool, name: &str) -> Result<usize> {
    let conn = pool.get()?;
    let n = conn
        .execute("DELETE FROM llm_prompts WHERE name = ?1", params![name])
        .context("delete llm_prompt")?;
    Ok(n)
}

fn row_to_prompt(r: &rusqlite::Row) -> rusqlite::Result<StoredPrompt> {
    Ok(StoredPrompt {
        name: r.get(0)?,
        text: r.get(1)?,
        version: r.get(2)?,
        hash: r.get(3)?,
        builtin_hash: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(dir: &std::path::Path) -> SqlitePool {
        let p = crate::open_pool(&dir.join("t.db")).unwrap();
        crate::migrate(&p).unwrap();
        p
    }

    #[test]
    fn config_roundtrip_and_upsert() {
        let tmp = tempfile::tempdir().unwrap();
        let p = pool(tmp.path());
        assert_eq!(get_config(&p).unwrap(), None);
        set_config(
            &p,
            &StoredLlmConfig {
                base_url: "http://x/v1".into(),
                model: "qwen".into(),
                api_key: Some("k".into()),
            },
        )
        .unwrap();
        let got = get_config(&p).unwrap().unwrap();
        assert_eq!(got.model, "qwen");
        assert_eq!(got.api_key.as_deref(), Some("k"));
        // 覆盖 + 空 key 归一 NULL。
        set_config(
            &p,
            &StoredLlmConfig {
                base_url: "http://y/v1".into(),
                model: "m2".into(),
                api_key: Some("  ".into()),
            },
        )
        .unwrap();
        let got = get_config(&p).unwrap().unwrap();
        assert_eq!(got.base_url, "http://y/v1");
        assert_eq!(got.api_key, None);
    }

    #[test]
    fn prompt_crud() {
        let tmp = tempfile::tempdir().unwrap();
        let p = pool(tmp.path());
        assert!(get_prompt(&p, "a").unwrap().is_none());
        set_prompt(&p, "a", "hello", "v1", "h1", Some("b1")).unwrap();
        set_prompt(&p, "b", "world", "v1", "h2", None).unwrap();
        let a = get_prompt(&p, "a").unwrap().unwrap();
        assert_eq!(a.text, "hello");
        assert_eq!(a.builtin_hash.as_deref(), Some("b1"));
        // 覆盖更新。
        set_prompt(&p, "a", "hi", "v2", "h3", Some("b1")).unwrap();
        assert_eq!(get_prompt(&p, "a").unwrap().unwrap().text, "hi");
        let all = list_prompts(&p).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "a");
        // 删除。
        assert_eq!(delete_prompt(&p, "a").unwrap(), 1);
        assert!(get_prompt(&p, "a").unwrap().is_none());
        assert_eq!(delete_prompt(&p, "nope").unwrap(), 0);
    }
}
