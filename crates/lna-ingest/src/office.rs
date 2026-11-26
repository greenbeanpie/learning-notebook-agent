use crate::formats;
use crate::parser::{self, Block, BlockKind, ParsedDoc};
use lna_types::{ErrorCode, LnaError, LnaResult};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedImageInfo {
    pub id: String,
    pub zip_entry: String,
    pub page: Option<u32>,
    pub mime_type: String,
    pub byte_size: u64,
}

#[derive(Debug, Clone)]
pub struct RichParseOutput {
    pub doc: ParsedDoc,
    pub embedded_images: Vec<EmbeddedImageInfo>,
}

pub fn parse_csv(bytes: &[u8]) -> LnaResult<ParsedDoc> {
    let text = parser::decode_text_bytes(bytes);
    let trimmed_bom = text.trim_start_matches('\u{FEFF}');
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(trimmed_bom.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for result in rdr.records() {
        let rec = result.map_err(|e| {
            LnaError::new(ErrorCode::SourceUnsupported, format!("Invalid CSV: {e}"))
        })?;
        let cols: Vec<String> = rec
            .iter()
            .map(|c| c.trim().replace('|', "\\|").replace(['\r', '\n'], " "))
            .collect();
        if cols.iter().any(|c| !c.is_empty()) {
            rows.push(cols);
        }
    }

    if rows.is_empty() {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "CSV file contains no readable rows",
        ));
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let pad_row = |r: &[String]| -> String {
        let mut cells = Vec::with_capacity(col_count);
        for i in 0..col_count {
            cells.push(r.get(i).map(String::as_str).unwrap_or(""));
        }
        format!("| {} |", cells.join(" | "))
    };
    let sep = format!("| {} |", vec!["---"; col_count].join(" | "));

    let mut blocks = Vec::new();
    let mut cursor = 0usize;

    if rows.len() == 1 {
        let tbl = format!("{}\n{}", pad_row(&rows[0]), sep);
        let len = tbl.len();
        blocks.push(Block {
            kind: BlockKind::Table,
            text: tbl,
            page: None,
            section_path: vec![],
            char_range: 0..len,
        });
    } else {
        let header_line = pad_row(&rows[0]);
        for (chunk_idx, data_chunk) in rows[1..].chunks(25).enumerate() {
            let mut lines = Vec::with_capacity(data_chunk.len() + 2);
            lines.push(header_line.clone());
            lines.push(sep.clone());
            for row in data_chunk {
                lines.push(pad_row(row));
            }
            let tbl = lines.join("\n");
            let start = cursor;
            cursor += tbl.len();
            let row_start = chunk_idx * 25 + 1;
            let row_end = row_start + data_chunk.len() - 1;
            blocks.push(Block {
                kind: BlockKind::Table,
                text: tbl,
                page: None,
                section_path: vec![format!("Rows {row_start}-{row_end}")],
                char_range: start..cursor,
            });
        }
    }

    let sample = blocks
        .iter()
        .take(3)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(ParsedDoc {
        title: None,
        language: Some(parser::detect_language(&sample)),
        page_count: None,
        blocks,
    })
}

pub fn extract_zip_embedded_image(archive_path: &Path, entry_name: &str) -> LnaResult<Vec<u8>> {
    let f = std::fs::File::open(archive_path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid archive: {e}"),
        )
    })?;
    let mut entry = archive.by_name(entry_name).map_err(|e| {
        LnaError::not_found(format!("Embedded image not found ({entry_name}): {e}"))
    })?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| LnaError::internal(e.to_string()))?;
    Ok(buf)
}

fn read_zip_entry_string(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(parser::decode_text_bytes(&bytes))
}

fn collect_zip_images(
    archive: &mut zip::ZipArchive<std::fs::File>,
    prefix_filter: &[&str],
) -> Vec<EmbeddedImageInfo> {
    let mut out = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            let lower = name.to_ascii_lowercase();
            let prefix_ok = prefix_filter.is_empty()
                || prefix_filter
                    .iter()
                    .any(|p| lower.starts_with(&p.to_ascii_lowercase()));
            if !prefix_ok {
                continue;
            }
            if let Some(ext) = lower.rsplit('.').next() {
                if formats::IMAGE_EXTENSIONS.contains(&ext) {
                    out.push(EmbeddedImageInfo {
                        id: format!("img_{}", out.len() + 1),
                        zip_entry: name,
                        page: None,
                        mime_type: formats::mime_type_for_ext(ext).to_string(),
                        byte_size: entry.size(),
                    });
                }
            }
        }
    }
    out
}

fn local_name_str(raw: &[u8]) -> &str {
    let s = std::str::from_utf8(raw).unwrap_or("");
    s.rsplit(':').next().unwrap_or(s)
}

fn attr_value<'a>(
    e: &'a quick_xml::events::BytesStart<'a>,
    reader: &Reader<&[u8]>,
    local_target: &str,
) -> Option<String> {
    for attr in e.attributes().flatten() {
        if local_name_str(attr.key.as_ref()).eq_ignore_ascii_case(local_target) {
            if let Ok(v) = attr.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder()) {
                return Some(v.into_owned());
            }
        }
    }
    None
}

fn format_markdown_table(rows: &[Vec<String>]) -> Option<String> {
    let clean_rows: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .cloned()
        .collect();
    if clean_rows.is_empty() {
        return None;
    }
    let col_count = clean_rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let pad = |r: &[String]| -> String {
        let mut cells = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let v = r
                .get(i)
                .map(|s| s.trim().replace('|', "\\|").replace(['\r', '\n'], " "))
                .unwrap_or_default();
            cells.push(v);
        }
        format!("| {} |", cells.join(" | "))
    };
    let mut lines = Vec::with_capacity(clean_rows.len() + 1);
    lines.push(pad(&clean_rows[0]));
    lines.push(format!("| {} |", vec!["---"; col_count].join(" | ")));
    for row in &clean_rows[1..] {
        lines.push(pad(row));
    }
    Some(lines.join("\n"))
}

pub fn parse_docx(path: &Path) -> LnaResult<RichParseOutput> {
    let f = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid DOCX archive: {e}"),
        )
    })?;

    let title = read_zip_entry_string(&mut archive, "docProps/core.xml")
        .and_then(|xml| extract_dc_title(&xml));

    let doc_xml = read_zip_entry_string(&mut archive, "word/document.xml").ok_or_else(|| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            "DOCX missing word/document.xml",
        )
    })?;

    let mut blocks = Vec::new();
    let mut section_stack: Vec<(u8, String)> = Vec::new();
    let mut cursor = 0usize;
    parse_wordprocessing_xml(&doc_xml, &mut blocks, &mut section_stack, &mut cursor);

    for extra in ["word/footnotes.xml", "word/endnotes.xml"] {
        if let Some(xml) = read_zip_entry_string(&mut archive, extra) {
            parse_wordprocessing_xml(&xml, &mut blocks, &mut section_stack, &mut cursor);
        }
    }

    let embedded_images = collect_zip_images(&mut archive, &["word/media/"]);
    let sample = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(RichParseOutput {
        doc: ParsedDoc {
            title,
            language: Some(parser::detect_language(&sample)),
            page_count: None,
            blocks,
        },
        embedded_images,
    })
}

fn parse_wordprocessing_xml(
    xml: &str,
    blocks: &mut Vec<Block>,
    section_stack: &mut Vec<(u8, String)>,
    cursor: &mut usize,
) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut in_p = false;
    let mut in_t = false;
    let mut in_del = false;
    let mut p_text = String::new();
    let mut p_heading: Option<u8> = None;
    let mut p_is_list = false;

    let mut table_depth = 0usize;
    let mut cur_table_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_text = String::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                match lname {
                    "tbl" => {
                        if table_depth == 0 {
                            cur_table_rows.clear();
                        }
                        table_depth += 1;
                    }
                    "tr" if table_depth == 1 => {
                        cur_row.clear();
                    }
                    "tc" if table_depth == 1 => {
                        in_cell = true;
                        cell_text.clear();
                    }
                    "p" => {
                        in_p = true;
                        p_text.clear();
                        p_heading = None;
                        p_is_list = false;
                    }
                    "pStyle" => {
                        if let Some(val) = attr_value(e, &reader, "val") {
                            p_heading = heading_level_from_style(&val);
                        }
                    }
                    "numPr" => {
                        p_is_list = true;
                    }
                    "t" | "instrText" if lname == "t" => {
                        in_t = true;
                    }
                    "delText" => {
                        in_del = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                match lname {
                    "pStyle" if in_p => {
                        if let Some(val) = attr_value(e, &reader, "val") {
                            p_heading = heading_level_from_style(&val);
                        }
                    }
                    "br" | "cr" if in_p && !in_del => {
                        p_text.push(' ');
                    }
                    "tab" if in_p && !in_del => {
                        p_text.push('\t');
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_t && !in_del {
                    if let Ok(t) = e.xml10_content() {
                        p_text.push_str(&t);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                match lname {
                    "t" => in_t = false,
                    "delText" => in_del = false,
                    "p" => {
                        in_p = false;
                        let trimmed = p_text.trim().to_string();
                        if !trimmed.is_empty() {
                            if in_cell {
                                if !cell_text.is_empty() {
                                    cell_text.push(' ');
                                }
                                cell_text.push_str(&trimmed);
                            } else if table_depth == 0 {
                                let kind = if let Some(lvl) = p_heading {
                                    while section_stack
                                        .last()
                                        .map(|(l, _)| *l >= lvl)
                                        .unwrap_or(false)
                                    {
                                        section_stack.pop();
                                    }
                                    section_stack.push((lvl, trimmed.clone()));
                                    BlockKind::Heading { level: lvl }
                                } else if p_is_list {
                                    BlockKind::ListItem
                                } else {
                                    BlockKind::Paragraph
                                };
                                let start = *cursor;
                                *cursor += trimmed.len();
                                blocks.push(Block {
                                    kind,
                                    text: trimmed,
                                    page: None,
                                    section_path: section_stack
                                        .iter()
                                        .map(|(_, s)| s.clone())
                                        .collect(),
                                    char_range: start..*cursor,
                                });
                            }
                        }
                        p_text.clear();
                    }
                    "tc" if table_depth == 1 => {
                        in_cell = false;
                        cur_row.push(cell_text.trim().to_string());
                        cell_text.clear();
                    }
                    "tr" if table_depth == 1 => {
                        if !cur_row.is_empty() {
                            cur_table_rows.push(std::mem::take(&mut cur_row));
                        }
                    }
                    "tbl" => {
                        table_depth = table_depth.saturating_sub(1);
                        if table_depth == 0 {
                            if let Some(tbl_md) = format_markdown_table(&cur_table_rows) {
                                let start = *cursor;
                                *cursor += tbl_md.len();
                                blocks.push(Block {
                                    kind: BlockKind::Table,
                                    text: tbl_md,
                                    page: None,
                                    section_path: section_stack
                                        .iter()
                                        .map(|(_, s)| s.clone())
                                        .collect(),
                                    char_range: start..*cursor,
                                });
                            }
                            cur_table_rows.clear();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn heading_level_from_style(val: &str) -> Option<u8> {
    let lower = val.trim().to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    for prefix in ["heading", "heading ", "toc", "标题", "标题 "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Ok(n) = rest.trim().parse::<u8>() {
                return Some(n.clamp(1, 6));
            }
        }
    }
    if let Ok(n) = lower.parse::<u8>() {
        if (1..=6).contains(&n) {
            return Some(n);
        }
    }
    None
}

fn extract_dc_title(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut in_title = false;
    let mut title = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if local_name_str(e.name().as_ref()) == "title" {
                    in_title = true;
                    title.clear();
                }
            }
            Ok(Event::Text(e)) if in_title => {
                if let Ok(t) = e.xml10_content() {
                    title.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                if local_name_str(e.name().as_ref()) == "title" {
                    let trimmed = title.trim().to_string();
                    if !trimmed.is_empty() {
                        return Some(trimmed);
                    }
                    in_title = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

pub fn parse_pptx(path: &Path) -> LnaResult<RichParseOutput> {
    let f = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid PPTX archive: {e}"),
        )
    })?;

    let title = read_zip_entry_string(&mut archive, "docProps/core.xml")
        .and_then(|xml| extract_dc_title(&xml));

    let mut slide_entries: Vec<(u32, String)> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            if let Some(rest) = name.strip_prefix("ppt/slides/slide") {
                if let Some(num_str) = rest.strip_suffix(".xml") {
                    if let Ok(n) = num_str.parse::<u32>() {
                        slide_entries.push((n, name));
                    }
                }
            }
        }
    }
    slide_entries.sort_by_key(|(n, _)| *n);

    if slide_entries.is_empty() {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "PPTX contains no slides",
        ));
    }

    let page_count = slide_entries.len() as i64;
    let mut media_page_map: HashMap<String, u32> = HashMap::new();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;

    for (slide_no, slide_entry) in slide_entries {
        let rels_path = format!("ppt/slides/_rels/slide{slide_no}.xml.rels");
        let mut notes_entry = format!("ppt/notesSlides/notesSlide{slide_no}.xml");
        if let Some(rels_xml) = read_zip_entry_string(&mut archive, &rels_path) {
            for target in parse_rels_targets(&rels_xml) {
                let norm = normalize_zip_rel("ppt/slides/", &target);
                if norm.starts_with("ppt/notesSlides/") && norm.ends_with(".xml") {
                    notes_entry = norm;
                } else if norm.starts_with("ppt/media/") {
                    media_page_map.entry(norm).or_insert(slide_no);
                }
            }
        }

        let mut slide_blocks = Vec::new();
        if let Some(xml) = read_zip_entry_string(&mut archive, &slide_entry) {
            parse_drawingml_slide(&xml, slide_no, &mut slide_blocks, &mut cursor);
        }
        if let Some(notes_xml) = read_zip_entry_string(&mut archive, &notes_entry) {
            let mut notes_blocks = Vec::new();
            let mut dummy_cur = cursor;
            parse_drawingml_slide(&notes_xml, slide_no, &mut notes_blocks, &mut dummy_cur);
            let notes_text: Vec<String> = notes_blocks
                .into_iter()
                .map(|b| b.text)
                .filter(|t| !t.trim().chars().all(|c| c.is_ascii_digit()))
                .collect();
            if !notes_text.is_empty() {
                let combined = format!("[演讲者备注]\n{}", notes_text.join("\n"));
                let start = cursor;
                cursor += combined.len();
                slide_blocks.push(Block {
                    kind: BlockKind::Paragraph,
                    text: combined,
                    page: Some(slide_no),
                    section_path: vec![format!("Slide {slide_no}")],
                    char_range: start..cursor,
                });
            }
        }
        blocks.extend(slide_blocks);
    }

    let mut embedded_images = collect_zip_images(&mut archive, &["ppt/media/"]);
    for img in &mut embedded_images {
        img.page = media_page_map.get(&img.zip_entry).copied();
    }

    let sample = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(RichParseOutput {
        doc: ParsedDoc {
            title,
            language: Some(parser::detect_language(&sample)),
            page_count: Some(page_count),
            blocks,
        },
        embedded_images,
    })
}

fn parse_rels_targets(rels_xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(rels_xml);
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if local_name_str(e.name().as_ref()) == "Relationship" {
                    if let Some(t) = attr_value(e, &reader, "Target") {
                        out.push(t);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn normalize_zip_rel(base_dir: &str, rel: &str) -> String {
    if let Some(stripped) = rel.strip_prefix('/') {
        return stripped.to_string();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn parse_drawingml_slide(xml: &str, slide_no: u32, blocks: &mut Vec<Block>, cursor: &mut usize) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let sec = vec![format!("Slide {slide_no}")];

    let mut in_p = false;
    let mut in_t = false;
    let mut p_text = String::new();
    let mut table_depth = 0usize;
    let mut cur_table_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_text = String::new();
    let mut is_first_text = true;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                match lname {
                    "tbl" => {
                        if table_depth == 0 {
                            cur_table_rows.clear();
                        }
                        table_depth += 1;
                    }
                    "tr" if table_depth == 1 => cur_row.clear(),
                    "tc" if table_depth == 1 => {
                        in_cell = true;
                        cell_text.clear();
                    }
                    "p" => {
                        in_p = true;
                        p_text.clear();
                    }
                    "t" => in_t = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                if in_p && (lname == "br" || lname == "tab") {
                    p_text.push(' ');
                }
            }
            Ok(Event::Text(e)) if in_t => {
                if let Ok(t) = e.xml10_content() {
                    p_text.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                let name_q = e.name();
                let lname = local_name_str(name_q.as_ref());
                match lname {
                    "t" => in_t = false,
                    "p" => {
                        in_p = false;
                        let trimmed = p_text.trim().to_string();
                        if !trimmed.is_empty() {
                            if in_cell {
                                if !cell_text.is_empty() {
                                    cell_text.push(' ');
                                }
                                cell_text.push_str(&trimmed);
                            } else if table_depth == 0 {
                                let kind = if is_first_text {
                                    is_first_text = false;
                                    BlockKind::Heading { level: 1 }
                                } else {
                                    BlockKind::Paragraph
                                };
                                let start = *cursor;
                                *cursor += trimmed.len();
                                blocks.push(Block {
                                    kind,
                                    text: trimmed,
                                    page: Some(slide_no),
                                    section_path: sec.clone(),
                                    char_range: start..*cursor,
                                });
                            }
                        }
                        p_text.clear();
                    }
                    "tc" if table_depth == 1 => {
                        in_cell = false;
                        cur_row.push(cell_text.trim().to_string());
                        cell_text.clear();
                    }
                    "tr" if table_depth == 1 => {
                        if !cur_row.is_empty() {
                            cur_table_rows.push(std::mem::take(&mut cur_row));
                        }
                    }
                    "tbl" => {
                        table_depth = table_depth.saturating_sub(1);
                        if table_depth == 0 {
                            if let Some(tbl_md) = format_markdown_table(&cur_table_rows) {
                                let start = *cursor;
                                *cursor += tbl_md.len();
                                blocks.push(Block {
                                    kind: BlockKind::Table,
                                    text: tbl_md,
                                    page: Some(slide_no),
                                    section_path: sec.clone(),
                                    char_range: start..*cursor,
                                });
                            }
                            cur_table_rows.clear();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

pub fn parse_epub(path: &Path) -> LnaResult<RichParseOutput> {
    let f = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid EPUB archive: {e}"),
        )
    })?;

    let opf_path = find_epub_opf_path(&mut archive).ok_or_else(|| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            "EPUB missing container.xml / OPF package",
        )
    })?;
    let opf_dir = opf_path
        .rsplit_once('/')
        .map(|(d, _)| format!("{d}/"))
        .unwrap_or_default();
    let opf_xml = read_zip_entry_string(&mut archive, &opf_path)
        .ok_or_else(|| LnaError::new(ErrorCode::SourceUnsupported, "Failed to read EPUB OPF"))?;

    let title = extract_dc_title(&opf_xml);
    let mut manifest: HashMap<String, (String, String)> = HashMap::new();
    let mut spine_ids: Vec<String> = Vec::new();
    {
        let mut reader = Reader::from_str(&opf_xml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    match local_name_str(e.name().as_ref()) {
                        "item" => {
                            if let (Some(id), Some(href)) =
                                (attr_value(e, &reader, "id"), attr_value(e, &reader, "href"))
                            {
                                let media_type =
                                    attr_value(e, &reader, "media-type").unwrap_or_default();
                                manifest.insert(id, (href, media_type));
                            }
                        }
                        "itemref" => {
                            if let Some(idref) = attr_value(e, &reader, "idref") {
                                spine_ids.push(idref);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }

    let mut content_paths: Vec<String> = Vec::new();
    for idref in &spine_ids {
        if let Some((href, media_type)) = manifest.get(idref) {
            if media_type.contains("html")
                || media_type.contains("xml")
                || href.ends_with(".xhtml")
                || href.ends_with(".html")
                || href.ends_with(".htm")
            {
                let decoded_href = percent_decode(href);
                content_paths.push(normalize_zip_rel(&opf_dir, &decoded_href));
            }
        }
    }
    if content_paths.is_empty() {
        for i in 0..archive.len() {
            if let Ok(entry) = archive.by_index(i) {
                let n = entry.name().map(|n| n.into_owned()).unwrap_or_default();
                let low = n.to_ascii_lowercase();
                if low.ends_with(".xhtml") || low.ends_with(".html") || low.ends_with(".htm") {
                    content_paths.push(n);
                }
            }
        }
        content_paths.sort();
    }

    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    for (chap_idx, cp) in content_paths.into_iter().enumerate() {
        if let Some(html) = read_zip_entry_string(&mut archive, &cp) {
            let parsed = parser::parse_html(&html);
            let page_no = Some((chap_idx + 1) as u32);
            for mut b in parsed.blocks {
                let len = b.text.len();
                b.page = page_no;
                b.char_range = cursor..(cursor + len);
                cursor += len;
                blocks.push(b);
            }
        }
    }

    let embedded_images = collect_zip_images(&mut archive, &[]);
    let sample = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(RichParseOutput {
        doc: ParsedDoc {
            title,
            language: Some(parser::detect_language(&sample)),
            page_count: None,
            blocks,
        },
        embedded_images,
    })
}

fn find_epub_opf_path(archive: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    if let Some(cxml) = read_zip_entry_string(archive, "META-INF/container.xml") {
        let mut reader = Reader::from_str(&cxml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    if local_name_str(e.name().as_ref()) == "rootfile" {
                        if let Some(fp) = attr_value(e, &reader, "full-path") {
                            return Some(fp);
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            if entry.name().is_ok_and(|n| n.to_ascii_lowercase().ends_with(".opf")) {
                return Some(entry.name().map(|n| n.into_owned()).unwrap_or_default());
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_odt(path: &Path) -> LnaResult<RichParseOutput> {
    let f = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid ODT archive: {e}"),
        )
    })?;

    let title =
        read_zip_entry_string(&mut archive, "meta.xml").and_then(|xml| extract_dc_title(&xml));
    let content_xml = read_zip_entry_string(&mut archive, "content.xml")
        .ok_or_else(|| LnaError::new(ErrorCode::SourceUnsupported, "ODT missing content.xml"))?;

    let mut blocks = Vec::new();
    let mut section_stack: Vec<(u8, String)> = Vec::new();
    let mut cursor = 0usize;

    let mut reader = Reader::from_str(&content_xml);
    reader.config_mut().trim_text(false);

    let mut cur_kind: Option<BlockKind> = None;
    let mut in_text_el = false;
    let mut cur_text = String::new();
    let mut in_list_item = false;

    let mut table_depth = 0usize;
    let mut cur_table_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_text = String::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name_q = e.name();
                let qname = std::str::from_utf8(name_q.as_ref()).unwrap_or("");
                let lname = local_name_str(name_q.as_ref());
                match qname {
                    "table:table" => {
                        if table_depth == 0 {
                            cur_table_rows.clear();
                        }
                        table_depth += 1;
                    }
                    "table:table-row" if table_depth == 1 => cur_row.clear(),
                    "table:table-cell" if table_depth == 1 => {
                        in_cell = true;
                        cell_text.clear();
                    }
                    "text:list-item" => in_list_item = true,
                    "text:h" => {
                        let lvl = attr_value(e, &reader, "outline-level")
                            .and_then(|s| s.parse::<u8>().ok())
                            .unwrap_or(1)
                            .clamp(1, 6);
                        cur_kind = Some(BlockKind::Heading { level: lvl });
                        in_text_el = true;
                        cur_text.clear();
                    }
                    "text:p" => {
                        cur_kind = Some(if in_list_item {
                            BlockKind::ListItem
                        } else {
                            BlockKind::Paragraph
                        });
                        in_text_el = true;
                        cur_text.clear();
                    }
                    _ if lname == "s" && in_text_el => {
                        let c = attr_value(e, &reader, "c")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(1)
                            .min(16);
                        for _ in 0..c {
                            cur_text.push(' ');
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                if in_text_el {
                    let name_q = e.name();
                    let qname = std::str::from_utf8(name_q.as_ref()).unwrap_or("");
                    match qname {
                        "text:s" => {
                            let c = attr_value(e, &reader, "c")
                                .and_then(|s| s.parse::<usize>().ok())
                                .unwrap_or(1)
                                .min(16);
                            for _ in 0..c {
                                cur_text.push(' ');
                            }
                        }
                        "text:tab" => cur_text.push('\t'),
                        "text:line-break" => cur_text.push(' '),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) if in_text_el => {
                if let Ok(t) = e.xml10_content() {
                    cur_text.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                let name_q = e.name();
                let qname = std::str::from_utf8(name_q.as_ref()).unwrap_or("");
                match qname {
                    "text:h" | "text:p" => {
                        in_text_el = false;
                        let trimmed = cur_text.trim().to_string();
                        if !trimmed.is_empty() {
                            if in_cell {
                                if !cell_text.is_empty() {
                                    cell_text.push(' ');
                                }
                                cell_text.push_str(&trimmed);
                            } else if table_depth == 0 {
                                let kind = cur_kind.take().unwrap_or(BlockKind::Paragraph);
                                if let BlockKind::Heading { level } = kind {
                                    while section_stack
                                        .last()
                                        .map(|(l, _)| *l >= level)
                                        .unwrap_or(false)
                                    {
                                        section_stack.pop();
                                    }
                                    section_stack.push((level, trimmed.clone()));
                                }
                                let start = cursor;
                                cursor += trimmed.len();
                                blocks.push(Block {
                                    kind,
                                    text: trimmed,
                                    page: None,
                                    section_path: section_stack
                                        .iter()
                                        .map(|(_, s)| s.clone())
                                        .collect(),
                                    char_range: start..cursor,
                                });
                            }
                        }
                        cur_text.clear();
                        cur_kind = None;
                    }
                    "text:list-item" => in_list_item = false,
                    "table:table-cell" if table_depth == 1 => {
                        in_cell = false;
                        cur_row.push(cell_text.trim().to_string());
                        cell_text.clear();
                    }
                    "table:table-row" if table_depth == 1 => {
                        if !cur_row.is_empty() {
                            cur_table_rows.push(std::mem::take(&mut cur_row));
                        }
                    }
                    "table:table" => {
                        table_depth = table_depth.saturating_sub(1);
                        if table_depth == 0 {
                            if let Some(tbl_md) = format_markdown_table(&cur_table_rows) {
                                let start = cursor;
                                cursor += tbl_md.len();
                                blocks.push(Block {
                                    kind: BlockKind::Table,
                                    text: tbl_md,
                                    page: None,
                                    section_path: section_stack
                                        .iter()
                                        .map(|(_, s)| s.clone())
                                        .collect(),
                                    char_range: start..cursor,
                                });
                            }
                            cur_table_rows.clear();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    let embedded_images = collect_zip_images(&mut archive, &["Pictures/"]);
    let sample = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(RichParseOutput {
        doc: ParsedDoc {
            title,
            language: Some(parser::detect_language(&sample)),
            page_count: None,
            blocks,
        },
        embedded_images,
    })
}

pub fn parse_rtf(bytes: &[u8]) -> LnaResult<ParsedDoc> {
    if !bytes.starts_with(b"{\\rtf") {
        let leading_trim = bytes
            .iter()
            .skip_while(|b| b.is_ascii_whitespace())
            .copied()
            .take(5)
            .collect::<Vec<_>>();
        if leading_trim != b"{\\rtf" {
            return Err(LnaError::new(
                ErrorCode::SourceUnsupported,
                "Invalid RTF header",
            ));
        }
    }

    #[derive(Clone, Copy)]
    struct RtfState {
        uc: usize,
        skip: bool,
        enc: &'static encoding_rs::Encoding,
    }

    const SKIP_DESTINATIONS: &[&str] = &[
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "object",
        "fldinst",
        "header",
        "footer",
        "headerl",
        "headerr",
        "headerf",
        "footerl",
        "footerr",
        "footerf",
        "listtable",
        "listoverridetable",
        "revtbl",
        "rsidtbl",
        "xmlnstbl",
        "themedata",
        "colorschememapping",
        "latentstyles",
        "datastore",
    ];

    let mut stack = vec![RtfState {
        uc: 1,
        skip: false,
        enc: encoding_rs::WINDOWS_1252,
    }];
    let mut out = String::new();
    let mut hex_buf: Vec<u8> = Vec::new();
    let mut skip_chars = 0usize;
    let mut ignorable_pending = false;
    let mut i = 0usize;

    let flush_hex = |hex_buf: &mut Vec<u8>,
                     enc: &'static encoding_rs::Encoding,
                     skip: bool,
                     out: &mut String| {
        if hex_buf.is_empty() {
            return;
        }
        if !skip {
            let (cow, _, _) = enc.decode(hex_buf);
            out.push_str(&cow);
        }
        hex_buf.clear();
    };

    while i < bytes.len() {
        let cur = *stack.last().unwrap();
        let b = bytes[i];
        match b {
            b'{' => {
                flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                stack.push(cur);
                ignorable_pending = false;
                i += 1;
            }
            b'}' => {
                flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                if stack.len() > 1 {
                    stack.pop();
                }
                ignorable_pending = false;
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                let esc = bytes[i];
                match esc {
                    b'\\' | b'{' | b'}' => {
                        flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        if skip_chars > 0 {
                            skip_chars -= 1;
                        } else if !cur.skip {
                            out.push(esc as char);
                        }
                        i += 1;
                    }
                    b'*' => {
                        flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        ignorable_pending = true;
                        i += 1;
                    }
                    b'\'' => {
                        i += 1;
                        if i + 1 < bytes.len() {
                            if let (Some(h), Some(l)) = (
                                (bytes[i] as char).to_digit(16),
                                (bytes[i + 1] as char).to_digit(16),
                            ) {
                                let byte_val = ((h << 4) | l) as u8;
                                if skip_chars > 0 {
                                    skip_chars -= 1;
                                } else if !cur.skip {
                                    hex_buf.push(byte_val);
                                }
                                i += 2;
                            }
                        }
                    }
                    b'~' => {
                        flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        if !cur.skip {
                            out.push(' ');
                        }
                        i += 1;
                    }
                    b'_' => {
                        flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        if !cur.skip {
                            out.push('-');
                        }
                        i += 1;
                    }
                    b'-' => {
                        i += 1;
                    }
                    b'\r' | b'\n' => {
                        flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        if !cur.skip {
                            out.push('\n');
                        }
                        i += 1;
                    }
                    _ if esc.is_ascii_alphabetic() => {
                        let start_w = i;
                        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        let word = std::str::from_utf8(&bytes[start_w..i]).unwrap_or("");
                        let mut param_str = String::new();
                        if i < bytes.len() && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                            param_str.push(bytes[i] as char);
                            i += 1;
                            while i < bytes.len() && bytes[i].is_ascii_digit() {
                                param_str.push(bytes[i] as char);
                                i += 1;
                            }
                        }
                        if i < bytes.len() && bytes[i] == b' ' {
                            i += 1;
                        }
                        let param_val = param_str.parse::<i32>().ok();

                        if word != "u" {
                            flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                        }

                        if ignorable_pending || SKIP_DESTINATIONS.contains(&word) {
                            if let Some(top) = stack.last_mut() {
                                top.skip = true;
                            }
                            ignorable_pending = false;
                            continue;
                        }
                        ignorable_pending = false;

                        match word {
                            "ansicpg" => {
                                if let Some(cp) = param_val {
                                    let enc = rtf_codepage_to_encoding(cp);
                                    for st in &mut stack {
                                        st.enc = enc;
                                    }
                                }
                            }
                            "uc" => {
                                if let Some(n) = param_val {
                                    if let Some(top) = stack.last_mut() {
                                        top.uc = n.clamp(0, 8) as usize;
                                    }
                                }
                            }
                            "u" => {
                                flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                                if let Some(n) = param_val {
                                    let code_u16 = n as i16 as u16;
                                    if !cur.skip {
                                        if let Some(ch) = char::from_u32(code_u16 as u32) {
                                            out.push(ch);
                                        }
                                    }
                                    skip_chars = cur.uc;
                                }
                            }
                            "par" | "line" => {
                                if !cur.skip {
                                    out.push_str("\n\n");
                                }
                            }
                            "tab" | "cell" => {
                                if !cur.skip {
                                    out.push('\t');
                                }
                            }
                            "row" => {
                                if !cur.skip {
                                    out.push('\n');
                                }
                            }
                            "emdash" => {
                                if !cur.skip {
                                    out.push('—');
                                }
                            }
                            "endash" => {
                                if !cur.skip {
                                    out.push('–');
                                }
                            }
                            "lquote" => {
                                if !cur.skip {
                                    out.push('‘');
                                }
                            }
                            "rquote" => {
                                if !cur.skip {
                                    out.push('’');
                                }
                            }
                            "ldblquote" => {
                                if !cur.skip {
                                    out.push('“');
                                }
                            }
                            "rdblquote" => {
                                if !cur.skip {
                                    out.push('”');
                                }
                            }
                            "bullet" if !cur.skip => out.push_str("• "),
                            _ => {}
                        }
                    }
                    _ => {
                        ignorable_pending = false;
                        i += 1;
                    }
                }
            }
            b'\r' | b'\n' => {
                i += 1;
            }
            _ => {
                flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);
                ignorable_pending = false;
                if skip_chars > 0 {
                    skip_chars -= 1;
                } else if !cur.skip {
                    out.push(b as char);
                }
                i += 1;
            }
        }
    }
    let cur = *stack.last().unwrap();
    flush_hex(&mut hex_buf, cur.enc, cur.skip, &mut out);

    Ok(parser::parse_plain_text(&out))
}

fn rtf_codepage_to_encoding(cp: i32) -> &'static encoding_rs::Encoding {
    match cp {
        936 => encoding_rs::GBK,
        950 => encoding_rs::BIG5,
        932 => encoding_rs::SHIFT_JIS,
        949 => encoding_rs::EUC_KR,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        65001 => encoding_rs::UTF_8,
        _ => encoding_rs::WINDOWS_1252,
    }
}

pub fn parse_hwpx(path: &Path) -> LnaResult<RichParseOutput> {
    let f = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid HWPX archive: {e}"),
        )
    })?;

    let mut sections: Vec<(u32, String)> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            let low = name.to_ascii_lowercase();
            if low.starts_with("contents/section") && low.ends_with(".xml") {
                let digits: String = low
                    .trim_start_matches("contents/section")
                    .trim_end_matches(".xml")
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                let idx = digits.parse::<u32>().unwrap_or(sections.len() as u32);
                sections.push((idx, name));
            }
        }
    }
    sections.sort_by_key(|(idx, _)| *idx);

    if sections.is_empty() {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "HWPX archive contains no section XML files",
        ));
    }

    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    for (idx, sec_name) in sections {
        if let Some(xml) = read_zip_entry_string(&mut archive, &sec_name) {
            parse_hwpx_section(&xml, idx + 1, &mut blocks, &mut cursor);
        }
    }

    let embedded_images = collect_zip_images(&mut archive, &["BinData/"]);
    let sample = blocks
        .iter()
        .take(5)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(RichParseOutput {
        doc: ParsedDoc {
            title: None,
            language: Some(parser::detect_language(&sample)),
            page_count: None,
            blocks,
        },
        embedded_images,
    })
}

fn parse_hwpx_section(xml: &str, section_no: u32, blocks: &mut Vec<Block>, cursor: &mut usize) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let sec = vec![format!("Section {section_no}")];

    let mut in_p = false;
    let mut in_t = false;
    let mut p_text = String::new();
    let mut table_depth = 0usize;
    let mut cur_table_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut in_cell = false;
    let mut cell_text = String::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match local_name_str(e.name().as_ref()) {
                "tbl" => {
                    if table_depth == 0 {
                        cur_table_rows.clear();
                    }
                    table_depth += 1;
                }
                "tr" if table_depth == 1 => cur_row.clear(),
                "tc" if table_depth == 1 => {
                    in_cell = true;
                    cell_text.clear();
                }
                "p" => {
                    in_p = true;
                    p_text.clear();
                }
                "t" => in_t = true,
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match local_name_str(e.name().as_ref()) {
                "tab" if in_p => p_text.push('\t'),
                "lineBreak" | "br" if in_p => p_text.push(' '),
                _ => {}
            },
            Ok(Event::Text(e)) if in_t => {
                if let Ok(t) = e.xml10_content() {
                    p_text.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => match local_name_str(e.name().as_ref()) {
                "t" => in_t = false,
                "p" => {
                    in_p = false;
                    let trimmed = p_text.trim().to_string();
                    if !trimmed.is_empty() {
                        if in_cell {
                            if !cell_text.is_empty() {
                                cell_text.push(' ');
                            }
                            cell_text.push_str(&trimmed);
                        } else if table_depth == 0 {
                            let start = *cursor;
                            *cursor += trimmed.len();
                            blocks.push(Block {
                                kind: BlockKind::Paragraph,
                                text: trimmed,
                                page: None,
                                section_path: sec.clone(),
                                char_range: start..*cursor,
                            });
                        }
                    }
                    p_text.clear();
                }
                "tc" if table_depth == 1 => {
                    in_cell = false;
                    cur_row.push(cell_text.trim().to_string());
                    cell_text.clear();
                }
                "tr" if table_depth == 1 => {
                    if !cur_row.is_empty() {
                        cur_table_rows.push(std::mem::take(&mut cur_row));
                    }
                }
                "tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        if let Some(tbl_md) = format_markdown_table(&cur_table_rows) {
                            let start = *cursor;
                            *cursor += tbl_md.len();
                            blocks.push(Block {
                                kind: BlockKind::Table,
                                text: tbl_md,
                                page: None,
                                section_path: sec.clone(),
                                char_range: start..*cursor,
                            });
                        }
                        cur_table_rows.clear();
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

pub fn parse_hwp(path: &Path) -> LnaResult<ParsedDoc> {
    let file = std::fs::File::open(path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut comp = cfb::CompoundFile::open(file).map_err(|e| {
        LnaError::new(
            ErrorCode::SourceUnsupported,
            format!("Invalid HWP 5.0 compound file: {e}"),
        )
    })?;

    let header = read_cfb_stream(&mut comp, &["/FileHeader", "FileHeader"]).unwrap_or_default();
    if header.len() < 40 || !header.starts_with(b"HWP Document File") {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "Unrecognized HWP file header",
        ));
    }
    let flags = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    let compressed = (flags & 0x01) != 0;
    let encrypted = (flags & 0x02) != 0;
    if encrypted {
        return Err(LnaError::new(
            ErrorCode::SourceEncrypted,
            "HWP file is password-protected",
        ));
    }

    let mut section_paths: Vec<(u32, String)> = Vec::new();
    for entry in comp.walk() {
        let p = entry.path().to_string_lossy().replace('\\', "/");
        if let Some(rest) = p
            .strip_prefix("/BodyText/Section")
            .or_else(|| p.strip_prefix("BodyText/Section"))
        {
            if let Ok(idx) = rest.parse::<u32>() {
                section_paths.push((idx, p));
            }
        }
    }
    section_paths.sort_by_key(|(idx, _)| *idx);

    let mut all_paragraphs: Vec<String> = Vec::new();
    for (_, sp) in section_paths {
        let Some(raw) = read_cfb_stream(&mut comp, &[sp.as_str()]) else {
            continue;
        };
        let data = if compressed {
            let mut dec = Vec::new();
            let mut d = flate2::read::DeflateDecoder::new(Cursor::new(&raw));
            if d.read_to_end(&mut dec).is_err() {
                dec.clear();
                let mut z = flate2::read::ZlibDecoder::new(Cursor::new(&raw));
                if z.read_to_end(&mut dec).is_err() {
                    continue;
                }
            }
            dec
        } else {
            raw
        };
        extract_hwp_section_paragraphs(&data, &mut all_paragraphs);
    }

    if all_paragraphs.is_empty() {
        if let Some(prv) = read_cfb_stream(&mut comp, &["/PrvText", "PrvText"]) {
            let text = decode_utf16le_lossy(&prv);
            return Ok(parser::parse_plain_text(&text));
        }
    }

    if all_paragraphs.is_empty() {
        return Err(LnaError::new(
            ErrorCode::SourceUnsupported,
            "HWP file contains no extractable text",
        ));
    }

    let combined = all_paragraphs.join("\n\n");
    Ok(parser::parse_plain_text(&combined))
}

fn extract_hwp_section_paragraphs(data: &[u8], out: &mut Vec<String>) {
    const HWPTAG_PARA_TEXT: u32 = 67;
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        let hdr = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
        let tag_id = hdr & 0x3FF;
        let mut size = ((hdr >> 20) & 0xFFF) as usize;
        if size == 0xFFF {
            if pos + 4 > data.len() {
                break;
            }
            size = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            pos += 4;
        }
        if pos + size > data.len() {
            break;
        }
        let payload = &data[pos..pos + size];
        pos += size;

        if tag_id == HWPTAG_PARA_TEXT {
            let units: Vec<u16> = payload
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            let mut text_units: Vec<u16> = Vec::with_capacity(units.len());
            let mut i = 0usize;
            while i < units.len() {
                let code = units[i];
                if code < 32 {
                    match code {
                        10 | 13 => text_units.push(b'\n' as u16),
                        24 => text_units.push(b'-' as u16),
                        30 | 31 => text_units.push(b' ' as u16),
                        0 | 25..=29 => {}
                        _ => {
                            if code == 9 && (i + 7 >= units.len() || units[i + 7] != 9) {
                                text_units.push(b'\t' as u16);
                            } else if i + 7 < units.len() {
                                if code == 9 {
                                    text_units.push(b'\t' as u16);
                                }
                                i += 8;
                                continue;
                            }
                        }
                    }
                    i += 1;
                } else {
                    text_units.push(code);
                    i += 1;
                }
            }
            let s = String::from_utf16_lossy(&text_units);
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                out.push(trimmed);
            }
        }
    }
}

fn read_cfb_stream(comp: &mut cfb::CompoundFile<std::fs::File>, names: &[&str]) -> Option<Vec<u8>> {
    for &name in names {
        if let Ok(mut s) = comp.open_stream(name) {
            let mut buf = Vec::new();
            if s.read_to_end(&mut buf).is_ok() {
                return Some(buf);
            }
        }
    }
    None
}

fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}
