use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, DrawingArea, GestureClick, GestureDrag, Orientation, Overlay, Popover, ScrolledWindow};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::rc::Rc;

#[derive(Clone)]
struct MindNode {
    id: String,
    label: String,
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
    // Hit boxes for toggle buttons: (cx, cy, radius, node_id)
    toggle_hits: Vec<(f64, f64, f64, String)>,
    // Hit boxes for node cards: (x, y, w, h, label)
    node_hits: Vec<(f64, f64, f64, f64, String)>,
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

fn rounded_rect(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * PI / 2.0);
    cr.close_path();
}

pub fn build_mindmap_widget(payload: &Value, en: bool, is_kg: bool, on_action: Rc<dyn Fn(&str, &str)>) -> Overlay {
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
        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(MindNode {
            id,
            w: estimate_width(&label),
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

    let state = Rc::new(RefCell::new(TreeState {
        nodes,
        roots,
        collapsed: HashSet::new(),
        scale: 1.0,
        all_collapsed: false,
        is_kg: is_kg || payload.get("kind").and_then(Value::as_str) == Some("knowledge_graph"),
        toggle_hits: Vec::new(),
        node_hits: Vec::new(),
    }));

    let (init_w, init_h) = relayout(&mut state.borrow_mut());
    let area = DrawingArea::new();
    area.set_content_width(init_w as i32);
    area.set_content_height(init_h as i32);

    let st_draw = state.clone();
    area.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgb(0.114, 0.122, 0.141);
        let _ = cr.paint();

        let mut st = st_draw.borrow_mut();
        st.toggle_hits.clear();
        st.node_hits.clear();
        let scale = st.scale;
        cr.scale(scale, scale);

        let _ = (width, height);
        let roots = st.roots.clone();

        fn draw_edges(cr: &gtk4::cairo::Context, st: &TreeState, idx: usize) {
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
                cr.move_to(sx, sy);
                cr.curve_to(mx, sy, mx, ey, ex, ey);
                let _ = cr.stroke();
                draw_edges(cr, st, cidx);
            }
        }

        fn draw_nodes(
            cr: &gtk4::cairo::Context,
            nodes: &[MindNode],
            collapsed: &HashSet<String>,
            hits: &mut Vec<(f64, f64, f64, String)>,
            node_hits: &mut Vec<(f64, f64, f64, f64, String)>,
            is_kg: bool,
            idx: usize,
        ) {
            let node = &nodes[idx];
            node_hits.push((node.x, node.y, node.w, node.h, node.label.clone()));
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

            let is_col = collapsed.contains(&node.id);
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
                    draw_nodes(cr, nodes, collapsed, hits, node_hits, is_kg, cidx);
                }
            }
        }

        for &r in &roots {
            draw_edges(cr, &st, r);
        }
        let nodes_clone = st.nodes.clone();
        let col_clone = st.collapsed.clone();
        let mut thits = Vec::new();
        let mut nhits = Vec::new();
        for &r in &roots {
            draw_nodes(cr, &nodes_clone, &col_clone, &mut thits, &mut nhits, st.is_kg, r);
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
        let hit_label = st
            .node_hits
            .iter()
            .find(|(nx, ny, nw, nh, _)| sx >= *nx && sx <= nx + nw && sy >= *ny && sy <= ny + nh)
            .map(|(_, _, _, _, lbl)| lbl.clone());
        drop(st);
        if let Some(lbl) = hit_label {
            let pop = Popover::new();
            pop.set_parent(&area_click);
            pop.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            let menu = GtkBox::new(Orientation::Vertical, 4);
            for (act, key_en, key_zh) in [
                ("quiz", "Quiz on this concept", "针对此知识点生成测验"),
                ("mindmap", "Expand as Mind Map", "展开为详细思维导图"),
                ("chat", "Explain in Chat", "在对话中深入讲解"),
            ] {
                let b = Button::with_label(if en { key_en } else { key_zh });
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

    let scroll = ScrolledWindow::builder()
        .child(&area)
        .hexpand(true)
        .vexpand(true)
        .build();

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
    toggle_all.set_tooltip_text(Some(if en { "Expand / Collapse all" } else { "全部展开 / 收起" }));
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
    overlay.set_vexpand(true);
    overlay
}
