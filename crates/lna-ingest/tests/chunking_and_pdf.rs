//! Chunk boundaries (B12) and PDF parsing quality / error classification (B2).
use lna_ingest::chunker::{chunk_document, estimate_tokens};
use lna_ingest::parser::{parse_markdown, parse_pdf_from};
use lna_types::ErrorCode;
use std::path::{Path, PathBuf};

fn sentence(i: usize) -> String {
    format!("这是第{i}个用于测试分块边界的句子，它讨论向量空间的基与维数以及线性无关的含义。")
}

fn paragraph(from: usize, n: usize) -> String {
    (from..from + n).map(sentence).collect::<Vec<_>>().join("")
}

#[test]
fn b12_headings_never_merge_sections() {
    let md = format!(
        "# 总览\n\n## 第一节\n\n{}\n\n## 第二节\n\n{}\n",
        sentence(1),
        sentence(2)
    );
    let chunks = chunk_document(&parse_markdown(&md));
    assert!(chunks.len() >= 2, "{chunks:?}");
    for c in &chunks {
        assert!(
            !(c.content.contains("第一节") && c.content.contains("第二节")),
            "chunk spans two sections: {}",
            c.content
        );
    }
    let sections: Vec<_> = chunks.iter().map(|c| c.section_path.clone()).collect();
    assert!(sections
        .iter()
        .any(|s| s.as_deref().is_some_and(|s| s.contains("第一节"))));
    assert!(sections
        .iter()
        .any(|s| s.as_deref().is_some_and(|s| s.contains("第二节"))));
}

#[test]
fn b12_code_and_tables_stay_whole_up_to_600_tokens() {
    let code_lines: Vec<String> = (0..40)
        .map(|i| format!("let value_{i} = compute_basis_dimension({i});"))
        .collect();
    let md = format!(
        "## 示例\n\n{}\n\n```rust\n{}\n```\n\n{}\n",
        paragraph(1, 8),
        code_lines.join("\n"),
        paragraph(20, 3)
    );
    let chunks = chunk_document(&parse_markdown(&md));
    let holder: Vec<_> = chunks
        .iter()
        .filter(|c| c.content.contains("value_0"))
        .collect();
    assert_eq!(holder.len(), 1);
    assert!(
        holder[0].content.contains("value_39"),
        "code block split across chunks"
    );

    let table = "| a | b |\n| - | - |\n".to_string()
        + &(0..30)
            .map(|i| format!("| 行{i} | 值{i} |"))
            .collect::<Vec<_>>()
            .join("\n");
    let md = format!("## 表\n\n{}\n\n{table}\n", paragraph(1, 5));
    let chunks = chunk_document(&parse_markdown(&md));
    let holder: Vec<_> = chunks
        .iter()
        .filter(|c| c.content.contains("行0"))
        .collect();
    assert_eq!(holder.len(), 1);
    assert!(
        holder[0].content.contains("行29"),
        "table split across chunks"
    );
}

#[test]
fn b12_long_text_is_split_near_the_target_with_bounded_overlap() {
    let md = format!("## 长文\n\n{}\n", paragraph(1, 120));
    let chunks = chunk_document(&parse_markdown(&md));
    assert!(chunks.len() >= 4, "{} chunks", chunks.len());
    for c in &chunks {
        assert!(
            c.token_count <= 450 + 120,
            "chunk too large: {} tokens",
            c.token_count
        );
    }
    for pair in chunks.windows(2) {
        // the next chunk starts with sentences repeated from the end of the previous one
        let next_start: String = pair[1].content.chars().take(40).collect();
        assert!(
            pair[0].content.contains(&next_start),
            "no overlap between consecutive chunks"
        );
        // length of the repeated prefix
        let mut overlap = String::new();
        for s in pair[1].content.split_inclusive('。') {
            if pair[0].content.contains(s) {
                overlap.push_str(s);
            } else {
                break;
            }
        }
        assert!(
            estimate_tokens(&overlap) <= 66,
            "overlap {} tokens",
            estimate_tokens(&overlap)
        );
    }
    // chunk indexes are sequential
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c.chunk_index, i as i32);
    }
}

#[test]
fn b12_tiny_tail_is_merged_into_the_previous_chunk_of_the_same_section() {
    // About 470 tokens of body followed by a very short remainder.
    let md = format!("## 节\n\n{}\n\n{}\n", paragraph(1, 7), "结尾一句。");
    let chunks = chunk_document(&parse_markdown(&md));
    assert!(
        chunks
            .iter()
            .all(|c| c.token_count >= 80 || chunks.len() == 1),
        "{chunks:?}"
    );
    assert!(chunks.last().unwrap().content.contains("结尾一句"));
}

// ------------------------------------------------------------------------------- B2 (PDF)

fn lib_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if dir.join("pdfium.dll").exists() || dir.join("libpdfium.so").exists() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("LNA_REQUIRE_PDFIUM").is_none(),
        "LNA_REQUIRE_PDFIUM is set but no PDFium library was found in {}",
        dir.display()
    );
    eprintln!("skipped: PDFium library not present in {}", dir.display());
    None
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pdf")
        .join(name)
}

#[test]
fn b2_pdf_errors_are_classified_and_never_panic() {
    let Some(dir) = lib_dir() else { return };
    let cases = [
        ("encrypted.pdf", ErrorCode::SourceEncrypted),
        ("scanned.pdf", ErrorCode::SourceNeedsOcr),
        ("corrupt.pdf", ErrorCode::SourceUnsupported),
        ("garbage.pdf", ErrorCode::SourceUnsupported),
    ];
    for (name, code) in cases {
        let err = parse_pdf_from(&fixture(name), &dir).unwrap_err();
        assert_eq!(err.code, code, "{name}: {}", err.message);
        println!("{name}: {:?}", err.message);
        assert!(!err.message.contains('\n'), "{name}: one-line message");
    }
}

#[test]
fn b2_headers_footers_hyphenation_bookmarks_and_title() {
    let Some(dir) = lib_dir() else { return };
    let doc = parse_pdf_from(&fixture("multi_page.pdf"), &dir).expect("parses");
    assert_eq!(doc.page_count, Some(6));
    assert_eq!(doc.title.as_deref(), Some("Linear Algebra Notes"));
    let all: String = doc
        .blocks
        .iter()
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // running header and numbered footer are gone, body text is kept
    assert!(
        !all.contains("Linear Algebra Notes - Draft"),
        "header kept: {all}"
    );
    assert!(!all.contains("Page 3 of 6"), "numbered footer kept: {all}");
    assert!(all.contains("vector space and its basis"));
    // hyphenated word is repaired
    assert!(
        all.contains("dimension theorem"),
        "hyphenation: {:?}",
        all.chars().skip(60).take(40).collect::<Vec<_>>()
    );
    assert!(!all.contains("dimen- sion"));
    // pages are numbered
    assert_eq!(doc.blocks.first().and_then(|b| b.page), Some(1));
    assert_eq!(doc.blocks.last().and_then(|b| b.page), Some(6));
    // bookmarks become the section path
    let path_of = |page: u32| -> Vec<String> {
        doc.blocks
            .iter()
            .find(|b| b.page == Some(page))
            .unwrap()
            .section_path
            .clone()
    };
    assert_eq!(path_of(1), vec!["Chapter 1 Vector Spaces"]);
    assert_eq!(path_of(2), vec!["Chapter 1 Vector Spaces", "1.1 Subspaces"]);
    assert_eq!(path_of(4), vec!["Chapter 2 Linear Maps", "2.1 Rank"]);
    assert_eq!(path_of(5), vec!["Chapter 2 Linear Maps", "2.2 Eigenvalues"]);
    assert_eq!(path_of(6), vec!["Chapter 3 Projections"]);

    // and the chunks carry page numbers and section paths through to retrieval
    let chunks = chunk_document(&doc);
    assert!(chunks.iter().all(|c| c.page_start.is_some()));
    assert!(chunks.iter().any(|c| c
        .section_path
        .as_deref()
        .is_some_and(|s| s.contains("2.1 Rank"))));
}
