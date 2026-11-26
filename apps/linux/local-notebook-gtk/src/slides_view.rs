use crate::i18n::tr;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Expander, Frame, Label, Orientation, ScrolledWindow, Stack,
    StackTransitionType,
};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn render_slide_card(s: &Value, idx: usize, total: usize, deck_title: &str) -> GtkBox {
    let layout = s.get("layout").and_then(Value::as_str).unwrap_or("bullets");
    let title = s.get("title").and_then(Value::as_str).unwrap_or("Slide");
    let subtitle = s.get("subtitle").and_then(Value::as_str).unwrap_or("");
    let callout = s.get("callout").and_then(Value::as_str).unwrap_or("");

    let card = GtkBox::new(Orientation::Vertical, 10);
    card.set_margin_top(20);
    card.set_margin_bottom(16);
    card.set_margin_start(24);
    card.set_margin_end(24);
    card.set_hexpand(true);
    card.set_vexpand(true);

    let eyebrow = Label::new(Some(&format!("{} · {}/{}", layout.to_uppercase(), idx + 1, total)));
    eyebrow.set_xalign(0.0);
    eyebrow.add_css_class("lna-muted");
    card.append(&eyebrow);

    let title_lbl = Label::new(Some(title));
    title_lbl.set_xalign(0.0);
    title_lbl.set_wrap(true);
    title_lbl.set_markup(&format!(
        "<span size='{}' weight='bold'>{}</span>",
        if layout == "title" || layout == "section" || layout == "summary" {
            "xx-large"
        } else {
            "x-large"
        },
        glib::markup_escape_text(title)
    ));
    card.append(&title_lbl);

    if !subtitle.is_empty() {
        let sub_lbl = Label::new(Some(subtitle));
        sub_lbl.set_xalign(0.0);
        sub_lbl.set_wrap(true);
        sub_lbl.add_css_class("lna-muted");
        card.append(&sub_lbl);
    }

    let body = GtkBox::new(Orientation::Vertical, 8);
    body.set_vexpand(true);

    if let Some(formula) = s.get("formula").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
        let f_lbl = Label::new(None);
        f_lbl.set_wrap(true);
        f_lbl.set_markup(&format!(
            "<span size='large' weight='bold'>{}</span>",
            crate::md_view::math_markup(formula)
        ));
        f_lbl.set_justify(gtk4::Justification::Center);
        let f_frame = Frame::new(None);
        f_frame.set_child(Some(&f_lbl));
        body.append(&f_frame);
    }

    if let Some(cards) = s.get("cards").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let row = GtkBox::new(Orientation::Horizontal, 10);
        row.set_homogeneous(true);
        for c in cards.iter().take(4) {
            let cb = GtkBox::new(Orientation::Vertical, 4);
            cb.set_margin_top(10);
            cb.set_margin_bottom(10);
            cb.set_margin_start(10);
            cb.set_margin_end(10);
            let ct = c.get("title").and_then(Value::as_str).unwrap_or("");
            let cbody = c.get("body").and_then(Value::as_str).unwrap_or("");
            let tl = Label::new(None);
            tl.set_xalign(0.0);
            tl.set_wrap(true);
            tl.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(ct)));
            let bl = crate::md_view::inline_label(cbody);
            cb.append(&tl);
            cb.append(&bl);
            let f = Frame::new(None);
            f.set_child(Some(&cb));
            row.append(&f);
        }
        body.append(&row);
    } else if let Some(cols) = s.get("columns").and_then(Value::as_array).filter(|a| a.len() >= 2) {
        let row = GtkBox::new(Orientation::Horizontal, 12);
        row.set_homogeneous(true);
        for c in cols.iter().take(2) {
            let cb = GtkBox::new(Orientation::Vertical, 6);
            cb.set_margin_top(10);
            cb.set_margin_bottom(10);
            cb.set_margin_start(12);
            cb.set_margin_end(12);
            let h = c.get("heading").and_then(Value::as_str).unwrap_or("");
            if !h.is_empty() {
                let hl = Label::new(None);
                hl.set_xalign(0.0);
                hl.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(h)));
                cb.append(&hl);
            }
            if let Some(items) = c.get("items").and_then(Value::as_array) {
                for it in items.iter().filter_map(Value::as_str) {
                    let il = crate::md_view::inline_label(&format!("▸ {it}"));
                    cb.append(&il);
                }
            }
            let f = Frame::new(None);
            f.set_child(Some(&cb));
            row.append(&f);
        }
        body.append(&row);
    } else if let Some(metrics) = s.get("metrics").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let row = GtkBox::new(Orientation::Horizontal, 10);
        row.set_homogeneous(true);
        for m in metrics.iter().take(4) {
            let mb = GtkBox::new(Orientation::Vertical, 4);
            mb.set_margin_top(12);
            mb.set_margin_bottom(12);
            let val = m.get("value").and_then(Value::as_str).unwrap_or("");
            let lbl = m.get("label").and_then(Value::as_str).unwrap_or("");
            let vl = Label::new(None);
            vl.set_markup(&format!(
                "<span size='xx-large' weight='bold'>{}</span>",
                glib::markup_escape_text(val)
            ));
            let ll = Label::new(Some(lbl));
            ll.set_wrap(true);
            mb.append(&vl);
            mb.append(&ll);
            let f = Frame::new(None);
            f.set_child(Some(&mb));
            row.append(&f);
        }
        body.append(&row);
    } else if let Some(steps) = s.get("steps").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.set_homogeneous(true);
        for (si, st) in steps.iter().take(5).enumerate() {
            let sb = GtkBox::new(Orientation::Vertical, 4);
            sb.set_margin_top(8);
            sb.set_margin_bottom(8);
            sb.set_margin_start(8);
            sb.set_margin_end(8);
            let num = st.get("step").and_then(Value::as_str).unwrap_or("");
            let stt = st.get("title").and_then(Value::as_str).unwrap_or("");
            let det = st.get("detail").and_then(Value::as_str).unwrap_or("");
            let head = Label::new(None);
            head.set_xalign(0.0);
            head.set_wrap(true);
            head.set_markup(&format!(
                "<b>{} {}</b>",
                if num.is_empty() { format!("{:02}", si + 1) } else { num.to_string() },
                glib::markup_escape_text(stt)
            ));
            let dl = Label::new(Some(det));
            dl.set_xalign(0.0);
            dl.set_wrap(true);
            sb.append(&head);
            sb.append(&dl);
            let f = Frame::new(None);
            f.set_child(Some(&sb));
            row.append(&f);
        }
        body.append(&row);
    } else if let Some(tbl) = s.get("table").and_then(Value::as_object) {
        if let Some(hdrs) = tbl.get("headers").and_then(Value::as_array) {
            let h_line = hdrs.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("  |  ");
            let hl = Label::new(None);
            hl.set_xalign(0.0);
            hl.set_wrap(true);
            hl.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&h_line)));
            body.append(&hl);
        }
        if let Some(rows) = tbl.get("rows").and_then(Value::as_array) {
            for r in rows.iter().take(7) {
                if let Some(cells) = r.as_array() {
                    let line = cells.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("  |  ");
                    let rl = Label::new(Some(&line));
                    rl.set_xalign(0.0);
                    rl.set_wrap(true);
                    body.append(&rl);
                }
            }
        }
    }

    if let Some(bullets) = s.get("bullets").and_then(Value::as_array) {
        for b in bullets.iter().filter_map(Value::as_str).take(6) {
            if !b.trim().is_empty() {
                let text = if layout == "quote" {
                    format!("“{}”", b.trim())
                } else {
                    format!("▸  {}", b.trim())
                };
                let bl = crate::md_view::inline_label(&text);
                body.append(&bl);
            }
        }
    }

    card.append(&body);

    if !callout.is_empty() {
        let cl = Label::new(None);
        cl.set_xalign(0.0);
        cl.set_wrap(true);
        cl.set_markup(&format!("<b>★ {}</b>", crate::md_view::inline_markup(callout)));
        card.append(&cl);
    }

    let foot = Label::new(Some(&format!("{deck_title}   ·   {} / {total}", idx + 1)));
    foot.set_xalign(1.0);
    foot.add_css_class("lna-muted");
    card.append(&foot);
    card
}

/// Interactive animated PPT presentation viewer for Linux GTK4 with smooth
/// slide-left/right transitions, Play/Pause auto-advance, slide strip, and speaker notes.
pub fn build_slides_widget(payload: &Value, en: bool) -> GtkBox {
    let root = GtkBox::new(Orientation::Vertical, 10);
    root.set_vexpand(true);
    root.set_hexpand(true);

    let slides = payload
        .get("slides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if slides.is_empty() {
        root.append(&Label::new(Some(tr(en, "slides.no_slides_available"))));
        return root;
    }

    let deck_title = payload.get("title").and_then(Value::as_str).unwrap_or("Presentation");
    let total = slides.len();

    let stack = Stack::new();
    stack.set_transition_type(StackTransitionType::SlideLeftRight);
    stack.set_transition_duration(280);
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let mut notes_list = Vec::with_capacity(total);
    for (i, s) in slides.iter().enumerate() {
        let page = render_slide_card(s, i, total, deck_title);
        let sc = ScrolledWindow::builder()
            .child(&page)
            .min_content_height(320)
            .vexpand(true)
            .hexpand(true)
            .build();
        stack.add_named(&sc, Some(&format!("s{i}")));
        notes_list.push(
            s.get("speaker_notes")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }

    let stage_frame = Frame::new(None);
    stage_frame.set_vexpand(true);
    stage_frame.set_child(Some(&stack));
    root.append(&stage_frame);

    let idx_state = Rc::new(RefCell::new(0usize));
    let playing_state = Rc::new(RefCell::new(false));
    let notes_rc = Rc::new(notes_list);

    let counter = Label::new(Some(&format!("1 / {total}")));
    let notes_lbl = Label::new(Some(
        notes_rc
            .first()
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str)
            .unwrap_or(tr(en, "slides.no_speaker_notes")),
    ));
    notes_lbl.set_xalign(0.0);
    notes_lbl.set_wrap(true);
    notes_lbl.set_selectable(true);

    let update_view = {
        let stack_u = stack.clone();
        let counter_u = counter.clone();
        let notes_u = notes_lbl.clone();
        let notes_data = notes_rc.clone();
        Rc::new(move |new_idx: usize| {
            stack_u.set_visible_child_name(&format!("s{new_idx}"));
            counter_u.set_text(&format!("{} / {total}", new_idx + 1));
            let n = notes_data.get(new_idx).map(String::as_str).unwrap_or("");
            notes_u.set_text(if n.trim().is_empty() {
                tr(en, "slides.no_speaker_notes")
            } else {
                n
            });
        })
    };

    let nav = GtkBox::new(Orientation::Horizontal, 8);
    nav.set_halign(gtk4::Align::Center);
    let prev_btn = Button::with_label(tr(en, "slides.previous"));
    prev_btn.add_css_class("lna-pill-btn");
    let next_btn = Button::with_label(tr(en, "slides.next"));
    next_btn.add_css_class("lna-pill-btn");
    let play_btn = Button::with_label(tr(en, "slides.play_2"));
    play_btn.add_css_class("lna-primary-btn");

    {
        let idx_p = idx_state.clone();
        let upd = update_view.clone();
        prev_btn.connect_clicked(move |_| {
            let mut cur = idx_p.borrow_mut();
            if *cur > 0 {
                *cur -= 1;
                upd(*cur);
            }
        });
    }
    {
        let idx_n = idx_state.clone();
        let upd = update_view.clone();
        next_btn.connect_clicked(move |_| {
            let mut cur = idx_n.borrow_mut();
            if *cur + 1 < total {
                *cur += 1;
                upd(*cur);
            }
        });
    }
    {
        let idx_pl = idx_state.clone();
        let playing_pl = playing_state.clone();
        let upd = update_view.clone();
        let play_b = play_btn.clone();
        play_btn.connect_clicked(move |_| {
            let mut is_pl = playing_pl.borrow_mut();
            *is_pl = !*is_pl;
            if *is_pl {
                play_b.set_label(tr(en, "slides.pause_2"));
                let idx_t = idx_pl.clone();
                let playing_t = playing_pl.clone();
                let upd_t = upd.clone();
                let play_t = play_b.clone();
                glib::timeout_add_local(Duration::from_secs(5), move || {
                    if !*playing_t.borrow() {
                        return glib::ControlFlow::Break;
                    }
                    let mut cur = idx_t.borrow_mut();
                    if *cur + 1 < total {
                        *cur += 1;
                        upd_t(*cur);
                        glib::ControlFlow::Continue
                    } else {
                        *playing_t.borrow_mut() = false;
                        play_t.set_label(tr(en, "slides.play_2"));
                        glib::ControlFlow::Break
                    }
                });
            } else {
                play_b.set_label(tr(en, "slides.play_2"));
            }
        });
    }

    nav.append(&prev_btn);
    nav.append(&counter);
    nav.append(&next_btn);
    nav.append(&play_btn);
    root.append(&nav);

    // Clickable slide filmstrip
    let strip = GtkBox::new(Orientation::Horizontal, 6);
    for (i, s) in slides.iter().enumerate() {
        let t = s.get("title").and_then(Value::as_str).unwrap_or("");
        let short: String = t.chars().take(12).collect();
        let tb = Button::with_label(&format!("{}. {short}", i + 1));
        tb.add_css_class("lna-pill-btn");
        let idx_s = idx_state.clone();
        let upd_s = update_view.clone();
        tb.connect_clicked(move |_| {
            *idx_s.borrow_mut() = i;
            upd_s(i);
        });
        strip.append(&tb);
    }
    let strip_sc = ScrolledWindow::builder()
        .child(&strip)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    root.append(&strip_sc);

    let notes_exp = Expander::new(Some(tr(en, "slides.speaker_notes")));
    notes_exp.set_child(Some(&notes_lbl));
    root.append(&notes_exp);

    root
}
