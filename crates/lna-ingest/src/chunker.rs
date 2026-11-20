use crate::parser::{Block, BlockKind, ParsedDoc};

#[derive(Debug, Clone)]
pub struct ChunkDraft {
    pub chunk_index: i32,
    pub section_path: Option<String>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub char_start: i32,
    pub char_end: i32,
    pub content: String,
    pub token_count: i32,
}

/// Section 7.4: CJK chars = 1, other chars = len / 4, multiplied by 1.1 safety factor.
pub fn estimate_tokens(text: &str) -> i32 {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&c)
            || ('\u{3000}'..='\u{303F}').contains(&c)
            || ('\u{FF00}'..='\u{FFEF}').contains(&c)
        {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    let raw = (cjk as f64) + (other as f64) / 4.0;
    (raw * 1.1).ceil().max(1.0) as i32
}

/// Target 450 tokens, 60-token sentence overlap, 80-token tail merge within same section.
pub fn chunk_document(doc: &ParsedDoc) -> Vec<ChunkDraft> {
    const TARGET_TOKENS: i32 = 450;
    const OVERLAP_TOKENS: i32 = 60;
    const MIN_TAIL_TOKENS: i32 = 80;

    let mut chunks: Vec<ChunkDraft> = Vec::new();
    let mut cur_parts: Vec<String> = Vec::new();
    let mut cur_tokens = 0i32;
    let mut cur_section: Option<String> = None;
    let mut cur_page_start: Option<i32> = None;
    let mut cur_page_end: Option<i32> = None;
    let mut cur_char_start: i32 = 0;
    let mut cur_char_end: i32 = 0;

    let flush = |chunks: &mut Vec<ChunkDraft>,
                 cur_parts: &mut Vec<String>,
                 cur_tokens: &mut i32,
                 cur_section: &Option<String>,
                 cur_page_start: &mut Option<i32>,
                 cur_page_end: &mut Option<i32>,
                 cur_char_start: &mut i32,
                 cur_char_end: &mut i32,
                 carry_overlap: bool| {
        if cur_parts.is_empty() {
            return;
        }
        let content = cur_parts.join("\n\n");
        let idx = chunks.len() as i32;
        chunks.push(ChunkDraft {
            chunk_index: idx,
            section_path: cur_section.clone(),
            page_start: *cur_page_start,
            page_end: *cur_page_end,
            char_start: *cur_char_start,
            char_end: *cur_char_end,
            content: content.clone(),
            token_count: *cur_tokens,
        });

        cur_parts.clear();
        *cur_tokens = 0;
        *cur_page_start = None;
        *cur_page_end = None;
        *cur_char_start = *cur_char_end;

        if carry_overlap {
            let overlap = take_overlap_sentences(&content, OVERLAP_TOKENS);
            if !overlap.is_empty() {
                *cur_tokens = estimate_tokens(&overlap);
                cur_parts.push(overlap);
            }
        }
    };

    for block in &doc.blocks {
        let sec_str = if block.section_path.is_empty() {
            None
        } else {
            Some(block.section_path.join(" > "))
        };

        if matches!(block.kind, BlockKind::Heading { .. }) {
            flush(
                &mut chunks,
                &mut cur_parts,
                &mut cur_tokens,
                &cur_section,
                &mut cur_page_start,
                &mut cur_page_end,
                &mut cur_char_start,
                &mut cur_char_end,
                false,
            );
            cur_section = sec_str;
            cur_parts.push(block.text.clone());
            cur_tokens = estimate_tokens(&block.text);
            cur_page_start = block.page.map(|p| p as i32);
            cur_page_end = cur_page_start;
            cur_char_start = block.char_range.start as i32;
            cur_char_end = block.char_range.end as i32;
            continue;
        }

        if cur_section != sec_str && !cur_parts.is_empty() {
            flush(
                &mut chunks,
                &mut cur_parts,
                &mut cur_tokens,
                &cur_section,
                &mut cur_page_start,
                &mut cur_page_end,
                &mut cur_char_start,
                &mut cur_char_end,
                false,
            );
            cur_section = sec_str.clone();
        } else if cur_section.is_none() {
            cur_section = sec_str.clone();
        }

        let b_tokens = estimate_tokens(&block.text);
        if matches!(
            block.kind,
            BlockKind::Code | BlockKind::Table | BlockKind::Formula
        ) && b_tokens <= 600
        {
            if !cur_parts.is_empty() && cur_tokens + b_tokens > TARGET_TOKENS {
                flush(
                    &mut chunks,
                    &mut cur_parts,
                    &mut cur_tokens,
                    &cur_section,
                    &mut cur_page_start,
                    &mut cur_page_end,
                    &mut cur_char_start,
                    &mut cur_char_end,
                    true,
                );
            }
            push_block(
                block,
                &mut cur_parts,
                &mut cur_tokens,
                &mut cur_page_start,
                &mut cur_page_end,
                &mut cur_char_start,
                &mut cur_char_end,
            );
            continue;
        }

        if b_tokens > TARGET_TOKENS {
            for sent in split_sentences(&block.text) {
                let st = estimate_tokens(&sent);
                if !cur_parts.is_empty() && cur_tokens + st > TARGET_TOKENS {
                    flush(
                        &mut chunks,
                        &mut cur_parts,
                        &mut cur_tokens,
                        &cur_section,
                        &mut cur_page_start,
                        &mut cur_page_end,
                        &mut cur_char_start,
                        &mut cur_char_end,
                        true,
                    );
                }
                if cur_parts.is_empty() {
                    cur_char_start = block.char_range.start as i32;
                }
                cur_parts.push(sent);
                cur_tokens += st;
                update_span(
                    block,
                    &mut cur_page_start,
                    &mut cur_page_end,
                    &mut cur_char_end,
                );
            }
        } else {
            if !cur_parts.is_empty() && cur_tokens + b_tokens > TARGET_TOKENS {
                flush(
                    &mut chunks,
                    &mut cur_parts,
                    &mut cur_tokens,
                    &cur_section,
                    &mut cur_page_start,
                    &mut cur_page_end,
                    &mut cur_char_start,
                    &mut cur_char_end,
                    true,
                );
            }
            push_block(
                block,
                &mut cur_parts,
                &mut cur_tokens,
                &mut cur_page_start,
                &mut cur_page_end,
                &mut cur_char_start,
                &mut cur_char_end,
            );
        }
    }

    flush(
        &mut chunks,
        &mut cur_parts,
        &mut cur_tokens,
        &cur_section,
        &mut cur_page_start,
        &mut cur_page_end,
        &mut cur_char_start,
        &mut cur_char_end,
        false,
    );

    // Merge tiny tail (< 80 tokens) into previous chunk of same section
    if chunks.len() >= 2 {
        let last_small = chunks
            .last()
            .map(|c| c.token_count < MIN_TAIL_TOKENS)
            .unwrap_or(false);
        if last_small {
            let tail = chunks.pop().unwrap();
            let prev = chunks.last_mut().unwrap();
            if prev.section_path == tail.section_path {
                prev.content.push_str("\n\n");
                prev.content.push_str(&tail.content);
                prev.token_count += tail.token_count;
                prev.page_end = tail.page_end.or(prev.page_end);
                prev.char_end = tail.char_end;
            } else {
                chunks.push(tail);
            }
        }
    }

    chunks
}

fn push_block(
    block: &Block,
    cur_parts: &mut Vec<String>,
    cur_tokens: &mut i32,
    cur_page_start: &mut Option<i32>,
    cur_page_end: &mut Option<i32>,
    cur_char_start: &mut i32,
    cur_char_end: &mut i32,
) {
    if cur_parts.is_empty() {
        *cur_char_start = block.char_range.start as i32;
    }
    cur_parts.push(block.text.clone());
    *cur_tokens += estimate_tokens(&block.text);
    update_span(block, cur_page_start, cur_page_end, cur_char_end);
}

fn update_span(
    block: &Block,
    cur_page_start: &mut Option<i32>,
    cur_page_end: &mut Option<i32>,
    cur_char_end: &mut i32,
) {
    if let Some(p) = block.page.map(|x| x as i32) {
        if cur_page_start.is_none() {
            *cur_page_start = Some(p);
        }
        *cur_page_end = Some(p);
    }
    *cur_char_end = block.char_range.end as i32;
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | ';' | '；' | '\n') {
            let t = buf.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            buf.clear();
        }
    }
    let rem = buf.trim().to_string();
    if !rem.is_empty() {
        out.push(rem);
    }
    out
}

fn take_overlap_sentences(content: &str, max_tokens: i32) -> String {
    let sents = split_sentences(content);
    let mut chosen = Vec::new();
    let mut acc = 0i32;
    for s in sents.into_iter().rev() {
        let t = estimate_tokens(&s);
        if acc + t > max_tokens && !chosen.is_empty() {
            break;
        }
        acc += t;
        chosen.push(s);
    }
    chosen.reverse();
    chosen.join(" ")
}
