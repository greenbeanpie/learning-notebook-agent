use base64::Engine as _;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use image::{ImageEncoder, Rgb, RgbImage};
use lna_embedding::EmbeddingService;
use lna_ingest::chunker::chunk_document;
use lna_ingest::formats;
use lna_ingest::media::MEDIA_SETTINGS_KEY;
use lna_ingest::office;
use lna_ingest::parser::{self, BlockKind};
use lna_ingest::IngestService;
use lna_jobs::JobQueue;
use lna_llm::LlmClient;
use lna_secrets::KeyringSecretStore;
use lna_storage::{Database, StoragePaths};
use lna_types::{ErrorCode, ModelCapabilities, ProfileUpsertParams, ProtocolKind, SourceKind};
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lna_ext_{name}_{}_{}",
        std::process::id(),
        lna_types::now_ms()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tiny_png_bytes() -> Vec<u8> {
    let mut img = RgbImage::new(32, 32);
    for p in img.pixels_mut() {
        *p = Rgb([20, 120, 220]);
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut out))
        .write_image(img.as_raw(), 32, 32, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn build_zip_file(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(data).unwrap();
    }
    zw.finish().unwrap();
}

#[test]
fn format_classification_covers_every_requested_extension_and_plain_text_fallback() {
    let pdf_exts = ["pdf"];
    for e in pdf_exts {
        assert_eq!(formats::classify_extension(e), Some(SourceKind::Pdf));
    }
    for e in ["md", "markdown"] {
        assert_eq!(formats::classify_extension(e), Some(SourceKind::Md));
    }
    for e in ["txt", "py", "rs", "json", "yaml", "xml", "html", "tex", "log", "cs", "sql"] {
        assert_eq!(formats::classify_extension(e), Some(SourceKind::Txt));
    }
    assert_eq!(formats::classify_extension("docx"), Some(SourceKind::Docx));
    assert_eq!(formats::classify_extension("csv"), Some(SourceKind::Csv));
    assert_eq!(formats::classify_extension("pptx"), Some(SourceKind::Pptx));
    assert_eq!(formats::classify_extension("epub"), Some(SourceKind::Epub));
    assert_eq!(formats::classify_extension("odt"), Some(SourceKind::Odt));
    assert_eq!(formats::classify_extension("rtf"), Some(SourceKind::Rtf));
    assert_eq!(formats::classify_extension("hwp"), Some(SourceKind::Hwp));
    assert_eq!(formats::classify_extension("hwpx"), Some(SourceKind::Hwpx));

    for e in [
        "avif", "bmp", "gif", "ico", "jp2", "png", "webp", "tif", "tiff", "heic", "heif", "jpeg",
        "jpg", "jpe",
    ] {
        assert_eq!(
            formats::classify_extension(e),
            Some(SourceKind::Image),
            "image ext {e}"
        );
    }

    for e in [
        "aac", "aif", "aifc", "aiff", "amr", "au", "m4a", "mp3", "ogg", "opus", "ra", "snd",
        "wav", "wma",
    ] {
        assert_eq!(
            formats::classify_extension(e),
            Some(SourceKind::Audio),
            "audio ext {e}"
        );
    }

    for e in ["3g2", "3gp", "avi", "mp4", "mpeg"] {
        assert_eq!(
            formats::classify_extension(e),
            Some(SourceKind::Video),
            "video ext {e}"
        );
    }

    assert!(formats::is_probably_plain_text("hello, 这是一份纯文本\n第二行".as_bytes()));
    assert!(!formats::is_probably_plain_text(&[0x00, 0x01, 0xFF, 0x00, 0x10]));
}

#[test]
fn office_and_ebook_parsers_extract_text_tables_and_images() {
    let dir = temp_dir("office");

    // 1. CSV (>25 rows repeats header across table chunks)
    let mut csv_s = "姓名,分数,备注\n".to_string();
    for i in 1..=30 {
        csv_s.push_str(&format!("学生{i},{},第{i}名\n", 100 - i));
    }
    let csv_doc = office::parse_csv(csv_s.as_bytes()).unwrap();
    assert_eq!(csv_doc.blocks.len(), 2);
    assert!(csv_doc.blocks[1].text.contains("姓名"));
    assert!(csv_doc.blocks[1].text.contains("学生30"));

    // 2. DOCX
    let docx_path = dir.join("sample.docx");
    let png = tiny_png_bytes();
    let docx_core = r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>实验报告</dc:title></cp:coreProperties>"#;
    let docx_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>第一章 绪论</w:t></w:r></w:p>
    <w:p><w:r><w:t>这是 DOCX 正文段落内容。</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>指标</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>数值</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>准确率</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>98.5%</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;
    build_zip_file(
        &docx_path,
        &[
            ("docProps/core.xml", docx_core.as_bytes()),
            ("word/document.xml", docx_body.as_bytes()),
            ("word/media/image1.png", &png),
        ],
    );
    let docx_res = office::parse_docx(&docx_path).unwrap();
    assert_eq!(docx_res.doc.title.as_deref(), Some("实验报告"));
    assert!(docx_res.doc.blocks.iter().any(|b| b.text.contains("DOCX 正文段落")));
    assert!(docx_res.doc.blocks.iter().any(|b| matches!(b.kind, BlockKind::Table) && b.text.contains("98.5%")));
    assert_eq!(docx_res.embedded_images.len(), 1);
    let extracted_img =
        office::extract_zip_embedded_image(&docx_path, &docx_res.embedded_images[0].zip_entry)
            .unwrap();
    assert_eq!(extracted_img, png);

    // 3. PPTX (2 slides + speaker notes + image relationship mapped to page 2)
    let pptx_path = dir.join("deck.pptx");
    let slide1 = r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>第一页标题</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
    let slide2 = r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>系统架构图解</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
    let notes2 = r#"<?xml version="1.0" encoding="UTF-8"?><p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>演讲者提醒重点解释图表。</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#;
    let rels2 = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="image" Target="../media/arch.png"/></Relationships>"#;
    build_zip_file(
        &pptx_path,
        &[
            ("ppt/slides/slide1.xml", slide1.as_bytes()),
            ("ppt/slides/slide2.xml", slide2.as_bytes()),
            ("ppt/notesSlides/notesSlide2.xml", notes2.as_bytes()),
            ("ppt/slides/_rels/slide2.xml.rels", rels2.as_bytes()),
            ("ppt/media/arch.png", &png),
        ],
    );
    let pptx_res = office::parse_pptx(&pptx_path).unwrap();
    assert_eq!(pptx_res.doc.page_count, Some(2));
    assert!(pptx_res
        .doc
        .blocks
        .iter()
        .any(|b| b.page == Some(2) && b.text.contains("演讲者提醒")));
    assert_eq!(pptx_res.embedded_images[0].page, Some(2));

    // 4. EPUB
    let epub_path = dir.join("book.epub");
    let container = r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let opf = r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>电子书测试</dc:title></metadata><manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="i1" href="images/fig.png" media-type="image/png"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
    let ch1 = r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml"><body><h1>序言</h1><p>EPUB 正文测试内容。</p></body></html>"#;
    build_zip_file(
        &epub_path,
        &[
            ("META-INF/container.xml", container.as_bytes()),
            ("OEBPS/content.opf", opf.as_bytes()),
            ("OEBPS/ch1.xhtml", ch1.as_bytes()),
            ("OEBPS/images/fig.png", &png),
        ],
    );
    let epub_res = office::parse_epub(&epub_path).unwrap();
    assert_eq!(epub_res.doc.title.as_deref(), Some("电子书测试"));
    assert!(epub_res.doc.blocks.iter().any(|b| b.text.contains("EPUB 正文")));
    assert_eq!(epub_res.embedded_images.len(), 1);

    // 5. ODT
    let odt_path = dir.join("doc.odt");
    let odt_content = r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:text><text:h text:outline-level="1">ODT 标题</text:h><text:p>ODT 正文内容。</text:p></office:text></office:body></office:document-content>"#;
    build_zip_file(&odt_path, &[("content.xml", odt_content.as_bytes())]);
    let odt_res = office::parse_odt(&odt_path).unwrap();
    assert!(odt_res.doc.blocks.iter().any(|b| b.text.contains("ODT 正文内容")));

    // 6. RTF with GBK ansicpg936 hex + Unicode escape + ignored fonttbl
    let (gbk_bytes, _, _) = encoding_rs::GBK.encode("中文内容");
    let gbk_hex: String = gbk_bytes.iter().map(|b| format!("\\'{b:02x}")).collect();
    let rtf_payload = format!(
        "{{\\rtf1\\ansi\\ansicpg936\\uc1{{\\fonttbl{{\\f0 SimSun;}}}}{{\\*\\generator test}}Hello {gbk_hex}\\u27979?\\par Second line}}"
    );
    let rtf_doc = office::parse_rtf(rtf_payload.as_bytes()).unwrap();
    let rtf_joined: String = rtf_doc.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
    assert!(rtf_joined.contains("Hello 中文内容测"), "got: {rtf_joined}");
    assert!(rtf_joined.contains("Second line"), "got: {rtf_joined}");
    assert!(!rtf_joined.contains("SimSun"));

    // 7. HWPX
    let hwpx_path = dir.join("hangul.hwpx");
    let hwpx_sec = r#"<?xml version="1.0" encoding="UTF-8"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p><hp:run><hp:t>안녕하세요 HWPX 테스트입니다.</hp:t></hp:run></hp:p></hs:sec>"#;
    build_zip_file(&hwpx_path, &[("Contents/section0.xml", hwpx_sec.as_bytes())]);
    let hwpx_res = office::parse_hwpx(&hwpx_path).unwrap();
    assert!(hwpx_res.doc.blocks.iter().any(|b| b.text.contains("HWPX 테스트")));

    // 8. HWP 5.0 OLE2 compound file (compressed deflate stream)
    let hwp_path = dir.join("legacy.hwp");
    {
        let file = std::fs::File::create(&hwp_path).unwrap();
        let mut comp = cfb::CompoundFile::create(file).unwrap();
        let mut fh = vec![0u8; 256];
        fh[..17].copy_from_slice(b"HWP Document File");
        fh[36..40].copy_from_slice(&1u32.to_le_bytes()); // bit 0 = compressed
        comp.create_stream("/FileHeader")
            .unwrap()
            .write_all(&fh)
            .unwrap();
        comp.create_storage("/BodyText").unwrap();

        let text_u16: Vec<u16> = "한글 HWP 본문 테스트".encode_utf16().collect();
        let payload_len = text_u16.len() * 2;
        let rec_hdr: u32 = 67u32 | ((payload_len as u32) << 20);
        let mut raw_sec = Vec::new();
        raw_sec.extend_from_slice(&rec_hdr.to_le_bytes());
        for u in text_u16 {
            raw_sec.extend_from_slice(&u.to_le_bytes());
        }
        let mut deflater = DeflateEncoder::new(Vec::new(), Compression::default());
        deflater.write_all(&raw_sec).unwrap();
        let comp_bytes = deflater.finish().unwrap();
        comp.create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&comp_bytes)
            .unwrap();
        comp.flush().unwrap();
    }
    let hwp_doc = office::parse_hwp(&hwp_path).unwrap();
    assert!(hwp_doc.blocks.iter().any(|b| b.text.contains("한글 HWP 본문")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sqlite_migrates_cleanly_from_v1_to_v2_and_accepts_extended_kinds() {
    let dir = temp_dir("mig");
    let db_path = dir.join("notebook.db");

    // Seed an authentic v1 DB using 0001_init.sql and user_version = 1
    {
        let conn = Connection::open(&db_path).unwrap();
        let init_sql = include_str!("../../../migrations/0001_init.sql");
        conn.execute_batch(init_sql).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO notebooks (id, title, created_at, updated_at) VALUES ('nb1', 'Legacy', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (id, notebook_id, title, kind, content_hash, created_at, updated_at)
             VALUES ('s1', 'nb1', 'old.pdf', 'pdf', 'h1', 1, 1)",
            [],
        )
        .unwrap();
    }

    let paths = StoragePaths::from_root(&dir).unwrap();
    let db = Database::open(paths).unwrap();
    let srcs = lna_storage::list_sources(&db, "nb1").unwrap();
    assert_eq!(srcs.len(), 1);
    assert_eq!(srcs[0].kind, SourceKind::Pdf);

    // Verify inserting previously disallowed kind='docx' / 'video' now succeeds
    db.write(|conn| {
        conn.execute(
            "INSERT INTO sources (id, notebook_id, title, kind, content_hash, created_at, updated_at)
             VALUES ('s2', 'nb1', 'clip.mp4', 'video', 'h2', 2, 2)",
            [],
        )
        .map_err(|e| lna_types::LnaError::internal(e.to_string()))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        lna_storage::get_source(&db, "s2").unwrap().kind,
        SourceKind::Video
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn audio_video_toggle_and_multimodal_modality_routing_work_end_to_end() {
    let dir = temp_dir("ingest_mm");
    let paths = StoragePaths::from_root(&dir).unwrap();
    let db = Database::open(paths).unwrap();
    let secrets: Arc<dyn lna_secrets::SecretStore> = Arc::new(KeyringSecretStore::new(true));
    let jobs = JobQueue::new(db.clone());
    let embedding = EmbeddingService::new(db.clone(), secrets.clone());
    let llm = LlmClient::new();
    let ingest = IngestService::new(db.clone(), jobs, embedding, llm, secrets.clone());

    let nb = lna_storage::create_notebook(&db, "MediaTest", None).unwrap();

    // 1. Disable audio/video -> add_file must reject .mp4 and .mp3 immediately
    lna_storage::set_setting_json(&db, MEDIA_SETTINGS_KEY, &json!({ "audio_video_enabled": false }))
        .unwrap();
    let mp4_path = dir.join("lecture.mp4");
    std::fs::write(&mp4_path, b"fake video stream bytes").unwrap();
    let err = ingest.add_file(&nb.id, &mp4_path).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceUnsupported);

    // 2. Re-enable audio/video and configure a secondary Gemini profile ONLY for video (chat stays inactive/different)
    lna_storage::set_setting_json(&db, MEDIA_SETTINGS_KEY, &json!({ "audio_video_enabled": true }))
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 32768];
        let n = sock.read(&mut buf).await.unwrap();
        let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(req_str.contains("inlineData") && req_str.contains("video/mp4"));
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"# 内容概要\\n\\n[00:05] 视频讲解了基与维数定义。\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":120,\"candidatesTokenCount\":35}}\n\n"
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            sse.len(),
            sse
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
    });

    // Save active text-only profile
    lna_storage::upsert_profile(
        &db,
        &ProfileUpsertParams {
            id: Some("prof_chat".into()),
            display_name: "DeepSeek Chat".into(),
            protocol: ProtocolKind::ChatCompletion,
            base_url: "https://127.0.0.1:1/v1".into(),
            model_id: "deepseek-chat".into(),
            show_thinking: false,
            effort: None,
            context_window: 32000,
            max_output_tokens: 4000,
            extra_headers: BTreeMap::new(),
            input_price_per_m: None,
            output_price_per_m: None,
            capabilities: Some(ModelCapabilities::default()),
            supports_vision: Some(false),
            supports_audio: Some(false),
            supports_video: Some(false),
            activate: true,
        },
    )
    .unwrap();

    // Save inactive Gemini profile assigned specifically to Vision + Video
    lna_storage::upsert_profile(
        &db,
        &ProfileUpsertParams {
            id: Some("prof_gemini_vid".into()),
            display_name: "Gemini Video".into(),
            protocol: ProtocolKind::Gemini,
            base_url: format!("http://{addr}/v1beta"),
            model_id: "gemini-2.5-flash".into(),
            show_thinking: false,
            effort: None,
            context_window: 1000000,
            max_output_tokens: 8000,
            extra_headers: BTreeMap::new(),
            input_price_per_m: None,
            output_price_per_m: None,
            capabilities: None,
            supports_vision: Some(true),
            supports_audio: Some(true),
            supports_video: Some(true),
            activate: false,
        },
    )
    .unwrap();

    let (src, dup) = ingest.add_file(&nb.id, &mp4_path).unwrap();
    assert!(!dup);
    assert_eq!(src.kind, SourceKind::Video);
    ingest.process_ingest_job(&src.id).await.unwrap();
    srv.await.unwrap();

    let preview = ingest.source_preview(&src.id, None).unwrap();
    assert!(!preview.is_empty());
    assert!(preview.iter().any(|c| c.content.contains("基与维数")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn arbitrary_utf16_and_bmp_transcoding_work() {
    // 1. UTF-16LE BOM plain text decoding
    let mut utf16_bytes = vec![0xFFu8, 0xFEu8];
    for u in "线性代数讲义 UTF-16 测试".encode_utf16() {
        utf16_bytes.extend_from_slice(&u.to_le_bytes());
    }
    assert!(formats::is_probably_plain_text(&utf16_bytes));
    let decoded = parser::decode_text_bytes(&utf16_bytes);
    assert!(decoded.contains("线性代数讲义 UTF-16 测试"));

    // 2. BMP -> PNG transcoding via pure-Rust image crate for LLM compatibility
    let mut bmp_img = RgbImage::new(16, 16);
    for p in bmp_img.pixels_mut() {
        *p = Rgb([250, 10, 10]);
    }
    let mut bmp_bytes = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp_bytes)
        .write_image(bmp_img.as_raw(), 16, 16, image::ExtendedColorType::Rgb8)
        .unwrap();
    let (mime, out_bytes) = lna_ingest::media::prepare_image_for_protocol(
        &bmp_bytes,
        "bmp",
        None,
        ProtocolKind::ChatCompletion,
    )
    .unwrap();
    assert_eq!(mime, "image/png");
    assert_eq!(&out_bytes[..4], b"\x89PNG");
    let _ = base64::engine::general_purpose::STANDARD.encode(&out_bytes);
    let _ = chunk_document(&parser::parse_plain_text(&decoded));
}

#[tokio::test]
async fn scanned_pdf_triggers_vision_ocr_with_page_numbers() {
    let pdfium_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if !pdfium_dir.join("pdfium.dll").exists() && !pdfium_dir.join("libpdfium.so").exists() {
        eprintln!("skipped: no PDFium in target/debug");
        return;
    }
    std::env::set_var("LNA_PDFIUM_DIR", &pdfium_dir);

    let dir = temp_dir("scanned_ocr");
    let paths = StoragePaths::from_root(&dir).unwrap();
    let db = Database::open(paths).unwrap();
    let secrets: Arc<dyn lna_secrets::SecretStore> = Arc::new(KeyringSecretStore::new(true));
    let jobs = JobQueue::new(db.clone());
    let embedding = EmbeddingService::new(db.clone(), secrets.clone());
    let llm = LlmClient::new();
    let ingest = IngestService::new(db.clone(), jobs, embedding, llm, secrets.clone());
    let nb = lna_storage::create_notebook(&db, "ScannedPdf", None).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::with_capacity(262144);
        let mut tmp = [0u8; 8192];
        let mut hend = None;
        let mut clen = 0usize;
        while let Ok(n) = sock.read(&mut tmp).await {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if hend.is_none() {
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    hend = Some(pos + 4);
                    for line in String::from_utf8_lossy(&buf[..pos + 4]).lines() {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                clen = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                }
            }
            if let Some(h) = hend {
                if buf.len() >= h + clen {
                    break;
                }
            }
        }
        let req_str = String::from_utf8_lossy(&buf).to_string();
        assert!(req_str.contains("inlineData") && req_str.contains("image/jpeg"));
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"=== PAGE 1 ===\\n第一页扫描图像识别出的数学定理。\\n=== PAGE 2 ===\\n第二页证明步骤。\\n=== PAGE 3 ===\\n第三页总结。\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n"
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            sse.len(),
            sse
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
    });

    lna_storage::upsert_profile(
        &db,
        &ProfileUpsertParams {
            id: Some("prof_vis".into()),
            display_name: "Gemini OCR".into(),
            protocol: ProtocolKind::Gemini,
            base_url: format!("http://{addr}/v1beta"),
            model_id: "gemini-2.5-flash".into(),
            show_thinking: false,
            effort: None,
            context_window: 100000,
            max_output_tokens: 4000,
            extra_headers: BTreeMap::new(),
            input_price_per_m: None,
            output_price_per_m: None,
            capabilities: None,
            supports_vision: Some(true),
            supports_audio: Some(false),
            supports_video: Some(false),
            activate: true,
        },
    )
    .unwrap();

    let scanned = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pdf/scanned.pdf");
    let (src, _) = ingest.add_file(&nb.id, &scanned).unwrap();
    ingest.process_ingest_job(&src.id).await.unwrap();
    srv.await.unwrap();

    let chunks = ingest.source_preview(&src.id, None).unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| c.content.contains("第一页扫描图像识别出的数学定理") && c.page_start == Some(1)));
    assert!(chunks.iter().any(|c| c.content.contains("第二页证明步骤") && c.page_end == Some(3)));

    let _ = std::fs::remove_dir_all(&dir);
}

#[ignore]
#[tokio::test]
async fn real_library_strang_pdf_and_md_parse_test() {
    let md_path = PathBuf::from(r"D:\Library\线性代数导论-第五版.md");
    let pdf_path = PathBuf::from(
        r"D:\Library\线性代数导论：第五版 (（美）Gilbert Strang著 海昕, 文军, 屈龙江, 钱旭译).pdf",
    );
    assert!(md_path.exists(), "missing {}", md_path.display());
    assert!(pdf_path.exists(), "missing {}", pdf_path.display());

    let dir = temp_dir("real_strang");
    let paths = StoragePaths::from_root(&dir).unwrap();
    let db = Database::open(paths).unwrap();
    let secrets: Arc<dyn lna_secrets::SecretStore> = Arc::new(KeyringSecretStore::new(true));
    let jobs = JobQueue::new(db.clone());
    let embedding = EmbeddingService::new(db.clone(), secrets.clone());
    let llm = LlmClient::new();
    let ingest = IngestService::new(db.clone(), jobs, embedding, llm, secrets.clone());
    let nb = lna_storage::create_notebook(&db, "StrangTest", None).unwrap();

    // 1. Test real Markdown file (~1.3 MB)
    let t0 = std::time::Instant::now();
    let (md_src, _) = ingest.add_file(&nb.id, &md_path).unwrap();
    assert_eq!(md_src.kind, SourceKind::Md);
    ingest.process_ingest_job(&md_src.id).await.unwrap();
    let md_elapsed = t0.elapsed();
    let md_chunks = ingest.source_preview(&md_src.id, None).unwrap();
    let md_tokens: i64 = md_chunks.iter().map(|c| c.token_count as i64).sum();
    let md_info = lna_storage::get_source(&db, &md_src.id).unwrap();
    println!(
        "[MD RESULT] status={:?} elapsed={:.2?} chunks={} total_tokens={} lang={:?} first_section={:?}",
        md_info.status,
        md_elapsed,
        md_chunks.len(),
        md_tokens,
        md_info.language,
        md_chunks.first().and_then(|c| c.section_path.clone())
    );
    assert!(!md_chunks.is_empty());

    // 2. Test real PDF file (~48 MB)
    let t_pdf = std::time::Instant::now();
    let extract = parser::parse_pdf_detailed(&pdf_path).unwrap();
    let pdf_scan_elapsed = t_pdf.elapsed();
    let total_pages = extract.page_meta.len();
    let img_pages = extract
        .page_meta
        .iter()
        .filter(|m| m.image_count > 0)
        .count();
    let text_blocks = extract.doc.blocks.len();
    println!(
        "[PDF STRUCTURE] scan_elapsed={:.2?} pages={} selectable_text_blocks={} ocr_needed_pages={} pages_with_images={} doc_title={:?}",
        pdf_scan_elapsed,
        total_pages,
        text_blocks,
        extract.ocr_needed_pages.len(),
        img_pages,
        extract.doc.title
    );
    for meta in extract.page_meta.iter().take(5) {
        println!(
            "  sample page {}: chars={} images={} needs_ocr={} sec={:?}",
            meta.page, meta.char_count, meta.image_count, meta.needs_ocr, meta.section_path
        );
    }

    // Benchmark PDFium JPEG render + Windows WinRT OCR on sample OCR pages + dense content page 4
    let mut sample_pages: Vec<u32> = if !extract.ocr_needed_pages.is_empty() {
        let len = extract.ocr_needed_pages.len();
        [0, len / 10, len / 4, len / 2, len.saturating_sub(1)]
            .into_iter()
            .filter_map(|i| extract.ocr_needed_pages.get(i).copied())
            .collect()
    } else {
        vec![1, 5, 10]
    };
    if total_pages >= 4 {
        sample_pages.push(4);
    }
    let mut uniq_pages = sample_pages;
    uniq_pages.sort_unstable();
    uniq_pages.dedup();

    let t_ren = std::time::Instant::now();
    let rendered = parser::render_pdf_pages_to_jpeg(&pdf_path, &uniq_pages, 1400).unwrap();
    let ren_elapsed = t_ren.elapsed();
    let mut tmp_paths = Vec::new();
    for (pno, jpg) in &rendered {
        let p = dir.join(format!("sample_{pno}.jpg"));
        std::fs::write(&p, jpg).unwrap();
        tmp_paths.push((*pno, p));
    }
    let refs: Vec<&std::path::Path> = tmp_paths.iter().map(|(_, p)| p.as_path()).collect();
    let t_ocr = std::time::Instant::now();
    let ocr_texts = lna_ingest::win_ocr::windows_ocr_image_files(&refs);
    let ocr_elapsed = t_ocr.elapsed();
    println!(
        "[PDF SAMPLE RENDER+OCR] sample_pages={:?} render_elapsed={:.2?} win_ocr_elapsed={:.2?}",
        uniq_pages, ren_elapsed, ocr_elapsed
    );
    for ((pno, _), txt_opt) in tmp_paths.iter().zip(ocr_texts.iter()) {
        let preview: String = txt_opt
            .as_deref()
            .unwrap_or("<EMPTY>")
            .chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ");
        println!("  [OCR PAGE {pno}] chars={} preview={preview:?}", txt_opt.as_deref().map(|s| s.chars().count()).unwrap_or(0));
    }

    // If most pages already have selectable text OR <= 40 OCR pages, run full ingest;
    // even for large scanned books, run full ingest so we test end-to-end (log progress/timing).
    if extract.ocr_needed_pages.len() <= 60 {
        let t_full = std::time::Instant::now();
        let (pdf_src, _) = ingest.add_file(&nb.id, &pdf_path).unwrap();
        ingest.process_ingest_job(&pdf_src.id).await.unwrap();
        let full_elapsed = t_full.elapsed();
        let pdf_chunks = ingest.source_preview(&pdf_src.id, None).unwrap();
        let pdf_info = lna_storage::get_source(&db, &pdf_src.id).unwrap();
        println!(
            "[PDF FULL INGEST] elapsed={:.2?} status={:?} chunks={} tokens={}",
            full_elapsed, pdf_info.status, pdf_chunks.len(), pdf_info.token_count
        );
        assert!(!pdf_chunks.is_empty());
    }

    let _ = std::fs::remove_dir_all(&dir);
}
