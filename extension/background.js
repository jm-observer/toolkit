// toolkit-link — Chrome 扩展 background service worker。
// 把抖音 tab 的 URL 与 .douyin.com cookie 推送给 toolkit-server。

// ⚠️ 修改 SERVER 指向你的 toolkit-server。本机开发用 127.0.0.1；远程部署用 g10 IP 等。
const SERVER = "http://127.0.0.1:8788";

const EXT_VERSION = "0.1.0";
const COOKIE_DEBOUNCE_MS = 1000;
const DOUYIN_DOMAIN = ".douyin.com";

// ---------------- session id ----------------

async function getSessionId() {
  const got = await chrome.storage.local.get("session_id");
  if (got.session_id) return got.session_id;
  const id = crypto.randomUUID();
  await chrome.storage.local.set({ session_id: id });
  return id;
}

// ---------------- HTTP helpers ----------------

async function post(path, body) {
  try {
    const resp = await fetch(SERVER + path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      console.warn(`[toolkit-link] ${path} status=${resp.status}`);
    }
    return resp;
  } catch (e) {
    console.warn(`[toolkit-link] ${path} failed: ${e.message}`);
  }
}

// ---------------- hello ----------------

let helloSent = false;

async function helloIfNeeded() {
  if (helloSent) return;
  helloSent = true;
  const session_id = await getSessionId();
  await post("/api/browser/hello", {
    session_id,
    user_agent: navigator.userAgent,
    extension_version: EXT_VERSION,
  });
  // hello 后立刻推一次 cookie 全量
  await pushCookieSnapshot();
}

// ---------------- URL change ----------------

function isDouyinUrl(url) {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.hostname.endsWith(".douyin.com") || u.hostname === "douyin.com";
  } catch {
    return false;
  }
}

chrome.tabs.onUpdated.addListener(async (tabId, info, tab) => {
  if (info.status !== "complete" && !info.url) return;
  const url = tab.url || info.url;
  if (!isDouyinUrl(url)) return;
  await helloIfNeeded();
  const session_id = await getSessionId();
  await post("/api/browser/url", {
    session_id,
    tab_id: tabId,
    url,
    title: tab.title || null,
  });
});

// ---------------- cookies ----------------

let cookieDebounce = null;

async function pushCookieSnapshot() {
  const session_id = await getSessionId();
  const cookies = await chrome.cookies.getAll({ domain: DOUYIN_DOMAIN });
  if (!cookies.length) return;
  const parsed = {};
  for (const c of cookies) parsed[c.name] = c.value;
  const raw_header = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  await post("/api/browser/cookie", { session_id, raw_header, parsed });
}

chrome.cookies.onChanged.addListener((change) => {
  const dom = change.cookie.domain || "";
  if (!dom.includes("douyin.com")) return;
  if (cookieDebounce) clearTimeout(cookieDebounce);
  cookieDebounce = setTimeout(() => {
    cookieDebounce = null;
    pushCookieSnapshot().catch((e) => console.warn(e));
  }, COOKIE_DEBOUNCE_MS);
});

// ---------------- service worker activation ----------------
// MV3 service worker 受事件驱动，启动时主动 hello 一次。

(async () => {
  try {
    await helloIfNeeded();
  } catch (e) {
    console.warn("[toolkit-link] startup hello failed:", e);
  }
})();
