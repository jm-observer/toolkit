// Tauri 2 全局 API：__TAURI__.core.invoke / event.listen。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

function fmt(v) {
  try { return JSON.stringify(v, null, 2); } catch { return String(v); }
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

// ============ 抖音登录窗 ============
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

// init
loadSettings();
pingServer();
initialThs();
setInterval(pingServer, 15000);
