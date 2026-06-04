# toolkit-link Chrome 扩展

把 Chrome 的当前抖音 tab URL 与 `.douyin.com` 域 Cookie 推送给本地或远程 `toolkit-server`。

## 安装（开发者模式）

1. 修改 [`background.js`](background.js) 顶部的 `SERVER` 常量，指向你的 toolkit-server：
   - 本机：`http://127.0.0.1:8788`
   - g10 远程：`http://192.168.0.68:8788`
2. Chrome 打开 `chrome://extensions/`，右上角打开「开发者模式」。
3. 点「加载已解压的扩展程序」，选中本目录（`extension/`）。
4. 扩展加载完成后会立即向 server 发 `hello` + 当前 cookie 快照。

## 工作机制

| 时机 | 动作 |
|---|---|
| Service worker 启动 | `POST /api/browser/hello`，附 session_id（持久存 chrome.storage） |
| 抖音 tab URL 变化 | `POST /api/browser/url` |
| `.douyin.com` cookie 变化（debounce 1s） | `POST /api/browser/cookie`，附 raw header + parsed |
| Hello 后 | 立刻推一次完整 cookie 快照 |

## 隐私

- 仅推送抖音域 cookie；其他网站 cookie **不发**
- 仅在 tab URL 命中抖音域时推 URL
- 没有任何远程指令通道：server 不能让扩展执行任意 JS

## 调试

- Chrome 扩展页面点扩展卡片的「Service worker」链接打开 DevTools 看 console
- 期望日志：`[toolkit-link]` 前缀的 warn / error；正常时无输出
- 在 toolkit-server 控制台（`http://<SERVER>/`）第 1 节「Cookie 状态」点刷新，能看到字段计数说明推送到了

## TODO（后续 Plan）

- popup 配置 SERVER 地址（目前需改 background.js）
- URL 模式识别后在当前 tab 显示 page action 徽章（如「这是博主主页，可收藏」）
- token 鉴权
