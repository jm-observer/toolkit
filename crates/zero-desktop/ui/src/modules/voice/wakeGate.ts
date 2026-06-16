// 唤醒词门控（wake gate）。设计：docs/voice-command-agent-design.md §3.1。
//
// 判断一条优化后的文本是否「以唤醒词开头」。ASR 对 zero 极不稳定，所以做
// 前缀模糊匹配 + 小编辑距离容错。命中则剥掉唤醒词前缀返回指令文本；剥后为空
// 或未命中返回 null（→ 走普通听写，不送 zero）。

/** 默认唤醒词变体表（可配，§3.1）。 */
export const DEFAULT_WAKE_WORDS = ['zero', 'Zero', '泽罗', '知乎', '零', '子萝'];

/** Levenshtein 编辑距离（短串，O(n*m) 足够）。 */
function editDistance(a: string, b: string): number {
  const m = a.length;
  const n = b.length;
  if (m === 0) return n;
  if (n === 0) return m;
  let prev = new Array<number>(n + 1);
  let curr = new Array<number>(n + 1);
  for (let j = 0; j <= n; j++) prev[j] = j;
  for (let i = 1; i <= m; i++) {
    curr[0] = i;
    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(prev[j] + 1, curr[j - 1] + 1, prev[j - 1] + cost);
    }
    [prev, curr] = [curr, prev];
  }
  return prev[n];
}

/** 一个唤醒词允许的最大编辑距离：越短越严，避免误命中。 */
function allowedDistance(word: string): number {
  if (word.length <= 1) return 0;
  if (word.length <= 3) return 1;
  return 2;
}

// 命中后需要剥掉的中文标点 / 分隔符（zero、 你好 → 你好）。
const SEP_RE = /^[\s,，.。!！?？:：;；、~～-]+/;

function stripLeadingSep(s: string): string {
  return s.replace(SEP_RE, '');
}

function normalize(s: string): string {
  return s.trim().toLowerCase();
}

/**
 * 门控判定。
 * @returns 命中且剥前缀后非空 → 指令文本；否则 null。
 */
export function wakeGate(
  rawText: string,
  wakeWords: string[] = DEFAULT_WAKE_WORDS,
): string | null {
  const text = rawText.trim();
  if (!text) return null;
  const lower = text.toLowerCase();

  let bestPrefixLen = -1;

  for (const wRaw of wakeWords) {
    const w = normalize(wRaw);
    if (!w) continue;
    const head = lower.slice(0, w.length);
    if (head.length < w.length) continue;

    // 精确前缀优先；否则按编辑距离容错。
    const dist = head === w ? 0 : editDistance(head, w);
    if (dist <= allowedDistance(w)) {
      if (w.length > bestPrefixLen) bestPrefixLen = w.length;
    }
  }

  if (bestPrefixLen < 0) return null;

  const remainder = stripLeadingSep(text.slice(bestPrefixLen));
  return remainder.length > 0 ? remainder : null;
}
