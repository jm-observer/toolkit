// Tauri 2 全局 API：__TAURI__.core.invoke / event.listen。
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

function fmt(v) {
  try { return JSON.stringify(v, null, 2); } catch { return String(v); }
}

function fmtDuration(secs) {
  if (secs == null) return "?";
  if (secs < 0) return "已过期";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  if (d > 0) return `${d}d${h ? h + "h" : ""}`;
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h${m ? m + "m" : ""}`;
  return `${m}m`;
}

// ============ toast ============
let toastTimer = null;
function toast(msg, kind) {
  const t = $("toast");
  t.textContent = msg;
  t.className = "show" + (kind ? " toast-" + kind : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.className = ""; }, 3200);
}

// ============ status 行渲染 ============
function setStatus(rowId, valueId, text, kind, tooltip) {
  $(rowId).className = "status" + (rowId === "connStatus" ? " clickable" : "") + " " + (kind || "");
  $(valueId).textContent = text;
  if (tooltip) $(rowId).title = tooltip;
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
  toast("已保存");
  pingServer();
  refreshServerCookie();
};
document.querySelectorAll("button[data-preset]").forEach((b) => {
  b.onclick = () => { $("server").value = b.dataset.preset; };
});

// ============ 登录按钮 ============
$("login").onclick = () => invoke("cmd_open_login");
$("closeLogin").onclick = () => invoke("cmd_close_login");
$("thsLogin").onclick = () => invoke("cmd_open_ths_login");
$("thsClose").onclick = () => invoke("cmd_close_ths_login");
$("inspect").onclick = async () => {
  $("detail").textContent = "inspecting…";
  try {
    const r = await invoke("cmd_inspect_cookies");
    $("detail").textContent = JSON.stringify(r, null, 2);
  } catch (e) {
    $("detail").textContent = String(e);
  }
};

$("trackCreator").onclick = async () => {
  $("detail").textContent = "解析中…";
  try {
    const r = await invoke("cmd_track_current_creator");
    $("detail").textContent = JSON.stringify(r, null, 2);
    toast(`✓ 已加入博主库: ${r.nickname || r.unique_id || "?"}`);
  } catch (e) {
    $("detail").textContent = String(e);
    toast("解析失败", "err");
  }
};

// ============ G10 连接 pill ============
async function pingServer() {
  setStatus("connStatus", "connValue", "…", "");
  try {
    const r = await invoke("cmd_ping_server");
    if (r.state === "unconfigured") {
      setStatus("connStatus", "connValue", "未配置", "", "请填 server base 并保存");
    } else if (r.state === "ok") {
      setStatus("connStatus", "connValue", `ok · ${r.latency_ms}ms`, "ok",
        `${r.server_base}\nversion: ${r.server_version || "?"}`);
    } else if (r.state === "http_err") {
      setStatus("connStatus", "connValue", `HTTP ${r.status}`, "warn", r.server_base);
    } else {
      setStatus("connStatus", "connValue", "不可达", "err",
        (r.error || "") + "\n" + (r.server_base || ""));
    }
  } catch (e) {
    setStatus("connStatus", "connValue", "err", "err", String(e));
  }
}
$("connStatus").onclick = pingServer;

// ============ 抖音 cookie 状态（综合 uploader + G10 cookie_status） ============
let lastUploader = null;
let lastG10 = null;

function recomputeDouyin() {
  // 优先级：uploader 实时事件 > G10 落地状态
  if (lastUploader) {
    const p = lastUploader;
    switch (p.state) {
      case "uploaded":
      case "unchanged":
        setStatus("douyinStatus", "douyinValue",
          `已同步 · ${p.fields || "?"} 字段`, "ok",
          p.at ? "上次同步: " + p.at : "");
        return;
      case "waiting_login":
        const miss = (p.missing || []).join(",");
        setStatus("douyinStatus", "douyinValue",
          miss ? `登录中 · 等 ${miss}` : "登录中", "warn", p.hint || "");
        return;
      case "no_login_window":
        // 落到 G10 兜底
        break;
      case "unconfigured":
        setStatus("douyinStatus", "douyinValue", "未配置 server", "", "");
        return;
      case "error":
        setStatus("douyinStatus", "douyinValue", "err", "err", p.error || "");
        return;
    }
  }
  // 未开登录窗：看 G10 是不是还有效
  if (lastG10) {
    const b = lastG10;
    if (b.logged_in && b.has_required) {
      setStatus("douyinStatus", "douyinValue",
        `G10 仍有效 · ${b.fields || 0}`, "ok",
        `user_uid=${b.user_uid || "?"}\n（未开登录窗也可用业务接口）`);
    } else if (b.has_required) {
      setStatus("douyinStatus", "douyinValue", "G10 cookie 失效", "warn",
        "字段齐但 self_info 校验失败，建议重登");
    } else if ((b.fields || 0) > 0) {
      setStatus("douyinStatus", "douyinValue",
        `G10 缺字段 · ${(b.missing || []).join(",")}`, "warn", fmt(b));
    } else {
      setStatus("douyinStatus", "douyinValue", "无 cookie", "", "G10 上无任何抖音 cookie，请点「抖音登录」");
    }
    return;
  }
  setStatus("douyinStatus", "douyinValue", "—", "", "");
}

function recomputeMsToken() {
  // 优先 uploader 事件里的 missing；其次根据 douyin 整体状态推断
  if (lastUploader) {
    const p = lastUploader;
    if (p.state === "uploaded" || p.state === "unchanged") {
      setStatus("msStatus", "msValue", "有", "ok");
      return;
    }
    if (p.state === "waiting_login") {
      const missing = p.missing || [];
      if (missing.includes("msToken")) {
        setStatus("msStatus", "msValue", "缺", "warn",
          "登录后在登录窗里浏览首页 / 视频几秒，前端 SDK 会写入 msToken");
        return;
      }
      setStatus("msStatus", "msValue", "等登录", "");
      return;
    }
    if (p.state === "no_login_window") {
      setStatus("msStatus", "msValue", "无登录窗", "");
      return;
    }
  }
  setStatus("msStatus", "msValue", "—", "");
}

// ============ uploader 事件接收 ============
listen("uploader:status", (e) => {
  lastUploader = e.payload || {};
  $("detail").textContent = fmt(lastUploader);
  recomputeDouyin();
  recomputeMsToken();
  if (lastUploader.state === "uploaded") {
    toast(`✓ 抖音 cookie 已同步 · ${lastUploader.fields || 0} 字段`);
    refreshServerCookie();
    refreshLoginExpiry();
  }
});

// ============ G10 server cookie 状态 ============
async function refreshServerCookie() {
  try {
    const r = await invoke("cmd_check_server_cookie");
    if (r.state === "ok") {
      lastG10 = r.body || {};
    } else {
      lastG10 = null;
    }
    recomputeDouyin();
  } catch (e) {
    lastG10 = null;
    recomputeDouyin();
  }
}

// ============ 同花顺 status ============
async function refreshThs() {
  try {
    const r = await invoke("cmd_ths_status");
    if (!r.exists) {
      setStatus("thsStatus", "thsValue", "无 cookie", "",
        "请点「同花顺登录」完成登录");
    } else if (!r.has_required) {
      setStatus("thsStatus", "thsValue",
        `缺 ${(r.missing || []).join(",")}`, "warn", fmt(r));
    } else if (r.ticket_is_session) {
      setStatus("thsStatus", "thsValue", "session 票", "warn",
        "ticket 是 session cookie，关窗即失效。登录时务必勾「记住我」。");
    } else {
      let tail = "";
      if (r.ticket_expires_at) {
        const expTs = new Date(r.ticket_expires_at).getTime() / 1000;
        const now = Date.now() / 1000;
        tail = ` · 余 ${fmtDuration(expTs - now)}`;
      }
      setStatus("thsStatus", "thsValue", `已登录 · ${r.count}${tail}`, "ok",
        `ticket 过期: ${r.ticket_expires_at || "?"}`);
    }
  } catch (e) {
    setStatus("thsStatus", "thsValue", "err", "err", String(e));
  }
}
listen("ths:status", (e) => {
  const p = e.payload || {};
  if (p.state === "saved") {
    toast(`✓ 同花顺 cookie 已落盘 · ${p.count || 0} 条`);
    refreshThs();
  }
});

// ============ 登录 cookie 失效时间 ============
async function refreshLoginExpiry() {
  try {
    const r = await invoke("cmd_login_expiry");
    const line = $("expiryInfo");
    if (r.state === "no_window") {
      line.textContent = "登录失效：—（登录窗未开）";
      line.style.color = "var(--muted)";
      return;
    }
    if (!r.critical || r.critical.length === 0) {
      line.textContent = `登录失效：未检测到关键 cookie（${r.cookies_total || 0} 条）`;
      line.style.color = "var(--warn)";
      return;
    }
    const remaining = r.earliest_remaining_secs;
    const at = r.earliest_expires_at ? r.earliest_expires_at.slice(0, 19).replace("T", " ") : "?";
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
    $("expiryInfo").textContent = "登录失效：" + String(e);
  }
}

// ============ 复制日志 ============
$("copyDetail").onclick = async () => {
  const text = $("detail").textContent || "";
  try { await navigator.clipboard.writeText(text); toast("已复制"); }
  catch (e) {
    const r = document.createRange(); r.selectNodeContents($("detail"));
    const sel = window.getSelection(); sel.removeAllRanges(); sel.addRange(r);
    try { document.execCommand("copy"); toast("已复制"); }
    catch (e2) { toast("复制失败", "err"); }
    sel.removeAllRanges();
  }
};

// ============ workspace ============
invoke("cmd_workspace_path").then((s) => ($("wsPath").textContent = s));

// init
loadSettings();
pingServer();
refreshThs();
refreshServerCookie();
refreshLoginExpiry();
setInterval(pingServer, 15000);
setInterval(refreshServerCookie, 30000);
setInterval(refreshLoginExpiry, 60000);
setInterval(refreshThs, 60000);
