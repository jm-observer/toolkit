//! 通用文本归一与切块。
//!
//! **做**：换行规范化（`\r\n` / `\r` → `\n`）、段落分隔统一（连续 ≥2 个 `\n` → `\n\n`）、
//! 段内空白折叠、首段 `title_hint` 抽取、按字符上限切块（切点优先级：段落 > 句末 > 次级 > 字符）+
//! overlap 回退。
//!
//! **不做**（属调用方职责）：NFKC 归一、Markdown 解析、HTML 清洗、抖音口语化处理、去重。

/// 归一文本：换行/空白折叠。返回的文本中：
/// - 段落分隔 = `"\n\n"`（且仅在段落之间出现，首尾无）
/// - 段落内所有连续空白（含 tab / 全角空格 U+3000 / 普通空格）折叠为单 ASCII 空格
/// - 单 `\n` 在段内被视作空白，等同于其他空白折叠
/// - 整体前后 trim
pub fn normalize(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // 1. 行尾归一
    let s = text.replace("\r\n", "\n").replace('\r', "\n");
    // 2. 按 "≥2 个连续 \n" 切分为段落（保留单 \n 留待段内折叠）
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // 计算连续 \n 数量
            let mut j = i;
            while j < bytes.len() && bytes[j] == b'\n' {
                j += 1;
            }
            if j - i >= 2 {
                // 段落边界
                paragraphs.push(collapse_inner_whitespace(&current));
                current.clear();
            } else {
                // 单 \n → 当作空白
                current.push(' ');
            }
            i = j;
        } else {
            // 按字符前进（处理多字节 UTF-8）
            let ch = s[i..].chars().next().unwrap();
            current.push(ch);
            i += ch.len_utf8();
        }
    }
    paragraphs.push(collapse_inner_whitespace(&current));
    paragraphs.retain(|p| !p.is_empty());
    paragraphs.join("\n\n")
}

fn collapse_inner_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_ws = true; // 段首吃空白
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
                last_was_ws = true;
            }
        } else {
            out.push(ch);
            last_was_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// 抽首段作 title_hint，长度 ≤ `limit` 字符时返回，否则 `None`。
pub fn extract_title_hint(normalized: &str, limit: usize) -> Option<String> {
    let first_para = normalized.split("\n\n").next()?;
    let count = first_para.chars().count();
    if count <= limit && count > 0 {
        Some(first_para.to_string())
    } else {
        None
    }
}

/// 按字符上限切块。`max_chars` 上界 + `overlap` 重叠字符；切点优先级：
/// 段落分隔 `\n\n` → 句末 `。.!?！？` → 次级 `；;，,、` → 字符。
///
/// `normalized` 应是 [`normalize`] 输出（即段落分隔统一为 `\n\n`）。
pub fn chunk_text(normalized: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    if normalized.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= max_chars {
        return vec![normalized.to_string()];
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut start: usize = 0;
    while start < chars.len() {
        let end_hard = (start + max_chars).min(chars.len());
        let end = if end_hard == chars.len() {
            end_hard
        } else {
            find_best_split(&chars, start, end_hard).unwrap_or(end_hard)
        };
        let slice: String = chars[start..end].iter().collect();
        let slice = slice.trim().to_string();
        if !slice.is_empty() {
            chunks.push(slice);
        }
        if end >= chars.len() {
            break;
        }
        // 下一块起点：在段落分隔后无 overlap；否则回退 overlap
        let next_start = if is_paragraph_boundary(&chars, end) {
            end
        } else {
            end.saturating_sub(overlap).max(start + 1)
        };
        if next_start <= start {
            // 防御：极端切点 + overlap 导致回退到原点；强制至少前进 1
            start += 1;
        } else {
            start = next_start;
        }
    }
    chunks
}

fn is_paragraph_boundary(chars: &[char], pos: usize) -> bool {
    // 切点恰好在 \n\n 之后？检查 [pos-2..pos] == "\n\n"
    pos >= 2 && chars[pos - 1] == '\n' && chars[pos - 2] == '\n'
}

/// 在 `[start, end_excl)` 区间内从右往左找最佳切点。
/// 返回值是 `chars` 绝对索引（chunk 的右开端）。
fn find_best_split(chars: &[char], start: usize, end_excl: usize) -> Option<usize> {
    if end_excl <= start {
        return None;
    }
    // 段落分隔 "\n\n"
    let mut i = end_excl;
    while i > start + 1 {
        if chars[i - 1] == '\n' && chars[i - 2] == '\n' {
            return Some(i);
        }
        i -= 1;
    }
    // 句末标点
    const SENTENCE_END: &[char] = &['。', '.', '!', '?', '！', '？'];
    let mut i = end_excl;
    while i > start {
        if SENTENCE_END.contains(&chars[i - 1]) {
            return Some(i);
        }
        i -= 1;
    }
    // 次级标点
    const SECONDARY: &[char] = &['；', ';', '，', ',', '、'];
    let mut i = end_excl;
    while i > start {
        if SECONDARY.contains(&chars[i - 1]) {
            return Some(i);
        }
        i -= 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_passes_through_clean_text() {
        let out = normalize("hello world");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn normalize_collapses_whitespace_and_crlf() {
        let input = "alpha\r\n\r\nbeta   gamma\n\n\n\ndelta\t epsilon  \r\n";
        let out = normalize(input);
        assert_eq!(out, "alpha\n\nbeta gamma\n\ndelta epsilon");
    }

    #[test]
    fn normalize_handles_fullwidth_space() {
        // U+3000 IDEOGRAPHIC SPACE 也算 whitespace
        let out = normalize("中文\u{3000}空格");
        assert_eq!(out, "中文 空格");
    }

    #[test]
    fn normalize_drops_empty_paragraphs() {
        let out = normalize("\n\n\n\n   \n\nactual\n\n");
        assert_eq!(out, "actual");
    }

    #[test]
    fn extract_title_hint_short_first_para() {
        let n = normalize("Title\n\nbody");
        assert_eq!(extract_title_hint(&n, 60), Some("Title".to_string()));
    }

    #[test]
    fn extract_title_hint_too_long_returns_none() {
        let long = "x".repeat(70);
        let n = normalize(&format!("{long}\n\nbody"));
        assert_eq!(extract_title_hint(&n, 60), None);
    }

    #[test]
    fn chunk_text_short_returns_single() {
        let text = "alpha beta gamma";
        let chunks = chunk_text(text, 800, 80);
        assert_eq!(chunks, vec![text.to_string()]);
    }

    #[test]
    fn chunk_text_splits_at_paragraph() {
        let text = "first paragraph stuff.\n\nsecond paragraph stuff.";
        // max=30 forces split; 段落分隔位于位置 ~22 之后
        let chunks = chunk_text(text, 30, 5);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].contains("first"));
        assert!(chunks.iter().any(|c| c.contains("second")));
    }

    #[test]
    fn chunk_text_splits_at_sentence_when_no_paragraph() {
        let text = "abc. def. ghi. jkl. mno.";
        // max=10 → 应在句末切
        let chunks = chunk_text(text, 10, 2);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            // 每个 chunk 都不是从空白开始（trim）
            assert_eq!(c.trim(), c);
        }
    }

    #[test]
    fn chunk_text_overlap_carries_context() {
        let text = "AAAAAA BBBBBB CCCCCC DDDDDD EEEEEE FFFFFF";
        let chunks = chunk_text(text, 15, 5);
        assert!(chunks.len() >= 2);
        // 第二块应非空（具体重叠由 split 点决定）
        assert!(!chunks[1].is_empty());
    }

    #[test]
    fn chunk_text_progresses_when_no_split_point() {
        // 全是单字符无标点，应该走 hard cut；不能死循环
        let text: String = "a".repeat(50);
        let chunks = chunk_text(&text, 10, 3);
        assert!(chunks.len() >= 4);
    }

    #[test]
    fn chunk_text_no_paragraph_overlap_carries() {
        let text = "abc def ghi jkl mno pqr stu vwx yz0 123 456";
        let chunks = chunk_text(text, 15, 4);
        // 验证每块都非空且 trim 干净
        for c in &chunks {
            assert!(!c.is_empty());
            assert_eq!(c.trim(), c);
        }
    }
}
