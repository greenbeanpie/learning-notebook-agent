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
    render_pptx_in(payload, out_path, None)
}

fn deck_uses_builds_or_figures(payload: &Value) -> bool {
    payload
        .get("slides")
        .and_then(Value::as_array)
        .is_some_and(|a| {
            a.iter().any(|s| {
                s.get("figure").is_some_and(|f| f.is_object())
                    || s.get("builds")
                        .and_then(Value::as_array)
                        .is_some_and(|b| !b.is_empty())
            })
        })
}

/// Like `render_pptx`; `asset_dir` holds `figures/<slide id>_<step>.png` written by
/// `lecture::render_figure_assets`. Decks with figures or step builds always use the built-in
/// generator, which embeds the figures and writes click-by-click animations (`<p:timing>`).
pub fn render_pptx_in(payload: &Value, out_path: &Path, asset_dir: Option<&Path>) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    if !deck_uses_builds_or_figures(payload) && try_render_pptx_with_python(payload, out_path) {
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
  <Default Extension="png" ContentType="image/png"/>
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
        let (lines, line_ids): (Vec<String>, Vec<Vec<String>>) = body_lines(s).into_iter().unzip();
        let bullets = lines
            .iter()
            .map(|t| {
                format!(
                    "<a:p><a:r><a:rPr lang=\"zh-CN\" sz=\"1600\"/><a:t>{}</a:t></a:r></a:p>",
                    xml_escape(t)
                )
            })
            .collect::<String>();

        // figure: cumulative PNG per build step, stacked so later layers cover earlier ones
        let fig_steps = crate::lecture::figure_part_steps(s);
        let mut pics: Vec<(i64, Vec<u8>)> = Vec::new();
        if let (Some(dir), true) = (asset_dir, s.get("figure").is_some_and(|f| f.is_object())) {
            let sid = crate::lecture::slide_file_id(s, idx);
            let max = fig_steps.values().copied().max().unwrap_or(0);
            for k in 0..=max {
                if k > 0 && !fig_steps.values().any(|v| *v == k) {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(dir.join("figures").join(format!("{sid}_{k}.png")))
                {
                    pics.push((k, bytes));
                }
            }
        }
        let body_cx: i64 = if pics.is_empty() {
            10_972_800
        } else {
            5_200_000
        };
        let mut pic_xml = String::new();
        let mut pic_ids: Vec<(i64, usize)> = Vec::new();
        if !pics.is_empty() {
            let meta = s.pointer("/figure/meta");
            let fw = meta
                .and_then(|m| m.get("width"))
                .and_then(Value::as_f64)
                .unwrap_or(800.0);
            let fh = meta
                .and_then(|m| m.get("height"))
                .and_then(Value::as_f64)
                .unwrap_or(600.0);
            let (bx, by, bw, bh): (i64, i64, i64, i64) = if lines.is_empty() {
                (1_524_000, 1_500_000, 9_144_000, 5_000_000)
            } else {
                (6_000_000, 1_500_000, 5_600_000, 4_900_000)
            };
            let scale = (bw as f64 / fw).min(bh as f64 / fh);
            let (cx, cy) = ((fw * scale) as i64, (fh * scale) as i64);
            let (x, y) = (bx + (bw - cx) / 2, by + (bh - cy) / 2);
            let mut rels = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n",
            );
            for (pi, (k, bytes)) in pics.iter().enumerate() {
                zip.start_file(format!("ppt/media/fig{i}_{k}.png"), opts)
                    .map_err(zip_err)?;
                zip.write_all(bytes).map_err(io_err)?;
                rels.push_str(&format!(
                    "  <Relationship Id=\"rIdFig{k}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/fig{i}_{k}.png\"/>\n"
                ));
                let shape_id = 10 + pi;
                pic_ids.push((*k, shape_id));
                pic_xml.push_str(&format!(
                    "      <p:pic><p:nvPicPr><p:cNvPr id=\"{shape_id}\" name=\"Figure {k}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed=\"rIdFig{k}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>\n"
                ));
            }
            rels.push_str("</Relationships>");
            zip.start_file(format!("ppt/slides/_rels/slide{i}.xml.rels"), opts)
                .map_err(zip_err)?;
            zip.write_all(rels.as_bytes()).map_err(io_err)?;
        }
        let timing = build_timing_xml(s, &line_ids, &pic_ids);

        let slide_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
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
        <p:spPr><a:xfrm><a:off x="609600" y="1600200"/><a:ext cx="{body_cx}" cy="4572000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
        <p:txBody><a:bodyPr/><a:lstStyle/>{bullets}</p:txBody>
      </p:sp>
{pic_xml}    </p:spTree>
  </p:cSld>{timing}
</p:sld>"#
        );
        zip.start_file(format!("ppt/slides/slide{i}.xml"), opts)
            .map_err(zip_err)?;
        zip.write_all(slide_xml.as_bytes()).map_err(io_err)?;
    }

    zip.finish().map_err(zip_err)?;
    Ok(())
}

/// Body paragraphs of a slide for the built-in pptx writer, each with the element ids it
/// shows (same scheme as `lecture::slide_element_ids`) so builds can animate paragraphs.
fn body_lines(s: &Value) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let text = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    if let Some(sub) = s
        .get("subtitle")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
    {
        out.push((sub.trim().to_string(), vec!["subtitle".into()]));
    }
    if let Some(formula) = s
        .get("formula")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
    {
        out.push((
            format!("Formula: {}", formula.trim()),
            vec!["formula".into()],
        ));
    }
    for (ci, c) in s
        .get("cards")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let (ct, cb) = (text(c, "title"), text(c, "body"));
        if !ct.is_empty() || !cb.is_empty() {
            out.push((format!("■ {ct} — {cb}"), vec![format!("card{}", ci + 1)]));
        }
    }
    for (ci, c) in s
        .get("columns")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let col = format!("col{}", ci + 1);
        let h = text(c, "heading");
        if !h.is_empty() {
            out.push((format!("【{h}】"), vec![col.clone()]));
        }
        for (ii, it) in c
            .get("items")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .enumerate()
        {
            out.push((
                format!("  • {it}"),
                vec![col.clone(), format!("{col}.i{}", ii + 1)],
            ));
        }
    }
    for (mi, m) in s
        .get("metrics")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let line = format!(
            "{} · {} {}",
            text(m, "value"),
            text(m, "label"),
            text(m, "detail")
        )
        .trim()
        .to_string();
        out.push((line, vec![format!("metric{}", mi + 1)]));
    }
    for (si, st) in s
        .get("steps")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let num = text(st, "step");
        let prefix = if num.is_empty() {
            format!("{}.", si + 1)
        } else {
            num
        };
        out.push((
            format!("{prefix} {}: {}", text(st, "title"), text(st, "detail")),
            vec![format!("step{}", si + 1)],
        ));
    }
    if let Some(tbl) = s.get("table").and_then(|v| v.as_object()) {
        if let Some(hdrs) = tbl.get("headers").and_then(|v| v.as_array()) {
            let hline = hdrs
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" | ");
            if !hline.is_empty() {
                out.push((hline, vec!["table".into()]));
            }
        }
        for (ri, r) in tbl
            .get("rows")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .take(12)
            .enumerate()
        {
            if let Some(cells) = r.as_array() {
                let line = cells
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ");
                out.push((line, vec!["table".into(), format!("table.r{}", ri + 1)]));
            }
        }
    }
    for (bi, t) in s
        .get("bullets")
        .and_then(|b| b.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .enumerate()
    {
        if !t.trim().is_empty() {
            out.push((format!("• {}", t.trim()), vec![format!("b{}", bi + 1)]));
        }
    }
    if let Some(callout) = s
        .get("callout")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
    {
        out.push((format!("★ {}", callout.trim()), vec!["callout".into()]));
    }
    out
}

/// Click-by-click entrance animations for a slide's builds: body paragraphs (shape id 3) and
/// figure layers (picture shape ids). Returns the `<p:timing>` element or an empty string.
fn build_timing_xml(s: &Value, line_ids: &[Vec<String>], pic_ids: &[(i64, usize)]) -> String {
    let Some(builds) = s
        .get("builds")
        .and_then(Value::as_array)
        .filter(|b| !b.is_empty())
    else {
        return String::new();
    };
    let mut steps: Vec<i64> = builds
        .iter()
        .filter_map(|b| b.get("step").and_then(Value::as_i64))
        .collect();
    steps.sort();
    steps.dedup();
    let mut clicks = String::new();
    let mut ctn = 3;
    let mut any_text = false;
    for st in steps {
        let mut paras: Vec<usize> = Vec::new();
        let mut appear = false;
        let mut entrance = false;
        for b in builds
            .iter()
            .filter(|b| b.get("step").and_then(Value::as_i64) == Some(st))
        {
            let eff = b.get("effect").and_then(Value::as_str).unwrap_or("fade");
            if eff == "emphasis" || eff == "dim_others" {
                continue;
            }
            entrance = true;
            appear |= eff == "appear";
            let targets: Vec<&str> = b
                .get("targets")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for (li, ids) in line_ids.iter().enumerate() {
                if ids.iter().any(|id| targets.contains(&id.as_str())) && !paras.contains(&li) {
                    paras.push(li);
                }
            }
        }
        let pic = pic_ids.iter().find(|(k, _)| *k == st).map(|(_, id)| *id);
        if !entrance || (paras.is_empty() && pic.is_none()) {
            continue;
        }
        any_text |= !paras.is_empty();
        let mut targets: Vec<String> = paras
            .iter()
            .map(|p| {
                format!(
                    r#"<p:spTgt spid="3"><p:txEl><p:pRg st="{p}" end="{p}"/></p:txEl></p:spTgt>"#
                )
            })
            .collect();
        if let Some(id) = pic {
            targets.push(format!(r#"<p:spTgt spid="{id}"/>"#));
        }
        let (click_id, group_id) = (ctn, ctn + 1);
        ctn += 2;
        let mut effects = String::new();
        for (ti, tgt) in targets.iter().enumerate() {
            let (eid, sid, aid) = (ctn, ctn + 1, ctn + 2);
            ctn += 3;
            let node = if ti == 0 { "clickEffect" } else { "withEffect" };
            let (preset, anim) = if appear {
                (1, String::new())
            } else {
                (
                    10,
                    format!(
                        r#"<p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn id="{aid}" dur="500"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr></p:animEffect>"#
                    ),
                )
            };
            effects.push_str(&format!(
                r#"<p:par><p:cTn id="{eid}" presetID="{preset}" presetClass="entr" presetSubtype="0" fill="hold" grpId="0" nodeType="{node}"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="{sid}" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set>{anim}</p:childTnLst></p:cTn></p:par>"#
            ));
        }
        clicks.push_str(&format!(
            r#"<p:par><p:cTn id="{click_id}" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="{group_id}" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par>"#
        ));
    }
    if clicks.is_empty() {
        return String::new();
    }
    let bld = if any_text {
        r#"<p:bldLst><p:bldP spid="3" grpId="0" build="p"/></p:bldLst>"#
    } else {
        ""
    };
    format!(
        r#"
  <p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{clicks}</p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst>{bld}</p:timing>"#
    )
}

/// Font stack for SVG text: a CJK-capable face first, since the renderers fall back to
/// whatever "sans-serif" maps to, which on some systems has no Chinese glyphs.
const FONT: &str = "'Microsoft YaHei','PingFang SC','Noto Sans CJK SC','Segoe UI',sans-serif";

/// Rough rendered width of `text` in em units (CJK is one em, Latin about half).
fn text_em(text: &str) -> f32 {
    text.chars()
        .map(|c| match c {
            ' ' => 0.32,
            c if (c as u32) >= 0x2E80 => 1.0,
            c if c.is_ascii_uppercase() || c.is_ascii_digit() => 0.62,
            _ => 0.55,
        })
        .sum()
}

/// Greedy wrap of `text` into lines no wider than `max_w` pixels at `font_px`.
fn wrap_lines(text: &str, font_px: f32, max_w: f32) -> Vec<String> {
    let budget = (max_w / font_px).max(1.0);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut w = 0.0f32;
    for c in text.chars() {
        let cw = text_em(c.encode_utf8(&mut [0; 4]));
        if w + cw > budget && !cur.is_empty() {
            // inside a Latin run, prefer breaking at the last space
            let split = if c == ' ' {
                None
            } else {
                cur.rfind(' ').filter(|i| *i > cur.len() / 2)
            };
            if let Some(i) = split {
                let rest = cur[i + 1..].to_string();
                cur.truncate(i);
                lines.push(std::mem::take(&mut cur));
                w = text_em(&rest);
                cur = rest;
            } else {
                lines.push(std::mem::take(&mut cur));
                w = 0.0;
            }
            if c == ' ' {
                continue;
            }
        }
        cur.push(c);
        w += cw;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Render the infographic as a self-contained SVG: header, then cards laid out in
/// columns by orientation, each sized to its wrapped text.
pub fn render_infographic_svg(payload: &Value, out_path: &Path) -> LnaResult<()> {
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p).map_err(|e| LnaError::internal(e.to_string()))?;
    }
    // the SVG cannot typeset TeX, so any `$...$` the model wrote becomes Unicode math
    let get = |v: &Value, k: &str| -> String {
        crate::texfmt::tex_to_unicode(v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim())
            .replace('$', "")
    };
    let title_raw = {
        let t = get(payload, "title");
        if t.is_empty() {
            "Infographic".to_string()
        } else {
            t
        }
    };
    let subtitle_raw = get(payload, "subtitle");
    let blocks = payload
        .get("blocks")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    // portrait: one column; landscape/square: two columns
    let cols = if get(payload, "orientation") == "portrait" {
        1usize
    } else {
        2
    };
    let card_w = if cols == 1 { 720.0f32 } else { 552.0 };
    let gap = 24.0f32;
    let margin = 36.0f32;
    let width = margin * 2.0 + cols as f32 * card_w + (cols as f32 - 1.0) * gap;
    let inner = width - margin * 2.0;
    let accents = [
        "#1a73e8", "#0f9d58", "#673ab7", "#e37400", "#00838f", "#c5221f",
    ];

    let title_lines = wrap_lines(&title_raw, 30.0, inner - 56.0);
    let sub_lines = wrap_lines(&subtitle_raw, 16.0, inner - 56.0);
    let header_h = 28.0 + title_lines.len() as f32 * 38.0 + sub_lines.len() as f32 * 24.0 + 20.0;

    struct Card {
        head: String,
        metric: Vec<String>,
        metric_px: f32,
        body: Vec<String>,
        h: f32,
    }
    let text_w = card_w - 40.0;
    let cards: Vec<Card> = blocks
        .iter()
        .map(|b| {
            let head = wrap_lines(&get(b, "heading"), 16.0, text_w)
                .into_iter()
                .next()
                .unwrap_or_default();
            let metric_raw = get(b, "metric");
            // shrink the metric until it fits on one line, else wrap at the smallest size
            let mut px = 26.0f32;
            while px > 15.0 && text_em(&metric_raw) * px > text_w {
                px -= 1.0;
            }
            let metric = if metric_raw.is_empty() {
                vec![]
            } else if text_em(&metric_raw) * px > text_w {
                wrap_lines(&metric_raw, px, text_w)
            } else {
                vec![metric_raw]
            };
            let body = wrap_lines(&get(b, "summary"), 14.0, text_w);
            let h = 40.0
                + 14.0
                + metric.len() as f32 * (px + 8.0)
                + 8.0
                + body.len() as f32 * 21.0
                + 18.0;
            Card {
                head,
                metric,
                metric_px: px,
                body,
                h,
            }
        })
        .collect();

    let rows: Vec<&[Card]> = cards.chunks(cols).collect();
    let row_h: Vec<f32> = rows
        .iter()
        .map(|r| r.iter().map(|c| c.h).fold(0.0, f32::max))
        .collect();
    let body_h: f32 = row_h.iter().sum::<f32>() + gap * row_h.len() as f32;
    let height = (32.0 + header_h + gap + body_h + 24.0).ceil() as i32;
    let width_i = width as i32;

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width_i}" height="{height}" viewBox="0 0 {width_i} {height}">
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
  <rect width="{width_i}" height="{height}" fill="url(#bgGrad)" rx="18"/>
  <rect x="{margin}" y="32" width="{inner}" height="{header_h}" rx="14" fill="url(#hdrGrad)"/>
"##
    );
    let mut ty = 32.0 + 28.0 + 26.0;
    for l in &title_lines {
        svg.push_str(&format!(
            "  <text x=\"{}\" y=\"{ty}\" fill=\"#ffffff\" font-size=\"30\" font-weight=\"700\" font-family=\"{FONT}\">{}</text>\n",
            margin + 28.0,
            xml_escape(l)
        ));
        ty += 38.0;
    }
    ty -= 8.0;
    for l in &sub_lines {
        svg.push_str(&format!(
            "  <text x=\"{}\" y=\"{ty}\" fill=\"#dbe7ff\" font-size=\"16\" font-family=\"{FONT}\">{}</text>\n",
            margin + 28.0,
            xml_escape(l)
        ));
        ty += 24.0;
    }

    let mut y = 32.0 + header_h + gap;
    for (ri, r) in rows.iter().enumerate() {
        for (ci, card) in r.iter().enumerate() {
            let x = margin + ci as f32 * (card_w + gap);
            let accent = accents[(ri * cols + ci) % accents.len()];
            let h = row_h[ri];
            svg.push_str(&format!(
                r##"  <rect x="{x}" y="{y}" width="{card_w}" height="{h}" rx="14" fill="#ffffff" stroke="#d8e1f0" stroke-width="1.5"/>
  <rect x="{x}" y="{y}" width="{card_w}" height="40" rx="14" fill="{accent}"/>
  <text x="{}" y="{}" fill="#ffffff" font-size="16" font-weight="700" font-family="{FONT}">{}</text>
"##,
                x + 20.0,
                y + 26.0,
                xml_escape(&card.head)
            ));
            let mut cy = y + 40.0 + 14.0;
            for l in &card.metric {
                cy += card.metric_px;
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{cy}\" fill=\"{accent}\" font-size=\"{}\" font-weight=\"700\" font-family=\"{FONT}\">{}</text>\n",
                    x + 20.0,
                    card.metric_px,
                    xml_escape(l)
                ));
                cy += 8.0;
            }
            cy += 8.0;
            for l in &card.body {
                cy += 21.0;
                svg.push_str(&format!(
                    "  <text x=\"{}\" y=\"{}\" fill=\"#3c4043\" font-size=\"14\" font-family=\"{FONT}\">{}</text>\n",
                    x + 20.0,
                    cy - 5.0,
                    xml_escape(l)
                ));
            }
        }
        y += row_h[ri] + gap;
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
                let label = crate::validate::strip_citation_markers(
                    n.get("label").and_then(|x| x.as_str()).unwrap_or(""),
                );
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

/// The infographic as a PNG with its real card layout (the SVG rasterized with system fonts).
/// Windows' own SVG image source draws no `<text>`, so clients show this picture instead.
pub fn render_infographic_png(payload: &Value, out_path: &Path) -> LnaResult<()> {
    let tmp = out_path.with_extension("svg.tmp");
    render_infographic_svg(payload, &tmp)?;
    let svg = std::fs::read_to_string(&tmp).map_err(|e| LnaError::internal(e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    let png = crate::figure::svg_to_png(&svg?, 1.5).map_err(LnaError::internal)?;
    std::fs::write(out_path, png).map_err(|e| LnaError::internal(e.to_string()))
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

    if artifact_type == "infographic" {
        return render_infographic_png(payload, out_path);
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
        "slides" | "lecture" => {
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
    fn infographic_svg_wraps_long_text_inside_cards() {
        let tmp = std::env::temp_dir().join("lna_wrap_test.svg");
        let long = "向量空间由全体列向量组成，并对加法与数乘封闭；".repeat(6);
        let payload = json!({
            "title": "t", "subtitle": "s", "orientation": "landscape",
            "blocks": [
                { "heading": "h1", "metric": "行空间 r · 列空间 r · 零空间 n−r · 左零空间 m−r", "summary": long },
                { "heading": "h2", "metric": "dim=r", "summary": "short" }
            ]
        });
        render_infographic_svg(&payload, &tmp).unwrap();
        let svg = std::fs::read_to_string(&tmp).unwrap();
        let body: Vec<&str> = svg
            .lines()
            .filter(|l| l.contains("font-size=\"14\""))
            .collect();
        assert!(body.len() > 3, "summary should wrap");
        for l in body {
            let text = l
                .split('>')
                .nth(1)
                .unwrap_or("")
                .split('<')
                .next()
                .unwrap_or("");
            assert!(text_em(text) * 14.0 <= 513.0, "line too wide: {text}");
        }
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
