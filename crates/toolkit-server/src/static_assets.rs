use axum::response::Html;

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8" />
<title>toolkit-server</title>
<style>
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
       max-width: 720px; margin: 2em auto; padding: 0 1em; }
code { background: #f4f4f4; padding: 2px 4px; border-radius: 3px; }
ul { line-height: 1.7; }
</style>
</head>
<body>
<h1>toolkit-server</h1>
<p>骨架 OK（Plan 1）。富 UI 是 Plan 3 的事，本页仅作为占位与 endpoint 速查。</p>
<h2>Endpoints</h2>
<ul>
<li><code>GET /api/web/health</code> — 健康检查</li>
<li><code>POST /api/web/tasks</code> — 提交任务 <code>{kind, input, callback_url?}</code></li>
<li><code>GET /api/web/tasks</code> — 列表 <code>?kind=&amp;state=&amp;limit=</code></li>
<li><code>GET /api/web/tasks/{task_id}</code> — 任务详情</li>
<li><code>GET /api/agent/health</code> — Agent 命名空间健康检查（占位）</li>
<li><code>POST /api/browser/hello</code> — 扩展握手</li>
<li><code>POST /api/browser/url</code> — 当前 tab URL 推送</li>
<li><code>POST /api/browser/cookie</code> — 抖音 cookie 推送</li>
</ul>
</body>
</html>"#;

pub async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}
