use lna_types::{LnaError, LnaResult};
use serde_json::Value;
use std::io::Write;
use std::path::Path;

/// Render a minimal valid OOXML .pptx file from SlidesPayload JSON.
pub fn render_pptx(payload: &Value, out_path: &Path) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    let file = std::fs::File::create(out_path).map_err(|e| LnaError::internal(e.to_string()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();

    let slides = payload
        .get("slides")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let slide_count = slides.len().max(1);

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

/// Render an SVG Infographic from InfographicPayload JSON.
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
    let style = payload
        .get("style")
        .and_then(|x| x.as_str())
        .unwrap_or("auto");
    let (bg, card_bg, accent) = match style {
        "cute" => ("#1e1b2e", "#2b2640", "#f872b9"),
        "clay" => ("#1f2428", "#2d3339", "#ecdf77"),
        "sketch" => ("#18181b", "#27272a", "#3fdda0"),
        "anime" => ("#131726", "#1d243b", "#76a0ff"),
        _ => ("#131314", "#1e1f20", "#8ab4f8"),
    };

    let blocks = payload
        .get("blocks")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    let height = 180 + blocks.len().max(1) * 130;

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="{height}" viewBox="0 0 900 {height}">
  <rect width="900" height="{height}" fill="{bg}" rx="16"/>
  <text x="48" y="64" fill="#e3e3e3" font-size="28" font-weight="700" font-family="sans-serif">{title}</text>
  <text x="48" y="96" fill="#9aa0a6" font-size="15" font-family="sans-serif">{subtitle}</text>
"##
    );

    for (i, blk) in blocks.iter().enumerate() {
        let y = 130 + i * 124;
        let h = xml_escape(blk.get("heading").and_then(|x| x.as_str()).unwrap_or(""));
        let m = xml_escape(blk.get("metric").and_then(|x| x.as_str()).unwrap_or(""));
        let s = xml_escape(blk.get("summary").and_then(|x| x.as_str()).unwrap_or(""));
        svg.push_str(&format!(
            r##"  <rect x="48" y="{y}" width="804" height="106" rx="14" fill="{card_bg}" stroke="{accent}" stroke-opacity="0.35"/>
  <text x="72" y="{}" fill="{accent}" font-size="22" font-weight="700" font-family="sans-serif">{m}</text>
  <text x="260" y="{}" fill="#e3e3e3" font-size="16" font-weight="600" font-family="sans-serif">{h}</text>
  <text x="260" y="{}" fill="#c4c7c5" font-size="13" font-family="sans-serif">{s}</text>
"##,
            y + 58,
            y + 42,
            y + 72
        ));
    }
    svg.push_str("</svg>");
    std::fs::write(out_path, svg).map_err(|e| LnaError::internal(e.to_string()))
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
}
