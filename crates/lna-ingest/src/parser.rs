use lna_types::{ErrorCode, LnaError, LnaResult};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum BlockKind {
    Heading { level: u8 },
    Paragraph,
    ListItem,
    Table,
    Code,
    Formula,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub page: Option<u32>,
    pub section_path: Vec<String>,
    pub char_range: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub title: Option<String>,
    pub language: Option<String>,
    pub page_count: Option<i64>,
    pub blocks: Vec<Block>,
}

pub fn decode_text_bytes(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    let enc = det.guess(None, true);
    let (cow, _, _) = enc.decode(bytes);
    cow.into_owned()
}

pub fn parse_markdown(text: &str) -> ParsedDoc {
    let mut blocks = Vec::new();
    let mut section_stack: Vec<(u8, String)> = Vec::new();
    let mut cursor = 0usize;
    let mut cur_text = String::new();
    let mut cur_kind: Option<BlockKind> = None;
    let mut title = None;

    let parser = Parser::new(text);
    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                cur_text.clear();
                let lvl = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                cur_kind = Some(BlockKind::Heading { level: lvl });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(BlockKind::Heading { level }) = cur_kind.take() {
                    let t = cur_text.trim().to_string();
                    if !t.is_empty() {
                        if title.is_none() && level == 1 {
                            title = Some(t.clone());
                        }
                        while section_stack
                            .last()
                            .map(|(l, _)| *l >= level)
                            .unwrap_or(false)
                        {
                            section_stack.pop();
                        }
                        section_stack.push((level, t.clone()));
                        let start = cursor;
                        cursor += t.len();
                        blocks.push(Block {
                            kind: BlockKind::Heading { level },
                            text: t,
                            page: None,
                            section_path: section_stack.iter().map(|(_, s)| s.clone()).collect(),
                            char_range: start..cursor,
                        });
                    }
                    cur_text.clear();
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                cur_text.clear();
                cur_kind = Some(BlockKind::Code);
            }
            Event::End(TagEnd::CodeBlock) => {
                let t = cur_text.trim().to_string();
                if !t.is_empty() {
                    let start = cursor;
                    cursor += t.len();
                    blocks.push(Block {
                        kind: BlockKind::Code,
                        text: t,
                        page: None,
                        section_path: section_stack.iter().map(|(_, s)| s.clone()).collect(),
                        char_range: start..cursor,
                    });
                }
                cur_text.clear();
                cur_kind = None;
            }
            Event::Start(Tag::Paragraph) => {
                cur_text.clear();
                cur_kind = Some(BlockKind::Paragraph);
            }
            Event::End(TagEnd::Paragraph) => {
                let t = cur_text.trim().to_string();
                if !t.is_empty() {
                    let start = cursor;
                    cursor += t.len();
                    let kind = if t.starts_with("$$") && t.ends_with("$$") {
                        BlockKind::Formula
                    } else {
                        BlockKind::Paragraph
                    };
                    blocks.push(Block {
                        kind,
                        text: t,
                        page: None,
                        section_path: section_stack.iter().map(|(_, s)| s.clone()).collect(),
                        char_range: start..cursor,
                    });
                }
                cur_text.clear();
                cur_kind = None;
            }
            Event::Start(Tag::Item) => {
                cur_text.clear();
                cur_kind = Some(BlockKind::ListItem);
            }
            Event::End(TagEnd::Item) => {
                let t = cur_text.trim().to_string();
                if !t.is_empty() {
                    let start = cursor;
                    cursor += t.len();
                    blocks.push(Block {
                        kind: BlockKind::ListItem,
                        text: t,
                        page: None,
                        section_path: section_stack.iter().map(|(_, s)| s.clone()).collect(),
                        char_range: start..cursor,
                    });
                }
                cur_text.clear();
                cur_kind = None;
            }
            Event::Text(t) | Event::Code(t) => {
                cur_text.push_str(&t);
            }
            Event::SoftBreak | Event::HardBreak => {
                cur_text.push(' ');
            }
            _ => {}
        }
    }

    let lang = detect_language(text);
    ParsedDoc {
        title,
        language: Some(lang),
        page_count: None,
        blocks,
    }
}

pub fn parse_plain_text(text: &str) -> ParsedDoc {
    let mut blocks = Vec::new();
    let mut section_path = Vec::new();
    let mut cursor = 0usize;
    for raw_para in text.split("\n\n") {
        let trimmed = raw_para.trim();
        if trimmed.is_empty() {
            continue;
        }
        let is_heading = trimmed.lines().count() == 1
            && (trimmed.starts_with("第") && (trimmed.contains('章') || trimmed.contains('节'))
                || trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                    && trimmed.contains('.'));
        let kind = if is_heading {
            section_path = vec![trimmed.to_string()];
            BlockKind::Heading { level: 1 }
        } else {
            BlockKind::Paragraph
        };
        let start = cursor;
        cursor += trimmed.len();
        blocks.push(Block {
            kind,
            text: trimmed.to_string(),
            page: None,
            section_path: section_path.clone(),
            char_range: start..cursor,
        });
    }
    ParsedDoc {
        title: None,
        language: Some(detect_language(text)),
        page_count: None,
        blocks,
    }
}

pub fn parse_html(html: &str) -> ParsedDoc {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty());

    let mut blocks = Vec::new();
    let mut section_stack: Vec<(u8, String)> = Vec::new();
    let mut cursor = 0usize;

    if let Ok(sel) = Selector::parse("h1,h2,h3,h4,p,li,pre") {
        for el in doc.select(&sel) {
            let tag = el.value().name();
            if tag == "p"
                && el.ancestors().skip(1).any(|a| {
                    a.value()
                        .as_element()
                        .map(|e| e.name() == "li")
                        .unwrap_or(false)
                })
            {
                continue;
            }
            let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if text.is_empty() {
                continue;
            }
            let kind = match tag {
                "h1" | "h2" | "h3" | "h4" => {
                    let lvl = tag[1..].parse::<u8>().unwrap_or(1);
                    while section_stack
                        .last()
                        .map(|(l, _)| *l >= lvl)
                        .unwrap_or(false)
                    {
                        section_stack.pop();
                    }
                    section_stack.push((lvl, text.clone()));
                    BlockKind::Heading { level: lvl }
                }
                "pre" => BlockKind::Code,
                "li" => BlockKind::ListItem,
                _ => BlockKind::Paragraph,
            };
            let start = cursor;
            cursor += text.len();
            blocks.push(Block {
                kind,
                text,
                page: None,
                section_path: section_stack.iter().map(|(_, s)| s.clone()).collect(),
                char_range: start..cursor,
            });
        }
    }

    let sample: String = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    ParsedDoc {
        title,
        language: Some(detect_language(&sample)),
        page_count: None,
        blocks,
    }
}

/// Directories searched for the PDFium library, in order: `LNA_PDFIUM_DIR`, the executable's
/// directory, the current directory.
fn pdfium_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("LNA_PDFIUM_DIR") {
        dirs.push(std::path::PathBuf::from(d));
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        dirs.push(d);
    }
    dirs.push(std::path::PathBuf::from("./"));
    dirs
}

/// Loads PDFium from the first directory that has it, optionally falling back to the system
/// library. A missing library is `CAPABILITY_MISSING` with a one-line, actionable message.
fn bind_pdfium(
    dirs: &[std::path::PathBuf],
    allow_system: bool,
) -> LnaResult<Box<dyn pdfium_render::prelude::PdfiumLibraryBindings>> {
    use pdfium_render::prelude::*;

    let mut last_err = None;
    for d in dirs {
        match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(d)) {
            Ok(b) => return Ok(b),
            Err(e) => last_err = Some(e),
        }
    }
    if allow_system {
        match Pdfium::bind_to_system_library() {
            Ok(b) => return Ok(b),
            Err(e) => last_err = Some(e),
        }
    }
    // The loader error is a multi-line Debug dump; keep it in the log and
    // give the user a one-line, actionable message.
    tracing::warn!(error = ?last_err, "PDFium library could not be loaded");
    Err(LnaError::new(
        ErrorCode::CapabilityMissing,
        "PDFium library not found: place pdfium.dll (libpdfium.so on Linux) next to the application",
    ))
}

pub fn parse_pdf(path: &Path) -> LnaResult<ParsedDoc> {
    parse_pdf_with(path, &pdfium_search_dirs(), true)
}

/// Parses a PDF using the PDFium library found in `lib_dir` only (tests, tools).
pub fn parse_pdf_from(path: &Path, lib_dir: &Path) -> LnaResult<ParsedDoc> {
    parse_pdf_with(path, &[lib_dir.to_path_buf()], false)
}

/// Header/footer detection key: running page numbers ("Page 3 of 10") must still match.
fn header_key(line: &str) -> String {
    line.chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

/// Flattens the bookmark tree into `(page_index, depth, title)` in document order.
fn collect_bookmarks(
    bookmark: Option<pdfium_render::prelude::PdfBookmark<'_>>,
    depth: usize,
    out: &mut Vec<(usize, usize, String)>,
) {
    let mut current = bookmark;
    while let Some(bm) = current {
        let title = bm.title().unwrap_or_default().trim().to_string();
        let page = bm
            .destination()
            .and_then(|d| d.page_index().ok())
            .map(|p| p as usize);
        if let (Some(page), false) = (page, title.is_empty()) {
            out.push((page, depth, title));
        }
        collect_bookmarks(bm.first_child(), depth + 1, out);
        current = bm.next_sibling();
    }
}

fn parse_pdf_with(
    path: &Path,
    lib_dirs: &[std::path::PathBuf],
    allow_system_lib: bool,
) -> LnaResult<ParsedDoc> {
    use pdfium_render::prelude::*;

    let bindings = bind_pdfium(lib_dirs, allow_system_lib)?;
    let pdfium = Pdfium::new(bindings);
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("password") || msg.to_lowercase().contains("encrypt") {
            LnaError::new(ErrorCode::SourceEncrypted, "PDF is password-protected")
        } else if msg.contains("FormatError") {
            LnaError::new(
                ErrorCode::SourceUnsupported,
                "PDF file is damaged or not a valid PDF",
            )
        } else {
            let one_line = msg.split_whitespace().collect::<Vec<_>>().join(" ");
            LnaError::new(
                ErrorCode::SourceUnsupported,
                format!("Failed to open PDF: {one_line}"),
            )
        }
    })?;

    let page_count = doc.pages().len() as usize;
    if page_count > 3000 {
        return Err(LnaError::new(
            ErrorCode::SourceTooLarge,
            format!("PDF page count {page_count} exceeds 3000 limit"),
        ));
    }

    let mut page_lines: Vec<Vec<String>> = Vec::with_capacity(page_count);
    let mut line_freq: HashMap<String, usize> = HashMap::new();
    let mut sparse_pages = 0usize;

    for page in doc.pages().iter() {
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        if text.trim().chars().count() < 20 {
            sparse_pages += 1;
        }
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if let Some(first) = lines.first() {
            *line_freq.entry(header_key(first)).or_insert(0) += 1;
        }
        if let Some(last) = lines.last() {
            *line_freq.entry(header_key(last)).or_insert(0) += 1;
        }
        page_lines.push(lines);
    }

    if page_count > 0 && sparse_pages * 2 > page_count {
        return Err(LnaError::new(
            ErrorCode::SourceNeedsOcr,
            "More than half of PDF pages lack selectable text (OCR required)",
        ));
    }

    let header_footer_threshold = (page_count / 3).max(3);
    let mut outline: Vec<(usize, usize, String)> = Vec::new();
    collect_bookmarks(doc.bookmarks().root(), 0, &mut outline);
    // Section path of a page = the chain of bookmarks in effect at its first position.
    let section_for_page = |page_index: usize| -> Vec<String> {
        let mut stack: Vec<(usize, String)> = Vec::new();
        for (bm_page, depth, title) in &outline {
            if *bm_page > page_index {
                continue;
            }
            stack.retain(|(d, _)| d < depth);
            stack.push((*depth, title.clone()));
        }
        stack.into_iter().map(|(_, title)| title).collect()
    };
    let mut blocks = Vec::new();
    let mut cursor = 0usize;

    for (idx, lines) in page_lines.into_iter().enumerate() {
        let page_no = (idx + 1) as u32;
        let filtered: Vec<String> = lines
            .into_iter()
            .filter(|l| {
                line_freq.get(&header_key(l)).copied().unwrap_or(0) < header_footer_threshold
            })
            .collect();

        // Fix line-end hyphenation and group into paragraphs
        // PDFium reports an end-of-line hyphen as U+0002; soft hyphens are U+00AD.
        let ends_with_hyphen =
            |s: &str| s.ends_with('-') || s.ends_with('\u{2}') || s.ends_with('\u{ad}');
        let mut joined = String::new();
        for l in filtered {
            if ends_with_hyphen(&joined)
                && l.chars()
                    .next()
                    .map(|c| c.is_ascii_lowercase())
                    .unwrap_or(false)
            {
                joined.pop();
                joined.push_str(&l);
            } else {
                if !joined.is_empty() {
                    joined.push(' ');
                }
                joined.push_str(&l);
            }
        }
        joined.retain(|c| c != '\u{2}' && c != '\u{ad}');
        if !joined.is_empty() {
            let start = cursor;
            cursor += joined.len();
            blocks.push(Block {
                kind: BlockKind::Paragraph,
                text: joined,
                page: Some(page_no),
                section_path: section_for_page(idx),
                char_range: start..cursor,
            });
        }
    }

    let sample: String = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let title = doc
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(ParsedDoc {
        title,
        language: Some(detect_language(&sample)),
        page_count: Some(page_count as i64),
        blocks,
    })
}

pub fn detect_language(sample: &str) -> String {
    let cjk = sample
        .chars()
        .filter(|&c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
        .count();
    let total = sample.chars().count().max(1);
    if cjk * 5 > total {
        "zh".to_string()
    } else {
        "en".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_pdf_returns_err_without_panic() {
        let res = parse_pdf(Path::new("/nonexistent_lna_test.pdf"));
        assert!(res.is_err());
    }

    /// B-06: without the PDFium library the failure must be `CAPABILITY_MISSING`
    /// with a single-line message (the UI shows it inline).
    #[test]
    fn missing_pdfium_library_is_capability_missing() {
        let nowhere = std::env::temp_dir().join("lna_no_pdfium_here");
        let err = match bind_pdfium(&[nowhere], false) {
            Ok(_) => panic!("binding must fail when no library exists"),
            Err(e) => e,
        };
        assert_eq!(err.code, ErrorCode::CapabilityMissing);
        assert!(!err.message.contains('\n'), "message: {}", err.message);
        assert!(err.message.contains("pdfium"), "message: {}", err.message);
    }

    fn minimal_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 18 Tf 20 100 Td ({text}) Tj ET");
        let objs = [
            "<</Type/Catalog/Pages 2 0 R>>".to_string(),
            "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(),
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 144]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>".to_string(),
            format!("<</Length {}>>\nstream\n{stream}\nendstream", stream.len()),
            "<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// B-06: with PDFium available a text PDF parses into page-numbered blocks.
    /// Needs `pwsh scripts/fetch-pdfium.ps1 -Dest target/debug`; set `LNA_REQUIRE_PDFIUM=1`
    /// (CI does) to turn a missing library into a failure instead of a skip.
    #[test]
    fn parses_text_pdf_into_page_numbered_blocks() {
        let lib_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
        if !lib_dir.join("pdfium.dll").exists() && !lib_dir.join("libpdfium.so").exists() {
            assert!(
                std::env::var_os("LNA_REQUIRE_PDFIUM").is_none(),
                "LNA_REQUIRE_PDFIUM is set but no PDFium library was found in {}",
                lib_dir.display()
            );
            eprintln!(
                "skipped: PDFium library not present in {}",
                lib_dir.display()
            );
            return;
        }
        let path = std::env::temp_dir().join(format!("lna_pdf_test_{}.pdf", std::process::id()));
        std::fs::write(
            &path,
            minimal_pdf("A basis spans the vector space and is independent."),
        )
        .unwrap();
        let doc = parse_pdf_with(&path, &[lib_dir], false).expect("PDF should parse");
        let _ = std::fs::remove_file(&path);

        assert_eq!(doc.page_count, Some(1));
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].page, Some(1));
        assert!(
            doc.blocks[0].text.contains("basis spans the vector space"),
            "text: {}",
            doc.blocks[0].text
        );
        assert_eq!(doc.language.as_deref(), Some("en"));
    }
}
