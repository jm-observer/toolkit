//! 抖音 web API 客户端。参数配方与签名接法照 Phase 0 验证通过的方案（见 memory
//! `project_douyin_v2_recipe`）：`query = urlencode(params)` → `a_bogus = sign(query)` →
//! 请求 `?{query}&a_bogus={a_bogus}`，**签名串与发送串逐字节一致**。
//!
//! 已验证可用：profile（aweme_count）/ user_post（列作品，需干净出口 IP）/ aweme_detail
//! （无水印下载 URL）。search 被 `verify_check` 锁，返回 `anti_bot` 由上层兜底。

use crate::sign::Abogus;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

const BASE: &str = "https://www.douyin.com";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
const FP: &str = "1920|1080|1920|1040|0|0|0|0|1920|1040|1920|1080|24|24|Win32";

/// 业务错误：`kind` 对齐 v1 错误码契约（cookie_missing / anti_bot / not_found /
/// network_error / api_failure / parse_error）。
#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    pub kind: &'static str,
}

impl ApiError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.kind)
    }
}

impl std::error::Error for ApiError {}

type ApiResult<T> = Result<T, ApiError>;

/// 与 Python `urllib.parse.quote_plus` 对齐：unreserved = `A-Za-z0-9_.-~`，空格→`+`，
/// 其余 `%XX`（大写）。签名串与发送串共用此编码，保证一致。
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 按顺序拼 `k=v&k=v`（k/v 均 quote_plus）。顺序即签名顺序，不可乱。
fn urlencode(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// 一页作品。
#[derive(Debug, Clone)]
pub struct WorksPage {
    pub aweme_ids: Vec<String>,
    pub items: Vec<Value>,
    pub has_more: bool,
    pub max_cursor: i64,
    pub status_code: i64,
}

pub struct DouyinClient {
    http: reqwest::Client,
    cookie_header: String,
    s_v_web_id: String,
    uifid: String,
    ms_token: String,
    webid: String,
}

impl DouyinClient {
    /// 从 cookie 字段表构造。要求至少含 `s_v_web_id`（verifyFp/fp）与 `msToken`。
    pub fn from_cookies(cookies: &HashMap<String, String>) -> ApiResult<Self> {
        let s_v_web_id = cookies.get("s_v_web_id").cloned().unwrap_or_default();
        let ms_token = cookies.get("msToken").cloned().unwrap_or_default();
        // msToken 非必需（实测 2026-06-10）：a-bogus 签名只依赖 query、不含 msToken；
        // self_info / profile.other / aweme.post 在空 msToken 下返回数据与带 msToken 完全
        // 一致、不触发风控。抖音 web 新登录的纯净 profile 本就常常不写 msToken cookie，
        // 故这里**不**因空 msToken 报错，留空照常发。
        if ms_token.is_empty() {
            log::debug!("msToken 为空，按空值继续（实测不影响 profile/list/detail）");
        }
        let uifid = cookies.get("UIFID").cloned().unwrap_or_default();
        let cookie_header = cookies
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .build()
            .map_err(|e| ApiError::new("network_error", format!("构造 HTTP 客户端失败: {e}")))?;
        Ok(Self {
            http,
            cookie_header,
            s_v_web_id,
            uifid,
            ms_token,
            webid: String::new(),
        })
    }

    /// 设置真实 webid（抓主页 HTML 得到，可选；Phase 0 证实留空也能用）。
    pub fn with_webid(mut self, webid: impl Into<String>) -> Self {
        self.webid = webid.into();
        self
    }

    /// 公共尾部参数（版本/指纹/cookie 派生），与 Phase 0 `common()` 一致。
    fn tail(&self, params: &mut Vec<(&str, String)>) {
        let tail: &[(&str, &str)] = &[
            ("from_user_page", "1"),
            ("update_version_code", "170400"),
            ("pc_client_type", "1"),
            ("pc_libra_divert", "Windows"),
            ("support_h265", "1"),
            ("support_dash", "1"),
            ("version_code", "290100"),
            ("version_name", "29.1.0"),
            ("cookie_enabled", "true"),
            ("screen_width", "1920"),
            ("screen_height", "1080"),
            ("browser_language", "zh-CN"),
            ("browser_platform", "Win32"),
            ("browser_name", "Edge"),
            ("browser_version", "130.0.0.0"),
            ("browser_online", "true"),
            ("engine_name", "Blink"),
            ("engine_version", "130.0.0.0"),
            ("os_name", "Windows"),
            ("os_version", "10"),
            ("cpu_core_num", "16"),
            ("device_memory", "8"),
            ("platform", "PC"),
            ("downlink", "10"),
            ("effective_type", "4g"),
            ("round_trip_time", "100"),
        ];
        for (k, v) in tail {
            params.push((k, v.to_string()));
        }
        params.push(("uifid", self.uifid.clone()));
        params.push(("webid", self.webid.clone()));
        params.push(("verifyFp", self.s_v_web_id.clone()));
        params.push(("fp", self.s_v_web_id.clone()));
        params.push(("msToken", self.ms_token.clone()));
    }

    /// 头部基础三件套。
    fn head(params: &mut Vec<(&'static str, String)>) {
        params.push(("device_platform", "webapp".into()));
        params.push(("aid", "6383".into()));
        params.push(("channel", "channel_pc_web".into()));
    }

    /// 签名并 GET，返回 JSON。空 200 视为风控信号。
    async fn signed_get(
        &self,
        path: &str,
        params: &[(&str, String)],
        referer: &str,
    ) -> ApiResult<Value> {
        let query = urlencode(params);
        let a_bogus = Abogus::new(FP, UA).sign(&query, "");
        let url = format!("{BASE}{path}?{query}&a_bogus={a_bogus}");
        let resp = self
            .http
            .get(&url)
            .header("referer", referer)
            .header("accept", "application/json, text/plain, */*")
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("cookie", &self.cookie_header)
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .map_err(|e| ApiError::new("network_error", format!("请求失败: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ApiError::new("network_error", format!("读取响应失败: {e}")))?;
        if text.is_empty() {
            return Err(ApiError::new(
                "anti_bot",
                format!("空 200 响应（风控）status={status}"),
            ));
        }
        serde_json::from_str(&text).map_err(|e| {
            ApiError::new(
                "parse_error",
                format!("非 JSON 响应: {e}; body={}", &text[..text.len().min(160)]),
            )
        })
    }

    /// cookie 自检：调 `query/user/`，登录态下返回 user_uid。返回原始 JSON。
    pub async fn self_info(&self) -> ApiResult<Value> {
        let referer = format!("{BASE}/");
        let mut params: Vec<(&str, String)> = Vec::new();
        Self::head(&mut params);
        self.tail(&mut params);
        self.signed_get("/aweme/v1/web/query/user/", &params, &referer)
            .await
    }

    /// 跟随重定向解析短链（v.douyin.com/xxx）到最终 URL。
    pub async fn resolve_redirect(&self, url: &str) -> ApiResult<String> {
        let resp = self
            .http
            .get(url)
            .header("user-agent", UA)
            .send()
            .await
            .map_err(|e| ApiError::new("network_error", format!("短链解析失败: {e}")))?;
        Ok(resp.url().to_string())
    }

    /// 用户资料：返回 (nickname, aweme_count, 原始 user JSON)。
    pub async fn user_profile(&self, sec_uid: &str) -> ApiResult<(String, i64, Value)> {
        let referer = format!("{BASE}/user/{sec_uid}");
        let mut params: Vec<(&str, String)> = Vec::new();
        Self::head(&mut params);
        params.push(("publish_video_strategy_type", "2".into()));
        params.push(("source", "channel_pc_web".into()));
        params.push(("sec_user_id", sec_uid.into()));
        params.push(("personal_center_strategy", "1".into()));
        self.tail(&mut params);
        let j = self
            .signed_get("/aweme/v1/web/user/profile/other/", &params, &referer)
            .await?;
        let user = j.get("user").cloned().unwrap_or(Value::Null);
        if !user.is_object() {
            return Err(ApiError::new(
                "api_failure",
                format!("无 user 字段, status_code={:?}", j.get("status_code")),
            ));
        }
        let nickname = user
            .get("nickname")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let aweme_count = user
            .get("aweme_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        Ok((nickname, aweme_count, user))
    }

    /// 取一页作品。
    pub async fn user_post_page(&self, sec_uid: &str, max_cursor: i64) -> ApiResult<WorksPage> {
        let referer = format!("{BASE}/user/{sec_uid}");
        let mut params: Vec<(&str, String)> = Vec::new();
        Self::head(&mut params);
        params.push(("sec_user_id", sec_uid.into()));
        params.push(("max_cursor", max_cursor.to_string()));
        params.push(("locate_query", "false".into()));
        params.push(("show_live_replay_strategy", "1".into()));
        params.push((
            "need_time_list",
            if max_cursor == 0 {
                "1".into()
            } else {
                "0".into()
            },
        ));
        params.push(("time_list_query", "0".into()));
        params.push(("whale_cut_token", "".into()));
        params.push(("cut_version", "1".into()));
        params.push(("count", "18".into()));
        params.push(("publish_video_strategy_type", "2".into()));
        self.tail(&mut params);
        let j = self
            .signed_get("/aweme/v1/web/aweme/post/", &params, &referer)
            .await?;
        let items: Vec<Value> = j
            .get("aweme_list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let aweme_ids = items
            .iter()
            .filter_map(|a| a.get("aweme_id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        Ok(WorksPage {
            aweme_ids,
            items,
            has_more: j.get("has_more").and_then(|v| v.as_i64()).unwrap_or(0) == 1,
            max_cursor: j.get("max_cursor").and_then(|v| v.as_i64()).unwrap_or(0),
            status_code: j.get("status_code").and_then(|v| v.as_i64()).unwrap_or(-1),
        })
    }

    /// 列全量作品（翻页直到 has_more=false 或游标不前进）。返回去重后的 aweme JSON 列表与翻页数。
    pub async fn list_all_works(
        &self,
        sec_uid: &str,
        max_pages: usize,
    ) -> ApiResult<(Vec<Value>, usize, bool)> {
        let mut cursor = 0i64;
        let mut all: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut pages = 0usize;
        let mut throttled = false;
        loop {
            if pages >= max_pages {
                break;
            }
            let page = self.user_post_page(sec_uid, cursor).await?;
            pages += 1;
            for a in &page.items {
                if let Some(id) = a.get("aweme_id").and_then(|v| v.as_str()) {
                    if seen.insert(id.to_string()) {
                        all.push(a.clone());
                    }
                }
            }
            // shadow-throttle 信号：页面给了游标骨架但 items 被抽稀。
            if !page.items.is_empty() && page.items.len() < 5 {
                throttled = true;
            }
            if !page.has_more || page.max_cursor == cursor {
                break;
            }
            cursor = page.max_cursor;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Ok((all, pages, throttled))
    }

    /// 作品详情：返回 (desc, 无水印 play_addr url 列表, 原始 aweme_detail)。
    pub async fn aweme_detail(&self, aweme_id: &str) -> ApiResult<(String, Vec<String>, Value)> {
        let referer = format!("{BASE}/video/{aweme_id}");
        let mut params: Vec<(&str, String)> = Vec::new();
        Self::head(&mut params);
        params.push(("aweme_id", aweme_id.into()));
        self.tail(&mut params);
        let j = self
            .signed_get("/aweme/v1/web/aweme/detail/", &params, &referer)
            .await?;
        let detail = j.get("aweme_detail").cloned().unwrap_or(Value::Null);
        if !detail.is_object() {
            return Err(ApiError::new(
                "not_found",
                format!("无 aweme_detail, status_code={:?}", j.get("status_code")),
            ));
        }
        let desc = detail
            .get("desc")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let urls = detail
            .get("video")
            .and_then(|v| v.get("play_addr"))
            .and_then(|v| v.get("url_list"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|u| u.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok((desc, urls, detail))
    }

    /// 用户搜索：被 verify_check 锁时返回 anti_bot。
    pub async fn search_user(&self, keyword: &str, count: i64) -> ApiResult<Vec<Value>> {
        let referer = format!("{BASE}/search/{}?type=user", quote_plus(keyword));
        let mut params: Vec<(&str, String)> = Vec::new();
        Self::head(&mut params);
        params.push(("search_channel", "aweme_user_web".into()));
        params.push((
            "search_filter_value",
            r#"{"douyin_user_fans":[""],"douyin_user_type":[""]}"#.into(),
        ));
        params.push(("keyword", keyword.into()));
        params.push(("search_source", "switch_tab".into()));
        params.push(("query_correct_type", "1".into()));
        params.push(("is_filter_search", "1".into()));
        params.push(("offset", "0".into()));
        params.push(("count", count.to_string()));
        params.push(("need_filter_settings", "1".into()));
        params.push(("list_type", "single".into()));
        self.tail(&mut params);
        let j = self
            .signed_get("/aweme/v1/web/discover/search/", &params, &referer)
            .await?;
        if let Some(nil) = j
            .get("search_nil_info")
            .and_then(|v| v.get("search_nil_type"))
            .and_then(|v| v.as_str())
        {
            if nil == "verify_check" {
                return Err(ApiError::new(
                    "anti_bot",
                    "搜索被风控 verify_check，请改用主页 URL",
                ));
            }
        }
        Ok(j.get("user_list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plus_matches_python() {
        assert_eq!(quote_plus("a b"), "a+b");
        assert_eq!(quote_plus("MS4wLjAB-_.~"), "MS4wLjAB-_.~");
        assert_eq!(
            quote_plus(r#"{"k":["v"]}"#),
            "%7B%22k%22%3A%5B%22v%22%5D%7D"
        );
        assert_eq!(quote_plus("a=b&c"), "a%3Db%26c");
    }

    #[test]
    fn urlencode_order_preserved() {
        let p = vec![("b", "2".to_string()), ("a", "1".to_string())];
        assert_eq!(urlencode(&p), "b=2&a=1");
    }
}
