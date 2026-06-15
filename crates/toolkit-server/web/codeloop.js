// Codeloop 双栏视图：选一对会话 → 双栏轮询增量消息（Plan 2）
// + 启动复核循环、状态条、ASK_USER 模拟弹窗（Plan 5）。

const POLL_MS = 1500;
const API = "/api/web/codeloop";
const TASKS_API = "/api/web/tasks";

// 每一侧的运行态。
const sides = {
  claude: makeSide("claude"),
  codex: makeSide("codex"),
};

// 复核循环运行态。
const loop = {
  taskId: null,
  answeredSeq: -1, // 已应答的最大 seq，避免重复弹窗
  pendingSeq: null, // 当前弹窗对应的 seq
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

// ── 复核循环（Plan 5） ──────────────────────────────────

async function startLoop() {
  const claudeId = sides.claude.sessionId;
  const codexId = sides.codex.sessionId;
  const targetPath = document.getElementById("target-path").value.trim();
  if (!claudeId || !codexId) {
    alert("请先各选一个 Claude / Codex 会话");
    return;
  }
  if (!targetPath) {
    alert("请填写 target_path");
    return;
  }
  const body = {
    claude: { session_id: claudeId },
    codex: { session_id: codexId },
    target_path: targetPath,
    mode: document.getElementById("mode-select").value,
    max_rounds: parseInt(document.getElementById("max-rounds").value, 10) || 5,
    wait_for_claude_idle: document.getElementById("wait-idle").checked,
  };
  const btn = document.getElementById("start-loop");
  btn.disabled = true;
  try {
    const res = await fetch(`${API}/submit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const data = await res.json();
    if (!res.ok) {
      alert("启动失败：" + (data.error || res.status));
      btn.disabled = false;
      return;
    }
    loop.taskId = data.task_id;
    loop.answeredSeq = -1;
    loop.pendingSeq = null;
  } catch (e) {
    alert("启动失败：" + e);
    btn.disabled = false;
  }
}

function setLoopStatus(text, cls) {
  const el = document.getElementById("loop-status");
  el.textContent = "循环：" + text;
  el.className = cls;
}

async function pollTask() {
  if (!loop.taskId) return;
  let res;
  try {
    res = await fetch(`${TASKS_API}/${loop.taskId}`);
  } catch {
    return;
  }
  if (!res.ok) return;
  const t = await res.json();
  const p = t.progress || {};
  const maxRounds = parseInt(document.getElementById("max-rounds").value, 10) || 5;

  // 轮次 / verdict 显示。
  document.getElementById("loop-rounds").textContent =
    p.round != null ? `轮次 ${p.round}/${maxRounds}` : "";
  document.getElementById("loop-verdict").textContent =
    p.verdict != null && p.verdict !== "parse_failed" ? `判定 ${p.verdict}` : "";

  // 任务状态 → 循环状态条。
  if (t.state === "succeeded") {
    const fv = (t.output && t.output.final_verdict) || p.final_verdict || "done";
    setLoopStatus(fv, fv === "pass" ? "done" : "aborted");
    document.getElementById("start-loop").disabled = false;
  } else if (t.state === "failed") {
    setLoopStatus("failed: " + (t.error || ""), "failed");
    document.getElementById("start-loop").disabled = false;
  } else if (t.state === "interrupted") {
    setLoopStatus("interrupted（server 重启）", "aborted");
    document.getElementById("start-loop").disabled = false;
  } else {
    setLoopStatus(p.phase || t.state || "running", "running");
  }

  // ASK_USER 挂起 → 弹窗。
  if (p.phase === "awaiting_input" && p.seq != null) {
    if (p.seq > loop.answeredSeq && loop.pendingSeq !== p.seq) {
      showModal(p.seq, p.question || {});
    }
  } else if (loop.pendingSeq == null) {
    hideModal();
  }
}

function showModal(seq, question) {
  loop.pendingSeq = seq;
  document.getElementById("modal-question").textContent =
    question.question || "需要你拍板";
  const opts = document.getElementById("modal-options");
  opts.innerHTML = "";
  for (const o of question.options || []) {
    const b = document.createElement("button");
    b.textContent = o;
    b.addEventListener("click", () => sendAnswer(seq, o));
    opts.appendChild(b);
  }
  document.getElementById("modal-free-input").value = "";
  document.getElementById("modal-overlay").classList.remove("hidden");
}

function hideModal() {
  document.getElementById("modal-overlay").classList.add("hidden");
  loop.pendingSeq = null;
}

async function sendAnswer(seq, text) {
  if (!text || !text.trim()) {
    alert("请输入答复");
    return;
  }
  try {
    const res = await fetch(`${API}/${loop.taskId}/answer`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ seq, text }),
    });
    if (!res.ok) {
      const d = await res.json().catch(() => ({}));
      alert("应答失败：" + (d.error || res.status));
      return;
    }
    loop.answeredSeq = Math.max(loop.answeredSeq, seq);
    hideModal();
  } catch (e) {
    alert("应答失败：" + e);
  }
}

async function tick() {
  await Promise.all([pollSide(sides.claude), pollSide(sides.codex), pollTask()]);
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
  document.getElementById("start-loop").addEventListener("click", startLoop);
  document.getElementById("modal-free-send").addEventListener("click", () => {
    if (loop.pendingSeq != null) {
      sendAnswer(loop.pendingSeq, document.getElementById("modal-free-input").value);
    }
  });

  loadSessions();
  updatePollState();
  setInterval(tick, POLL_MS);
}

document.addEventListener("DOMContentLoaded", init);
