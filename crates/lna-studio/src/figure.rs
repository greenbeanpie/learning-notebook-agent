//! Declarative figure DSL → SVG (+ optional PNG) with per-part geometry and a render report.
//!
//! The model never writes SVG. It describes *what* to draw (function plots, implicit curves,
//! 3-D surfaces, geometry/physics diagrams); this module lays it out, so we know the exact
//! position of every part (for the laser pointer and step-by-step builds) and can report
//! problems (curves off-screen, overlapping labels, cramped ranges) back to the model.
//!
//! Spec (all kinds):
//! ```json
//! { "kind": "plot|implicit|plot3d|diagram", "title": "...", "aspect": "4:3|16:9|1:1|3:4",
//!   "elements": [ { "id": "c1", "type": "...", ... } ] }
//! ```
//! See `FIGURE_DSL_GUIDE` for the full element reference given to the model.

use crate::expr::{Expr, VAR_T, VAR_X, VAR_Y};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// Reference text handed to the model in the clean-context figure call.
pub const FIGURE_DSL_GUIDE: &str = r#"FIGURE DSL (JSON). Never write SVG. Coordinates are math coordinates (y up).
Top level: {"kind": "plot"|"plot3d"|"diagram", "title"?: str, "aspect"?: "4:3"|"16:9"|"1:1"|"3:4", "elements": [...]}
Every element has a unique short "id" (letters/digits/_). Optional on any element: "label", "color" (primary|accent|red|blue|green|orange|purple|gray|black), "dashed": bool, "rotate_deg" (counter-clockwise, about "about":[x,y] or the element centre).
Expressions: plain math text, e.g. "x^2-2x+1", "2sin(x)", "sqrt(1-x^2)", "e^(-x)", "ln(x)", "lg(x)", "log(2, x)", "abs(x)", "pi". No TeX.

kind "plot" (2-D Cartesian; equal or free scaling):
  plot-level keys: "x_range": [a,b] (required), "y_range": [c,d] (optional; auto if omitted), "grid": bool, "axis_labels": ["x","y"], "equal_scale": bool
  {"type":"curve","expr":"x^2-1","domain"?:[a,b]}            y = f(x)
  {"type":"param","x":"cos(t)","y":"sin(t)","t_range":[0,6.283]}
  {"type":"implicit","expr":"x^2+y^2=1"}                      F(x,y)=0
  {"type":"area","top":"x^2","bottom"?:"0","from":0,"to":1}   shaded region between curves
  {"type":"point","at":[1,0]}   (at items may be numbers or expressions like "sqrt(2)")
  {"type":"vline","at":2} / {"type":"hline","at":0}           asymptotes/reference lines
  {"type":"segment","from":[0,0],"to":"P"}  {"type":"vector","from":[0,0],"to":[1,2]}
  {"type":"text","at":[1,2],"text":"..."}
kind "plot3d":
  keys: "x_range", "y_range", "z_range"?, "azimuth"? (deg, default -60), "elevation"? (deg, default 25), "axis_labels"?: ["x","y","z"]
  {"type":"surface","expr":"x*y"}   z = f(x,y)
  {"type":"curve3d","x":"cos(t)","y":"sin(t)","z":"t/6","t_range":[0,12.57]}
  {"type":"point3d","at":[1,1,1]}  {"type":"vector3d","from":[0,0,0],"to":[1,1,1]}
kind "diagram" (geometry / physics sketch, always equal scaling; ranges auto-fit):
  {"type":"point","at":[0,0],"label":"A"}      points can be referenced by id anywhere a coordinate is expected
  {"type":"segment","from":"A","to":"B"}  {"type":"ray","from":"A","through":"B"}  {"type":"line","through":["A","B"]}
  {"type":"vector","from":"A","to":[2,1],"label":"F₁"}   arrow (forces, velocities)
  {"type":"polygon","points":["A","B","C"],"fill"?:bool}
  {"type":"circle","center":"O","r":1}  {"type":"arc","center":"O","r":1,"start_deg":0,"end_deg":90,"arrow"?:bool}
  {"type":"angle","vertex":"B","from":"A","to":"C","label"?:"θ"}   (right angles drawn as a square automatically)
  {"type":"rect","center":[0,0],"w":2,"h":1,"fill"?:bool}     blocks / boxes; combine with rotate_deg for slopes
  {"type":"spring","from":"A","to":"B","coils"?:8}
  {"type":"ground","from":[-3,0],"to":[3,0]}                 hatched surface / wall / incline
  {"type":"axes","origin":[0,0],"x_len":4,"y_len":3,"labels"?:["x","y"]}
  {"type":"text","at":[1,1],"text":"m"}
  {"type":"group","rotate_deg":30,"about":[0,0],"translate"?:[dx,dy],"children":[...]}   rotate several parts together
Keep it minimal and exact: only what the explanation needs, correct values, short labels (≤ 12 chars)."#;

const MAX_ELEMENTS: usize = 120;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PartGeom {
    pub id: String,
    /// Normalized [x, y, w, h] in 0..1 relative to the whole figure (y downwards).
    pub bbox: [f64; 4],
    /// Natural orientation on screen in degrees (clockwise positive, screen coordinates).
    pub angle_deg: f64,
    /// What the part hangs on: one group per attachment point (an end of a line, a corner,
    /// an arrow tip) listing the other parts drawn through that point; a lone text part has
    /// one group with its nearest part. Used to keep a part from appearing before them.
    pub anchors: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct FigureOutput {
    pub width: f64,
    pub height: f64,
    pub svg: String,
    pub parts: Vec<PartGeom>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl FigureOutput {
    pub fn part_ids(&self) -> Vec<String> {
        self.parts.iter().map(|p| p.id.clone()).collect()
    }

    /// Compact geometry/report JSON stored next to the figure in the payload.
    pub fn meta_json(&self) -> Value {
        json!({
            "width": self.width,
            "height": self.height,
            "parts": self.parts.iter().map(|p| {
                let mut o = json!({
                    "id": p.id,
                    "bbox": p.bbox.iter().map(|v| (v * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
                    "angle_deg": (p.angle_deg * 10.0).round() / 10.0,
                });
                if !p.anchors.is_empty() {
                    o["anchors"] = json!(p.anchors);
                }
                o
            }).collect::<Vec<_>>(),
        })
    }

    /// Human/model-readable render report used in the review loop.
    pub fn report_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "Canvas {}x{} px. Parts (normalized bbox x,y,w,h):",
            self.width, self.height
        );
        for p in &self.parts {
            let _ = writeln!(
                s,
                "- {}: [{:.2}, {:.2}, {:.2}, {:.2}]{}",
                p.id,
                p.bbox[0],
                p.bbox[1],
                p.bbox[2],
                p.bbox[3],
                if p.angle_deg.abs() > 0.5 {
                    format!(" angle {:.0}°", p.angle_deg)
                } else {
                    String::new()
                }
            );
        }
        if self.errors.is_empty() && self.warnings.is_empty() {
            s.push_str("No problems detected.\n");
        }
        for e in &self.errors {
            let _ = writeln!(s, "ERROR: {}", e);
        }
        for w in &self.warnings {
            let _ = writeln!(s, "WARNING: {}", w);
        }
        s
    }
}

// ---------------------------------------------------------------------------------------------
// primitives

#[derive(Debug, Clone)]
enum Prim {
    Path {
        d: String,
        stroke: String,
        width: f64,
        dashed: bool,
        fill: Option<(String, f64)>,
    },
    Circle {
        c: (f64, f64),
        r: f64,
        stroke: Option<String>,
        fill: Option<String>,
        width: f64,
    },
    Text {
        at: (f64, f64),
        text: String,
        size: f64,
        color: String,
        anchor: &'static str,
        italic: bool,
    },
}

#[derive(Debug, Clone)]
struct Part {
    id: String,
    prims: Vec<Prim>,
    min: (f64, f64),
    max: (f64, f64),
    angle: f64,
    /// Parts drawn behind everything else (grid/axes/areas).
    layer: u8,
    /// Clip to the plot viewport instead of the whole canvas (curves, areas, guide lines).
    clip_plot: bool,
}

impl Part {
    fn new(id: &str, layer: u8) -> Part {
        Part {
            id: id.to_string(),
            prims: Vec::new(),
            min: (f64::INFINITY, f64::INFINITY),
            max: (f64::NEG_INFINITY, f64::NEG_INFINITY),
            angle: 0.0,
            layer,
            clip_plot: false,
        }
    }
    fn grow(&mut self, p: (f64, f64)) {
        if p.0.is_finite() && p.1.is_finite() {
            self.min.0 = self.min.0.min(p.0);
            self.min.1 = self.min.1.min(p.1);
            self.max.0 = self.max.0.max(p.0);
            self.max.1 = self.max.1.max(p.1);
        }
    }
    fn has_extent(&self) -> bool {
        self.min.0.is_finite() && self.max.0.is_finite()
    }
    fn polyline(&mut self, pts: &[(f64, f64)], stroke: &str, width: f64, dashed: bool) {
        if pts.len() < 2 {
            return;
        }
        let mut d = String::new();
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(
                d,
                "{}{:.2},{:.2}",
                if i == 0 { "M" } else { " L" },
                p.0,
                p.1
            );
            self.grow(*p);
        }
        self.prims.push(Prim::Path {
            d,
            stroke: stroke.into(),
            width,
            dashed,
            fill: None,
        });
    }
    fn polygon(
        &mut self,
        pts: &[(f64, f64)],
        stroke: &str,
        width: f64,
        fill: Option<(&str, f64)>,
        dashed: bool,
    ) {
        if pts.len() < 3 {
            return;
        }
        let mut d = String::new();
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(
                d,
                "{}{:.2},{:.2}",
                if i == 0 { "M" } else { " L" },
                p.0,
                p.1
            );
            self.grow(*p);
        }
        d.push_str(" Z");
        self.prims.push(Prim::Path {
            d,
            stroke: stroke.into(),
            width,
            dashed,
            fill: fill.map(|(c, o)| (c.to_string(), o)),
        });
    }
    fn dot(&mut self, c: (f64, f64), r: f64, color: &str, hollow: bool) {
        self.grow((c.0 - r, c.1 - r));
        self.grow((c.0 + r, c.1 + r));
        self.prims.push(Prim::Circle {
            c,
            r,
            stroke: Some(color.into()),
            fill: if hollow {
                Some("#ffffff".into())
            } else {
                Some(color.into())
            },
            width: 1.6,
        });
    }
    fn arrowhead(&mut self, tip: (f64, f64), from: (f64, f64), color: &str, size: f64) {
        let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len);
        let back = (tip.0 - ux * size, tip.1 - uy * size);
        let l = (back.0 - uy * size * 0.45, back.1 + ux * size * 0.45);
        let r = (back.0 + uy * size * 0.45, back.1 - ux * size * 0.45);
        self.polygon(&[tip, l, r], color, 1.0, Some((color, 1.0)), false);
    }
    fn text(
        &mut self,
        at: (f64, f64),
        text: &str,
        size: f64,
        color: &str,
        anchor: &'static str,
    ) -> LabelBox {
        let w = text_width(text, size);
        let x0 = match anchor {
            "middle" => at.0 - w / 2.0,
            "end" => at.0 - w,
            _ => at.0,
        };
        let bx = LabelBox {
            id: self.id.clone(),
            text: text.to_string(),
            x0,
            y0: at.1 - size * 0.8,
            x1: x0 + w,
            y1: at.1 + size * 0.25,
        };
        self.grow((bx.x0, bx.y0));
        self.grow((bx.x1, bx.y1));
        let italic = is_math_symbol(text);
        self.prims.push(Prim::Text {
            at,
            text: text.to_string(),
            size,
            color: color.into(),
            anchor,
            italic,
        });
        bx
    }
}

#[derive(Debug, Clone)]
struct LabelBox {
    id: String,
    text: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

fn text_width(s: &str, size: f64) -> f64 {
    s.chars()
        .map(|c| {
            if (c as u32) > 0x2E80 {
                size
            } else if c.is_ascii_uppercase() {
                size * 0.66
            } else {
                size * 0.56
            }
        })
        .sum()
}

fn is_math_symbol(s: &str) -> bool {
    let n = s.chars().count();
    n > 0
        && n <= 4
        && s.chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() && (c as u32) < 0x2E80)
}

fn palette(name: &str) -> &'static str {
    match name.trim().to_ascii_lowercase().as_str() {
        "accent" | "orange" => "#e8710a",
        "red" => "#d93025",
        "blue" => "#1a73e8",
        "green" => "#188038",
        "purple" => "#8430ce",
        "gray" | "grey" => "#80868b",
        "black" => "#202124",
        "teal" => "#00897b",
        _ => "#1a56db",
    }
}

// ---------------------------------------------------------------------------------------------
// attachment (which parts a part hangs on), in screen coordinates so 2-D and 3-D work alike

/// A point of one part closer than this to another part's stroke touches it.
const TOUCH_PX: f64 = 3.0;
/// A lone text part farther than this from every drawn part belongs to none.
const TEXT_NEAR_PX: f64 = 20.0;

#[derive(Default)]
struct AttachShape {
    /// Points the part is attached by: open line ends, corners of small polygons, dot centres.
    keypts: Vec<(f64, f64)>,
    segs: Vec<((f64, f64), (f64, f64))>,
    dots: Vec<((f64, f64), f64)>,
    /// Centre and half the smaller side of the text box, for parts made only of text.
    text: Option<((f64, f64), f64)>,
}

fn path_subpaths(d: &str) -> Vec<Vec<(f64, f64)>> {
    let mut subs: Vec<Vec<(f64, f64)>> = Vec::new();
    for tok in d.split_whitespace() {
        let (cmd, rest) = match tok.chars().next() {
            Some(c) if c.is_ascii_alphabetic() => (Some(c), &tok[1..]),
            _ => (None, tok),
        };
        let Some((x, y)) = rest.split_once(',') else {
            continue;
        };
        let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) else {
            continue;
        };
        if cmd == Some('M') || subs.is_empty() {
            subs.push(Vec::new());
        }
        if let Some(s) = subs.last_mut() {
            s.push((x, y));
        }
    }
    subs
}

fn shape_of(p: &Part) -> AttachShape {
    let mut sh = AttachShape::default();
    let mut text_box: Option<(f64, f64, f64, f64)> = None;
    for prim in &p.prims {
        match prim {
            Prim::Path { d, .. } => {
                let closed = d.trim_end().ends_with('Z');
                for sub in path_subpaths(d) {
                    for w in sub.windows(2) {
                        sh.segs.push((w[0], w[1]));
                    }
                    if closed && sub.len() > 2 {
                        sh.segs.push((sub[sub.len() - 1], sub[0]));
                        // small polygons (arrow heads, planes, blocks) attach by their corners;
                        // many-sided ones are areas or round shapes
                        if sub.len() <= 8 {
                            sh.keypts.extend(sub.iter().copied());
                        }
                    } else if let (Some(a), Some(b)) = (sub.first(), sub.last()) {
                        sh.keypts.push(*a);
                        sh.keypts.push(*b);
                    }
                }
            }
            Prim::Circle { c, r, .. } => {
                sh.dots.push((*c, *r));
                sh.keypts.push(*c);
            }
            Prim::Text {
                at,
                text,
                size,
                anchor,
                ..
            } => {
                let w = text_width(text, *size);
                let x0 = match *anchor {
                    "middle" => at.0 - w / 2.0,
                    "end" => at.0 - w,
                    _ => at.0,
                };
                let (y0, y1) = (at.1 - size * 0.8, at.1 + size * 0.25);
                text_box = Some(match text_box {
                    None => (x0, y0, x0 + w, y1),
                    Some(b) => (b.0.min(x0), b.1.min(y0), b.2.max(x0 + w), b.3.max(y1)),
                });
            }
        }
    }
    if sh.segs.is_empty() && sh.dots.is_empty() {
        if let Some((x0, y0, x1, y1)) = text_box {
            sh.text = Some((
                ((x0 + x1) / 2.0, (y0 + y1) / 2.0),
                ((x1 - x0).min(y1 - y0)) / 2.0,
            ));
        }
    }
    sh
}

fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-9 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

/// Distance from a point to the drawn geometry of a shape (infinite when it has none).
fn shape_dist(sh: &AttachShape, p: (f64, f64)) -> f64 {
    let s = sh
        .segs
        .iter()
        .map(|(a, b)| seg_dist(p, *a, *b))
        .fold(f64::INFINITY, f64::min);
    let d = sh
        .dots
        .iter()
        .map(|(c, r)| (((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() - r).max(0.0))
        .fold(f64::INFINITY, f64::min);
    s.min(d)
}

/// For every part, the groups of other parts it hangs on (see [`PartGeom::anchors`]).
fn part_anchors(parts: &[&Part]) -> HashMap<String, Vec<Vec<String>>> {
    let shapes: Vec<AttachShape> = parts.iter().map(|p| shape_of(p)).collect();
    let mut out = HashMap::new();
    for (i, sh) in shapes.iter().enumerate() {
        let mut groups: Vec<Vec<String>> = Vec::new();
        if let Some((c, half)) = sh.text {
            let near = shapes
                .iter()
                .enumerate()
                .filter(|(j, o)| *j != i && o.text.is_none())
                .map(|(j, o)| (j, shape_dist(o, c) - half))
                .filter(|(_, d)| *d <= TEXT_NEAR_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((j, _)) = near {
                groups.push(vec![parts[j].id.clone()]);
            }
        } else {
            for kp in &sh.keypts {
                let mut g: Vec<String> = shapes
                    .iter()
                    .enumerate()
                    .filter(|(j, o)| {
                        *j != i && parts[*j].id != parts[i].id && shape_dist(o, *kp) <= TOUCH_PX
                    })
                    .map(|(j, _)| parts[j].id.clone())
                    .collect();
                g.sort();
                g.dedup();
                if !g.is_empty() && !groups.contains(&g) {
                    groups.push(g);
                }
            }
        }
        if !groups.is_empty() {
            out.insert(parts[i].id.clone(), groups);
        }
    }
    out
}

const AUTO_COLORS: [&str; 6] = [
    "#1a56db", "#d93025", "#188038", "#e8710a", "#8430ce", "#00897b",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn s_of(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => Expr::parse(s)
            .ok()
            .filter(|e| !e.uses(VAR_X) && !e.uses(VAR_Y))
            .map(|e| e.eval_x(0.0)),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

fn range2(v: Option<&Value>) -> Option<(f64, f64)> {
    let a = v?.as_array()?;
    if a.len() != 2 {
        return None;
    }
    let (lo, hi) = (num(&a[0])?, num(&a[1])?);
    (hi > lo).then_some((lo, hi))
}

fn nice_ticks(lo: f64, hi: f64, target: usize) -> Vec<f64> {
    let span = hi - lo;
    if span.is_nan() || span <= 0.0 || !span.is_finite() {
        return vec![];
    }
    let raw = span / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi + step * 1e-9 && out.len() < 40 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

fn fmt_tick(v: f64) -> String {
    let pi = std::f64::consts::PI;
    let k = v / (pi / 2.0);
    if v != 0.0
        && (k - k.round()).abs() < 1e-9
        && k.round().abs() <= 8.0
        && (v - v.round()).abs() > 1e-6
    {
        let k = k.round() as i64;
        return match k {
            1 => "π/2".into(),
            -1 => "-π/2".into(),
            2 => "π".into(),
            -2 => "-π".into(),
            _ if k % 2 == 0 => format!("{}π", k / 2),
            _ => format!("{}π/2", k),
        };
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

// ---------------------------------------------------------------------------------------------
// coordinate mapping

#[derive(Debug, Clone, Copy)]
struct View {
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    left: f64,
    top: f64,
    w: f64,
    h: f64,
}

impl View {
    fn map(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.left + (x - self.x0) / (self.x1 - self.x0) * self.w,
            self.top + (self.y1 - y) / (self.y1 - self.y0) * self.h,
        )
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    fn scale(&self) -> f64 {
        self.w / (self.x1 - self.x0)
    }
}

fn canvas_size(spec: &Value) -> (f64, f64) {
    match s_of(spec, "aspect").as_str() {
        "16:9" => (960.0, 540.0),
        "1:1" => (640.0, 640.0),
        "3:4" => (540.0, 720.0),
        _ => (800.0, 600.0),
    }
}

// ---------------------------------------------------------------------------------------------
// public entry points

/// Renders a figure spec. `hidden` parts are omitted from the SVG (used for per-step PNGs);
/// part geometry is always reported for every part.
pub fn render_figure(spec: &Value, hidden: &HashSet<String>) -> FigureOutput {
    let (w, h) = canvas_size(spec);
    let mut ctx = Ctx {
        w,
        h,
        parts: Vec::new(),
        labels: Vec::new(),
        errors: Vec::new(),
        warnings: Vec::new(),
        ids: HashSet::new(),
        plot_clip: None,
    };
    let title = s_of(spec, "title");
    let top_pad = if title.is_empty() { 18.0 } else { 46.0 };
    let elements: Vec<Value> = spec
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if elements.is_empty() {
        ctx.errors.push("figure has no elements".into());
    }
    if elements.len() > MAX_ELEMENTS {
        ctx.errors.push(format!(
            "too many elements ({}); keep it under {}",
            elements.len(),
            MAX_ELEMENTS
        ));
    }
    let kind = s_of(spec, "kind");
    match kind.as_str() {
        "plot" | "implicit" | "function" | "" => render_plot(spec, &elements, &mut ctx, top_pad),
        "plot3d" | "3d" | "surface" => render_plot3d(spec, &elements, &mut ctx, top_pad),
        "diagram" | "geometry" | "physics" => render_diagram(&elements, &mut ctx, top_pad),
        other => ctx.errors.push(format!(
            "unknown figure kind '{}' (use plot, plot3d or diagram)",
            other
        )),
    }
    if !title.is_empty() {
        let mut p = Part::new("title", 0);
        let lb = p.text((w / 2.0, 30.0), &title, 20.0, "#202124", "middle");
        ctx.labels.push(lb);
        ctx.parts.push(p);
    }
    ctx.check_labels();
    ctx.finish(hidden)
}

/// Rasterizes an SVG produced by `render_figure` (fonts from the system).
pub fn svg_to_png(svg: &str, scale: f32) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    static FONTS: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    let db = FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            std::sync::Arc::new(db)
        })
        .clone();
    let opt = usvg::Options {
        fontdb: db,
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree
        .size()
        .to_int_size()
        .scale_by(scale)
        .ok_or("bad size")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("pixmap alloc failed")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------

struct Ctx {
    w: f64,
    h: f64,
    parts: Vec<Part>,
    labels: Vec<LabelBox>,
    errors: Vec<String>,
    warnings: Vec<String>,
    ids: HashSet<String>,
    /// Plot viewport (x, y, w, h) in px for `Part::clip_plot` parts.
    plot_clip: Option<[f64; 4]>,
}

impl Ctx {
    fn part_id(&mut self, el: &Value, idx: usize, ty: &str) -> String {
        let mut id = s_of(el, "id");
        if id.is_empty() {
            id = format!("{}{}", ty, idx + 1);
        }
        id = id
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .collect();
        if id.is_empty() || id == "title" || id == "axes" || id == "grid" {
            id = format!("{}{}", ty, idx + 1);
        }
        if !self.ids.insert(id.clone()) {
            self.errors.push(format!("duplicate element id '{}'", id));
            let mut n = 2;
            while !self.ids.insert(format!("{}_{}", id, n)) {
                n += 1;
            }
            id = format!("{}_{}", id, n);
        }
        id
    }

    fn color(&self, el: &Value, idx: usize) -> String {
        let c = s_of(el, "color");
        if c.is_empty() {
            AUTO_COLORS[idx % AUTO_COLORS.len()].to_string()
        } else if c.starts_with('#')
            && (c.len() == 7 || c.len() == 4)
            && c[1..].chars().all(|h| h.is_ascii_hexdigit())
        {
            c
        } else {
            palette(&c).to_string()
        }
    }

    fn label(
        &mut self,
        part: &mut Part,
        at: (f64, f64),
        el: &Value,
        color: &str,
        anchor: &'static str,
    ) {
        let label = crate::texfmt::tex_to_unicode(&s_of(el, "label"));
        if !label.is_empty() {
            let lb = part.text(at, &label, 17.0, color, anchor);
            self.labels.push(lb);
        }
    }

    fn check_labels(&mut self) {
        let (w, h) = (self.w, self.h);
        for l in &self.labels {
            if l.x0 < 0.0 || l.y0 < 0.0 || l.x1 > w || l.y1 > h {
                self.warnings.push(format!(
                    "label '{}' of part '{}' is clipped by the canvas edge",
                    l.text, l.id
                ));
            }
        }
        for i in 0..self.labels.len() {
            for j in (i + 1)..self.labels.len() {
                let (a, b) = (&self.labels[i], &self.labels[j]);
                let ox = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
                let oy = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
                let area = ((a.x1 - a.x0) * (a.y1 - a.y0))
                    .min((b.x1 - b.x0) * (b.y1 - b.y0))
                    .max(1.0);
                if ox * oy / area > 0.25 && !(a.id == b.id && (a.id == "axes" || a.id == "grid")) {
                    self.warnings.push(format!(
                        "labels '{}' ({}) and '{}' ({}) overlap",
                        a.text, a.id, b.text, b.id
                    ));
                }
            }
        }
    }

    fn finish(mut self, hidden: &HashSet<String>) -> FigureOutput {
        let (w, h) = (self.w, self.h);
        // content occupancy check (ignoring axes/grid/title)
        let mut cmin = (f64::INFINITY, f64::INFINITY);
        let mut cmax = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in self.parts.iter().filter(|p| p.layer > 0 && p.has_extent()) {
            cmin = (cmin.0.min(p.min.0), cmin.1.min(p.min.1));
            cmax = (cmax.0.max(p.max.0), cmax.1.max(p.max.1));
        }
        if cmin.0.is_finite() {
            let frac = ((cmax.0 - cmin.0) * (cmax.1 - cmin.1)) / (w * h);
            if frac < 0.06 {
                self.warnings.push(format!(
                    "content occupies only {:.0}% of the canvas; tighten the ranges so key features are visible",
                    frac * 100.0
                ));
            }
        }
        let mut svg = String::new();
        let _ = write!(
            svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="Segoe UI, Microsoft YaHei, PingFang SC, Noto Sans CJK SC, Noto Sans, sans-serif"><defs><clipPath id="fig-clip"><rect x="0" y="0" width="{w}" height="{h}"/></clipPath>"##
        );
        let pc = self.plot_clip.unwrap_or([0.0, 0.0, w, h]);
        let _ = write!(
            svg,
            r##"<clipPath id="fig-plot"><rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/></clipPath></defs><rect x="0" y="0" width="{w}" height="{h}" rx="14" fill="#ffffff"/>"##,
            pc[0] - 2.0,
            pc[1] - 2.0,
            pc[2] + 4.0,
            pc[3] + 4.0
        );
        let mut order: Vec<&Part> = self.parts.iter().collect();
        order.sort_by_key(|p| p.layer);
        for p in order {
            if hidden.contains(&p.id) {
                continue;
            }
            let _ = write!(
                svg,
                r#"<g id="{}" data-part="{}" clip-path="url(#{})">"#,
                esc(&p.id),
                esc(&p.id),
                if p.clip_plot { "fig-plot" } else { "fig-clip" }
            );
            for prim in &p.prims {
                match prim {
                    Prim::Path {
                        d,
                        stroke,
                        width,
                        dashed,
                        fill,
                    } => {
                        let (f, fo) = fill.clone().unwrap_or(("none".into(), 1.0));
                        let _ = write!(
                            svg,
                            r#"<path d="{}" stroke="{}" stroke-width="{:.2}" fill="{}" fill-opacity="{:.2}" stroke-linejoin="round" stroke-linecap="round"{}/>"#,
                            d,
                            stroke,
                            width,
                            f,
                            fo,
                            if *dashed {
                                r#" stroke-dasharray="7 5""#
                            } else {
                                ""
                            }
                        );
                    }
                    Prim::Circle {
                        c,
                        r,
                        stroke,
                        fill,
                        width,
                    } => {
                        let _ = write!(
                            svg,
                            r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" stroke="{}" stroke-width="{:.2}" fill="{}"/>"#,
                            c.0,
                            c.1,
                            r,
                            stroke.as_deref().unwrap_or("none"),
                            width,
                            fill.as_deref().unwrap_or("none")
                        );
                    }
                    Prim::Text {
                        at,
                        text,
                        size,
                        color,
                        anchor,
                        italic,
                    } => {
                        let _ = write!(
                            svg,
                            r#"<text x="{:.2}" y="{:.2}" font-size="{:.1}" fill="{}" text-anchor="{}"{}>{}</text>"#,
                            at.0,
                            at.1,
                            size,
                            color,
                            anchor,
                            if *italic {
                                r#" font-style="italic" font-family="Cambria Math, Times New Roman, Noto Serif, serif""#
                            } else {
                                ""
                            },
                            esc(text)
                        );
                    }
                }
            }
            svg.push_str("</g>");
        }
        svg.push_str("</svg>");
        let visible: Vec<&Part> = self
            .parts
            .iter()
            .filter(|p| p.has_extent() && !hidden.contains(&p.id))
            .collect();
        let mut anchors = part_anchors(&visible);
        let parts = self
            .parts
            .iter()
            .filter(|p| p.has_extent())
            .map(|p| {
                let x0 = (p.min.0 / w).clamp(0.0, 1.0);
                let y0 = (p.min.1 / h).clamp(0.0, 1.0);
                let x1 = (p.max.0 / w).clamp(0.0, 1.0);
                let y1 = (p.max.1 / h).clamp(0.0, 1.0);
                PartGeom {
                    id: p.id.clone(),
                    bbox: [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)],
                    angle_deg: p.angle,
                    anchors: anchors.remove(&p.id).unwrap_or_default(),
                }
            })
            .collect();
        FigureOutput {
            width: w,
            height: h,
            svg,
            parts,
            errors: self.errors,
            warnings: self.warnings,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2-D plots

fn parse_expr_field(
    ctx: &mut Ctx,
    id: &str,
    el: &Value,
    key: &str,
    allowed: &[usize],
    names: &str,
) -> Option<Expr> {
    let mut src = s_of(el, key);
    if src.is_empty() {
        ctx.errors
            .push(format!("element '{}' is missing \"{}\"", id, key));
        return None;
    }
    // explicit functions written as "y = x^2" / "f(x) = ..." / "z = xy": keep the right side
    if !(key == "expr" && allowed.contains(&VAR_Y)) || src.trim_start().starts_with("z") {
        if let Some(pos) = src.rfind('=') {
            src = src[pos + 1..].to_string();
        }
    }
    let parsed = if key == "expr" && allowed.contains(&VAR_Y) {
        Expr::parse_equation(&src)
    } else {
        Expr::parse(&src)
    };
    match parsed.and_then(|e| e.check_vars(allowed, names).map(|_| e)) {
        Ok(e) => Some(e),
        Err(err) => {
            ctx.errors.push(format!(
                "element '{}': cannot parse {} \"{}\": {}",
                id, key, src, err
            ));
            None
        }
    }
}

/// Resolves a coordinate: [x, y] (numbers or constant expressions) or a point id.
fn coord(v: Option<&Value>, points: &HashMap<String, (f64, f64)>) -> Result<(f64, f64), String> {
    match v {
        Some(Value::Array(a)) if a.len() >= 2 => match (num(&a[0]), num(&a[1])) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(format!("bad coordinate {}", Value::Array(a.clone()))),
        },
        Some(Value::String(s)) => points
            .get(s.trim())
            .copied()
            .ok_or_else(|| format!("unknown point '{}'", s)),
        Some(other) => Err(format!("bad coordinate {}", other)),
        None => Err("missing coordinate".into()),
    }
}

fn render_plot(spec: &Value, elements: &[Value], ctx: &mut Ctx, top_pad: f64) {
    let (w, h) = (ctx.w, ctx.h);
    let (x0, x1) = match range2(spec.get("x_range")) {
        Some(r) => r,
        None => {
            if spec.get("x_range").is_some() {
                ctx.errors
                    .push("x_range must be [min, max] with min < max".into());
            }
            (-5.0, 5.0)
        }
    };
    // collect explicit points first (so other elements can reference them)
    let mut points: HashMap<String, (f64, f64)> = HashMap::new();
    for el in elements {
        if s_of(el, "type") == "point" {
            if let Ok(p) = coord(el.get("at"), &points) {
                points.insert(s_of(el, "id"), p);
            }
        }
    }
    // y range: explicit or auto from samples
    let (y0, y1) = match range2(spec.get("y_range")) {
        Some(r) => r,
        None => {
            let mut ys: Vec<f64> = Vec::new();
            for el in elements {
                match s_of(el, "type").as_str() {
                    "curve" | "function" => {
                        if let Ok(e) = Expr::parse(&strip_lhs(&s_of(el, "expr"))) {
                            let (a, b) = range2(el.get("domain")).unwrap_or((x0, x1));
                            for i in 0..=200 {
                                let x = a + (b - a) * i as f64 / 200.0;
                                let y = e.eval_x(x);
                                if y.is_finite() {
                                    ys.push(y);
                                }
                            }
                        }
                    }
                    "param" => {
                        if let (Ok(ex), Ok(ey)) =
                            (Expr::parse(&s_of(el, "x")), Expr::parse(&s_of(el, "y")))
                        {
                            let (a, b) =
                                range2(el.get("t_range")).unwrap_or((0.0, std::f64::consts::TAU));
                            for i in 0..=200 {
                                let t = a + (b - a) * i as f64 / 200.0;
                                let (x, y) = (ex.eval_t(t), ey.eval_t(t));
                                if y.is_finite() && x >= x0 && x <= x1 {
                                    ys.push(y);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            ys.extend(points.values().map(|p| p.1));
            if ys.is_empty() {
                let half = (x1 - x0) * 0.375;
                (-half, half)
            } else {
                ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let lo = ys[(ys.len() as f64 * 0.04) as usize];
                let hi = ys[((ys.len() as f64 * 0.96) as usize).min(ys.len() - 1)];
                let pad = ((hi - lo) * 0.1).max(0.5);
                (lo - pad, hi + pad)
            }
        }
    };
    let (left, right, bottom) = (58.0, 26.0, 46.0);
    let mut view = View {
        x0,
        x1,
        y0,
        y1,
        left,
        top: top_pad,
        w: w - left - right,
        h: h - top_pad - bottom,
    };
    if spec
        .get("equal_scale")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        // shrink the plot area so 1 unit is equally long on both axes
        let sx = view.w / (x1 - x0);
        let sy = view.h / (y1 - y0);
        let s = sx.min(sy);
        let (nw, nh) = (s * (x1 - x0), s * (y1 - y0));
        view.left += (view.w - nw) / 2.0;
        view.top += (view.h - nh) / 2.0;
        view.w = nw;
        view.h = nh;
    }
    draw_axes_2d(ctx, spec, &view);
    ctx.plot_clip = Some([view.left, view.top, view.w, view.h]);
    let first_el_part = ctx.parts.len();

    for (idx, el) in elements.iter().enumerate() {
        let ty = s_of(el, "type");
        let id = ctx.part_id(el, idx, if ty.is_empty() { "el" } else { &ty });
        let color = ctx.color(el, idx);
        let dashed = el.get("dashed").and_then(Value::as_bool).unwrap_or(false);
        match ty.as_str() {
            "curve" | "function" => {
                let Some(e) = parse_expr_field(ctx, &id, el, "expr", &[VAR_X], "x") else {
                    continue;
                };
                let (a, b) = range2(el.get("domain")).unwrap_or((x0, x1));
                let n = 900;
                let pts: Vec<(f64, f64)> = (0..=n)
                    .map(|i| {
                        let x = a + (b - a) * i as f64 / n as f64;
                        (x, e.eval_x(x))
                    })
                    .collect();
                let mut part = Part::new(&id, 2);
                let visible = draw_sampled(&mut part, &pts, &view, &color, dashed);
                if visible == 0 {
                    ctx.warnings.push(format!(
                        "curve '{}' is entirely outside the visible range",
                        id
                    ));
                } else if visible * 10 < pts.len() {
                    ctx.warnings.push(format!(
                        "only {}% of curve '{}' is visible; adjust x_range/y_range",
                        visible * 100 / pts.len(),
                        id
                    ));
                }
                if let Some(lp) = label_point(&pts, &view) {
                    // keep labels inside the plot: right half → text runs leftwards
                    if lp.0 > view.left + view.w * 0.6 {
                        ctx.label(&mut part, (lp.0 - 10.0, lp.1 - 10.0), el, &color, "end");
                    } else {
                        ctx.label(&mut part, (lp.0 + 8.0, lp.1 - 10.0), el, &color, "start");
                    }
                }
                ctx.parts.push(part);
            }
            "param" => {
                let ex = parse_expr_field(ctx, &id, el, "x", &[VAR_T], "t");
                let ey = parse_expr_field(ctx, &id, el, "y", &[VAR_T], "t");
                let (Some(ex), Some(ey)) = (ex, ey) else {
                    continue;
                };
                let (a, b) = range2(el.get("t_range")).unwrap_or((0.0, std::f64::consts::TAU));
                let n = 900;
                let pts: Vec<(f64, f64)> = (0..=n)
                    .map(|i| {
                        let t = a + (b - a) * i as f64 / n as f64;
                        (ex.eval_t(t), ey.eval_t(t))
                    })
                    .collect();
                let mut part = Part::new(&id, 2);
                let visible = draw_sampled(&mut part, &pts, &view, &color, dashed);
                if visible == 0 {
                    ctx.warnings.push(format!(
                        "parametric curve '{}' is entirely outside the visible range",
                        id
                    ));
                }
                if let Some(lp) = label_point(&pts, &view) {
                    // keep labels inside the plot: right half → text runs leftwards
                    if lp.0 > view.left + view.w * 0.6 {
                        ctx.label(&mut part, (lp.0 - 10.0, lp.1 - 10.0), el, &color, "end");
                    } else {
                        ctx.label(&mut part, (lp.0 + 8.0, lp.1 - 10.0), el, &color, "start");
                    }
                }
                ctx.parts.push(part);
            }
            "implicit" => {
                let Some(e) = parse_expr_field(ctx, &id, el, "expr", &[VAR_X, VAR_Y], "x, y")
                else {
                    continue;
                };
                let mut part = Part::new(&id, 2);
                let segs = marching_squares(&e, &view, 180, 140);
                if segs.is_empty() {
                    ctx.warnings.push(format!(
                        "implicit curve '{}' has no visible points in the current range",
                        id
                    ));
                }
                let mut d = String::new();
                for (p, q) in &segs {
                    let _ = write!(d, "M{:.2},{:.2} L{:.2},{:.2} ", p.0, p.1, q.0, q.1);
                    part.grow(*p);
                    part.grow(*q);
                }
                if !d.is_empty() {
                    part.prims.push(Prim::Path {
                        d,
                        stroke: color.clone(),
                        width: 2.6,
                        dashed,
                        fill: None,
                    });
                }
                if let Some((p, _)) = segs
                    .iter()
                    .max_by(|a, b| (a.0 .0 - a.0 .1).partial_cmp(&(b.0 .0 - b.0 .1)).unwrap())
                {
                    ctx.label(&mut part, (p.0 + 6.0, p.1 - 8.0), el, &color, "start");
                }
                ctx.parts.push(part);
            }
            "area" => {
                let Some(top) = parse_expr_field(ctx, &id, el, "top", &[VAR_X], "x") else {
                    continue;
                };
                let bottom = if s_of(el, "bottom").is_empty() {
                    Expr::parse("0").ok()
                } else {
                    parse_expr_field(ctx, &id, el, "bottom", &[VAR_X], "x")
                };
                let Some(bottom) = bottom else { continue };
                let (a, b) = match (el.get("from").and_then(num), el.get("to").and_then(num)) {
                    (Some(a), Some(b)) if b > a => (a, b),
                    _ => {
                        ctx.errors
                            .push(format!("area '{}' needs numeric \"from\" < \"to\"", id));
                        continue;
                    }
                };
                let n = 240;
                let mut poly = Vec::new();
                for i in 0..=n {
                    let x = a + (b - a) * i as f64 / n as f64;
                    let y = top.eval_x(x);
                    if y.is_finite() {
                        poly.push(view.map(x, y.clamp(y0 - (y1 - y0), y1 + (y1 - y0))));
                    }
                }
                for i in (0..=n).rev() {
                    let x = a + (b - a) * i as f64 / n as f64;
                    let y = bottom.eval_x(x);
                    if y.is_finite() {
                        poly.push(view.map(x, y.clamp(y0 - (y1 - y0), y1 + (y1 - y0))));
                    }
                }
                let mut part = Part::new(&id, 1);
                part.polygon(&poly, "none", 0.0, Some((&color, 0.22)), false);
                clamp_part(&mut part, &view);
                let mid = view.map(
                    (a + b) / 2.0,
                    (top.eval_x((a + b) / 2.0) + bottom.eval_x((a + b) / 2.0)) / 2.0,
                );
                ctx.label(&mut part, mid, el, &color, "middle");
                ctx.parts.push(part);
            }
            "point" => {
                let p = match coord(el.get("at"), &points) {
                    Ok(p) => p,
                    Err(e) => {
                        ctx.errors.push(format!("point '{}': {}", id, e));
                        continue;
                    }
                };
                if !view.contains(p.0, p.1) {
                    ctx.warnings.push(format!(
                        "point '{}' ({}, {}) lies outside the visible range",
                        id,
                        fmt_tick(p.0),
                        fmt_tick(p.1)
                    ));
                }
                let mut part = Part::new(&id, 3);
                let sp = view.map(p.0, p.1);
                part.dot(
                    sp,
                    5.0,
                    &color,
                    el.get("hollow").and_then(Value::as_bool).unwrap_or(false),
                );
                ctx.label(&mut part, (sp.0 + 8.0, sp.1 - 9.0), el, &color, "start");
                ctx.parts.push(part);
            }
            "vline" | "hline" => {
                let Some(at) = el.get("at").and_then(num) else {
                    ctx.errors.push(format!("'{}' needs numeric \"at\"", id));
                    continue;
                };
                let mut part = Part::new(&id, 1);
                let (a, b) = if ty == "vline" {
                    (view.map(at, y0), view.map(at, y1))
                } else {
                    (view.map(x0, at), view.map(x1, at))
                };
                part.polyline(
                    &[a, b],
                    &color,
                    1.8,
                    el.get("dashed").and_then(Value::as_bool).unwrap_or(true),
                );
                part.angle = if ty == "vline" { 90.0 } else { 0.0 };
                let lp = if ty == "vline" {
                    (b.0 + 6.0, b.1 + 18.0)
                } else {
                    (b.0 - 6.0, b.1 - 8.0)
                };
                ctx.label(
                    &mut part,
                    lp,
                    el,
                    &color,
                    if ty == "vline" { "start" } else { "end" },
                );
                ctx.parts.push(part);
            }
            "segment" | "vector" | "arrow" => {
                let (a, b) = match (coord(el.get("from"), &points), coord(el.get("to"), &points)) {
                    (Ok(a), Ok(b)) => (a, b),
                    (Err(e), _) | (_, Err(e)) => {
                        ctx.errors.push(format!("'{}': {}", id, e));
                        continue;
                    }
                };
                let (sa, sb) = (view.map(a.0, a.1), view.map(b.0, b.1));
                let mut part = Part::new(&id, 3);
                part.polyline(&[sa, sb], &color, 2.4, dashed);
                if ty != "segment" {
                    part.arrowhead(sb, sa, &color, 13.0);
                }
                part.angle = (sb.1 - sa.1).atan2(sb.0 - sa.0).to_degrees();
                ctx.label(
                    &mut part,
                    ((sa.0 + sb.0) / 2.0 + 8.0, (sa.1 + sb.1) / 2.0 - 8.0),
                    el,
                    &color,
                    "start",
                );
                ctx.parts.push(part);
            }
            "text" => {
                let p = match coord(el.get("at"), &points) {
                    Ok(p) => p,
                    Err(e) => {
                        ctx.errors.push(format!("text '{}': {}", id, e));
                        continue;
                    }
                };
                let text = crate::texfmt::tex_to_unicode(&s_of(el, "text"));
                let mut part = Part::new(&id, 3);
                let lb = part.text(view.map(p.0, p.1), &text, 17.0, &color, "middle");
                ctx.labels.push(lb);
                ctx.parts.push(part);
            }
            "" => ctx
                .errors
                .push(format!("element #{} has no \"type\"", idx + 1)),
            other => ctx.errors.push(format!(
                "element '{}': unknown type '{}' for a plot",
                id, other
            )),
        }
    }
    // curves, regions and guide lines are clipped to the plot viewport
    for p in ctx.parts.iter_mut().skip(first_el_part) {
        if p.layer <= 2 {
            p.clip_plot = true;
        }
    }
}

/// "y = x^2" / "f(x) = ..." / "z = xy" → right-hand side; plain expressions unchanged.
fn strip_lhs(src: &str) -> String {
    match src.rfind('=') {
        Some(pos) => src[pos + 1..].to_string(),
        None => src.to_string(),
    }
}

fn draw_axes_2d(ctx: &mut Ctx, spec: &Value, v: &View) {
    let grid = spec.get("grid").and_then(Value::as_bool).unwrap_or(true);
    let xt = nice_ticks(v.x0, v.x1, ((v.w / 80.0) as usize).clamp(4, 12));
    let yt = nice_ticks(v.y0, v.y1, ((v.h / 60.0) as usize).clamp(4, 10));
    let mut gpart = Part::new("grid", 0);
    if grid {
        for x in &xt {
            gpart.polyline(&[v.map(*x, v.y0), v.map(*x, v.y1)], "#e8eaed", 1.0, false);
        }
        for y in &yt {
            gpart.polyline(&[v.map(v.x0, *y), v.map(v.x1, *y)], "#e8eaed", 1.0, false);
        }
    }
    ctx.parts.push(gpart);
    let ax_y = if v.y0 <= 0.0 && v.y1 >= 0.0 {
        0.0
    } else {
        v.y0
    };
    let ax_x = if v.x0 <= 0.0 && v.x1 >= 0.0 {
        0.0
    } else {
        v.x0
    };
    let mut a = Part::new("axes", 0);
    let (xa, xb) = (v.map(v.x0, ax_y), v.map(v.x1, ax_y));
    let (ya, yb) = (v.map(ax_x, v.y0), v.map(ax_x, v.y1));
    a.polyline(&[xa, (xb.0 + 10.0, xb.1)], "#3c4043", 1.6, false);
    a.arrowhead((xb.0 + 14.0, xb.1), xa, "#3c4043", 10.0);
    a.polyline(&[ya, (yb.0, yb.1 - 10.0)], "#3c4043", 1.6, false);
    a.arrowhead((yb.0, yb.1 - 14.0), ya, "#3c4043", 10.0);
    let names: Vec<String> = spec
        .get("axis_labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["x".into(), "y".into()]);
    let mut labels = Vec::new();
    if let Some(n) = names.first() {
        // above the arrow tip, clear of the last tick label
        labels.push(a.text((xb.0 + 14.0, xb.1 - 10.0), n, 17.0, "#3c4043", "end"));
    }
    if let Some(n) = names.get(1) {
        // left of the arrow tip
        labels.push(a.text((yb.0 - 10.0, yb.1 - 2.0), n, 17.0, "#3c4043", "end"));
    }
    for x in &xt {
        if *x == ax_x && ax_x == 0.0 && v.y0 <= 0.0 && v.y1 >= 0.0 {
            continue;
        }
        let p = v.map(*x, ax_y);
        a.polyline(&[(p.0, p.1 - 4.0), (p.0, p.1 + 4.0)], "#3c4043", 1.2, false);
        if (v.x1 - *x).abs() < (v.x1 - v.x0) * 0.02 {
            continue; // the arrow head sits there
        }
        a.text((p.0, p.1 + 19.0), &fmt_tick(*x), 13.0, "#5f6368", "middle");
    }
    for y in &yt {
        if *y == ax_y && ax_y == 0.0 && v.x0 <= 0.0 && v.x1 >= 0.0 {
            continue;
        }
        let p = v.map(ax_x, *y);
        a.polyline(&[(p.0 - 4.0, p.1), (p.0 + 4.0, p.1)], "#3c4043", 1.2, false);
        if (v.y1 - *y).abs() < (v.y1 - v.y0) * 0.02 {
            continue;
        }
        a.text(
            (p.0 - 8.0, p.1 + 4.5),
            &fmt_tick(*y),
            13.0,
            "#5f6368",
            "end",
        );
    }
    if v.x0 <= 0.0 && v.x1 >= 0.0 && v.y0 <= 0.0 && v.y1 >= 0.0 {
        let o = v.map(0.0, 0.0);
        a.text((o.0 - 7.0, o.1 + 18.0), "O", 14.0, "#5f6368", "end");
    }
    ctx.labels.extend(labels);
    ctx.parts.push(a);
}

/// Draws a sampled curve, breaking at non-finite values and discontinuities; returns how
/// many samples were inside the visible range.
fn draw_sampled(part: &mut Part, pts: &[(f64, f64)], v: &View, color: &str, dashed: bool) -> usize {
    let span_y = v.y1 - v.y0;
    let lim = |y: f64| y.clamp(v.y0 - span_y * 0.5, v.y1 + span_y * 0.5);
    let mut visible = 0;
    let mut run: Vec<(f64, f64)> = Vec::new();
    let mut prev: Option<(f64, f64)> = None;
    let flush = |run: &mut Vec<(f64, f64)>, part: &mut Part| {
        if run.len() >= 2 {
            let pts: Vec<(f64, f64)> = std::mem::take(run);
            part.polyline(&pts, color, 2.6, dashed);
        } else {
            run.clear();
        }
    };
    for &(x, y) in pts {
        if !x.is_finite() || !y.is_finite() {
            flush(&mut run, part);
            prev = None;
            continue;
        }
        let inside = v.contains(x, y);
        if inside {
            visible += 1;
        }
        if let Some((_, py)) = prev {
            // discontinuity: huge jump across the visible band (e.g. tan x, 1/x)
            let jump = (y - py).abs();
            let crosses = (py > v.y1 && y < v.y0) || (py < v.y0 && y > v.y1);
            if jump > span_y * 2.0 && crosses {
                flush(&mut run, part);
            }
        }
        // keep points slightly outside so curves leave the frame naturally; clip path trims them
        if y >= v.y0 - span_y * 0.5 && y <= v.y1 + span_y * 0.5 {
            run.push(v.map(x, lim(y)));
        } else {
            if !run.is_empty() {
                run.push(v.map(x, lim(y)));
            }
            flush(&mut run, part);
        }
        prev = Some((x, y));
    }
    flush(&mut run, part);
    clamp_part(part, v);
    visible
}

/// Restricts a part's bbox to the plot viewport (geometry outside is clipped in the SVG).
fn clamp_part(part: &mut Part, v: &View) {
    if !part.has_extent() {
        return;
    }
    let (l, t, r, b) = (v.left, v.top, v.left + v.w, v.top + v.h);
    part.min = (part.min.0.clamp(l, r), part.min.1.clamp(t, b));
    part.max = (part.max.0.clamp(l, r), part.max.1.clamp(t, b));
}

fn label_point(pts: &[(f64, f64)], v: &View) -> Option<(f64, f64)> {
    // rightmost visible sample well inside the band (not at the top/bottom edge), so the
    // label stays readable; fall back to any visible sample
    let span = v.y1 - v.y0;
    let inner = |x: f64, y: f64| {
        x.is_finite()
            && y.is_finite()
            && v.contains(x, y)
            && x < v.x1 - (v.x1 - v.x0) * 0.08
            && y > v.y0 + span * 0.12
            && y < v.y1 - span * 0.15
    };
    pts.iter()
        .rev()
        .find(|(x, y)| inner(*x, *y))
        .or_else(|| {
            pts.iter()
                .rev()
                .find(|(x, y)| x.is_finite() && y.is_finite() && v.contains(*x, *y))
        })
        .map(|(x, y)| v.map(*x, *y))
}

fn marching_squares(e: &Expr, v: &View, nx: usize, ny: usize) -> Vec<((f64, f64), (f64, f64))> {
    let dx = (v.x1 - v.x0) / nx as f64;
    let dy = (v.y1 - v.y0) / ny as f64;
    let mut grid = vec![0.0f64; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            grid[j * (nx + 1) + i] = e.eval_xy(v.x0 + i as f64 * dx, v.y0 + j as f64 * dy);
        }
    }
    let g = |i: usize, j: usize| grid[j * (nx + 1) + i];
    let mut segs = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let c = [g(i, j), g(i + 1, j), g(i + 1, j + 1), g(i, j + 1)];
            if c.iter().any(|x| !x.is_finite()) {
                continue;
            }
            let corners = [
                (v.x0 + i as f64 * dx, v.y0 + j as f64 * dy),
                (v.x0 + (i + 1) as f64 * dx, v.y0 + j as f64 * dy),
                (v.x0 + (i + 1) as f64 * dx, v.y0 + (j + 1) as f64 * dy),
                (v.x0 + i as f64 * dx, v.y0 + (j + 1) as f64 * dy),
            ];
            let mut cross = Vec::with_capacity(4);
            for k in 0..4 {
                let (a, b) = (c[k], c[(k + 1) % 4]);
                if (a <= 0.0) != (b <= 0.0) {
                    let t = a / (a - b);
                    // reject sign flips across poles (e.g. 1/x) where values explode
                    if a.abs() + b.abs() > 1e6 {
                        continue;
                    }
                    let (pa, pb) = (corners[k], corners[(k + 1) % 4]);
                    cross.push(v.map(pa.0 + (pb.0 - pa.0) * t, pa.1 + (pb.1 - pa.1) * t));
                }
            }
            if cross.len() == 2 {
                segs.push((cross[0], cross[1]));
            } else if cross.len() == 4 {
                segs.push((cross[0], cross[1]));
                segs.push((cross[2], cross[3]));
            }
        }
    }
    segs
}

// ---------------------------------------------------------------------------------------------
// 3-D plots

struct Proj {
    ca: f64,
    sa: f64,
    ce: f64,
    se: f64,
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Proj {
    /// Normalizes to the unit cube [-1,1]^3 and projects; returns (screen_x, screen_up, depth).
    fn project(&self, p: [f64; 3]) -> (f64, f64, f64) {
        let n: Vec<f64> = (0..3)
            .map(|i| (p[i] - self.lo[i]) / (self.hi[i] - self.lo[i]) * 2.0 - 1.0)
            .collect();
        let xr = n[0] * self.ca - n[1] * self.sa;
        let yr = n[0] * self.sa + n[1] * self.ca;
        let up = n[2] * self.ce + yr * self.se;
        let depth = yr * self.ce - n[2] * self.se;
        (xr, up, depth)
    }
}

fn colormap(t: f64) -> String {
    // viridis-like 5 stops
    let stops = [
        (68.0, 1.0, 84.0),
        (59.0, 82.0, 139.0),
        (33.0, 145.0, 140.0),
        (94.0, 201.0, 98.0),
        (253.0, 231.0, 37.0),
    ];
    let t = t.clamp(0.0, 1.0) * 4.0;
    let i = (t.floor() as usize).min(3);
    let f = t - i as f64;
    let (a, b) = (stops[i], stops[i + 1]);
    format!(
        "#{:02x}{:02x}{:02x}",
        (a.0 + (b.0 - a.0) * f) as u8,
        (a.1 + (b.1 - a.1) * f) as u8,
        (a.2 + (b.2 - a.2) * f) as u8
    )
}

fn render_plot3d(spec: &Value, elements: &[Value], ctx: &mut Ctx, top_pad: f64) {
    let (w, h) = (ctx.w, ctx.h);
    let xr = range2(spec.get("x_range")).unwrap_or((-2.0, 2.0));
    let yr = range2(spec.get("y_range")).unwrap_or(xr);
    let az = spec
        .get("azimuth")
        .and_then(num)
        .unwrap_or(-60.0)
        .to_radians();
    let el_ = spec
        .get("elevation")
        .and_then(num)
        .unwrap_or(25.0)
        .clamp(-89.0, 89.0)
        .to_radians();
    // z range: explicit or from samples
    let mut zs = Vec::new();
    for el in elements {
        match s_of(el, "type").as_str() {
            "surface" => {
                if let Ok(e) = Expr::parse(&strip_lhs(&s_of(el, "expr"))) {
                    for i in 0..=24 {
                        for j in 0..=24 {
                            let x = xr.0 + (xr.1 - xr.0) * i as f64 / 24.0;
                            let y = yr.0 + (yr.1 - yr.0) * j as f64 / 24.0;
                            let z = e.eval_xy(x, y);
                            if z.is_finite() {
                                zs.push(z);
                            }
                        }
                    }
                }
            }
            "point3d" => {
                if let Some(z) = el.get("at").and_then(|a| a.get(2)).and_then(num) {
                    zs.push(z);
                }
            }
            _ => {}
        }
    }
    let zr = range2(spec.get("z_range")).unwrap_or_else(|| {
        if zs.is_empty() {
            (-1.0, 1.0)
        } else {
            zs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let lo = zs[(zs.len() as f64 * 0.02) as usize];
            let hi = zs[((zs.len() as f64 * 0.98) as usize).min(zs.len() - 1)];
            if hi - lo < 1e-9 {
                (lo - 1.0, hi + 1.0)
            } else {
                // round outwards to nice tick values
                let t = nice_ticks(lo, hi, 4);
                let step = if t.len() >= 2 { t[1] - t[0] } else { hi - lo };
                ((lo / step).floor() * step, (hi / step).ceil() * step)
            }
        }
    });
    let pr = Proj {
        ca: az.cos(),
        sa: az.sin(),
        ce: el_.cos(),
        se: el_.sin(),
        lo: [xr.0, yr.0, zr.0],
        hi: [xr.1, yr.1, zr.1],
    };
    // fit the unit cube's projection into the canvas
    let mut bmin = (f64::INFINITY, f64::INFINITY);
    let mut bmax = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for cx in [-1.0, 1.0] {
        for cy in [-1.0, 1.0] {
            for cz in [-1.0, 1.0] {
                let n = [
                    pr.lo[0] + (cx + 1.0) / 2.0 * (pr.hi[0] - pr.lo[0]),
                    pr.lo[1] + (cy + 1.0) / 2.0 * (pr.hi[1] - pr.lo[1]),
                    pr.lo[2] + (cz + 1.0) / 2.0 * (pr.hi[2] - pr.lo[2]),
                ];
                let (sx, su, _) = pr.project(n);
                bmin = (bmin.0.min(sx), bmin.1.min(su));
                bmax = (bmax.0.max(sx), bmax.1.max(su));
            }
        }
    }
    let (aw, ah) = (w - 80.0, h - top_pad - 50.0);
    let s = (aw / (bmax.0 - bmin.0)).min(ah / (bmax.1 - bmin.1));
    let cx = 40.0 + aw / 2.0;
    let cy = top_pad + 10.0 + ah / 2.0;
    let mid = ((bmin.0 + bmax.0) / 2.0, (bmin.1 + bmax.1) / 2.0);
    let to_screen = |p: [f64; 3]| -> ((f64, f64), f64) {
        let (sx, su, d) = pr.project(p);
        ((cx + (sx - mid.0) * s, cy - (su - mid.1) * s), d)
    };

    // axes from the min corner
    // drawn on top so axis names stay readable in front of the surface
    let mut axes = Part::new("axes", 5);
    let o = [xr.0, yr.0, zr.0];
    let ends = [[xr.1, yr.0, zr.0], [xr.0, yr.1, zr.0], [xr.0, yr.0, zr.1]];
    let names: Vec<String> = spec
        .get("axis_labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["x".into(), "y".into(), "z".into()]);
    let (so, _) = to_screen(o);
    for (k, e) in ends.iter().enumerate() {
        let (se, _) = to_screen(*e);
        axes.polyline(&[so, se], "#3c4043", 1.6, false);
        axes.arrowhead(se, so, "#3c4043", 10.0);
        let dir = (se.0 - so.0, se.1 - so.1);
        let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1.0);
        if let Some(n) = names.get(k) {
            let lb = axes.text(
                (se.0 + dir.0 / len * 16.0, se.1 + dir.1 / len * 16.0 + 6.0),
                n,
                17.0,
                "#3c4043",
                "middle",
            );
            ctx.labels.push(lb);
        }
        // min/max tick values
        let (lo, hi) = [(xr.0, xr.1), (yr.0, yr.1), (zr.0, zr.1)][k];
        let mid_p = (so.0 + dir.0 * 0.92, so.1 + dir.1 * 0.92);
        axes.text(
            (
                mid_p.0 - dir.1 / len * 14.0,
                mid_p.1 + dir.0 / len * 14.0 + 4.0,
            ),
            &fmt_tick(hi),
            12.0,
            "#5f6368",
            "middle",
        );
        let _ = lo;
    }
    axes.text(
        (so.0 - 10.0, so.1 + 16.0),
        &format!(
            "({}, {}, {})",
            fmt_tick(xr.0),
            fmt_tick(yr.0),
            fmt_tick(zr.0)
        ),
        11.0,
        "#80868b",
        "end",
    );
    ctx.parts.push(axes);

    for (idx, el) in elements.iter().enumerate() {
        let ty = s_of(el, "type");
        let id = ctx.part_id(el, idx, if ty.is_empty() { "el" } else { &ty });
        let color = ctx.color(el, idx);
        match ty.as_str() {
            "surface" => {
                let Some(e) = parse_expr_field(ctx, &id, el, "expr", &[VAR_X, VAR_Y], "x, y")
                else {
                    continue;
                };
                let n = 26usize;
                #[allow(clippy::type_complexity)]
                let mut quads: Vec<(f64, Vec<(f64, f64)>, f64)> = Vec::new();
                let zat = |i: usize, j: usize| {
                    let x = xr.0 + (xr.1 - xr.0) * i as f64 / n as f64;
                    let y = yr.0 + (yr.1 - yr.0) * j as f64 / n as f64;
                    let z = e.eval_xy(x, y);
                    [x, y, z]
                };
                let mut clipped = 0usize;
                for i in 0..n {
                    for j in 0..n {
                        let ps = [zat(i, j), zat(i + 1, j), zat(i + 1, j + 1), zat(i, j + 1)];
                        if ps.iter().any(|p| !p[2].is_finite()) {
                            continue;
                        }
                        if ps.iter().any(|p| {
                            p[2] < zr.0 - (zr.1 - zr.0) * 0.05 || p[2] > zr.1 + (zr.1 - zr.0) * 0.05
                        }) {
                            clipped += 1;
                            continue;
                        }
                        let mut depth = 0.0;
                        let mut zsum = 0.0;
                        let mut pts = Vec::with_capacity(4);
                        for p in ps {
                            let (sp, d) = to_screen(p);
                            depth += d;
                            zsum += p[2];
                            pts.push(sp);
                        }
                        quads.push((depth / 4.0, pts, (zsum / 4.0 - zr.0) / (zr.1 - zr.0)));
                    }
                }
                if quads.is_empty() {
                    ctx.warnings.push(format!(
                        "surface '{}' has no visible patches; check z_range",
                        id
                    ));
                } else if clipped * 4 > n * n {
                    ctx.warnings.push(format!(
                        "{}% of surface '{}' lies outside z_range",
                        clipped * 100 / (n * n),
                        id
                    ));
                }
                quads.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                let mut part = Part::new(&id, 1);
                for (_, pts, t) in &quads {
                    let c = colormap(*t);
                    part.polygon(pts, "#1f1f1f", 0.35, Some((&c, 0.92)), false);
                }
                let top = quads
                    .iter()
                    .flat_map(|q| q.1.iter())
                    .fold((0.0, f64::INFINITY), |m, p| if p.1 < m.1 { *p } else { m });
                if top.1.is_finite() {
                    ctx.label(
                        &mut part,
                        (top.0 + 10.0, top.1 - 8.0),
                        el,
                        "#202124",
                        "start",
                    );
                }
                ctx.parts.push(part);
            }
            "curve3d" => {
                let ex = parse_expr_field(ctx, &id, el, "x", &[VAR_T], "t");
                let ey = parse_expr_field(ctx, &id, el, "y", &[VAR_T], "t");
                let ez = parse_expr_field(ctx, &id, el, "z", &[VAR_T], "t");
                let (Some(ex), Some(ey), Some(ez)) = (ex, ey, ez) else {
                    continue;
                };
                let (a, b) = range2(el.get("t_range")).unwrap_or((0.0, std::f64::consts::TAU));
                let pts: Vec<(f64, f64)> = (0..=600)
                    .map(|i| {
                        let t = a + (b - a) * i as f64 / 600.0;
                        to_screen([ex.eval_t(t), ey.eval_t(t), ez.eval_t(t)]).0
                    })
                    .filter(|p| p.0.is_finite() && p.1.is_finite())
                    .collect();
                let mut part = Part::new(&id, 2);
                part.polyline(
                    &pts,
                    &color,
                    2.6,
                    el.get("dashed").and_then(Value::as_bool).unwrap_or(false),
                );
                if let Some(lp) = pts.last() {
                    ctx.label(&mut part, (lp.0 + 8.0, lp.1 - 6.0), el, &color, "start");
                }
                ctx.parts.push(part);
            }
            "point3d" => {
                let p = el
                    .get("at")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 3)
                    .and_then(|a| Some([num(&a[0])?, num(&a[1])?, num(&a[2])?]));
                let Some(p) = p else {
                    ctx.errors
                        .push(format!("point3d '{}' needs \"at\": [x, y, z]", id));
                    continue;
                };
                let mut part = Part::new(&id, 3);
                let (sp, _) = to_screen(p);
                part.dot(sp, 5.0, &color, false);
                ctx.label(&mut part, (sp.0 + 8.0, sp.1 - 8.0), el, &color, "start");
                ctx.parts.push(part);
            }
            "vector3d" | "segment3d" => {
                let get = |k: &str| {
                    el.get(k)
                        .and_then(Value::as_array)
                        .filter(|a| a.len() == 3)
                        .and_then(|a| Some([num(&a[0])?, num(&a[1])?, num(&a[2])?]))
                };
                let (Some(a), Some(b)) = (get("from"), get("to")) else {
                    ctx.errors
                        .push(format!("'{}' needs \"from\" and \"to\" as [x, y, z]", id));
                    continue;
                };
                let (sa, sb) = (to_screen(a).0, to_screen(b).0);
                let mut part = Part::new(&id, 3);
                part.polyline(&[sa, sb], &color, 2.4, false);
                if ty == "vector3d" {
                    part.arrowhead(sb, sa, &color, 12.0);
                }
                part.angle = (sb.1 - sa.1).atan2(sb.0 - sa.0).to_degrees();
                ctx.label(&mut part, (sb.0 + 8.0, sb.1 - 6.0), el, &color, "start");
                ctx.parts.push(part);
            }
            "" => ctx
                .errors
                .push(format!("element #{} has no \"type\"", idx + 1)),
            other => ctx.errors.push(format!(
                "element '{}': unknown type '{}' for plot3d",
                id, other
            )),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// diagrams

#[derive(Debug, Clone, Copy)]
struct Xf {
    // affine 2x3: x' = a*x + b*y + c ; y' = d*x + e*y + f
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Xf {
    fn id() -> Xf {
        Xf {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 1.0,
            f: 0.0,
        }
    }
    fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        (
            self.a * p.0 + self.b * p.1 + self.c,
            self.d * p.0 + self.e * p.1 + self.f,
        )
    }
    /// self ∘ other (apply other first)
    fn then(&self, other: &Xf) -> Xf {
        // returns transform t = self(other(p))
        Xf {
            a: self.a * other.a + self.b * other.d,
            b: self.a * other.b + self.b * other.e,
            c: self.a * other.c + self.b * other.f + self.c,
            d: self.d * other.a + self.e * other.d,
            e: self.d * other.b + self.e * other.e,
            f: self.d * other.c + self.e * other.f + self.f,
        }
    }
    fn rotation(deg: f64, about: (f64, f64)) -> Xf {
        let (s, c) = deg.to_radians().sin_cos();
        Xf {
            a: c,
            b: -s,
            c: about.0 - c * about.0 + s * about.1,
            d: s,
            e: c,
            f: about.1 - s * about.0 - c * about.1,
        }
    }
    fn translation(dx: f64, dy: f64) -> Xf {
        Xf {
            a: 1.0,
            b: 0.0,
            c: dx,
            d: 0.0,
            e: 1.0,
            f: dy,
        }
    }
    fn angle_deg(&self) -> f64 {
        self.d.atan2(self.a).to_degrees()
    }
}

/// A diagram element after transforms, in logical coordinates.
#[derive(Debug, Clone)]
enum Shape {
    Point {
        p: (f64, f64),
        hollow: bool,
    },
    Poly {
        pts: Vec<(f64, f64)>,
        closed: bool,
        arrow: bool,
        fill: bool,
    },
    Circle {
        c: (f64, f64),
        r: f64,
        fill: bool,
    },
    Arc {
        c: (f64, f64),
        r: f64,
        a0: f64,
        a1: f64,
        arrow: bool,
    },
    Angle {
        v: (f64, f64),
        a: (f64, f64),
        b: (f64, f64),
    },
    Ray {
        from: (f64, f64),
        dir: (f64, f64),
        both: bool,
    },
    Ground {
        a: (f64, f64),
        b: (f64, f64),
    },
    Text {
        at: (f64, f64),
        text: String,
    },
    Axes {
        o: (f64, f64),
        xl: f64,
        yl: f64,
        names: Vec<String>,
    },
}

struct DItem {
    id: String,
    el: Value,
    color: String,
    shape: Shape,
    angle: f64,
}

fn render_diagram(elements: &[Value], ctx: &mut Ctx, top_pad: f64) {
    // pass 1: flatten groups, compute transforms, collect points
    let mut flat: Vec<(Value, Xf)> = Vec::new();
    fn flatten(
        els: &[Value],
        xf: Xf,
        out: &mut Vec<(Value, Xf)>,
        depth: usize,
        errors: &mut Vec<String>,
    ) {
        for el in els {
            if s_of(el, "type") == "group" {
                if depth > 6 {
                    errors.push("groups nested too deeply".into());
                    continue;
                }
                let mut g = xf;
                if let Some(t) = el
                    .get("translate")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 2)
                {
                    if let (Some(dx), Some(dy)) = (num(&t[0]), num(&t[1])) {
                        g = g.then(&Xf::translation(dx, dy));
                    }
                }
                if let Some(deg) = el.get("rotate_deg").and_then(num) {
                    let about = el
                        .get("about")
                        .and_then(Value::as_array)
                        .filter(|a| a.len() == 2)
                        .and_then(|a| Some((num(&a[0])?, num(&a[1])?)))
                        .unwrap_or((0.0, 0.0));
                    g = g.then(&Xf::rotation(deg, about));
                }
                let children = el
                    .get("children")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                // the group itself is an addressable part when it has an id
                if !s_of(el, "id").is_empty() {
                    out.push((json!({"type": "__group", "id": s_of(el, "id"), "members": children.iter().map(|c| s_of(c, "id")).collect::<Vec<_>>()}), g));
                }
                flatten(&children, g, out, depth + 1, errors);
            } else {
                out.push((el.clone(), xf));
            }
        }
    }
    flatten(elements, Xf::id(), &mut flat, 0, &mut ctx.errors);

    // points (raw coordinates may reference earlier points; resolve iteratively)
    let mut points: HashMap<String, (f64, f64)> = HashMap::new();
    for _round in 0..3 {
        for (el, xf) in &flat {
            if s_of(el, "type") == "point" {
                let id = s_of(el, "id");
                if points.contains_key(&id) {
                    continue;
                }
                if let Ok(p) = coord_ref(el.get("at"), &points) {
                    let p = apply_own_rotation(el, xf, &[p]).0[0];
                    points.insert(id, p);
                }
            }
        }
    }

    // pass 2: shapes
    let mut items: Vec<DItem> = Vec::new();
    let mut groups: Vec<(String, Vec<String>, f64)> = Vec::new();
    for (idx, (el, xf)) in flat.iter().enumerate() {
        let ty = s_of(el, "type");
        if ty == "__group" {
            let members = el
                .get("members")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let gid = ctx.part_id(el, idx, "group");
            groups.push((gid, members, xf.angle_deg()));
            continue;
        }
        let id = if ty == "point" && !s_of(el, "id").is_empty() {
            // points keep their id exactly (they are referenced elsewhere)
            ctx.part_id(el, idx, "point")
        } else {
            ctx.part_id(el, idx, if ty.is_empty() { "el" } else { &ty })
        };
        let color = if s_of(el, "color").is_empty()
            && matches!(
                ty.as_str(),
                "segment"
                    | "polygon"
                    | "circle"
                    | "arc"
                    | "angle"
                    | "rect"
                    | "block"
                    | "ground"
                    | "spring"
                    | "line"
                    | "ray"
                    | "axes"
                    | "text"
                    | "point"
            ) {
            "#202124".to_string()
        } else {
            ctx.color(el, idx)
        };
        let res: Result<(Shape, f64), String> = (|| {
            let c = |k: &str| coord_ref(el.get(k), &points);
            let own = |pts: Vec<((f64, f64), bool)>| apply_own_rotation(el, xf, &pts);
            Ok(match ty.as_str() {
                "point" => {
                    let p = *points
                        .get(&s_of(el, "id"))
                        .ok_or_else(|| format!("point '{}' has a bad \"at\"", id))?;
                    (
                        Shape::Point {
                            p,
                            hollow: el.get("hollow").and_then(Value::as_bool).unwrap_or(false),
                        },
                        0.0,
                    )
                }
                "segment" | "vector" | "arrow" | "force" => {
                    let (p, a) = own(vec![c("from")?, c("to")?]);
                    let ang = seg_angle(p[0], p[1]);
                    let _ = a;
                    (
                        Shape::Poly {
                            pts: p,
                            closed: false,
                            arrow: ty != "segment",
                            fill: false,
                        },
                        ang,
                    )
                }
                "ray" => {
                    let (p, _) = own(vec![c("from")?, c("through")?]);
                    let ang = seg_angle(p[0], p[1]);
                    (
                        Shape::Ray {
                            from: p[0],
                            dir: (p[1].0 - p[0].0, p[1].1 - p[0].1),
                            both: false,
                        },
                        ang,
                    )
                }
                "line" => {
                    let thr = el
                        .get("through")
                        .and_then(Value::as_array)
                        .filter(|a| a.len() == 2)
                        .ok_or("line needs \"through\": [P, Q]")?;
                    let (p, _) = own(vec![
                        coord_ref(Some(&thr[0]), &points)?,
                        coord_ref(Some(&thr[1]), &points)?,
                    ]);
                    let ang = seg_angle(p[0], p[1]);
                    (
                        Shape::Ray {
                            from: p[0],
                            dir: (p[1].0 - p[0].0, p[1].1 - p[0].1),
                            both: true,
                        },
                        ang,
                    )
                }
                "polygon" | "triangle" => {
                    let arr = el
                        .get("points")
                        .and_then(Value::as_array)
                        .ok_or("polygon needs \"points\"")?;
                    let raw: Result<Vec<_>, _> =
                        arr.iter().map(|v| coord_ref(Some(v), &points)).collect();
                    let raw = raw?;
                    if raw.len() < 3 {
                        return Err("polygon needs at least 3 points".into());
                    }
                    let (p, _) = own(raw);
                    (
                        Shape::Poly {
                            pts: p,
                            closed: true,
                            arrow: false,
                            fill: el.get("fill").and_then(Value::as_bool).unwrap_or(false),
                        },
                        0.0,
                    )
                }
                "rect" | "block" | "box" => {
                    let (ctr, _) = c("center").or_else(|_| c("at"))?;
                    let (w, h) = (
                        el.get("w").and_then(num).ok_or("rect needs \"w\"")?,
                        el.get("h").and_then(num).ok_or("rect needs \"h\"")?,
                    );
                    if w <= 0.0 || h <= 0.0 {
                        return Err("rect w/h must be positive".into());
                    }
                    // corners are literal geometry even when the centre is a named point
                    let raw = vec![
                        ((ctr.0 - w / 2.0, ctr.1 - h / 2.0), false),
                        ((ctr.0 + w / 2.0, ctr.1 - h / 2.0), false),
                        ((ctr.0 + w / 2.0, ctr.1 + h / 2.0), false),
                        ((ctr.0 - w / 2.0, ctr.1 + h / 2.0), false),
                    ];
                    let (p, ang) = own(raw);
                    (
                        Shape::Poly {
                            pts: p,
                            closed: true,
                            arrow: false,
                            fill: el
                                .get("fill")
                                .and_then(Value::as_bool)
                                .unwrap_or(ty == "block"),
                        },
                        -ang,
                    )
                }
                "circle" => {
                    let r = el
                        .get("r")
                        .and_then(num)
                        .filter(|r| *r > 0.0)
                        .ok_or("circle needs positive \"r\"")?;
                    let (p, _) = own(vec![c("center")?]);
                    (
                        Shape::Circle {
                            c: p[0],
                            r,
                            fill: el.get("fill").and_then(Value::as_bool).unwrap_or(false),
                        },
                        0.0,
                    )
                }
                "arc" => {
                    let r = el
                        .get("r")
                        .and_then(num)
                        .filter(|r| *r > 0.0)
                        .ok_or("arc needs positive \"r\"")?;
                    let (p, ang) = own(vec![c("center")?]);
                    let a0 = el.get("start_deg").and_then(num).unwrap_or(0.0) + ang;
                    let a1 = el.get("end_deg").and_then(num).unwrap_or(90.0) + ang;
                    (
                        Shape::Arc {
                            c: p[0],
                            r,
                            a0,
                            a1,
                            arrow: el.get("arrow").and_then(Value::as_bool).unwrap_or(false),
                        },
                        0.0,
                    )
                }
                "angle" => {
                    let (p, _) = own(vec![c("vertex")?, c("from")?, c("to")?]);
                    (
                        Shape::Angle {
                            v: p[0],
                            a: p[1],
                            b: p[2],
                        },
                        0.0,
                    )
                }
                "spring" => {
                    let (p, _) = own(vec![c("from")?, c("to")?]);
                    let coils = el
                        .get("coils")
                        .and_then(num)
                        .unwrap_or(8.0)
                        .clamp(2.0, 30.0) as usize;
                    let ang = seg_angle(p[0], p[1]);
                    (
                        Shape::Poly {
                            pts: spring_points(p[0], p[1], coils),
                            closed: false,
                            arrow: false,
                            fill: false,
                        },
                        ang,
                    )
                }
                "ground" | "wall" | "surface" | "incline" => {
                    let (p, _) = own(vec![c("from")?, c("to")?]);
                    let ang = seg_angle(p[0], p[1]);
                    (Shape::Ground { a: p[0], b: p[1] }, ang)
                }
                "axes" => {
                    let o = c("origin").map(|(p, _)| p).unwrap_or((0.0, 0.0));
                    let xl = el.get("x_len").and_then(num).unwrap_or(4.0);
                    let yl = el.get("y_len").and_then(num).unwrap_or(3.0);
                    let names = el
                        .get("labels")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_else(|| vec!["x".into(), "y".into()]);
                    (Shape::Axes { o, xl, yl, names }, 0.0)
                }
                "text" | "label" => {
                    let (p, _) = own(vec![c("at")?]);
                    (
                        Shape::Text {
                            at: p[0],
                            text: crate::texfmt::tex_to_unicode(&s_of(el, "text")),
                        },
                        0.0,
                    )
                }
                "" => return Err("element has no \"type\"".into()),
                other => return Err(format!("unknown type '{}' for a diagram", other)),
            })
        })();
        match res {
            Ok((shape, angle)) => items.push(DItem {
                id,
                el: el.clone(),
                color,
                shape,
                angle,
            }),
            Err(e) => ctx.errors.push(format!("element '{}': {}", id, e)),
        }
    }
    if items.is_empty() {
        return;
    }

    // bounds (rays/lines don't define bounds)
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut grow = |p: (f64, f64)| {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    };
    for it in &items {
        match &it.shape {
            Shape::Point { p, .. } => grow(*p),
            Shape::Poly { pts, .. } => pts.iter().for_each(|p| grow(*p)),
            Shape::Circle { c, r, .. } | Shape::Arc { c, r, .. } => {
                grow((c.0 - r, c.1 - r));
                grow((c.0 + r, c.1 + r));
            }
            Shape::Angle { v, .. } => grow(*v),
            Shape::Ray { from, .. } => grow(*from),
            Shape::Ground { a, b } => {
                grow(*a);
                grow(*b);
            }
            Shape::Text { at, .. } => grow(*at),
            Shape::Axes { o, xl, yl, .. } => {
                grow(*o);
                grow((o.0 + xl, o.1 + yl));
            }
        }
    }
    if !lo.0.is_finite() {
        return;
    }
    let span = ((hi.0 - lo.0).max(hi.1 - lo.1)).max(1e-6);
    let pad = span * 0.12;
    let (mut x0, mut x1, mut y0, mut y1) = (lo.0 - pad, hi.0 + pad, lo.1 - pad, hi.1 + pad);
    if x1 - x0 < 1e-6 {
        x0 -= 1.0;
        x1 += 1.0;
    }
    if y1 - y0 < 1e-6 {
        y0 -= 1.0;
        y1 += 1.0;
    }
    // equal scaling: expand the shorter side to the canvas aspect
    let (aw, ah) = (ctx.w - 40.0, ctx.h - top_pad - 24.0);
    let s = (aw / (x1 - x0)).min(ah / (y1 - y0));
    let (cw, ch) = (aw / s, ah / s);
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let view = View {
        x0: mx - cw / 2.0,
        x1: mx + cw / 2.0,
        y0: my - ch / 2.0,
        y1: my + ch / 2.0,
        left: 20.0,
        top: top_pad,
        w: aw,
        h: ah,
    };
    let px = view.scale();
    let far = span * 4.0;

    for it in &items {
        let mut part = Part::new(
            &it.id,
            match it.shape {
                Shape::Ground { .. } | Shape::Axes { .. } => 1,
                Shape::Point { .. } | Shape::Text { .. } => 4,
                _ => 2,
            },
        );
        part.angle = it.angle;
        let dashed = it
            .el
            .get("dashed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let col = it.color.as_str();
        match &it.shape {
            Shape::Point { p, hollow } => {
                let sp = view.map(p.0, p.1);
                part.dot(sp, 4.5, col, *hollow);
                let off = it
                    .el
                    .get("label_pos")
                    .and_then(Value::as_str)
                    .unwrap_or("ne");
                let (dx, dy, anchor) = match off {
                    "n" => (0.0, -10.0, "middle"),
                    "s" => (0.0, 22.0, "middle"),
                    "e" => (10.0, 6.0, "start"),
                    "w" => (-10.0, 6.0, "end"),
                    "sw" => (-8.0, 20.0, "end"),
                    "se" => (8.0, 20.0, "start"),
                    "nw" => (-8.0, -8.0, "end"),
                    _ => (8.0, -8.0, "start"),
                };
                let mut el = it.el.clone();
                if it.el.get("label").is_none() && s_of(&el, "id").chars().count() <= 2 {
                    // short point ids (A, B, O, P1) double as labels unless a label key is given
                    el["label"] = Value::String(s_of(&el, "id"));
                }
                ctx.label(&mut part, (sp.0 + dx, sp.1 + dy), &el, "#202124", anchor);
            }
            Shape::Poly {
                pts,
                closed,
                arrow,
                fill,
            } => {
                let sp: Vec<(f64, f64)> = pts.iter().map(|p| view.map(p.0, p.1)).collect();
                if *closed {
                    part.polygon(
                        &sp,
                        col,
                        2.2,
                        if *fill { Some((col, 0.18)) } else { None },
                        dashed,
                    );
                } else {
                    let thick = if *arrow { 2.8 } else { 2.2 };
                    if *arrow && sp.len() == 2 {
                        // stop the shaft short of the tip so the head stays crisp
                        let (a, b) = (sp[0], sp[1]);
                        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt().max(1e-6);
                        let shaft_end =
                            (b.0 - (b.0 - a.0) / len * 8.0, b.1 - (b.1 - a.1) / len * 8.0);
                        part.polyline(&[a, shaft_end], col, thick, dashed);
                        part.arrowhead(b, a, col, 15.0);
                    } else {
                        part.polyline(&sp, col, thick, dashed);
                    }
                }
                let (lx, ly) = if *arrow && sp.len() == 2 {
                    (sp[1].0 + 8.0, sp[1].1 - 8.0)
                } else {
                    let n = sp.len() as f64;
                    let c = sp
                        .iter()
                        .fold((0.0, 0.0), |m, p| (m.0 + p.0 / n, m.1 + p.1 / n));
                    (c.0 + 8.0, c.1 - 8.0)
                };
                ctx.label(&mut part, (lx, ly), &it.el, col, "start");
            }
            Shape::Circle { c, r, fill } => {
                let sc = view.map(c.0, c.1);
                let rr = r * px;
                part.grow((sc.0 - rr, sc.1 - rr));
                part.grow((sc.0 + rr, sc.1 + rr));
                part.prims.push(Prim::Circle {
                    c: sc,
                    r: rr,
                    stroke: Some(col.into()),
                    fill: if *fill { Some("#e8f0fe".into()) } else { None },
                    width: 2.2,
                });
                ctx.label(
                    &mut part,
                    (sc.0 + rr * 0.72 + 6.0, sc.1 - rr * 0.72 - 6.0),
                    &it.el,
                    col,
                    "start",
                );
            }
            Shape::Arc {
                c,
                r,
                a0,
                a1,
                arrow,
            } => {
                let n = 64;
                let pts: Vec<(f64, f64)> = (0..=n)
                    .map(|i| {
                        let a = (a0 + (a1 - a0) * i as f64 / n as f64).to_radians();
                        view.map(c.0 + r * a.cos(), c.1 + r * a.sin())
                    })
                    .collect();
                part.polyline(&pts, col, 2.2, dashed);
                if *arrow && pts.len() > 3 {
                    part.arrowhead(pts[n], pts[n - 3], col, 12.0);
                }
                let mid = pts[n / 2];
                ctx.label(&mut part, (mid.0 + 8.0, mid.1 - 6.0), &it.el, col, "start");
            }
            Shape::Angle { v, a, b } => {
                let sv = view.map(v.0, v.1);
                let (sa, sb) = (view.map(a.0, a.1), view.map(b.0, b.1));
                let ang_a = (sa.1 - sv.1).atan2(sa.0 - sv.0);
                let ang_b = (sb.1 - sv.1).atan2(sb.0 - sv.0);
                let mut d = ang_b - ang_a;
                while d > std::f64::consts::PI {
                    d -= std::f64::consts::TAU;
                }
                while d < -std::f64::consts::PI {
                    d += std::f64::consts::TAU;
                }
                let rr = it.el.get("r").and_then(num).map(|r| r * px).unwrap_or(26.0);
                if (d.abs() - std::f64::consts::FRAC_PI_2).abs() < 0.02 {
                    let u = (ang_a.cos() * rr * 0.6, ang_a.sin() * rr * 0.6);
                    let w = (ang_b.cos() * rr * 0.6, ang_b.sin() * rr * 0.6);
                    part.polyline(
                        &[
                            (sv.0 + u.0, sv.1 + u.1),
                            (sv.0 + u.0 + w.0, sv.1 + u.1 + w.1),
                            (sv.0 + w.0, sv.1 + w.1),
                        ],
                        col,
                        1.6,
                        false,
                    );
                } else {
                    let pts: Vec<(f64, f64)> = (0..=32)
                        .map(|i| {
                            let t = ang_a + d * i as f64 / 32.0;
                            (sv.0 + rr * t.cos(), sv.1 + rr * t.sin())
                        })
                        .collect();
                    part.polyline(&pts, col, 1.6, false);
                }
                let mid = ang_a + d / 2.0;
                let lp = (
                    sv.0 + (rr + 16.0) * mid.cos(),
                    sv.1 + (rr + 16.0) * mid.sin() + 6.0,
                );
                ctx.label(&mut part, lp, &it.el, col, "middle");
            }
            Shape::Ray { from, dir, both } => {
                let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-9);
                let u = (dir.0 / len, dir.1 / len);
                let a = if *both {
                    (from.0 - u.0 * far, from.1 - u.1 * far)
                } else {
                    *from
                };
                let b = (from.0 + u.0 * far, from.1 + u.1 * far);
                part.polyline(&[view.map(a.0, a.1), view.map(b.0, b.1)], col, 2.0, dashed);
                clamp_part(&mut part, &view);
                let lp = view.map(from.0 + u.0 * span * 0.5, from.1 + u.1 * span * 0.5);
                ctx.label(&mut part, (lp.0 + 8.0, lp.1 - 8.0), &it.el, col, "start");
            }
            Shape::Ground { a, b } => {
                let (sa, sb) = (view.map(a.0, a.1), view.map(b.0, b.1));
                part.polyline(&[sa, sb], col, 2.4, false);
                let len = ((sb.0 - sa.0).powi(2) + (sb.1 - sa.1).powi(2))
                    .sqrt()
                    .max(1e-6);
                let (ux, uy) = ((sb.0 - sa.0) / len, (sb.1 - sa.1) / len);
                // hatch on the right-hand side of a→b (below a left-to-right floor)
                let (nx, ny) = (-uy, ux);
                let n = (len / 14.0) as usize;
                let mut d = String::new();
                for i in 0..=n {
                    let p = (sa.0 + ux * i as f64 * 14.0, sa.1 + uy * i as f64 * 14.0);
                    let q = (p.0 + nx * 10.0 - ux * 8.0, p.1 + ny * 10.0 - uy * 8.0);
                    let _ = write!(d, "M{:.2},{:.2} L{:.2},{:.2} ", p.0, p.1, q.0, q.1);
                    part.grow(q);
                }
                part.prims.push(Prim::Path {
                    d,
                    stroke: col.into(),
                    width: 1.2,
                    dashed: false,
                    fill: None,
                });
                ctx.label(
                    &mut part,
                    ((sa.0 + sb.0) / 2.0, (sa.1 + sb.1) / 2.0 - 10.0),
                    &it.el,
                    col,
                    "middle",
                );
            }
            Shape::Text { at, text } => {
                let lb = part.text(view.map(at.0, at.1), text, 17.0, col, "middle");
                ctx.labels.push(lb);
            }
            Shape::Axes { o, xl, yl, names } => {
                let so = view.map(o.0, o.1);
                let sx = view.map(o.0 + xl, o.1);
                let sy = view.map(o.0, o.1 + yl);
                part.polyline(&[so, sx], col, 1.6, false);
                part.arrowhead(sx, so, col, 11.0);
                part.polyline(&[so, sy], col, 1.6, false);
                part.arrowhead(sy, so, col, 11.0);
                if let Some(n) = names.first() {
                    let lb = part.text((sx.0 + 4.0, sx.1 + 20.0), n, 17.0, col, "end");
                    ctx.labels.push(lb);
                }
                if let Some(n) = names.get(1) {
                    let lb = part.text((sy.0 + 10.0, sy.1 + 4.0), n, 17.0, col, "start");
                    ctx.labels.push(lb);
                }
            }
        }
        ctx.parts.push(part);
    }

    // groups: union of member bboxes (geometry only, no prims)
    for (gid, members, ang) in groups {
        let mut g = Part::new(&gid, 2);
        g.angle = -ang;
        for p in ctx
            .parts
            .iter()
            .filter(|p| members.contains(&p.id) && p.has_extent())
        {
            let (a, b) = (p.min, p.max);
            g.grow(a);
            g.grow(b);
        }
        if g.has_extent() {
            ctx.parts.push(g);
        }
    }
}

fn seg_angle(a: (f64, f64), b: (f64, f64)) -> f64 {
    // math coords (y up) → screen angle (y down, clockwise positive)
    -(b.1 - a.1).atan2(b.0 - a.0).to_degrees()
}

/// Applies an element's own `rotate_deg` (about `about` or its centroid) and then the group
/// transform to literal coordinates; references to named points (`is_ref`) are already in
/// world coordinates and are left untouched. Returns the points and the total rotation
/// (math, CCW degrees).
fn apply_own_rotation(el: &Value, xf: &Xf, pts: &[((f64, f64), bool)]) -> (Vec<(f64, f64)>, f64) {
    let mut t = *xf;
    if let Some(deg) = el.get("rotate_deg").and_then(num) {
        let about = el
            .get("about")
            .and_then(Value::as_array)
            .filter(|a| a.len() == 2)
            .and_then(|a| Some((num(&a[0])?, num(&a[1])?)))
            .unwrap_or_else(|| {
                let n = pts.len().max(1) as f64;
                pts.iter()
                    .fold((0.0, 0.0), |m, (p, _)| (m.0 + p.0 / n, m.1 + p.1 / n))
            });
        t = t.then(&Xf::rotation(deg, about));
    }
    (
        pts.iter()
            .map(|(p, is_ref)| if *is_ref { *p } else { t.apply(*p) })
            .collect(),
        t.angle_deg(),
    )
}

/// Like `coord`, but also reports whether the value referenced a named point.
fn coord_ref(
    v: Option<&Value>,
    points: &HashMap<String, (f64, f64)>,
) -> Result<((f64, f64), bool), String> {
    let is_ref = matches!(v, Some(Value::String(_)));
    coord(v, points).map(|p| (p, is_ref))
}

fn spring_points(a: (f64, f64), b: (f64, f64), coils: usize) -> Vec<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let amp = (len * 0.06).max(0.05);
    let lead = len * 0.12;
    let mut pts = vec![a, (a.0 + ux * lead, a.1 + uy * lead)];
    let body = len - 2.0 * lead;
    let steps = coils * 2;
    for i in 1..steps {
        let t = lead + body * i as f64 / steps as f64;
        let s = if i % 2 == 1 { amp } else { -amp };
        pts.push((a.0 + ux * t + nx * s, a.1 + uy * t + ny * s));
    }
    pts.push((b.0 - ux * lead, b.1 - uy * lead));
    pts.push(b);
    pts
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn plots_a_parabola_with_parts_and_no_errors() {
        let spec = json!({"kind": "plot", "x_range": [-3, 3], "elements": [
            {"id": "f", "type": "curve", "expr": "x^2-2x", "label": "y=x²-2x"},
            {"id": "V", "type": "point", "at": [1, -1], "label": "顶点"}
        ]});
        let out = render_figure(&spec, &none());
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let ids = out.part_ids();
        assert!(
            ids.contains(&"f".to_string())
                && ids.contains(&"V".to_string())
                && ids.contains(&"axes".to_string())
        );
        assert!(out.svg.starts_with("<svg") && out.svg.contains("data-part=\"f\""));
        let v = out.parts.iter().find(|p| p.id == "V").unwrap();
        assert!(v.bbox[0] > 0.3 && v.bbox[0] < 0.8);
    }

    #[test]
    fn breaks_curves_at_discontinuities() {
        let spec = json!({"kind": "plot", "x_range": [-3, 3], "y_range": [-5, 5], "elements": [
            {"id": "h", "type": "curve", "expr": "1/x"},
            {"id": "t", "type": "curve", "expr": "tan(x)"}
        ]});
        let out = render_figure(&spec, &none());
        assert!(out.errors.is_empty());
        // 1/x must be drawn as at least two separate polylines (no vertical connector)
        let h_group = out
            .svg
            .split("data-part=\"h\"")
            .nth(1)
            .unwrap()
            .split("</g>")
            .next()
            .unwrap();
        assert!(h_group.matches("<path").count() >= 2, "{}", h_group);
        let t_group = out
            .svg
            .split("data-part=\"t\"")
            .nth(1)
            .unwrap()
            .split("</g>")
            .next()
            .unwrap();
        assert!(t_group.matches("<path").count() >= 3);
    }

    #[test]
    fn reports_bad_expressions_and_offscreen_content() {
        let spec = json!({"kind": "plot", "x_range": [0, 1], "y_range": [0, 1], "elements": [
            {"id": "bad", "type": "curve", "expr": "x^^2"},
            {"id": "far", "type": "curve", "expr": "x+100"},
            {"id": "P", "type": "point", "at": [5, 5]}
        ]});
        let out = render_figure(&spec, &none());
        assert!(out.errors.iter().any(|e| e.contains("bad")));
        assert!(out
            .warnings
            .iter()
            .any(|w| w.contains("far") && w.contains("outside")));
        assert!(out.warnings.iter().any(|w| w.contains("'P'")));
    }

    #[test]
    fn implicit_circle_and_surface_render() {
        let c = render_figure(
            &json!({"kind": "plot", "x_range": [-2, 2], "y_range": [-1.5, 1.5], "equal_scale": true, "elements": [
                {"id": "c", "type": "implicit", "expr": "x^2+y^2=1"}
            ]}),
            &none(),
        );
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let cp = c.parts.iter().find(|p| p.id == "c").unwrap();
        assert!(
            (cp.bbox[2] * c.width - cp.bbox[3] * c.height).abs() < 12.0,
            "circle should be round: {:?}",
            cp.bbox
        );

        let s = render_figure(
            &json!({"kind": "plot3d", "x_range": [-2, 2], "y_range": [-2, 2], "elements": [
                {"id": "s", "type": "surface", "expr": "x*y", "label": "z=xy"},
                {"id": "o", "type": "point3d", "at": [0, 0, 0]}
            ]}),
            &none(),
        );
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.svg.matches("<path").count() > 500);
        assert!(s.part_ids().contains(&"s".to_string()));
    }

    #[test]
    fn diagram_references_rotation_and_groups() {
        let spec = json!({"kind": "diagram", "elements": [
            {"id": "A", "type": "point", "at": [0, 0]},
            {"id": "B", "type": "point", "at": [4, 0]},
            {"id": "C", "type": "point", "at": [4, 3]},
            {"id": "slope", "type": "ground", "from": "A", "to": "C"},
            {"id": "tri", "type": "polygon", "points": ["A", "B", "C"]},
            {"id": "th", "type": "angle", "vertex": "A", "from": "B", "to": "C", "label": "θ"},
            {"id": "rt", "type": "angle", "vertex": "B", "from": "A", "to": "C"},
            {"id": "blk", "type": "group", "rotate_deg": 36.87, "about": [0, 0], "children": [
                {"id": "m", "type": "rect", "center": [2.5, 0.4], "w": 0.8, "h": 0.8, "label": "m"},
                {"id": "G", "type": "vector", "from": [2.5, 0.4], "to": [2.5, -0.8], "label": "G", "color": "red"}
            ]},
            {"id": "k", "type": "spring", "from": [-2, 0], "to": [-0.5, 0]},
            {"id": "bad", "type": "segment", "from": "Z", "to": "A"}
        ]});
        let out = render_figure(&spec, &none());
        assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
        assert!(out.errors[0].contains("unknown point 'Z'"));
        let m = out.parts.iter().find(|p| p.id == "m").unwrap();
        assert!(
            (m.angle_deg + 36.87).abs() < 0.5,
            "rect angle {}",
            m.angle_deg
        );
        let slope = out.parts.iter().find(|p| p.id == "slope").unwrap();
        assert!((slope.angle_deg + 36.87).abs() < 0.5);
        assert!(out.part_ids().contains(&"blk".to_string()));
        assert!(out.svg.contains(">θ<"));
    }

    #[test]
    fn hidden_parts_are_omitted_but_geometry_kept() {
        let spec = json!({"kind": "plot", "x_range": [-1, 1], "elements": [{"id": "f", "type": "curve", "expr": "x"}]});
        let hidden: HashSet<String> = ["f".to_string()].into_iter().collect();
        let out = render_figure(&spec, &hidden);
        assert!(!out.svg.contains("data-part=\"f\""));
        assert!(out.part_ids().contains(&"f".to_string()));
    }

    #[test]
    fn detects_overlapping_labels() {
        let spec = json!({"kind": "diagram", "elements": [
            {"id": "t1", "type": "text", "at": [0, 0], "text": "Hello"},
            {"id": "t2", "type": "text", "at": [0.01, 0], "text": "Hello"},
            {"id": "far", "type": "point", "at": [10, 10]}
        ]});
        let out = render_figure(&spec, &none());
        assert!(
            out.warnings.iter().any(|w| w.contains("overlap")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn parts_report_what_they_hang_on() {
        let spec = json!({"kind": "diagram", "elements": [
            {"id": "O", "type": "point", "at": [0, 0]},
            {"id": "V", "type": "vector", "from": "O", "to": [3, 0]},
            {"id": "h", "type": "segment", "from": [3, 0], "to": [3, -2], "dashed": true},
            {"id": "far", "type": "segment", "from": [-2, 2], "to": [-1, 2]},
            {"id": "t", "type": "text", "at": [3, -2.15], "text": "h"}
        ]});
        let out = render_figure(&spec, &none());
        let anchors = |id: &str| {
            out.parts
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .anchors
                .clone()
        };
        // the dashed line starts at the arrow tip
        assert!(
            anchors("h").iter().any(|g| g.contains(&"V".to_string())),
            "{:?}",
            anchors("h")
        );
        // the vector's tail sits on the point O
        assert!(
            anchors("V").iter().any(|g| g.contains(&"O".to_string())),
            "{:?}",
            anchors("V")
        );
        assert!(anchors("far").is_empty(), "{:?}", anchors("far"));
        // a lone text belongs to the part nearest to it
        assert_eq!(anchors("t"), vec![vec!["h".to_string()]]);
        assert!(out.meta_json()["parts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.get("anchors").is_some()));
    }

    #[test]
    fn png_rasterizes() {
        let out = render_figure(
            &json!({"kind": "plot", "x_range": [-1, 1], "elements": [{"id": "f", "type": "curve", "expr": "x^2", "label": "f"}]}),
            &none(),
        );
        let png = svg_to_png(&out.svg, 1.0).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}
