// Tauri 2 全局 API：__TAURI__.core.invoke / event.listen。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

async function loadSettings() {
  const s = await invoke("cmd_get_settings");
  $("server").value = s.server_base || "";
  $("token").value = s.auth_token || "";
  $("lastAt").textContent = s.last_uploaded_at || "—";
}

async function loadWorkspace() {
  $("wsPath").textContent = await invoke("cmd_workspace_path");
}

async function loadHistory() {
  const rows = await invoke("cmd_recent_uploads", { limit: 10 });
  if (!rows.length) {
    $("history").textContent = "—";
    return;
  }
  $("history").textContent = rows
    .map((r) => {
      const tag = r.success ? "✓" : "✗";
      const tail = r.success ? r.fields_count + " fields" : (r.error || "").slice(0, 60);
      return `${tag} ${r.ts}  ${tail}`;
    })
    .join("\n");
}

async function saveSettings() {
  await invoke("cmd_save_settings", {
    settings: {
      server_base: $("server").value.trim(),
      auth_token: $("token").value || null,
      last_uploaded_at: null,
    },
  });
  flash("已保存");
}

function flash(msg) {
  const s = $("state");
  const prev = s.textContent;
  s.textContent = msg;
  setTimeout(() => (s.textContent = prev), 1200);
}

document.querySelectorAll("button[data-preset]").forEach((b) => {
  b.onclick = () => {
    $("server").value = b.dataset.preset;
    flash("已填入");
  };
});

$("save").onclick = saveSettings;
$("login").onclick = () => invoke("cmd_open_login");
$("closeLogin").onclick = () => invoke("cmd_close_login");
$("force").onclick = () => invoke("cmd_force_upload_now");
$("copyDetail").onclick = async () => {
  const text = $("detail").textContent || "";
  try {
    await navigator.clipboard.writeText(text);
    flash("已复制");
  } catch (e) {
    // 兜底：选中 + execCommand（老 API，WebView2 仍支持）
    const r = document.createRange(); r.selectNodeContents($("detail"));
    const sel = window.getSelection(); sel.removeAllRanges(); sel.addRange(r);
    try { document.execCommand("copy"); flash("已复制"); }
    catch (e2) { flash("复制失败"); }
    sel.removeAllRanges();
  }
};

$("inspect").onclick = async () => {
  $("detail").textContent = "inspecting…";
  try {
    const r = await invoke("cmd_inspect_cookies");
    $("detail").textContent = JSON.stringify(r, null, 2);
  } catch (e) {
    $("detail").textContent = String(e);
  }
};
$("refreshHistory").onclick = loadHistory;

listen("uploader:status", (e) => {
  const p = e.payload || {};
  const s = $("state");
  s.className = "state-" + (p.state || "unchanged");
  s.textContent = p.state || "—";
  if (p.at) $("lastAt").textContent = p.at;
  $("detail").textContent = JSON.stringify(p, null, 2);
  if (p.state === "uploaded") loadHistory();
});

async function pingServer() {
  const p = $("connPill");
  p.textContent = "…";
  p.className = "pill pill-muted";
  try {
    const r = await invoke("cmd_ping_server");
    if (r.state === "unconfigured") {
      p.textContent = "未配置";
      p.className = "pill pill-muted";
      p.title = "请填 server base 并保存";
    } else if (r.state === "ok") {
      p.textContent = `已连接 · ${r.latency_ms}ms`;
      p.className = "pill pill-ok";
      p.title = `${r.server_base}\nversion: ${r.server_version || "?"}`;
    } else if (r.state === "http_err") {
      p.textContent = `HTTP ${r.status}`;
      p.className = "pill pill-warn";
      p.title = r.server_base;
    } else {
      p.textContent = "不可达";
      p.className = "pill pill-err";
      p.title = (r.error || "") + "\n" + (r.server_base || "");
    }
  } catch (e) {
    p.textContent = "err";
    p.className = "pill pill-err";
    p.title = String(e);
  }
}
$("connPill").onclick = pingServer;

async function refreshThs() {
  try {
    const r = await invoke("cmd_ths_status");
    const p = $("thsPill");
    if (!r.exists) {
      p.textContent = "未登录"; p.className = "pill pill-muted";
    } else if (!r.has_required) {
      p.textContent = "缺字段"; p.className = "pill pill-warn";
    } else if (r.ticket_is_session) {
      p.textContent = "session 票"; p.className = "pill pill-warn";
    } else {
      p.textContent = `已登录 · ${r.count}`; p.className = "pill pill-ok";
    }
    $("thsDetail").textContent = JSON.stringify(r, null, 2);
  } catch (e) {
    $("thsDetail").textContent = String(e);
  }
}
$("thsLogin").onclick = () => invoke("cmd_open_ths_login");
$("thsClose").onclick = () => invoke("cmd_close_ths_login");
$("thsRefresh").onclick = refreshThs;
listen("ths:status", (e) => {
  const p = e.payload || {};
  $("thsDetail").textContent = JSON.stringify(p, null, 2);
  if (p.state === "saved") refreshThs();
});

loadSettings();
loadWorkspace();
loadHistory();
pingServer();
refreshThs();
setInterval(pingServer, 15000);
