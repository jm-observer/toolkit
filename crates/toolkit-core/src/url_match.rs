//! 抖音 URL 模式识别。规则见 `docs/toolkit-rfc/2026-06-04-initial-skeleton/extension-contract.md` §五。

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UrlMatch {
    CreatorHome { sec_uid: String },
    CreatorHomeShort { short_code: String },
    Work { aweme_id: String },
    Search,
    None,
}

struct Patterns {
    creator_home: Regex,
    creator_home_share: Regex,
    creator_home_short: Regex,
    work: Regex,
    search: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        creator_home: Regex::new(r"^https?://www\.douyin\.com/user/(MS4w[A-Za-z0-9_\-]+)")
            .expect("compile creator_home regex"),
        creator_home_share: Regex::new(
            r"^https?://m\.douyin\.com/share/user/(MS4w[A-Za-z0-9_\-]+)",
        )
        .expect("compile creator_home_share regex"),
        creator_home_short: Regex::new(r"^https?://v\.douyin\.com/([A-Za-z0-9]+)/?")
            .expect("compile short regex"),
        work: Regex::new(r"^https?://www\.douyin\.com/video/(\d+)").expect("compile work regex"),
        search: Regex::new(r"^https?://www\.douyin\.com/search/").expect("compile search regex"),
    })
}

pub fn classify_url(url: &str) -> UrlMatch {
    let p = patterns();
    if let Some(c) = p.creator_home.captures(url) {
        return UrlMatch::CreatorHome {
            sec_uid: c[1].to_string(),
        };
    }
    if let Some(c) = p.creator_home_share.captures(url) {
        return UrlMatch::CreatorHome {
            sec_uid: c[1].to_string(),
        };
    }
    if let Some(c) = p.creator_home_short.captures(url) {
        return UrlMatch::CreatorHomeShort {
            short_code: c[1].to_string(),
        };
    }
    if let Some(c) = p.work.captures(url) {
        return UrlMatch::Work {
            aweme_id: c[1].to_string(),
        };
    }
    if p.search.is_match(url) {
        return UrlMatch::Search;
    }
    UrlMatch::None
}
