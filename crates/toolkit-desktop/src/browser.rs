//! 用 `headless_chrome` 启动有头 Chrome 子进程，承担抖音 / 同花顺登录窗职责。
//!
//! 切到 CDP 后能砍掉桥的 mstoken/signal 通道 + 整段 login_hook.js，直接读 cookie。
//!
//! **关于 msToken（实测结论 2026-06-10）**：msToken 缺失与「用 WebView2 还是真 Chrome」
//! 无关 —— 全新临时 profile 的纯净 Chrome（`navigator.webdriver=false`、无任何自动化
//! 开关）登录后同样拿不到 msToken（实测三种条件一致 0 个）。它是抖音 webmssdk 侧行为：
//! 只有**养熟的常驻 profile**才会被写入 msToken。因此关键不是登录用什么浏览器，而是
//! **复用一个持久的 `user_data_dir`**（见 `profile_dir`）：登录态跨重启保留，profile
//! 养熟后 msToken 落库并持久复用，复刻用户日常 Chrome 能用的状态。
//!
//! 每个平台一个 `Session`：lazy 起 Chrome、提供 cookie 读取、关窗 = drop browser
//! → Chrome 子进程随之退出（headless_chrome::Browser 的 Drop 已经处理）。

use anyhow::{anyhow, Context, Result};
use headless_chrome::protocol::cdp::types::Event;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptionsBuilder, Tab};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 单个平台的浏览器会话。`open` 用 Mutex 是为了让多个 Tauri command 共享。
#[derive(Default)]
pub struct Session {
    inner: Mutex<Option<Inner>>,
    /// 持久化 Chrome 配置目录。`None` 时 headless_chrome 每次起一个全新临时 profile
    /// （登录态不保留、profile 永远养不熟 → msToken 永远拿不到）。设成固定目录后
    /// 登录态跨重启保留，且 profile 能养熟让 msToken 落库。
    profile_dir: Option<PathBuf>,
    /// 从外发请求里 harvest 到的最新 msToken。webmssdk 现生成、不一定落 cookie（参考
    /// jiji262 cookie_fetcher 的做法），所以开 Network 域监听请求 URL/postData 兜底捞。
    /// 非必需（uploader 不强求），有则附进上传 cookie 集。
    ms_token: Arc<Mutex<Option<String>>>,
}

struct Inner {
    /// 持有 Browser 保证子进程不退出；drop 就 kill。
    _browser: Browser,
    tab: Arc<Tab>,
}

impl Session {
    /// 用持久 profile 目录建会话。每个平台传各自子目录（douyin / ths 隔离）。
    pub fn new(profile_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(None),
            profile_dir: Some(profile_dir),
            ms_token: Arc::new(Mutex::new(None)),
        }
    }

    pub fn open(&self, initial_url: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(inner) = guard.as_ref() {
            // 已开 → 拉回前台、并 navigate 到新 URL（若不同）
            let _ = inner.tab.bring_to_front();
            return Ok(());
        }
        let mut builder = LaunchOptionsBuilder::default();
        builder
            .headless(false)
            // 登录窗要常驻（用户扫码 / 浏览首页几分钟）。headless_chrome 默认
            // idle_browser_timeout=30s：30s 无 CDP 事件就拆掉事件循环 + 连接，
            // 之后 get_cookies 全报 "underlying connection is closed"，msToken 永远读不到。
            // 设成一天，等效于不超时。
            .idle_browser_timeout(Duration::from_secs(86_400))
            // 剔除 headless_chrome DEFAULT_ARGS 里硬带的 `--enable-automation`：它会让
            // `navigator.webdriver=true` 并弹「受自动化控制」横幅。这是基础反检测卫生
            // （配合下面的 AutomationControlled 关闭，navigator.webdriver 实测变 false）。
            // 注意：单靠它**并不能**让 msToken 出现 —— msToken 取决于 profile 是否养熟，
            // 见模块注释与 `profile_dir`。
            .ignore_default_args(vec![OsStr::new("--enable-automation")])
            .window_size(Some((1280, 860)))
            .args(vec![OsStr::new(
                "--disable-blink-features=AutomationControlled",
            )]);
        // 持久 profile：登录态跨重启保留 + profile 养熟后 msToken 落库复用。
        if let Some(dir) = &self.profile_dir {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建 Chrome profile 目录 {}", dir.display()))?;
            builder.user_data_dir(Some(dir.clone()));
        }
        let browser = Browser::new(builder.build().map_err(|e| anyhow!("LaunchOptions: {e}"))?)
            .context("Browser::new (Chrome 未安装或路径找不到)")?;
        let tab = browser.new_tab().context("Browser::new_tab")?;
        // 开 Network 域 + 挂请求监听，从外发请求里 harvest msToken（在 navigate 前挂好，
        // 不漏首屏请求）。失败不致命：harvest 只是兜底，登录态字段照常从 cookie 读。
        if let Err(e) = tab.call_method(Network::Enable {
            max_total_buffer_size: None,
            max_resource_buffer_size: None,
            max_post_data_size: None,
            report_direct_socket_traffic: None,
            enable_durable_messages: None,
        }) {
            log::warn!("Network.enable 失败（msToken harvest 不可用）: {e:#}");
        } else {
            let slot = self.ms_token.clone();
            let _ = tab.add_event_listener(Arc::new(move |ev: &Event| {
                if let Event::NetworkRequestWillBeSent(e) = ev {
                    let req = &e.params.request;
                    let found = extract_ms_token(&req.url)
                        .or_else(|| req.post_data.as_deref().and_then(extract_ms_token));
                    if let Some(tok) = found {
                        *slot.lock().unwrap() = Some(tok);
                    }
                }
            }));
        }
        tab.navigate_to(initial_url).context("navigate_to")?;
        // 不 wait_for_element body，因为抖音首页 SPA 加载有时极慢；让用户自然等。
        *guard = Some(Inner {
            _browser: browser,
            tab,
        });
        Ok(())
    }

    pub fn close(&self) {
        // drop Browser 会 kill Chrome 子进程
        *self.inner.lock().unwrap() = None;
        // 清掉 harvest 缓存，避免上一次登录的 msToken 串到下一次。
        *self.ms_token.lock().unwrap() = None;
    }

    /// 取从网络请求里 harvest 到的最新 msToken（若有）。
    pub fn harvested_ms_token(&self) -> Option<String> {
        self.ms_token.lock().unwrap().clone()
    }

    /// 是否有活的登录窗。**会探活**：若底层 CDP 连接已死（用户关掉 Chrome 窗 /
    /// Chrome 崩溃），自动复位成 None —— 否则面板会永久卡在「登录失效」，uploader
    /// 也会无限刷 "underlying connection is closed"。
    pub fn is_open(&self) -> bool {
        // 先短暂持锁取 tab 克隆，再到锁外探活，避免持锁期间阻塞在 CDP IO 上。
        let tab = { self.inner.lock().unwrap().as_ref().map(|i| i.tab.clone()) };
        let Some(tab) = tab else {
            return false;
        };
        if tab.get_target_info().is_ok() {
            true
        } else {
            // 连接已死 → 复位，让上层把它当作「无登录窗」处理。
            *self.inner.lock().unwrap() = None;
            false
        }
    }

    /// 拿 tab 的克隆（Arc），方便外部在 spawn_blocking 里调 CDP 同步 API。
    pub fn tab(&self) -> Option<Arc<Tab>> {
        self.inner.lock().unwrap().as_ref().map(|i| i.tab.clone())
    }

    /// 当前 URL；用 evaluate location.href 拿，比 last_navigation_response 稳。
    pub fn current_url(&self) -> Option<String> {
        let tab = self.tab()?;
        tab.evaluate("location.href", false)
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
    }
}

/// 从一段文本（请求 URL 或 postData）里抠出 `msToken=` 的值。取到 `&` / 引号 / 空白
/// 为止；空值或异常超长（>2048，参考 jiji262）视为无效。
fn extract_ms_token(s: &str) -> Option<String> {
    let start = s.find("msToken=")? + "msToken=".len();
    let val = s[start..]
        .split(['&', '"', '\'', ' ', '\n', '\r', '\t', ';'])
        .next()?;
    if val.is_empty() || val.len() > 2048 {
        return None;
    }
    Some(val.to_string())
}

#[cfg(test)]
mod tests {
    use super::extract_ms_token;

    #[test]
    fn ms_token_from_query() {
        let url =
            "https://www.douyin.com/aweme/v1/web/aweme/post/?aid=6383&msToken=AbC123-_x&a_bogus=z";
        assert_eq!(extract_ms_token(url).as_deref(), Some("AbC123-_x"));
    }

    #[test]
    fn ms_token_at_end() {
        assert_eq!(
            extract_ms_token("x=1&msToken=tail").as_deref(),
            Some("tail")
        );
    }

    #[test]
    fn ms_token_absent_or_empty() {
        assert_eq!(extract_ms_token("https://x/?a=1"), None);
        assert_eq!(extract_ms_token("msToken=&b=2"), None);
    }
}
