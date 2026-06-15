// Codeloop 双栏只读观测：选一对会话 → 各自轮询增量消息 → 双栏渲染。
// 本轮（Plan 2）只做只读观测，不做循环 / 弹窗 / 启动按钮（Plan 5）。

const POLL_MS = 1500;
const API = "/api/web/codeloop";

// 每一侧的运行态。
const sides = {
  claude: makeSide("claude"),
  codex: makeSide("codex"),
};

function makeSide(provider) {
  return {
    provider,
    sessionId: "",
    cursor: 0,
    autoScroll: true,
    el: null, // messages container，init 时绑定
    metaEl: null,
  };
}

function makeBubble(role, text) {
  const div = document.createElement("div");
  div.className = "bubble " + (role === "user" ? "user" : "assistant");
  const tag = document.createElement("span");
  tag.className = "role-tag";
  tag.textContent = role;
  div.appendChild(tag);
  div.appendChild(document.createTextNode(text));
  return div;
}

// 纯工具/思考消息：正文只由 [tool_use: ..] / [thinking] / [tool_result] 标记拼成。
function isToolOnly(text) {
  const stripped = text.replace(/\[(tool_use:[^\]]*|thinking|tool_result)\]/g, "").trim();
  return stripped.length === 0 && /\[(tool_use|thinking|tool_result)/.test(text);
}

function makeToolFold(text) {
  const det = document.createElement("details");
  det.className = "tool-fold";
  const sum = document.createElement("summary");
  sum.textContent = "🔧 工具调用 / 思考";
  det.appendChild(sum);
  const pre = document.createElement("pre");
  pre.textContent = text;
  det.appendChild(pre);
  return det;
}

function renderMessage(side, msg) {
  if (isToolOnly(msg.text)) {
    return makeToolFold(msg.text);
  }
  return makeBubble(msg.role, msg.text);
}

function nearBottom(el) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < 60;
}

async function loadSessions() {
  const res = await fetch(`${API}/sessions?limit=30`);
  if (!res.ok) return;
  const rows = await res.json();
  const claudeRows = rows.filter((r) => r.provider === "claude");
  const codexRows = rows.filter((r) => r.provider === "codex");
  fillSelect(document.getElementById("claude-select"), claudeRows);
  fillSelect(document.getElementById("codex-select"), codexRows);
}

function fillSelect(sel, rows) {
  const prev = sel.value;
  sel.innerHTML = '<option value="">（选择会话）</option>';
  for (const r of rows) {
    const opt = document.createElement("option");
    opt.value = r.id;
    const title = r.title && r.title.trim() ? r.title : r.id;
    opt.textContent = `[${r.status}] ${title}`;
    sel.appendChild(opt);
  }
  if (prev) sel.value = prev;
}

function resetSide(side, sessionId) {
  side.sessionId = sessionId;
  side.cursor = 0;
  side.autoScroll = true;
  side.el.innerHTML = "";
  if (!sessionId) {
    side.el.innerHTML = '<div class="empty-hint">未选择会话</div>';
    side.metaEl.textContent = "—";
  } else {
    side.metaEl.textContent = sessionId.slice(0, 8) + "…";
  }
}

async function pollSide(side) {
  if (!side.sessionId) return;
  let res;
  try {
    res = await fetch(
      `${API}/session/${side.provider}/${encodeURIComponent(side.sessionId)}/messages?after=${side.cursor}`
    );
  } catch {
    return;
  }
  if (!res.ok) return;
  const page = await res.json();
  if (page.messages && page.messages.length) {
    const stick = nearBottom(side.el) || side.autoScroll;
    for (const m of page.messages) {
      side.el.appendChild(renderMessage(side, m));
    }
    if (stick) side.el.scrollTop = side.el.scrollHeight;
  }
  side.cursor = page.cursor;
}

function updatePollState() {
  const el = document.getElementById("poll-state");
  const active = sides.claude.sessionId || sides.codex.sessionId;
  if (active) {
    el.textContent = "观测中 ●";
    el.className = "running";
  } else {
    el.textContent = "未配对";
    el.className = "idle";
  }
}

async function tick() {
  await Promise.all([pollSide(sides.claude), pollSide(sides.codex)]);
}

function init() {
  sides.claude.el = document.getElementById("claude-messages");
  sides.claude.metaEl = document.getElementById("claude-meta");
  sides.codex.el = document.getElementById("codex-messages");
  sides.codex.metaEl = document.getElementById("codex-meta");

  // 用户上滚则暂停自动滚动；滚回底部恢复。
  for (const key of ["claude", "codex"]) {
    const side = sides[key];
    side.el.addEventListener("scroll", () => {
      side.autoScroll = nearBottom(side.el);
    });
    resetSide(side, "");
  }

  document.getElementById("claude-select").addEventListener("change", (e) => {
    resetSide(sides.claude, e.target.value);
    updatePollState();
  });
  document.getElementById("codex-select").addEventListener("change", (e) => {
    resetSide(sides.codex, e.target.value);
    updatePollState();
  });
  document.getElementById("refresh-sessions").addEventListener("click", loadSessions);

  loadSessions();
  updatePollState();
  setInterval(tick, POLL_MS);
}

document.addEventListener("DOMContentLoaded", init);
