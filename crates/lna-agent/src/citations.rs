use std::collections::HashSet;

/// Streaming citation normalizer (Section 9.6 & C3 / B-09):
/// - Preserves math intervals like `[1,2]` and markdown links `[text](url)` verbatim.
/// - Converts single positive integer `[n]` / `【n】` into `[n]` if `n` is in `valid_nos`.
/// - Deletes invalid single citation numbers (e.g. `[99]`) and logs a warning.
pub struct CitationStreamNormalizer {
    buf: String,
    in_bracket: bool,
    valid_nos: HashSet<i32>,
}

impl CitationStreamNormalizer {
    pub fn new(valid_nos: HashSet<i32>) -> Self {
        Self {
            buf: String::new(),
            in_bracket: false,
            valid_nos,
        }
    }

    pub fn add_valid(&mut self, no: i32) {
        self.valid_nos.insert(no);
    }

    pub fn push(&mut self, chunk: &str) -> (String, Vec<i32>) {
        let mut out = String::new();
        let mut cites = Vec::new();
        for ch in chunk.chars() {
            if self.in_bracket {
                self.buf.push(ch);
                if ch == ']' || ch == '】' {
                    let raw = std::mem::take(&mut self.buf);
                    self.in_bracket = false;
                    let (norm, found) = self.classify_bracket(&raw);
                    out.push_str(&norm);
                    cites.extend(found);
                } else if self.buf.len() > 24 || ch == '[' || ch == '【' || ch == '\n' {
                    // Not a simple citation bracket; flush buffer verbatim
                    let mut raw = std::mem::take(&mut self.buf);
                    if ch == '[' || ch == '【' {
                        let last = raw.pop().unwrap();
                        out.push_str(&raw);
                        self.buf.push(last);
                        self.in_bracket = true;
                    } else {
                        self.in_bracket = false;
                        out.push_str(&raw);
                    }
                }
            } else if ch == '[' || ch == '【' {
                self.in_bracket = true;
                self.buf.push(ch);
            } else {
                out.push(ch);
            }
        }
        (out, cites)
    }

    pub fn finish(&mut self) -> (String, Vec<i32>) {
        if self.buf.is_empty() {
            return (String::new(), Vec::new());
        }
        let raw = std::mem::take(&mut self.buf);
        self.in_bracket = false;
        self.classify_bracket(&raw)
    }

    fn classify_bracket(&self, raw: &str) -> (String, Vec<i32>) {
        let is_cjk = raw.starts_with('【') && raw.ends_with('】');
        let is_ascii = raw.starts_with('[') && raw.ends_with(']');
        if !is_cjk && !is_ascii {
            return (raw.to_string(), Vec::new());
        }
        let inner = raw
            .trim_start_matches(['[', '【'])
            .trim_end_matches([']', '】'])
            .trim();
        // Only treat a single positive integer as a citation; preserve [1,2] math intervals, [text], etc.
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = inner.parse::<i32>() {
                if n > 0 {
                    if self.valid_nos.contains(&n) {
                        return (format!("[{n}]"), vec![n]);
                    } else {
                        tracing::warn!(citation_no = n, "Dropped out-of-range citation number");
                        return (String::new(), Vec::new());
                    }
                }
            }
        }
        (raw.to_string(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_math_interval_and_filters_invalid_citation() {
        let valid: HashSet<i32> = [1, 2].into_iter().collect();
        let mut norm = CitationStreamNormalizer::new(valid);
        let (out1, c1) =
            norm.push("维数定理[1]。区间 [1,2] 只是示例，无效引用[99]应删除，中文【2】保留。");
        let (out2, c2) = norm.finish();
        let full = format!("{out1}{out2}");
        let mut all_cites = c1;
        all_cites.extend(c2);
        assert!(full.contains("[1]"));
        assert!(full.contains("[1,2]"));
        assert!(!full.contains("[99]"));
        assert!(full.contains("[2]"));
        assert_eq!(all_cites, vec![1, 2]);
    }

    #[test]
    fn preserves_markdown_links_split_chunks_and_long_brackets() {
        let valid: HashSet<i32> = [1, 2].into_iter().collect();
        let mut norm = CitationStreamNormalizer::new(valid);
        let (a, c1) = norm.push("参考 [线性代数](https://example.com) 与区间 [1");
        let (b, c2) = norm.push(",2] 以及跨块引用 [");
        let (c, c3) = norm.push("1] 和 [这是一个超过二十四个字符的普通方括号说明文本]。");
        let (d, c4) = norm.finish();
        let full = format!("{a}{b}{c}{d}");
        assert!(full.contains("[线性代数](https://example.com)"));
        assert!(full.contains("[1,2]"));
        assert!(full.contains("[1]"));
        assert!(full.contains("[这是一个超过二十四个字符的普通方括号说明文本]"));
        let mut all = c1;
        all.extend(c2);
        all.extend(c3);
        all.extend(c4);
        assert_eq!(all, vec![1]);
    }
}
