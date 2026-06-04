// toolkit 控制台前端。无构建步骤，纯 ES module。
// 与 /api/web/* 一一对应；逻辑 = 拿表单值 → fetch → 展示。

const BASE = ""; // 同源；改成 "http://192.168.0.68:8788" 即可跨机部署

async function call(path, opts = {}) {
  const resp = await fetch(BASE + path, {
    headers: { "content-type": "application/json", ...(opts.headers || {}) },
    ...opts,
  });
  let json;
  try {
    json = await resp.json();
  } catch (_e) {
    json = { _raw: await resp.text() };
  }
  return { status: resp.status, ok: resp.ok, json };
}

function pretty(v) {
  return JSON.stringify(v, null, 2);
}

function show(elId, result) {
  const el = document.getElementById(elId);
  const tag = result.ok ? "" : `(HTTP ${result.status}) `;
  el.textContent = tag + pretty(result.json);
}

function parseIds(text) {
  return text
    .split(/[\s,;]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

// ---------------- handlers ----------------

const handlers = {
  async "refresh-cookie"() {
    show("cookie-status", await call("/api/web/douyin/cookie_status"));
  },

  async "set-cookie"() {
    const raw = document.getElementById("cookie-raw").value.trim();
    if (!raw) {
      show("cookie-set-result", {
        ok: false,
        status: 0,
        json: { error: "未输入 cookie" },
      });
      return;
    }
    const r = await call("/api/browser/cookie", {
      method: "POST",
      body: JSON.stringify({ raw_header: raw }),
    });
    show("cookie-set-result", r);
    // 顺便刷新 status
    show("cookie-status", await call("/api/web/douyin/cookie_status"));
  },

  async resolve() {
    const handle = document.getElementById("resolve-handle").value.trim();
    if (!handle) return;
    const q = new URLSearchParams({ handle });
    show("resolve-result", await call(`/api/web/douyin/creator?${q}`));
  },

  async "sync-works"() {
    const handle = document.getElementById("sync-handle").value.trim();
    const max_pages = parseInt(
      document.getElementById("sync-max-pages").value,
      10
    );
    if (!handle) return;
    const r = await call("/api/web/douyin/sync_works", {
      method: "POST",
      body: JSON.stringify({ handle, max_pages }),
    });
    show("sync-result", r);
    setTimeout(handlers["refresh-tasks"], 200);
  },

  async tags() {
    const unique_id = document.getElementById("tags-unique-id").value.trim();
    if (!unique_id) return;
    const q = new URLSearchParams({ unique_id });
    show("tags-result", await call(`/api/web/douyin/tags?${q}`));
  },

  async filter() {
    const unique_id = document.getElementById("filter-unique-id").value.trim();
    const tags = document.getElementById("filter-tags").value.trim();
    const match = document.getElementById("filter-match").value;
    if (!unique_id || !tags) return;
    const q = new URLSearchParams({ unique_id, tags, match });
    show("filter-result", await call(`/api/web/douyin/filter?${q}`));
  },

  async download() {
    const ids = parseIds(document.getElementById("download-ids").value);
    if (ids.length === 0) {
      show("download-result", {
        ok: false,
        status: 0,
        json: { error: "ids 为空" },
      });
      return;
    }
    const r = await call("/api/web/douyin/download", {
      method: "POST",
      body: JSON.stringify({ aweme_ids: ids }),
    });
    show("download-result", r);
    setTimeout(handlers["refresh-tasks"], 200);
  },

  async transcribe() {
    const ids = parseIds(document.getElementById("transcribe-ids").value);
    if (ids.length === 0) {
      show("transcribe-result", {
        ok: false,
        status: 0,
        json: { error: "ids 为空" },
      });
      return;
    }
    const vad = document.getElementById("transcribe-vad").checked;
    const unique_id = document
      .getElementById("transcribe-unique-id")
      .value.trim();
    const body = { aweme_ids: ids, vad };
    if (unique_id) body.unique_id = unique_id;
    const r = await call("/api/web/douyin/transcribe", {
      method: "POST",
      body: JSON.stringify(body),
    });
    show("transcribe-result", r);
    setTimeout(handlers["refresh-tasks"], 200);
  },

  async "kb-publish"() {
    const unique_id = document.getElementById("kb-unique-id").value.trim();
    if (!unique_id) return;
    const only_ids = parseIds(document.getElementById("kb-only-ids").value);
    const r = await call("/api/web/douyin/kb_publish", {
      method: "POST",
      body: JSON.stringify({ unique_id, only_ids }),
    });
    show("kb-result", r);
  },

  async "refresh-tasks"() {
    const state = document.getElementById("tasks-filter-state").value;
    const q = new URLSearchParams({ limit: "50" });
    if (state) q.set("state", state);
    const r = await call(`/api/web/tasks?${q}`);
    renderTasks(r.json);
  },
};

function renderTasks(tasks) {
  const tbody = document.querySelector("#tasks-table tbody");
  tbody.innerHTML = "";
  if (!Array.isArray(tasks)) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td colspan="5" style="color:#c62828">${pretty(tasks)}</td>`;
    tbody.appendChild(tr);
    return;
  }
  for (const t of tasks) {
    const tr = document.createElement("tr");
    const cell = (text, cls) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (cls) td.className = cls;
      return td;
    };
    tr.appendChild(cell(t.task_id));
    tr.appendChild(cell(t.kind));
    tr.appendChild(cell(t.state, `cell-state ${t.state}`));
    tr.appendChild(cell(t.created_at || ""));
    const tdRaw = document.createElement("td");
    const raw = document.createElement("div");
    raw.className = "raw";
    const sketch = {};
    if (t.output) sketch.output = t.output;
    else if (t.progress) sketch.progress = t.progress;
    if (t.error) sketch.error = t.error;
    raw.textContent = pretty(sketch);
    tdRaw.appendChild(raw);
    tr.appendChild(tdRaw);
    tbody.appendChild(tr);
  }
}

// 单一委托：所有 [data-action] 按钮
document.addEventListener("click", (e) => {
  const btn = e.target.closest("[data-action]");
  if (!btn) return;
  const action = btn.dataset.action;
  const fn = handlers[action];
  if (!fn) {
    console.warn("unknown action:", action);
    return;
  }
  fn().catch((err) => {
    console.error(action, err);
    alert(`${action} failed: ${err.message}`);
  });
});

// 启动：拉一次 cookie status + 任务列表，并启动自动刷新
handlers["refresh-cookie"]();
handlers["refresh-tasks"]();
setInterval(() => {
  const auto = document.getElementById("tasks-auto");
  if (auto && auto.checked) handlers["refresh-tasks"]();
}, 3000);
