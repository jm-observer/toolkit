// 简易 JS：直接 fetch / api/* —— 与 server 同源，无 CSP 问题。
const $ = (id) => document.getElementById(id);
const fmt = (v) => (v == null ? "—" : JSON.stringify(v, null, 2));

async function api(path, init = {}) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  const text = await res.text();
  let body;
  try { body = JSON.parse(text); } catch { body = text; }
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}: ${typeof body === "string" ? body : body.error || text}`);
  return body;
}

// -------- tabs --------
document.querySelectorAll("nav#tabs button").forEach((b) => {
  b.onclick = () => {
    document.querySelectorAll("nav#tabs button").forEach((x) => x.classList.remove("active"));
    document.querySelectorAll("main section").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    $("tab-" + b.dataset.tab).classList.add("active");
    if (b.dataset.tab === "tasks") refreshTasks();
  };
});

// -------- 概览 --------
async function refreshHealth() {
  try {
    const h = await api("/api/web/health");
    $("health-detail").textContent = fmt(h);
    $("pill-health").textContent = "health: ok";
    $("pill-health").className = "pill ok";
  } catch (e) {
    $("health-detail").textContent = e.message;
    $("pill-health").textContent = "health: err";
    $("pill-health").className = "pill err";
  }
}

async function refreshCookie() {
  try {
    const c = await api("/api/web/douyin/cookie_status");
    $("cookie-detail").textContent = fmt(c);
    const ok = c.has_required && c.logged_in;
    $("pill-cookie").textContent = ok ? `cookie: ok (${c.fields} fields)` : "cookie: 待登录";
    $("pill-cookie").className = ok ? "pill ok" : "pill warn";
  } catch (e) {
    $("cookie-detail").textContent = e.message;
    $("pill-cookie").textContent = "cookie: err";
    $("pill-cookie").className = "pill err";
  }
}

async function refreshOverviewTasks() {
  try {
    const r = await api("/api/web/tasks?limit=10");
    const rows = (r.tasks || r || []).slice(0, 10);
    renderTaskRows($("overview-tasks").querySelector("tbody"), rows);
  } catch (e) {
    $("overview-tasks").querySelector("tbody").innerHTML =
      `<tr><td colspan="4">${e.message}</td></tr>`;
  }
}

$("overview-refresh").onclick = () => {
  refreshHealth();
  refreshCookie();
  refreshOverviewTasks();
};

function renderTaskRows(tbody, rows) {
  if (!rows.length) {
    tbody.innerHTML = `<tr><td colspan="5"><span style="color:var(--muted)">无</span></td></tr>`;
    return;
  }
  tbody.innerHTML = rows
    .map((t) => {
      const tid = t.task_id || t.id || "—";
      return `<tr data-id="${tid}">
        <td><code>${tid}</code></td>
        <td>${t.kind || "—"}</td>
        <td class="state-${t.state || ""}">${t.state || "—"}</td>
        <td>${t.created_at || "—"}</td>
        <td>${t.updated_at || "—"}</td>
      </tr>`;
    })
    .join("");
  tbody.querySelectorAll("tr").forEach((tr) => {
    tr.onclick = () => {
      $("task-id").value = tr.dataset.id;
      document.querySelector('nav#tabs button[data-tab="tasks"]').click();
      fetchTaskDetail();
    };
  });
}

// -------- 抖音 --------
document.querySelectorAll('#tab-douyin button[data-act]').forEach((b) => {
  b.onclick = async () => {
    const handle = $("dy-handle").value.trim();
    const out = $("dy-out");
    out.textContent = "loading…";
    try {
      let path;
      if (b.dataset.act === "creator") path = `/api/web/douyin/creator?handle=${encodeURIComponent(handle)}`;
      else if (b.dataset.act === "works") path = `/api/web/douyin/works?handle=${encodeURIComponent(handle)}`;
      else if (b.dataset.act === "cookie_status") path = "/api/web/douyin/cookie_status";
      else if (b.dataset.act === "tags") path = `/api/web/douyin/tags?unique_id=${encodeURIComponent($("dy-uid").value.trim())}`;
      else if (b.dataset.act === "filter")
        path = `/api/web/douyin/filter?unique_id=${encodeURIComponent($("dy-uid").value.trim())}&tags=${encodeURIComponent($("dy-tags").value.trim())}&match=${$("dy-match").value}`;
      const r = await api(path);
      const target = b.dataset.act === "tags" || b.dataset.act === "filter" ? $("dy-filter-out") : out;
      target.textContent = fmt(r);
    } catch (e) { out.textContent = e.message; }
  };
});

document.querySelectorAll('#tab-douyin button[data-task]').forEach((b) => {
  b.onclick = async () => {
    const out = $("dy-task-out");
    let body;
    try { body = JSON.parse($("dy-task-input").value || "{}"); }
    catch (e) { out.textContent = "JSON parse: " + e.message; return; }
    out.textContent = "submitting…";
    try {
      const r = await api(`/api/web/douyin/${b.dataset.task}`, {
        method: "POST", body: JSON.stringify(body),
      });
      out.textContent = fmt(r);
    } catch (e) { out.textContent = e.message; }
  };
});

// -------- 任务 --------
async function refreshTasks() {
  const tbody = $("tasks-table").querySelector("tbody");
  const params = new URLSearchParams();
  const k = $("tasks-kind").value.trim(); if (k) params.set("kind", k);
  const st = $("tasks-state").value.trim(); if (st) params.set("state", st);
  const lim = $("tasks-limit").value.trim(); if (lim) params.set("limit", lim);
  try {
    const r = await api("/api/web/tasks?" + params);
    renderTaskRows(tbody, r.tasks || r || []);
  } catch (e) {
    tbody.innerHTML = `<tr><td colspan="5">${e.message}</td></tr>`;
  }
}
$("tasks-refresh").onclick = refreshTasks;

async function fetchTaskDetail() {
  const id = $("task-id").value.trim();
  if (!id) return;
  $("task-detail").textContent = "loading…";
  try {
    const r = await api("/api/web/tasks/" + encodeURIComponent(id));
    $("task-detail").textContent = fmt(r);
  } catch (e) { $("task-detail").textContent = e.message; }
}
$("task-detail-btn").onclick = fetchTaskDetail;

$("submit-btn").onclick = async () => {
  const out = $("submit-out");
  let input;
  try { input = JSON.parse($("submit-input").value || "{}"); }
  catch (e) { out.textContent = "JSON parse: " + e.message; return; }
  const body = { kind: $("submit-kind").value.trim(), input };
  const cb = $("submit-cb").value.trim();
  if (cb) body.callback_url = cb;
  out.textContent = "submitting…";
  try {
    const r = await api("/api/web/tasks", { method: "POST", body: JSON.stringify(body) });
    out.textContent = fmt(r);
    refreshTasks();
  } catch (e) { out.textContent = e.message; }
};

// -------- 知识库 --------
$("kb-btn").onclick = async () => {
  const out = $("kb-out");
  const uid = $("kb-uid").value.trim();
  const ids = $("kb-ids").value.split(/[,\s]+/).filter(Boolean);
  out.textContent = "publishing…";
  try {
    const r = await api("/api/web/douyin/kb_publish", {
      method: "POST",
      body: JSON.stringify({ unique_id: uid, only_ids: ids }),
    });
    out.textContent = fmt(r);
  } catch (e) { out.textContent = e.message; }
};

// -------- desktop 桥（127.0.0.1:28788） --------
const DESKTOP_BRIDGE = "http://127.0.0.1:28788";
let lastDesktopCtx = null;

async function refreshDesktop() {
  const pill = $("pill-desktop");
  try {
    const res = await fetch(DESKTOP_BRIDGE + "/context", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    const ctx = await res.json();
    lastDesktopCtx = ctx;
    const loginOk = ctx.login && ctx.login.has_window;
    const msOk = ctx.ms_token && ctx.ms_token.present;
    const thsOk = ctx.ths && ctx.ths.has_required && !ctx.ths.ticket_is_session;
    const parts = [];
    parts.push(loginOk ? "login✓" : "login✗");
    parts.push(msOk ? "msToken✓" : "msToken✗");
    parts.push(thsOk ? "ths✓" : "ths-");
    pill.textContent = "desktop: " + parts.join(" ");
    pill.className = "pill " + (loginOk && msOk ? "ok" : "warn");
    const det = $("ctx-detail");
    if (det) det.textContent = JSON.stringify(ctx, null, 2);
  } catch (e) {
    lastDesktopCtx = null;
    pill.textContent = "desktop: 离线";
    pill.className = "pill err";
    pill.title = String(e);
    const det = $("ctx-detail");
    if (det && !det.textContent.startsWith("{")) {
      det.textContent = "未连接到 desktop 桥 (127.0.0.1:28788): " + e.message;
    }
  }
}

document.addEventListener("DOMContentLoaded", () => {
  const r = document.getElementById("ctx-refresh");
  if (r) r.onclick = refreshDesktop;
  const u = document.getElementById("ctx-use-url");
  if (u) u.onclick = () => {
    const url = lastDesktopCtx && lastDesktopCtx.login && lastDesktopCtx.login.url;
    if (!url) { alert("desktop login 窗口未打开，或还没导航"); return; }
    $("dy-handle").value = url;
    $("dy-handle").scrollIntoView({behavior: "smooth", block: "nearest"});
  };
  const rn = document.getElementById("ctx-resolve-now");
  if (rn) rn.onclick = async () => {
    // 先拿最新 desktop URL（不依赖 10s 缓存）
    let url;
    try {
      const r = await fetch(DESKTOP_BRIDGE + "/login-url", { cache: "no-store" });
      const j = await r.json();
      if (!j.has_window) { alert("desktop 登录窗未打开。请到桌面端点「抖音登录」。"); return; }
      url = j.url;
      if (!url) { alert("desktop 已有登录窗但 URL 为空。"); return; }
    } catch (e) {
      alert("拉 desktop 桥失败 (127.0.0.1:28788): " + e.message); return;
    }
    $("dy-handle").value = url;
    $("ctx-detail").textContent = `→ 当前 URL: ${url}\n→ 调 GET /api/web/douyin/creator ...`;
    try {
      const creator = await api(`/api/web/douyin/creator?handle=${encodeURIComponent(url)}`);
      $("ctx-detail").textContent = `URL: ${url}\n\n${JSON.stringify(creator, null, 2)}`;
      // 同时同步到博主区下面的 pre 框，方便用户继续点 list works
      $("dy-out").textContent = JSON.stringify(creator, null, 2);
      // 自动把 unique_id 灌进标签区，省一次复制
      if (creator.unique_id) $("dy-uid").value = creator.unique_id;
    } catch (e) {
      $("ctx-detail").textContent = `URL: ${url}\n\nresolve 失败: ${e.message}`;
    }
  };
});
$("pill-desktop").onclick = refreshDesktop;

// init
refreshHealth();
refreshCookie();
refreshOverviewTasks();
refreshDesktop();
// 30s 自动刷新概览状态；desktop 桥 10s 一次
setInterval(() => { refreshHealth(); refreshCookie(); }, 30000);
setInterval(refreshDesktop, 10000);
