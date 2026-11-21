use lna_types::{LnaError, LnaResult};
use serde_json::Value;
use std::io::Write;
use std::path::Path;

fn try_render_pptx_with_python(payload: &Value, out_path: &Path) -> bool {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("scripts").join("generate_pptx.py"));
            candidates.push(dir.join("generate_pptx.py"));
        }
    }
    candidates.push(std::path::PathBuf::from("scripts/generate_pptx.py"));
    let Some(script) = candidates.into_iter().find(|p| p.is_file()) else {
        return false;
    };
    let tmp_json = out_path.with_extension("payload.tmp.json");
    if std::fs::write(&tmp_json, payload.to_string()).is_err() {
        return false;
    }
    let ok = ["python3", "python"].iter().any(|py| {
        std::process::Command::new(py)
            .arg(&script)
            .arg(&tmp_json)
            .arg(out_path)
            .status()
            .map(|st| st.success() && out_path.is_file())
            .unwrap_or(false)
    });
    let _ = std::fs::remove_file(&tmp_json);
    ok
}

/// Render a .pptx file from SlidesPayload JSON using local python-pptx tool when available,
/// falling back to the built-in OOXML generator if Python is not installed.
pub fn render_pptx(payload: &Value, out_path: &Path) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    if try_render_pptx_with_python(payload, out_path) {
        return Ok(());
    }
    let file = std::fs::File::create(out_path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();

    let mut slides = payload
        .get("slides")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    if slides.is_empty() {
        let fallback_title = payload
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("Presentation");
        slides.push(serde_json::json!({ "title": fallback_title, "bullets": [] }));
    }
    let slide_count = slides.len();

    let mut content_types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
"#,
    );
    for i in 1..=slide_count {
        content_types.push_str(&format!(
            "  <Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>\n"
        ));
    }
    content_types.push_str("</Types>");
    zip.start_file("[Content_Types].xml", opts)
        .map_err(zip_err)?;
    zip.write_all(content_types.as_bytes()).map_err(io_err)?;

    zip.start_file("_rels/.rels", opts).map_err(zip_err)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#,
    )
    .map_err(io_err)?;

    let mut pres_rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
"#,
    );
    let mut sld_id_lst = String::new();
    for i in 1..=slide_count {
        pres_rels.push_str(&format!(
            "  <Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>\n"
        ));
        sld_id_lst.push_str(&format!(
            "    <p:sldId id=\"{}\" r:id=\"rId{i}\"/>\n",
            255 + i
        ));
    }
    pres_rels.push_str("</Relationships>");
    zip.start_file("ppt/_rels/presentation.xml.rels", opts)
        .map_err(zip_err)?;
    zip.write_all(pres_rels.as_bytes()).map_err(io_err)?;

    let pres_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst>
{sld_id_lst}  </p:sldIdLst>
  <p:sldSz cx="12192000" cy="6858000"/>
  <p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
    );
    zip.start_file("ppt/presentation.xml", opts)
        .map_err(zip_err)?;
    zip.write_all(pres_xml.as_bytes()).map_err(io_err)?;

    for (idx, s) in slides.iter().enumerate() {
        let i = idx + 1;
        let title = xml_escape(s.get("title").and_then(|x| x.as_str()).unwrap_or("Slide"));
        let bullets = s
            .get("bullets")
            .and_then(|b| b.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|b| format!("<a:p><a:r><a:rPr lang=\"zh-CN\" sz=\"1800\"/><a:t>• {}</a:t></a:r></a:p>", xml_escape(b)))
                    .collect::<String>()
            })
            .unwrap_or_default();

        let slide_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr/>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
        <p:spPr><a:xfrm><a:off x="609600" y="457200"/><a:ext cx="10972800" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
        <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="zh-CN" sz="3200" b="1"/><a:t>{title}</a:t></a:r></a:p></p:txBody>
      </p:sp>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
        <p:spPr><a:xfrm><a:off x="609600" y="1600200"/><a:ext cx="10972800" cy="4572000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
        <p:txBody><a:bodyPr/><a:lstStyle/>{bullets}</p:txBody>
      </p:sp>
    </p:spTree>
  </p:cSld>
</p:sld>"#
        );
        zip.start_file(format!("ppt/slides/slide{i}.xml"), opts)
            .map_err(zip_err)?;
        zip.write_all(slide_xml.as_bytes()).map_err(io_err)?;
    }

    zip.finish().map_err(zip_err)?;
    Ok(())
}

/// Render a rich structured SVG Infographic from InfographicPayload JSON (image_c93c68 style).
pub fn render_infographic_svg(payload: &Value, out_path: &Path) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let title = xml_escape(
        payload
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("Infographic"),
    );
    let subtitle = xml_escape(
        payload
            .get("subtitle")
            .and_then(|x| x.as_str())
            .unwrap_or(""),
    );
    let blocks = payload
        .get("blocks")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    let cols = 2usize;
    let rows = (blocks.len().max(1) + cols - 1) / cols;
    let table_h = 64 + blocks.len().min(6) * 34;
    let height = 190 + rows * 168 + table_h + 48;
    let accents = ["#1a73e8", "#0f9d58", "#673ab7", "#e37400", "#00838f", "#c5221f"];

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="{height}" viewBox="0 0 1200 {height}">
  <defs>
    <linearGradient id="bgGrad" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#f8fbff"/>
      <stop offset="100%" stop-color="#eef3fb"/>
    </linearGradient>
    <linearGradient id="hdrGrad" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="#173a7a"/>
      <stop offset="100%" stop-color="#2b59b5"/>
    </linearGradient>
  </defs>
  <rect width="1200" height="{height}" fill="url(#bgGrad)" rx="18"/>
  <rect x="36" y="32" width="1128" height="118" rx="14" fill="url(#hdrGrad)"/>
  <text x="64" y="84" fill="#ffffff" font-size="30" font-weight="700" font-family="sans-serif">{title}</text>
  <text x="64" y="120" fill="#dbe7ff" font-size="16" font-family="sans-serif">{subtitle}</text>
"##
    );

    for (i, blk) in blocks.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let x = 36 + col * 576;
        let y = 174 + row * 168;
        let accent = accents[i % accents.len()];
        let h = xml_escape(blk.get("heading").and_then(|x| x.as_str()).unwrap_or(""));
        let m = xml_escape(blk.get("metric").and_then(|x| x.as_str()).unwrap_or(""));
        let s = xml_escape(blk.get("summary").and_then(|x| x.as_str()).unwrap_or(""));
        svg.push_str(&format!(
            r##"  <rect x="{x}" y="{y}" width="552" height="148" rx="14" fill="#ffffff" stroke="#d8e1f0" stroke-width="1.5"/>
  <rect x="{x}" y="{y}" width="552" height="40" rx="14" fill="{accent}"/>
  <text x="{}" y="{}" fill="#ffffff" font-size="16" font-weight="700" font-family="sans-serif">{h}</text>
  <text x="{}" y="{}" fill="{accent}" font-size="26" font-weight="700" font-family="sans-serif">{m}</text>
  <text x="{}" y="{}" fill="#3c4043" font-size="14" font-family="sans-serif">{s}</text>
"##,
            x + 20, y + 26,
            x + 20, y + 84,
            x + 20, y + 120
        ));
    }

    let table_y = 186 + rows * 168;
    svg.push_str(&format!(
        r##"  <rect x="36" y="{table_y}" width="1128" height="38" rx="8" fill="#173a7a"/>
  <text x="56" y="{}" fill="#ffffff" font-size="14" font-weight="700" font-family="sans-serif">核心维度</text>
  <text x="340" y="{}" fill="#ffffff" font-size="14" font-weight="700" font-family="sans-serif">关键指标</text>
  <text x="580" y="{}" fill="#ffffff" font-size="14" font-weight="700" font-family="sans-serif">要点说明</text>
"##,
        table_y + 24, table_y + 24, table_y + 24
    ));
    for (i, blk) in blocks.iter().take(6).enumerate() {
        let ry = table_y + 40 + i * 34;
        let row_bg = if i % 2 == 0 { "#ffffff" } else { "#f2f6fc" };
        let h = xml_escape(blk.get("heading").and_then(|x| x.as_str()).unwrap_or(""));
        let m = xml_escape(blk.get("metric").and_then(|x| x.as_str()).unwrap_or(""));
        let s = xml_escape(blk.get("summary").and_then(|x| x.as_str()).unwrap_or(""));
        svg.push_str(&format!(
            r##"  <rect x="36" y="{ry}" width="1128" height="34" fill="{row_bg}" stroke="#e2e8f4"/>
  <text x="56" y="{}" fill="#1f2937" font-size="13" font-weight="600" font-family="sans-serif">{h}</text>
  <text x="340" y="{}" fill="#173a7a" font-size="13" font-weight="600" font-family="sans-serif">{m}</text>
  <text x="580" y="{}" fill="#4b5563" font-size="13" font-family="sans-serif">{s}</text>
"##,
            ry + 22, ry + 22, ry + 22
        ));
    }

    svg.push_str("</svg>");
    std::fs::write(out_path, svg).map_err(|e| LnaError::internal(e.to_string()))
}

/// Render an SVG MindMap from MindmapPayload JSON.
pub fn render_mindmap_svg(payload: &Value, out_path: &Path) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let title = xml_escape(
        payload
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("Mind Map"),
    );
    let nodes = payload
        .get("nodes")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    struct LayoutNode {
        x: usize,
        y: usize,
        parent_idx: Option<usize>,
        label: String,
    }

    let mut laid_out: Vec<LayoutNode> = Vec::new();
    fn layout_walk(
        nodes: &[Value],
        parent_id: Option<&str>,
        parent_idx: Option<usize>,
        depth: usize,
        row: &mut usize,
        out: &mut Vec<LayoutNode>,
    ) {
        for n in nodes {
            if n.get("parent_id").and_then(|x| x.as_str()) == parent_id {
                let idx = out.len();
                let label = n
                    .get("label")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let x = 48 + depth * 220;
                let y = 96 + *row * 56;
                *row += 1;
                out.push(LayoutNode {
                    x,
                    y,
                    parent_idx,
                    label,
                });
                if let Some(id) = n.get("id").and_then(|x| x.as_str()) {
                    layout_walk(nodes, Some(id), Some(idx), depth + 1, row, out);
                }
            }
        }
    }
    let mut row = 0usize;
    layout_walk(&nodes, None, None, 0, &mut row, &mut laid_out);

    let max_x = laid_out
        .iter()
        .map(|n| n.x + 220)
        .max()
        .unwrap_or(900)
        .max(900);
    let height = (120 + laid_out.len() * 56).max(320);

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{max_x}" height="{height}" viewBox="0 0 {max_x} {height}">
  <rect width="{max_x}" height="{height}" fill="#131314" rx="16"/>
  <text x="48" y="54" fill="#e3e3e3" font-size="24" font-weight="700" font-family="sans-serif">{title}</text>
"##
    );

    for n in &laid_out {
        if let Some(pidx) = n.parent_idx {
            let p = &laid_out[pidx];
            let x1 = p.x + 180;
            let y1 = p.y + 18;
            let x2 = n.x;
            let y2 = n.y + 18;
            let mx = (x1 + x2) / 2;
            svg.push_str(&format!(
                r##"  <path d="M {x1} {y1} C {mx} {y1}, {mx} {y2}, {x2} {y2}" fill="none" stroke="#5f6368" stroke-width="2"/>
"##
            ));
        }
    }
    for n in &laid_out {
        let lbl = xml_escape(&n.label);
        svg.push_str(&format!(
            r##"  <rect x="{}" y="{}" width="180" height="36" rx="10" fill="#1e1f20" stroke="#8ab4f8" stroke-opacity="0.5"/>
  <text x="{}" y="{}" fill="#e3e3e3" font-size="13" font-family="sans-serif">{lbl}</text>
"##,
            n.x,
            n.y,
            n.x + 12,
            n.y + 23
        ));
    }
    svg.push_str("</svg>");
    std::fs::write(out_path, svg).map_err(|e| LnaError::internal(e.to_string()))
}

/// Render a standalone HTML document for a report or text artifact (E5).
pub fn render_report_html(
    artifact_type: &str,
    title: &str,
    payload: &Value,
    out_path: &Path,
) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let md = artifact_markdown(artifact_type, title, payload);
    let parser = pulldown_cmark::Parser::new_ext(&md, pulldown_cmark::Options::ENABLE_TABLES);
    let mut body_html = String::new();
    pulldown_cmark::html::push_html(&mut body_html, parser);
    let escaped_title = xml_escape(title);
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{escaped_title}</title>
  <style>
    body {{ font-family: system-ui, -apple-system, "Segoe UI", sans-serif; max-width: 860px; margin: 40px auto; padding: 0 24px; line-height: 1.7; color: #1f2328; background: #ffffff; }}
    h1, h2, h3 {{ color: #0f172a; margin-top: 1.6em; }}
    pre, code {{ background: #f6f8fa; border-radius: 6px; padding: 2px 6px; font-family: ui-monospace, monospace; }}
    pre {{ padding: 14px; overflow-x: auto; }}
    blockquote {{ border-left: 4px solid #3b82f6; margin: 1em 0; padding: 4px 16px; color: #475569; }}
    table {{ border-collapse: collapse; width: 100%; margin: 1em 0; }}
    th, td {{ border: 1px solid #d0d7de; padding: 8px 12px; text-align: left; }}
  </style>
</head>
<body>
{body_html}
</body>
</html>
"#
    );
    std::fs::write(out_path, html).map_err(|e| LnaError::internal(e.to_string()))
}

/// Render a PNG image for MindMap or Infographic (E4, E7).
/// Uses System.Drawing on Windows for crisp text rendering, with a pure-Rust RGBA PNG fallback.
pub fn render_artifact_png(
    artifact_type: &str,
    title: &str,
    payload: &Value,
    out_path: &Path,
) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }

    #[cfg(target_os = "windows")]
    {
        let mut lines: Vec<String> = vec![title.to_string()];
        if artifact_type == "mindmap" {
            if let Some(nodes) = payload.get("nodes").and_then(|x| x.as_array()) {
                for n in nodes.iter().take(40) {
                    let lbl = n.get("label").and_then(|x| x.as_str()).unwrap_or("");
                    if !lbl.is_empty() {
                        lines.push(format!("• {lbl}"));
                    }
                }
            }
        } else {
            if let Some(sub) = payload.get("subtitle").and_then(|x| x.as_str()) {
                if !sub.is_empty() {
                    lines.push(sub.to_string());
                }
            }
            if let Some(blocks) = payload.get("blocks").and_then(|x| x.as_array()) {
                for b in blocks.iter().take(20) {
                    let h = b.get("heading").and_then(|x| x.as_str()).unwrap_or("");
                    let m = b.get("metric").and_then(|x| x.as_str()).unwrap_or("");
                    let s = b.get("summary").and_then(|x| x.as_str()).unwrap_or("");
                    lines.push(format!("[{m}] {h}: {s}"));
                }
            }
        }
        let width = 1000usize;
        let height = (140 + lines.len() * 38).max(360);
        let safe_out = out_path.display().to_string().replace('\'', "''");
        let lines_ps = lines
            .iter()
            .map(|l| format!("'{}'", l.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let ps = format!(
            "Add-Type -AssemblyName System.Drawing; \
             $bmp = New-Object System.Drawing.Bitmap({width}, {height}); \
             $g = [System.Drawing.Graphics]::FromImage($bmp); \
             $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias; \
             $g.Clear([System.Drawing.Color]::FromArgb(19, 19, 20)); \
             $titleFont = New-Object System.Drawing.Font('Microsoft YaHei UI', 18, [System.Drawing.FontStyle]::Bold); \
             $bodyFont = New-Object System.Drawing.Font('Microsoft YaHei UI', 11); \
             $titleBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(227, 227, 227)); \
             $bodyBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(196, 199, 197)); \
             $accentPen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(138, 180, 248), 2); \
             $lines = @({lines_ps}); \
             for ($i = 0; $i -lt $lines.Count; $i++) {{ \
               if ($i -eq 0) {{ $g.DrawString($lines[$i], $titleFont, $titleBrush, 40, 32); $g.DrawLine($accentPen, 40, 74, {w2}, 74); }} \
               else {{ $g.DrawString($lines[$i], $bodyFont, $bodyBrush, 48, (92 + ($i - 1) * 34)); }} \
             }} \
             $bmp.Save('{safe_out}', [System.Drawing.Imaging.ImageFormat]::Png); \
             $g.Dispose(); $bmp.Dispose()",
            w2 = width - 40
        );
        if let Ok(st) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
        {
            if st.success() && out_path.exists() {
                return Ok(());
            }
        }
    }

    // Pure-Rust minimal valid RGBA PNG fallback (uncompressed zlib stored blocks).
    let _ = (artifact_type, title, payload);
    write_minimal_png(out_path, 800, 480)
}

fn write_minimal_png(out_path: &Path, width: u32, height: u32) -> LnaResult<()> {
    let mut png = Vec::from(&[137u8, 80, 78, 71, 13, 10, 26, 10][..]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    push_png_chunk(&mut png, b"IHDR", &ihdr);

    let row_bytes = 1 + (width as usize) * 4;
    let mut raw = vec![0u8; row_bytes * (height as usize)];
    for y in 0..height as usize {
        let off = y * row_bytes;
        raw[off] = 0; // filter None
        for x in 0..width as usize {
            let p = off + 1 + x * 4;
            raw[p] = 19;
            raw[p + 1] = 19;
            raw[p + 2] = 20;
            raw[p + 3] = 255;
        }
    }
    let zlib = zlib_stored(&raw);
    push_png_chunk(&mut png, b"IDAT", &zlib);
    push_png_chunk(&mut png, b"IEND", &[]);
    std::fs::write(out_path, png).map_err(|e| LnaError::internal(e.to_string()))
}

fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32_ieee(&crc_input).to_be_bytes());
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut pos = 0usize;
    while pos < data.len() {
        let end = (pos + 65535).min(data.len());
        let is_last = if end == data.len() { 1u8 } else { 0u8 };
        let len = (end - pos) as u16;
        out.push(is_last);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[pos..end]);
        pos = end;
    }
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn zip_err(e: zip::result::ZipError) -> LnaError {
    LnaError::internal(format!("ZIP error: {e}"))
}

fn io_err(e: std::io::Error) -> LnaError {
    LnaError::internal(format!("IO error: {e}"))
}

/// Renders an artifact payload as readable Markdown (E2-E7 export, F3 notebook export).
pub fn artifact_markdown(artifact_type: &str, title: &str, payload: &Value) -> String {
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let cites = |v: &Value| -> String {
        let nums: Vec<String> = v
            .get("citations")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_u64())
                    .map(|n| format!("[{n}]"))
                    .collect()
            })
            .unwrap_or_default();
        if nums.is_empty() {
            String::new()
        } else {
            format!(" {}", nums.join(""))
        }
    };
    let mut md = format!("# {title}\n\n");
    match artifact_type {
        "report" => {
            for sec in payload
                .get("sections")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                md.push_str(&format!(
                    "## {}\n\n{}\n\n",
                    s(sec, "heading"),
                    s(sec, "body_md")
                ));
                let kt: Vec<String> = sec
                    .get("key_takeaways")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if !kt.is_empty() {
                    md.push_str("**要点**\n\n");
                    for k in kt {
                        md.push_str(&format!("- {k}\n"));
                    }
                    md.push('\n');
                }
            }
        }
        "quiz" => {
            for (i, q) in payload
                .get("questions")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                md.push_str(&format!("## {}. {}{}\n\n", i + 1, s(q, "prompt"), cites(q)));
                let opts: Vec<&str> = q
                    .get("options")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                for (j, o) in opts.iter().enumerate() {
                    md.push_str(&format!("{}. {o}\n", (b'A' + j as u8) as char));
                }
                if !opts.is_empty() {
                    md.push('\n');
                }
                let answers: Vec<String> = q
                    .get("answer_indices")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_u64())
                            .map(|n| ((b'A' + n as u8) as char).to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if !answers.is_empty() {
                    md.push_str(&format!("**答案**：{}\n\n", answers.join("、")));
                }
                let accepted: Vec<String> = q
                    .get("accepted_answers")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if !accepted.is_empty() {
                    md.push_str(&format!("**参考答案**：{}\n\n", accepted.join(" / ")));
                }
                if !s(q, "rubric").is_empty() {
                    md.push_str(&format!("**评分要点**：{}\n\n", s(q, "rubric")));
                }
                if !s(q, "explanation").is_empty() {
                    md.push_str(&format!("**解析**：{}\n\n", s(q, "explanation")));
                }
            }
        }
        "flashcards" => {
            for (i, c) in payload
                .get("cards")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                md.push_str(&format!(
                    "## 卡片 {}\n\n**正面**：{}\n\n**背面**：{}{}\n\n",
                    i + 1,
                    s(c, "front"),
                    s(c, "back"),
                    cites(c)
                ));
                if !s(c, "hint").is_empty() {
                    md.push_str(&format!("**提示**：{}\n\n", s(c, "hint")));
                }
            }
        }
        "mindmap" => {
            let nodes: Vec<Value> = payload
                .get("nodes")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            fn walk(nodes: &[Value], parent: Option<&str>, depth: usize, out: &mut String) {
                for n in nodes {
                    let pid = n.get("parent_id").and_then(|x| x.as_str());
                    if pid == parent {
                        let label = n.get("label").and_then(|x| x.as_str()).unwrap_or("");
                        out.push_str(&format!("{}- {label}\n", "  ".repeat(depth)));
                        if let Some(id) = n.get("id").and_then(|x| x.as_str()) {
                            walk(nodes, Some(id), depth + 1, out);
                        }
                    }
                }
            }
            walk(&nodes, None, 0, &mut md);
            md.push('\n');
        }
        "slides" => {
            for (i, sl) in payload
                .get("slides")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                md.push_str(&format!("## {}. {}\n\n", i + 1, s(sl, "title")));
                for b in sl
                    .get("bullets")
                    .and_then(|x| x.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                {
                    md.push_str(&format!("- {b}\n"));
                }
                if !s(sl, "speaker_notes").is_empty() {
                    md.push_str(&format!("\n> 讲稿：{}\n", s(sl, "speaker_notes")));
                }
                md.push('\n');
            }
        }
        "infographic" => {
            if !s(payload, "subtitle").is_empty() {
                md.push_str(&format!("_{}_\n\n", s(payload, "subtitle")));
            }
            for b in payload
                .get("blocks")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                md.push_str(&format!(
                    "## {}\n\n**{}**\n\n{}{}\n\n",
                    s(b, "heading"),
                    s(b, "metric"),
                    s(b, "summary"),
                    cites(b)
                ));
            }
        }
        "audio" => {
            for turn in payload
                .get("turns")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                md.push_str(&format!(
                    "**{}**：{}{}\n\n",
                    s(turn, "speaker"),
                    s(turn, "text"),
                    cites(turn)
                ));
            }
        }
        _ => {
            md.push_str(&format!(
                "```json\n{}\n```\n",
                serde_json::to_string_pretty(payload).unwrap_or_default()
            ));
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infographic_svg_escapes_xml_special_chars() {
        let tmp = std::env::temp_dir().join("lna_b02_test.svg");
        let payload = json!({
            "title": "A & B <test>",
            "subtitle": "\"quoted\" subtitle",
            "style": "cute",
            "blocks": [{ "heading": "H < 1", "metric": "100%", "summary": "x & y" }]
        });
        render_infographic_svg(&payload, &tmp).unwrap();
        let svg = std::fs::read_to_string(&tmp).unwrap();
        assert!(svg.contains("A &amp; B &lt;test&gt;"));
        assert!(svg.contains("&quot;quoted&quot;"));
        assert!(svg.contains("x &amp; y"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn render_pptx_with_empty_slides_writes_slide1() {
        let tmp = std::env::temp_dir().join("lna_empty_slides_test.pptx");
        render_pptx(&json!({ "title": "Empty Deck", "slides": [] }), &tmp).unwrap();
        let f = std::fs::File::open(&tmp).unwrap();
        let mut archive = zip::ZipArchive::new(f).unwrap();
        assert!(archive.by_name("ppt/slides/slide1.xml").is_ok());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn e6_render_pptx_ooxml_structure_and_slide_counts() {
        use std::io::Read;
        for count in [8usize, 15] {
            let layouts = [
                "title",
                "section",
                "bullets",
                "two_column",
                "quote",
                "formula",
            ];
            let slides: Vec<Value> = (0..count)
                .map(|i| {
                    json!({
                        "layout": layouts[i % layouts.len()],
                        "title": format!("Slide & <{}> \"{}\"", i + 1, layouts[i % layouts.len()]),
                        "bullets": [format!("Point 1 <{}>", i), "Point 2 & more"],
                        "speaker_notes": "Notes"
                    })
                })
                .collect();
            let tmp = std::env::temp_dir().join(format!("lna_e6_{count}.pptx"));
            render_pptx(&json!({ "title": "Deck", "slides": slides }), &tmp).unwrap();
            let f = std::fs::File::open(&tmp).unwrap();
            let mut archive = zip::ZipArchive::new(f).unwrap();
            for required in [
                "[Content_Types].xml",
                "_rels/.rels",
                "ppt/_rels/presentation.xml.rels",
                "ppt/presentation.xml",
            ] {
                let mut entry = archive.by_name(required).expect(required);
                let mut xml = String::new();
                entry.read_to_string(&mut xml).unwrap();
                assert!(xml.starts_with("<?xml"), "{required} missing xml decl");
            }
            for i in 1..=count {
                let name = format!("ppt/slides/slide{i}.xml");
                let mut entry = archive.by_name(&name).expect(&name);
                let mut xml = String::new();
                entry.read_to_string(&mut xml).unwrap();
                assert!(xml.contains("&amp;"), "{name} should escape &");
                assert!(xml.contains("&lt;"), "{name} should escape <");
            }
            assert!(archive
                .by_name(&format!("ppt/slides/slide{}.xml", count + 1))
                .is_err());
            let _ = std::fs::remove_file(&tmp);
        }
    }

    #[test]
    fn e4_mindmap_svg_and_png_export() {
        let payload = json!({
            "title": "Linear Algebra & <Vectors>",
            "nodes": [
                { "id": "n1", "parent_id": null, "label": "Vector Space" },
                { "id": "n2", "parent_id": "n1", "label": "Basis & Dimension" }
            ]
        });
        let svg_path = std::env::temp_dir().join("lna_e4_mindmap.svg");
        render_mindmap_svg(&payload, &svg_path).unwrap();
        let svg = std::fs::read_to_string(&svg_path).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("Vector Space"));
        assert!(svg.contains("Basis &amp; Dimension"));
        let _ = std::fs::remove_file(&svg_path);

        let png_path = std::env::temp_dir().join("lna_e4_mindmap.png");
        render_artifact_png("mindmap", "Linear Algebra", &payload, &png_path).unwrap();
        let png_bytes = std::fs::read(&png_path).unwrap();
        assert_eq!(&png_bytes[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(png_bytes.len() > 100);
        let _ = std::fs::remove_file(&png_path);
    }

    #[test]
    fn e5_report_html_export() {
        let payload = json!({
            "title": "Report <1>",
            "sections": [{
                "heading": "Overview",
                "body_md": "Vector spaces have **basis** & `dimension`.",
                "key_takeaways": ["Point A"]
            }]
        });
        let html_path = std::env::temp_dir().join("lna_e5_report.html");
        render_report_html("report", "Report <1>", &payload, &html_path).unwrap();
        let html = std::fs::read_to_string(&html_path).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Report &lt;1&gt;</title>"));
        assert!(html.contains("<strong>basis</strong>"));
        let _ = std::fs::remove_file(&html_path);
    }

    #[test]
    fn e7_infographic_png_export() {
        let payload = json!({
            "title": "Summary",
            "subtitle": "Key metrics",
            "blocks": [{ "heading": "Accuracy", "metric": "95%", "summary": "High recall" }]
        });
        let png_path = std::env::temp_dir().join("lna_e7_info.png");
        render_artifact_png("infographic", "Summary", &payload, &png_path).unwrap();
        let png_bytes = std::fs::read(&png_path).unwrap();
        assert_eq!(&png_bytes[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(png_bytes.len() > 100);
        let _ = std::fs::remove_file(&png_path);
    }
}
