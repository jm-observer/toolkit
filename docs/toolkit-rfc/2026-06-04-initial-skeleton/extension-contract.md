# Chrome 扩展契约（extension ↔ toolkit-server）

> 规范扩展与 `toolkit-server` 之间的 HTTP 协议、扩展所需的 Chrome 权限、URL/Cookie 推送时机。**本契约在 Plan 1 完成时 server 侧 HTTP endpoint 已就绪；扩展本体的实现是 Plan 4。** 提前固化是为了让 Plan 1 写 endpoint 时不返工。

## 一、扩展形态

- **Manifest V3**，单文件 `manifest.json` + 一个 `background.js`（service worker）
- 不需要 popup / options 页（首版）；扩展只在 background 里干活
- 本地未上架商店——`chrome://extensions` 开发者模式加载未压缩扩展

## 二、Manifest 权限清单

```json
{
  "manifest_version": 3,
  "name": "toolkit-link",
  "version": "0.1.0",
  "description": "把当前抖音 tab 的 URL 与 Cookie 推送给 toolkit-server",
  "permissions": [
    "tabs",
    "cookies",
    "storage"
  ],
  "host_permissions": [
    "*://*.douyin.com/*",
    "http://192.168.0.68:*/*"
  ],
  "background": {
    "service_worker": "background.js"
  }
}
```

- `tabs`：监听 `chrome.tabs.onUpdated` 拿当前 URL
- `cookies`：监听 `chrome.cookies.onChanged` + `chrome.cookies.getAll({domain: '.douyin.com'})`
- `storage`：持久化 `session_id` / server 地址
- `host_permissions`：限制只能访问抖音域 + g10 服务地址

> g10 地址应可配置（写在扩展 storage 里），初版可硬编 `192.168.0.68`，后续 popup 加输入框

## 三、通信形态：HTTP POST，无长连接

扩展是事件驱动的：URL/Cookie 有事件时唤醒、POST 一次、回到休眠。无需长连接或心跳：

- service worker 由 `chrome.tabs.onUpdated` / `chrome.cookies.onChanged` 唤醒
- POST 完即可让 SW 自由休眠
- 服务端没有"主动推消息回扩展"的需求——按钮高亮等动作扩展用本地 URL 正则自己判断（详见 §五）

这是相对 WS 方案的关键简化：去掉 `tokio-tungstenite` 依赖、去掉连接状态管理、去掉心跳。

## 四、HTTP 端点

所有端点 base path `/api/browser/`，请求/响应 JSON。

### 4.1 POST /api/browser/hello

时机：扩展启动（service worker 首次唤醒），最多 5 分钟一次（用 storage 缓存上次时间）

请求：
```json
{
  "session_id": "<crypto.randomUUID() 持久存 storage>",
  "user_agent": "<navigator.userAgent>",
  "extension_version": "0.1.0"
}
```

响应：
```json
{ "server_version": "<crate version>", "accepted_at": "<ISO8601>" }
```

副作用：upsert `browser_sessions`（first_seen 取 INSERT 时；last_seen 每次 hello 更新）

### 4.2 POST /api/browser/url

时机：`chrome.tabs.onUpdated` 触发且 tab URL 命中抖音域

请求：
```json
{
  "session_id": "...",
  "tab_id": 12345,
  "url": "https://www.douyin.com/user/MS4w...",
  "title": "..."
}
```

响应：
```json
{ "matched": "creator_home" | "work" | "search" | null }
```

> server 仅返回 URL 模式识别结果（让扩展知道服务端怎么解析），**不**返回"应该亮哪个按钮"——按钮由扩展本地判定。

副作用：更新 `browser_sessions.current_url` + `last_seen`

### 4.3 POST /api/browser/cookie

时机：
- 扩展启动 hello 后立刻发一次
- `chrome.cookies.onChanged` 任一 `.douyin.com` cookie 变化后 debounce 1s 发一次

请求：
```json
{
  "session_id": "...",
  "raw_header": "msToken=xxx; ttwid=yyy; ...",
  "parsed": { "msToken": "xxx", "ttwid": "yyy" }
}
```

响应：
```json
{
  "accepted": true,
  "fields_count": 14,
  "has_required": ["msToken", "ttwid", "sessionid_ss"]
}
```

副作用：upsert `cookies` 单行，`status='unknown'`（等下次 douyin API 调用回写）

## 五、URL 识别规则

扩展和 server 都用同一份规则。Server 端是 `toolkit-core::url_match` 模块，扩展端复制一份 JS 实现（值小，重复可接受）。

| URL 模式 | 解析为 | 扩展可亮的本地按钮（Plan 4） |
|---|---|---|
| `https://www.douyin.com/user/MS4w<sec_uid>` | `creator_home` | "收藏当前博主" |
| `https://m.douyin.com/share/user/MS4w<sec_uid>` | `creator_home` | 同上 |
| `https://v.douyin.com/<short>/` | `creator_home_short` | 暂不亮（需 HEAD 跟随，Plan 4 决策） |
| `https://www.douyin.com/video/<aweme_id>` | `work` | "下载这条" / "录入这条"（Plan 4） |
| `https://www.douyin.com/search/...` | `search` | 无 |
| 其他 douyin 域 | `null` | 无 |

Plan 1 在 `toolkit-core::url_match` 落地全部模式 + 返回类型 `UrlMatch::{ CreatorHome, CreatorHomeShort, Work, Search, None }`。

## 六、Server 侧持久化

- `hello` → upsert `browser_sessions`
- `url` → 更新 `browser_sessions.current_url`、`last_seen`
- `cookie` → upsert `cookies` 单行（覆盖），`status='unknown'`

无内存映射、无长连接状态。

## 七、安全与边界

- **首版无鉴权**：endpoint 接受任何来源；信任前提是 g10 仅暴露在内网。后续 Plan 加扩展 → server 的 token 握手（写在扩展 storage，server 端校验 `X-Toolkit-Token` header）
- **CORS**：server 启用 `tower-http::cors` 允许 `chrome-extension://*` Origin
- **Cookie 不回传**：server 收到 cookie 后只入库，响应仅返回元信息（字段计数、关键字段命中），不 echo 任何 value
