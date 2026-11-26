use crate::i18n::{tr, trf};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, DrawingArea, GestureClick, GestureDrag, Orientation, Overlay, Popover, ScrolledWindow};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::rc::Rc;

#[derive(Clone)]
struct MindNode {
    id: String,
    label: String,
    summary: String,
    facets: Vec<String>,
    atomic: bool,
    expandable: bool,
    depth: usize,
    children: Vec<usize>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct TreeState {
    nodes: Vec<MindNode>,
    roots: Vec<usize>,
    collapsed: HashSet<String>,
    scale: f64,
    all_collapsed: bool,
    is_kg: bool,
    /// Where the user's view state is mirrored so a rebuilt widget can pick it up again.
    memory: Option<SharedMemory>,
    // Hit boxes for toggle buttons: (cx, cy, radius, node_id)
    toggle_hits: Vec<(f64, f64, f64, String)>,
    // Hit boxes for node cards: (x, y, w, h, node index)
    node_hits: Vec<(f64, f64, f64, f64, usize)>,
}

/// The user's local view state of a mind map that is rebuilt over and over (the live preview):
/// which branches they opened or closed (keyed by node id), the zoom and the scroll position.
#[derive(Default)]
pub struct MapMemory {
    seen: bool,
    /// Ids of the nodes that had children when the state was last recorded.
    parents: HashSet<String>,
    collapsed: HashSet<String>,
    scale: f64,
    scroll_x: f64,
    scroll_y: f64,
}

pub type SharedMemory = Rc<RefCell<MapMemory>>;

/// Callback of the node action menu: `(action, concept)`.
pub type ActionFn = Rc<dyn Fn(&str, &str)>;

pub fn new_memory() -> SharedMemory {
    Rc::new(RefCell::new(MapMemory::default()))
}

/// Initial collapsed set: branches the user already decided on keep their state, everything else
/// (first build, or a node that only just got children) follows the default of showing two levels.
fn initial_collapsed(nodes: &[MindNode], memory: Option<&MapMemory>) -> HashSet<String> {
    let known = memory.filter(|m| m.seen);
    nodes
        .iter()
        .filter(|n| !n.children.is_empty())
        .filter(|n| match known {
            Some(m) if m.parents.contains(&n.id) => m.collapsed.contains(&n.id),
            _ => n.depth >= 1,
        })
        .map(|n| n.id.clone())
        .collect()
}

impl TreeState {
    fn remember(&self) {
        let Some(mem) = &self.memory else { return };
        let mut m = mem.borrow_mut();
        m.seen = true;
        m.parents = self.nodes.iter().filter(|n| !n.children.is_empty()).map(|n| n.id.clone()).collect();
        m.collapsed = self.collapsed.clone();
        m.scale = self.scale;
    }
}

/// Restores a saved scroll offset once the scroller has its size, and mirrors later user scrolling.
fn track_scroll(adj: &gtk4::Adjustment, saved: Option<f64>, memory: Option<SharedMemory>, horizontal: bool) {
    let pending = Rc::new(Cell::new(saved.filter(|v| *v > 0.0)));
    {
        let pending = pending.clone();
        adj.connect_changed(move |a| {
            if a.page_size() > 0.0 {
                if let Some(target) = pending.take() {
                    a.set_value(target.min((a.upper() - a.page_size()).max(0.0)));
                }
            }
        });
    }
    adj.connect_value_changed(move |a| {
        if pending.get().is_some() {
            return;
        }
        if let Some(mem) = &memory {
            let mut m = mem.borrow_mut();
            if horizontal {
                m.scroll_x = a.value();
            } else {
                m.scroll_y = a.value();
            }
        }
    });
}

const NODE_H: f64 = 34.0;
const ROW_GAP: f64 = 14.0;
const COL_GAP: f64 = 66.0;
const TOGGLE_R: f64 = 10.5;
const PAD_X: f64 = 68.0;
const PAD_Y: f64 = 32.0;

fn strip_citation_markers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '[' || ch == '【' {
            let close = if ch == '[' { ']' } else { '】' };
            let mut j = i + 1;
            let mut has_digit = false;
            let mut valid = true;
            while j < chars.len() && chars[j] != close {
                let c = chars[j];
                if c.is_ascii_digit() {
                    has_digit = true;
                } else if !matches!(c, ' ' | ',' | '，' | '-' | '–' | '—' | '、') {
                    valid = false;
                    break;
                }
                j += 1;
            }
            if valid && has_digit && j < chars.len() && chars[j] == close {
                i = j + 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn estimate_width(label: &str) -> f64 {
    let mut u: f64 = 0.0;
    for ch in label.chars() {
        u += if ch as u32 > 127 { 13.5 } else { 7.5 };
    }
    (u + 26.0).clamp(96.0, 270.0)
}

fn assign_depth(nodes: &mut [MindNode], idx: usize, depth: usize) {
    nodes[idx].depth = depth;
    let children = nodes[idx].children.clone();
    for c in children {
        assign_depth(nodes, c, depth + 1);
    }
}

fn layout_node(
    nodes: &mut [MindNode],
    collapsed: &HashSet<String>,
    idx: usize,
    x: f64,
    cursor_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) -> f64 {
    nodes[idx].x = x;
    let w = nodes[idx].w;
    let children = nodes[idx].children.clone();
    let is_col = collapsed.contains(&nodes[idx].id);
    if children.is_empty() || is_col {
        nodes[idx].y = *cursor_y;
        *cursor_y += NODE_H + ROW_GAP;
    } else {
        let child_x = x + w + COL_GAP;
        let first_y = layout_node(nodes, collapsed, children[0], child_x, cursor_y, max_x, max_y);
        let mut last_y = first_y;
        for &c in &children[1..] {
            last_y = layout_node(nodes, collapsed, c, child_x, cursor_y, max_x, max_y);
        }
        nodes[idx].y = (first_y + last_y) / 2.0;
    }
    *max_x = max_x.max(nodes[idx].x + w + 60.0);
    *max_y = max_y.max(nodes[idx].y + NODE_H + 44.0);
    nodes[idx].y
}

fn relayout(state: &mut TreeState) -> (f64, f64) {
    let mut cursor_y = PAD_Y;
    let mut max_x = 420.0;
    let mut max_y = 320.0;
    let roots = state.roots.clone();
    for r in roots {
        layout_node(
            &mut state.nodes,
            &state.collapsed,
            r,
            PAD_X,
            &mut cursor_y,
            &mut max_x,
            &mut max_y,
        );
        cursor_y += ROW_GAP;
    }
    state.remember();
    (max_x, max_y)
}

fn node_colors(depth: usize, is_kg: bool) -> ((f64, f64, f64), (f64, f64, f64)) {
    if is_kg {
        return match depth {
            0 => ((0.16, 0.24, 0.35), (0.36, 0.54, 0.77)),
            1 => ((0.14, 0.21, 0.27), (0.27, 0.42, 0.54)),
            2 => ((0.14, 0.23, 0.22), (0.24, 0.42, 0.39)),
            _ => ((0.16, 0.22, 0.18), (0.28, 0.40, 0.32)),
        };
    }
    match depth {
        0 => ((0.26, 0.27, 0.39), (0.40, 0.42, 0.60)),
        1 => ((0.17, 0.20, 0.27), (0.27, 0.31, 0.42)),
        2 => ((0.15, 0.22, 0.23), (0.22, 0.33, 0.35)),
        _ => ((0.15, 0.22, 0.18), (0.24, 0.35, 0.28)),
    }
}

fn edge_color(depth: usize) -> (f64, f64, f64) {
    match depth {
        0 => (0.52, 0.55, 0.82),
        1 => (0.48, 0.65, 0.78),
        _ => (0.45, 0.70, 0.61),
    }
}

/// Visible region in layout coordinates.
struct Vis {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Vis {
    fn hits(&self, ax: f64, ay: f64, bx: f64, by: f64) -> bool {
        bx >= self.x0 && ax <= self.x1 && by >= self.y0 && ay <= self.y1
    }
}

fn rounded_rect(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * PI / 2.0);
    cr.close_path();
}

/// True when the card should carry a "+" marker: a collapsed branch, or a leaf the core marked `expandable`.
fn needs_plus(node: &MindNode, collapsed: &HashSet<String>) -> bool {
    (!node.children.is_empty() && collapsed.contains(&node.id)) || (node.children.is_empty() && node.expandable)
}

/// `read_only`: preview mode, a click on a card only expands / collapses it locally (no action menu).
/// `memory`: keeps the expand / collapse state, zoom and scroll across rebuilds of the same map.
pub fn build_mindmap_widget(
    payload: &Value,
    en: bool,
    is_kg: bool,
    read_only: bool,
    memory: Option<SharedMemory>,
    on_action: ActionFn,
) -> Overlay {
    let raw_nodes = payload
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut nodes: Vec<MindNode> = Vec::new();
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    let mut parent_ids: Vec<Option<String>> = Vec::new();

    for (i, n) in raw_nodes.iter().enumerate() {
        let id = n
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("n{i}"));
        let label = strip_citation_markers(n.get("label").and_then(Value::as_str).unwrap_or(""));
        let pid = n
            .get("parent_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let facets: Vec<String> = n
            .get("facets")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(MindNode {
            id,
            w: (estimate_width(&label) + if facets.is_empty() { 0.0 } else { 26.0 }).min(296.0),
            summary: n.get("summary").and_then(Value::as_str).unwrap_or("").to_string(),
            atomic: n.get("atomic").and_then(Value::as_bool).unwrap_or(false),
            expandable: n.get("expandable").and_then(Value::as_bool).unwrap_or(false),
            facets,
            label,
            depth: 0,
            children: Vec::new(),
            x: 0.0,
            y: 0.0,
            h: NODE_H,
        });
        parent_ids.push(pid);
    }

    let mut roots = Vec::new();
    for (idx, pid) in parent_ids.into_iter().enumerate() {
        if let Some(p) = pid {
            if let Some(&pidx) = id_to_idx.get(&p) {
                if pidx != idx {
                    nodes[pidx].children.push(idx);
                    continue;
                }
            }
        }
        roots.push(idx);
    }
    if roots.is_empty() && !nodes.is_empty() {
        roots.push(0);
    }
    for &r in &roots {
        assign_depth(&mut nodes, r, 0);
    }

    // Default view: only the first two levels (root + its children); every deeper branch starts
    // collapsed and collapsed subtrees are neither laid out nor drawn.
    let total_nodes = nodes.len();
    let collapsed_init = initial_collapsed(&nodes, memory.as_ref().map(|m| m.borrow()).as_deref());
    let all_collapsed = nodes
        .iter()
        .filter(|n| n.depth > 0 && !n.children.is_empty())
        .all(|n| collapsed_init.contains(&n.id));
    let (saved_scale, saved_x, saved_y) = memory
        .as_ref()
        .map(|m| m.borrow())
        .filter(|m| m.seen)
        .map(|m| (m.scale, Some(m.scroll_x), Some(m.scroll_y)))
        .unwrap_or((1.0, None, None));
    for n in nodes.iter_mut() {
        // Room for the "+" marker.
        if !n.children.is_empty() || n.expandable {
            n.w = (n.w + 16.0).min(312.0);
        }
    }

    let state = Rc::new(RefCell::new(TreeState {
        nodes,
        roots,
        collapsed: collapsed_init,
        scale: if saved_scale > 0.0 { saved_scale } else { 1.0 },
        all_collapsed,
        memory: memory.clone(),
        is_kg: is_kg || payload.get("kind").and_then(Value::as_str) == Some("knowledge_graph"),
        toggle_hits: Vec::new(),
        node_hits: Vec::new(),
    }));

    let (init_w, init_h) = relayout(&mut state.borrow_mut());
    let area = DrawingArea::new();
    let init_scale = state.borrow().scale;
    area.set_content_width((init_w * init_scale) as i32);
    area.set_content_height((init_h * init_scale) as i32);

    // The scroller is created first so the draw function can read the visible region from its
    // adjustments and skip everything outside it (viewport culling).
    let scroll = ScrolledWindow::builder().hexpand(true).vexpand(true).build();
    let hadj = scroll.hadjustment();
    let vadj = scroll.vadjustment();
    track_scroll(&hadj, saved_x, memory.clone(), true);
    track_scroll(&vadj, saved_y, memory.clone(), false);
    {
        let a = area.clone();
        hadj.connect_value_changed(move |_| a.queue_draw());
        let a = area.clone();
        vadj.connect_value_changed(move |_| a.queue_draw());
        let a = area.clone();
        hadj.connect_page_size_notify(move |_| a.queue_draw());
        let a = area.clone();
        vadj.connect_page_size_notify(move |_| a.queue_draw());
    }

    let st_draw = state.clone();
    let hadj_d = hadj.clone();
    let vadj_d = vadj.clone();
    area.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgb(0.114, 0.122, 0.141);
        let _ = cr.paint();

        let mut st = st_draw.borrow_mut();
        st.toggle_hits.clear();
        st.node_hits.clear();
        let scale = st.scale;
        cr.scale(scale, scale);

        // Visible rectangle in layout coordinates, plus a margin. Before the first allocation
        // (page size 0) everything counts as visible.
        let vis = if hadj_d.page_size() > 0.0 && vadj_d.page_size() > 0.0 {
            let m = 90.0;
            Vis {
                x0: hadj_d.value() / scale - m,
                y0: vadj_d.value() / scale - m,
                x1: (hadj_d.value() + hadj_d.page_size()) / scale + m,
                y1: (vadj_d.value() + vadj_d.page_size()) / scale + m,
            }
        } else {
            Vis { x0: f64::MIN, y0: f64::MIN, x1: f64::MAX, y1: f64::MAX }
        };
        let _ = (width, height);
        let roots = st.roots.clone();

        fn draw_edges(cr: &gtk4::cairo::Context, st: &TreeState, idx: usize, vis: &Vis) {
            let node = &st.nodes[idx];
            if st.collapsed.contains(&node.id) || node.children.is_empty() {
                return;
            }
            let sx = node.x + node.w + TOGGLE_R * 2.0 + 6.0;
            let sy = node.y + node.h / 2.0;
            let (er, eg, eb) = edge_color(node.depth);
            cr.set_source_rgba(er, eg, eb, 0.85);
            cr.set_line_width(1.6);

            for &cidx in &node.children {
                let c = &st.nodes[cidx];
                let ex = c.x;
                let ey = c.y + c.h / 2.0;
                let mx = (sx + ex) / 2.0;
                // A bezier stays inside the box spanned by its end points.
                if vis.hits(sx.min(ex), sy.min(ey), sx.max(ex), sy.max(ey)) {
                    cr.move_to(sx, sy);
                    cr.curve_to(mx, sy, mx, ey, ex, ey);
                    let _ = cr.stroke();
                }
                draw_edges(cr, st, cidx, vis);
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn draw_nodes(
            cr: &gtk4::cairo::Context,
            nodes: &[MindNode],
            collapsed: &HashSet<String>,
            hits: &mut Vec<(f64, f64, f64, String)>,
            node_hits: &mut Vec<(f64, f64, f64, f64, usize)>,
            is_kg: bool,
            vis: &Vis,
            idx: usize,
        ) {
            let node = &nodes[idx];
            let is_col = collapsed.contains(&node.id);
            // Card plus its toggle circle; off-screen nodes are skipped (children still visited,
            // they can lie in view even when the parent does not).
            if !vis.hits(node.x, node.y, node.x + node.w + TOGGLE_R * 2.0 + 8.0, node.y + node.h) {
                if !is_col {
                    for &cidx in &node.children {
                        draw_nodes(cr, nodes, collapsed, hits, node_hits, is_kg, vis, cidx);
                    }
                }
                return;
            }
            node_hits.push((node.x, node.y, node.w, node.h, idx));
            let ((br, bg, bb), (sr, sg, sb)) = node_colors(node.depth, is_kg);
            rounded_rect(cr, node.x, node.y, node.w, node.h, 8.0);
            cr.set_source_rgb(br, bg, bb);
            let _ = cr.fill_preserve();
            cr.set_source_rgb(sr, sg, sb);
            cr.set_line_width(1.0);
            let _ = cr.stroke();

            cr.set_source_rgb(0.91, 0.92, 0.95);
            cr.select_font_face("Sans", gtk4::cairo::FontSlant::Normal, gtk4::cairo::FontWeight::Normal);
            cr.set_font_size(12.0);
            cr.move_to(node.x + 11.0, node.y + node.h / 2.0 + 4.5);
            let _ = cr.show_text(&node.label);
            if needs_plus(node, collapsed) {
                cr.set_source_rgb(0.99, 0.84, 0.39);
                cr.select_font_face("Sans", gtk4::cairo::FontSlant::Normal, gtk4::cairo::FontWeight::Bold);
                cr.set_font_size(13.0);
                cr.move_to(node.x + node.w - 14.0, node.y + node.h / 2.0 + 4.5);
                let _ = cr.show_text("+");
            }
            if !node.facets.is_empty() {
                // Small count chip: this node carries sub-points (facets) listed in its popover.
                let txt = node.facets.len().to_string();
                let cw = 8.0 + 6.5 * txt.len() as f64;
                let cx = node.x + node.w - cw - 20.0;
                let cy = node.y + node.h / 2.0 - 8.0;
                rounded_rect(cr, cx, cy, cw, 16.0, 7.0);
                cr.set_source_rgba(0.31, 0.36, 0.44, 0.75);
                let _ = cr.fill();
                cr.set_source_rgb(0.84, 0.87, 0.93);
                cr.set_font_size(10.5);
                cr.move_to(cx + 4.0, cy + 12.0);
                let _ = cr.show_text(&txt);
            }

            if !node.children.is_empty() {
                let cx = node.x + node.w + 6.0 + TOGGLE_R;
                let cy = node.y + node.h / 2.0;
                cr.arc(cx, cy, TOGGLE_R, 0.0, 2.0 * PI);
                cr.set_source_rgb(0.20, 0.22, 0.27);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(0.35, 0.37, 0.45);
                cr.set_line_width(1.0);
                let _ = cr.stroke();

                cr.set_source_rgb(0.88, 0.90, 0.94);
                cr.set_font_size(11.0);
                cr.move_to(cx - 3.5, cy + 4.0);
                let _ = cr.show_text(if is_col { ">" } else { "<" });
                hits.push((cx, cy, TOGGLE_R + 4.0, node.id.clone()));
            }

            if !is_col {
                for &cidx in &node.children {
                    draw_nodes(cr, nodes, collapsed, hits, node_hits, is_kg, vis, cidx);
                }
            }
        }

        for &r in &roots {
            draw_edges(cr, &st, r, &vis);
        }
        let mut thits = Vec::new();
        let mut nhits = Vec::new();
        for &r in &roots {
            draw_nodes(cr, &st.nodes, &st.collapsed, &mut thits, &mut nhits, st.is_kg, &vis, r);
        }
        st.toggle_hits = thits;
        st.node_hits = nhits;
    });

    // Click gesture to toggle branches
    let click = GestureClick::new();
    let st_click = state.clone();
    let area_click = area.clone();
    click.connect_released(move |_, _, x, y| {
        let mut st = st_click.borrow_mut();
        let sx = x / st.scale;
        let sy = y / st.scale;
        let hit_id = st
            .toggle_hits
            .iter()
            .find(|(cx, cy, r, _)| (sx - cx).hypot(sy - cy) <= *r)
            .map(|(_, _, _, id)| id.clone());
        if let Some(id) = hit_id {
            if st.collapsed.contains(&id) {
                st.collapsed.remove(&id);
            } else {
                st.collapsed.insert(id);
            }
            let (w, h) = relayout(&mut st);
            let sc = st.scale;
            drop(st);
            area_click.set_content_width((w * sc) as i32);
            area_click.set_content_height((h * sc) as i32);
            area_click.queue_draw();
            return;
        }
        let hit_node = st
            .node_hits
            .iter()
            .find(|(nx, ny, nw, nh, _)| sx >= *nx && sx <= nx + nw && sy >= *ny && sy <= ny + nh)
            .and_then(|(_, _, _, _, i)| st.nodes.get(*i).cloned());
        // A click on a collapsed node expands it locally; in preview mode a click toggles it.
        if let Some(node) = hit_node.as_ref() {
            let is_col = st.collapsed.contains(&node.id);
            if !node.children.is_empty() && (is_col || read_only) {
                if is_col {
                    st.collapsed.remove(&node.id);
                } else {
                    st.collapsed.insert(node.id.clone());
                }
                let (w, h) = relayout(&mut st);
                let sc = st.scale;
                drop(st);
                area_click.set_content_width((w * sc) as i32);
                area_click.set_content_height((h * sc) as i32);
                area_click.queue_draw();
                return;
            }
        }
        drop(st);
        if read_only {
            return;
        }
        if let Some(node) = hit_node {
            let lbl = node.label.clone();
            let pop = Popover::new();
            pop.set_parent(&area_click);
            pop.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            let menu = GtkBox::new(Orientation::Vertical, 4);
            menu.set_width_request(260);
            if !node.summary.is_empty() {
                let s = crate::md_view::inline_label(&node.summary);
                s.set_max_width_chars(36);
                menu.append(&s);
            }
            if !node.facets.is_empty() {
                let head = gtk4::Label::new(Some(tr(en, "kg.facets")));
                head.set_xalign(0.0);
                head.add_css_class("lna-muted");
                menu.append(&head);
                for facet in &node.facets {
                    let fb = Button::new();
                    let fl = crate::md_view::inline_label(&format!("• {facet}"));
                    fl.set_selectable(false);
                    fl.set_max_width_chars(36);
                    fb.set_child(Some(&fl));
                    fb.add_css_class("lna-ghost-btn");
                    let cb = on_action.clone();
                    let concept = format!("{lbl}：{facet}");
                    let p_c = pop.clone();
                    fb.connect_clicked(move |_| {
                        p_c.popdown();
                        cb("chat", &concept);
                    });
                    menu.append(&fb);
                }
            }
            if node.depth > 0 {
                let rb = Button::with_label(tr(en, if node.atomic { "mm.already_minimal" } else { "mm.refine" }));
                rb.add_css_class("lna-pill-btn");
                rb.set_sensitive(!node.atomic);
                let cb = on_action.clone();
                let act = format!("refine:{}", node.id);
                let concept = lbl.clone();
                let p_c = pop.clone();
                rb.connect_clicked(move |_| {
                    p_c.popdown();
                    cb(&act, &concept);
                });
                menu.append(&rb);
            }
            for (act, label_key) in [
                ("quiz", "kg.action_quiz"),
                ("mindmap", "kg.action_mindmap"),
                ("chat", "kg.action_chat"),
            ] {
                let b = Button::with_label(tr(en, label_key));
                b.add_css_class("lna-pill-btn");
                let cb = on_action.clone();
                let concept = lbl.clone();
                let p_c = pop.clone();
                b.connect_clicked(move |_| {
                    p_c.popdown();
                    cb(act, &concept);
                });
                menu.append(&b);
            }
            pop.set_child(Some(&menu));
            pop.popup();
        }
    });
    area.add_controller(click);

    scroll.set_child(Some(&area));

    // Drag-to-pan on the mindmap canvas
    let drag = GestureDrag::new();
    let pan_origin = Rc::new(RefCell::new((0.0_f64, 0.0_f64)));
    {
        let scroll_b = scroll.clone();
        let origin_b = pan_origin.clone();
        drag.connect_drag_begin(move |_, _, _| {
            *origin_b.borrow_mut() = (scroll_b.hadjustment().value(), scroll_b.vadjustment().value());
        });
    }
    {
        let scroll_u = scroll.clone();
        let origin_u = pan_origin.clone();
        drag.connect_drag_update(move |_, ox, oy| {
            let (sx, sy) = *origin_u.borrow();
            scroll_u.hadjustment().set_value((sx - ox).max(0.0));
            scroll_u.vadjustment().set_value((sy - oy).max(0.0));
        });
    }
    area.add_controller(drag);

    // Floating left vertical controls (Expand/Collapse All, +, -)
    let ctrl_box = GtkBox::new(Orientation::Vertical, 6);
    ctrl_box.set_halign(gtk4::Align::Start);
    ctrl_box.set_valign(gtk4::Align::Start);
    ctrl_box.set_margin_start(14);
    ctrl_box.set_margin_top(16);

    let toggle_all = Button::with_label("↕");
    toggle_all.add_css_class("lna-pill-btn");
    toggle_all.set_tooltip_text(Some(tr(en, "mindmap.expand_collapse_all")));
    let st_ta = state.clone();
    let area_ta = area.clone();
    toggle_all.connect_clicked(move |_| {
        let mut st = st_ta.borrow_mut();
        st.all_collapsed = !st.all_collapsed;
        st.collapsed.clear();
        if st.all_collapsed {
            let ids: Vec<String> = st
                .nodes
                .iter()
                .filter(|n| n.depth > 0 && !n.children.is_empty())
                .map(|n| n.id.clone())
                .collect();
            for id in ids {
                st.collapsed.insert(id);
            }
        }
        let (w, h) = relayout(&mut st);
        let sc = st.scale;
        drop(st);
        area_ta.set_content_width((w * sc) as i32);
        area_ta.set_content_height((h * sc) as i32);
        area_ta.queue_draw();
    });

    let zoom_in = Button::with_label("+");
    zoom_in.add_css_class("lna-pill-btn");
    let st_zi = state.clone();
    let area_zi = area.clone();
    zoom_in.connect_clicked(move |_| {
        let mut st = st_zi.borrow_mut();
        st.scale = (st.scale + 0.15).min(2.2);
        let (w, h) = relayout(&mut st);
        let sc = st.scale;
        drop(st);
        area_zi.set_content_width((w * sc) as i32);
        area_zi.set_content_height((h * sc) as i32);
        area_zi.queue_draw();
    });

    let zoom_out = Button::with_label("−");
    zoom_out.add_css_class("lna-pill-btn");
    let st_zo = state.clone();
    let area_zo = area.clone();
    zoom_out.connect_clicked(move |_| {
        let mut st = st_zo.borrow_mut();
        st.scale = (st.scale - 0.15).max(0.45);
        let (w, h) = relayout(&mut st);
        let sc = st.scale;
        drop(st);
        area_zo.set_content_width((w * sc) as i32);
        area_zo.set_content_height((h * sc) as i32);
        area_zo.queue_draw();
    });

    ctrl_box.append(&toggle_all);
    ctrl_box.append(&zoom_in);
    ctrl_box.append(&zoom_out);

    let overlay = Overlay::new();
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&ctrl_box);
    if total_nodes > 400 {
        let hint = gtk4::Label::new(Some(&trf(en, "mindmap.large_hint_fmt", &[&total_nodes])));
        hint.add_css_class("lna-muted");
        hint.set_halign(gtk4::Align::Center);
        hint.set_valign(gtk4::Align::End);
        hint.set_margin_bottom(12);
        hint.set_can_target(false);
        overlay.add_overlay(&hint);
    }
    overlay.set_vexpand(true);
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, depth: usize, children: &[usize]) -> MindNode {
        MindNode {
            id: id.to_string(),
            label: id.to_string(),
            summary: String::new(),
            facets: Vec::new(),
            atomic: false,
            expandable: false,
            depth,
            children: children.to_vec(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: NODE_H,
        }
    }

    /// root -> a -> a1, root -> b -> b1, root -> c (leaf)
    fn tree() -> Vec<MindNode> {
        vec![
            node("root", 0, &[1, 3, 5]),
            node("a", 1, &[2]),
            node("a1", 2, &[]),
            node("b", 1, &[4]),
            node("b1", 2, &[]),
            node("c", 1, &[]),
        ]
    }

    fn state_of(nodes: Vec<MindNode>, collapsed: HashSet<String>, mem: &SharedMemory) -> TreeState {
        TreeState {
            roots: vec![0],
            nodes,
            collapsed,
            scale: 1.0,
            all_collapsed: false,
            is_kg: false,
            memory: Some(mem.clone()),
            toggle_hits: Vec::new(),
            node_hits: Vec::new(),
        }
    }

    #[test]
    fn first_build_shows_two_levels() {
        let c = initial_collapsed(&tree(), None);
        let want: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(c, want);
    }

    #[test]
    fn rebuild_keeps_the_users_choices() {
        let mem = new_memory();
        // The user opened "a" and closed "b" (b was already closed by default; root stays open).
        let mut st = state_of(tree(), initial_collapsed(&tree(), None), &mem);
        st.collapsed.remove("a");
        relayout(&mut st);

        // The stream delivers a bigger tree: "b" gets another child, "a1" becomes a parent, and
        // a brand-new parent "d" appears.
        let mut grown = tree();
        grown.push(node("a2", 3, &[]));
        grown[2].children = vec![6];
        grown.push(node("d", 1, &[8]));
        grown.push(node("d1", 2, &[]));
        grown[0].children.push(7);
        grown[2].depth = 2;

        let c = initial_collapsed(&grown, Some(&mem.borrow()));
        assert!(!c.contains("a"), "opened branch must stay open");
        assert!(c.contains("b"), "closed branch must stay closed");
        assert!(c.contains("a1"), "a node that only just got children follows the default");
        assert!(c.contains("d"), "a new parent follows the default");
        assert!(!c.contains("root"));
    }

    #[test]
    fn untouched_memory_behaves_like_a_first_build() {
        let mem = new_memory();
        let c = initial_collapsed(&tree(), Some(&mem.borrow()));
        assert_eq!(c.len(), 2);
    }
}
