// Tauri 2 全局 API：__TAURI__.core.invoke / event.listen。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

function fmt(v) {
  try { return JSON.stringify(v, null, 2); } catch { return String(v); }
}

let toastTimer = null;
function toast(msg, kind /* "" | "warn" | "err" */) {
  const t = $("toast");
  t.textContent = msg;
  t.className = "show" + (kind ? " toast-" + kind : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.className = ""; }, 3200);
}

// ============ 设置 ============
async function loadSettings() {
  const s = await invoke("cmd_get_settings");
  $("server").value = s.server_base || "";
  $("token").value = s.auth_token || "";
}

$("save").onclick = async () => {
  await invoke("cmd_save_settings", {
    settings: {
      server_base: $("server").value.trim(),
      auth_token: $("token").value || null,
      last_uploaded_at: null,
    },
  });
  flashPill($("connPill"), "已保存");
  pingServer();
};

document.querySelectorAll("button[data-preset]").forEach((b) => {
  b.onclick = () => { $("server").value = b.dataset.preset; };
});

// ============ 服务器 cookie 状态（g10 pill + 登录前预判） ============
let lastServerCookie = null; // { logged_in, has_required, fields, user_uid, missing }

async function refreshServerCookie() {
  setPill("g10Pill", "g10: …", "muted");
  try {
    const r = await invoke("cmd_check_server_cookie");
    if (r.state === "unconfigured") {
      setPill("g10Pill", "g10: 未配置", "muted");
      lastServerCookie = null;
      return;
    }
    if (r.state !== "ok") {
      setPill("g10Pill", "g10: " + r.state, "err");
      $("g10Pill").title = JSON.stringify(r, null, 2);
      lastServerCookie = null;
      return;
    }
    const b = r.body || {};
    lastServerCookie = b;
    if (b.logged_in && b.has_required) {
      setPill("g10Pill", `g10: 有效·${b.fields || 0}`, "ok");
      $("g10Pill").title = `登录态 OK\nuser_uid=${b.user_uid || "?"}\nfields=${b.fields}`;
    } else if (b.has_required) {
      setPill("g10Pill", "g10: 字段全但未登录", "warn");
      $("g10Pill").title = "logged_in=false（cookie 完整但 self_info 验证失败）";
    } else {
      setPill("g10Pill", `g10: 缺 ${(b.missing || []).join(",") || "字段"}`, "warn");
      $("g10Pill").title = JSON.stringify(b, null, 2);
    }
  } catch (e) {
    setPill("g10Pill", "g10: err", "err");
    $("g10Pill").title = String(e);
    lastServerCookie = null;
  }
}
$("g10Pill").onclick = refreshServerCookie;

// ============ 登录 cookie 失效时间 ============
function fmtDuration(secs) {
  if (secs == null) return "?";
  if (secs < 0) return "已过期";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  if (d > 0) return `${d}d${h}h`;
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h${m}m`;
  return `${m}m`;
}

async function refreshLoginExpiry() {
  try {
    const r = await invoke("cmd_login_expiry");
    const line = $("expiryInfo");
    if (r.state === "no_window") {
      line.textContent = "登录失效信息：未打开登录窗";
      line.style.color = "var(--muted)";
      return;
    }
    if (!r.critical || r.critical.length === 0) {
      line.textContent = `登录失效信息：未检测到关键 cookie（共 ${r.cookies_total || 0} 条）`;
      line.style.color = "var(--warn)";
      return;
    }
    const remaining = r.earliest_remaining_secs;
    const at = r.earliest_expires_at ? r.earliest_expires_at.slice(0, 19) : "?";
    let color = "var(--muted)";
    if (remaining != null) {
      if (remaining < 0) color = "var(--err)";
      else if (remaining < 3 * 86400) color = "var(--warn)";
      else color = "var(--ok)";
    }
    line.style.color = color;
    line.textContent = `登录失效：${at} (余 ${fmtDuration(remaining)})`;
    line.title = r.critical
      .map((c) => `${c.name}: ${c.is_session ? "session 票" : (c.expires_at || "?") + " (余 " + fmtDuration(c.remaining_secs) + ")"}`)
      .join("\n");
  } catch (e) {
    $("expiryInfo").textContent = "登录失效信息：" + String(e);
  }
}

// ============ 抖音登录窗（点了就开，不拦截） ============
$("login").onclick = () => invoke("cmd_open_login");
$("closeLogin").onclick = () => invoke("cmd_close_login");
$("force").onclick = () => invoke("cmd_force_upload_now");
$("inspect").onclick = async () => {
  $("detail").textContent = "inspecting…";
  try {
    const r = await invoke("cmd_inspect_cookies");
    $("detail").textContent = fmt(r);
  } catch (e) {
    $("detail").textContent = String(e);
  }
};

// ============ 同花顺登录窗 ============
$("thsLogin").onclick = () => invoke("cmd_open_ths_login");
$("thsClose").onclick = () => invoke("cmd_close_ths_login");

// ============ pill 渲染 ============
function setPill(id, text, kind) {
  const p = $(id);
  p.textContent = text;
  p.className = "pill pill-" + (kind || "muted") + (id === "connPill" ? " pill-clickable" : "");
}

function flashPill(p, msg) {
  const prev = { t: p.textContent, c: p.className };
  p.textContent = msg;
  setTimeout(() => { p.textContent = prev.t; p.className = prev.c; }, 1200);
}

// ============ connection pill ============
async function pingServer() {
  setPill("connPill", "conn: …", "muted");
  try {
    const r = await invoke("cmd_ping_server");
    const p = $("connPill");
    if (r.state === "unconfigured") {
      setPill("connPill", "conn: 未配置", "muted");
      p.title = "请填 server base";
    } else if (r.state === "ok") {
      setPill("connPill", `conn: ok · ${r.latency_ms}ms`, "ok");
      p.title = `${r.server_base}\nversion: ${r.server_version || "?"}`;
    } else if (r.state === "http_err") {
      setPill("connPill", `conn: HTTP ${r.status}`, "warn");
      p.title = r.server_base;
    } else {
      setPill("connPill", "conn: 不可达", "err");
      p.title = (r.error || "") + "\n" + (r.server_base || "");
    }
  } catch (e) {
    setPill("connPill", "conn: err", "err");
    $("connPill").title = String(e);
  }
}
$("connPill").onclick = pingServer;

// ============ uploader 状态 → login / msToken pill ============
listen("uploader:status", (e) => {
  const p = e.payload || {};
  $("detail").textContent = fmt(p);
  switch (p.state) {
    case "uploaded":
      setPill("loginPill", `login: 已传·${p.fields || 0}`, "ok");
      setPill("msPill", "msToken: 在", "ok");
      toast(`✓ cookie 已同步到 G10 · ${p.fields || 0} 字段`);
      // 上传成功顺手刷 g10 pill 和 expiry
      refreshServerCookie();
      refreshLoginExpiry();
      break;
    case "unchanged":
      setPill("loginPill", `login: ok·${p.fields || 0}`, "ok");
      setPill("msPill", "msToken: 在", "ok");
      break;
    case "waiting_login":
      const missing = (p.missing || []).join(",");
      setPill("loginPill", `login: 等 ${missing || "?"}`, "warn");
      setPill("msPill", missing.includes("msToken") ? "msToken: 缺" : "msToken: —", "warn");
      break;
    case "no_login_window":
      setPill("loginPill", "login: 未开窗", "muted");
      setPill("msPill", "msToken: —", "muted");
      break;
    case "unconfigured":
      setPill("loginPill", "login: 未配置 server", "muted");
      break;
    case "error":
      setPill("loginPill", "login: err", "err");
      $("loginPill").title = p.error || "";
      break;
    default:
      break;
  }
});

// ============ ths 状态 ============
listen("ths:status", (e) => {
  const p = e.payload || {};
  // 不抢占 detail（uploader 也写），ths 只更新 pill
  const r = p.report || p;
  if (p.state === "saved") {
    setPill("thsPill", `ths: ok·${p.count || 0}`, "ok");
  } else if (p.state === "waiting_login") {
    setPill("thsPill", "ths: 等登录", "warn");
  } else if (p.state === "no_login_window") {
    setPill("thsPill", "ths: 未开窗", "muted");
  } else if (p.state === "unchanged" && r && r.has_required) {
    setPill("thsPill", `ths: ok·${r.count || 0}`, "ok");
  } else if (p.state === "error") {
    setPill("thsPill", "ths: err", "err");
  }
});

// 启动时显式查一次 ths 落盘状态（watcher 还没 tick 时也能正确显示）
async function initialThs() {
  try {
    const r = await invoke("cmd_ths_status");
    if (!r.exists) setPill("thsPill", "ths: 未登录", "muted");
    else if (!r.has_required) setPill("thsPill", "ths: 缺字段", "warn");
    else if (r.ticket_is_session) setPill("thsPill", "ths: session 票", "warn");
    else setPill("thsPill", `ths: ok·${r.count}`, "ok");
  } catch (e) {}
}

// ============ 复制详情 ============
$("copyDetail").onclick = async () => {
  const text = $("detail").textContent || "";
  try { await navigator.clipboard.writeText(text); flashPill($("connPill"), "已复制"); }
  catch (e) {
    const r = document.createRange(); r.selectNodeContents($("detail"));
    const sel = window.getSelection(); sel.removeAllRanges(); sel.addRange(r);
    try { document.execCommand("copy"); flashPill($("connPill"), "已复制"); }
    catch (e2) { flashPill($("connPill"), "复制失败"); }
    sel.removeAllRanges();
  }
};

// ============ workspace 显示 ============
invoke("cmd_workspace_path").then(s => $("wsPath").textContent = s);

// 同花顺落盘也吐 toast
listen("ths:status", (e) => {
  const p = e.payload || {};
  if (p.state === "saved") {
    toast(`✓ 同花顺 cookie 已落盘 · ${p.count || 0} 条`);
  }
});

// init
loadSettings();
pingServer();
initialThs();
refreshServerCookie();
refreshLoginExpiry();
setInterval(pingServer, 15000);
setInterval(refreshServerCookie, 30000);
setInterval(refreshLoginExpiry, 60000);
