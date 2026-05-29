//! 实网集成测试（默认 #[ignore]，需真实 cookie + 干净出口 IP）。
//!
//! 运行：设置 `DOUYIN_COOKIE_JSON` 指向 `{value:{...}}` 结构的 cookies.json，然后
//! `cargo test -p douyin --test live -- --ignored --nocapture`。
//! 默认读 `D:\git\_study\validate\cookies.json`（仓外）。

use douyin::api::DouyinClient;
use std::collections::HashMap;

const SEC_UID: &str =
    "MS4wLjABAAAAvIkc2yZHESf0PO_64Fx9GnIUSy2xjokIph4rMbUhF80ktqz5EECBUeQa0bZdk3kM";

fn load_cookies() -> HashMap<String, String> {
    let path = std::env::var("DOUYIN_COOKIE_JSON")
        .unwrap_or_else(|_| r"D:\git\_study\validate\cookies.json".to_string());
    let raw = std::fs::read_to_string(&path).expect("读取 cookies.json");
    let j: serde_json::Value = serde_json::from_str(&raw).expect("解析 cookies.json");
    let obj = j
        .get("value")
        .unwrap_or(&j)
        .as_object()
        .expect("value 非对象");
    obj.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

#[tokio::test]
#[ignore = "需真实 cookie + 网络 + 干净出口 IP"]
async fn live_profile() {
    let c = DouyinClient::from_cookies(&load_cookies()).unwrap();
    let (nick, count, _) = c.user_profile(SEC_UID).await.unwrap();
    println!("profile: nickname={nick} aweme_count={count}");
    assert!(count > 0, "aweme_count 应 > 0");
}

#[tokio::test]
#[ignore = "需真实 cookie + 网络 + 干净出口 IP"]
async fn live_list_all_works() {
    let c = DouyinClient::from_cookies(&load_cookies()).unwrap();
    let (_, expected, _) = c.user_profile(SEC_UID).await.unwrap();
    let (works, pages, throttled) = c.list_all_works(SEC_UID, 60).await.unwrap();
    println!(
        "list_all_works: got={} expected={} pages={} throttled={}",
        works.len(),
        expected,
        pages,
        throttled
    );
    // 干净 IP 下应拿全（≥95%）。被抽稀的 IP 会远低于此。
    assert!(
        works.len() as i64 >= expected * 95 / 100,
        "覆盖率不足：{}/{}（出口 IP 可能被抽稀）",
        works.len(),
        expected
    );
}

#[tokio::test]
#[ignore = "需真实 cookie + 网络 + 干净出口 IP"]
async fn live_detail_download_url() {
    let c = DouyinClient::from_cookies(&load_cookies()).unwrap();
    let (works, _, _) = c.list_all_works(SEC_UID, 1).await.unwrap();
    let aid = works[0].get("aweme_id").unwrap().as_str().unwrap();
    let (desc, urls, _) = c.aweme_detail(aid).await.unwrap();
    println!(
        "detail: aweme_id={aid} desc={} url_count={}",
        &desc.chars().take(20).collect::<String>(),
        urls.len()
    );
    assert!(!urls.is_empty(), "应拿到 play_addr 下载 URL");
    assert!(urls[0].starts_with("http"));
}
