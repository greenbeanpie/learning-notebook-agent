//! Native Markdown + math rendering for GTK: Pango markup in plain `Label`s, no WebKit.
//!
//! Parsing is shared with the core (`lna_studio::mdlite`), TeX goes through
//! `lna_studio::texfmt` (Unicode math; unmappable scripts use Pango `<sup>` / `<sub>`).

use crate::i18n::tr;
use gtk4::prelude::*;
use gtk4::{pango, Box as GtkBox, Button, Grid, Label, Orientation, ScrolledWindow, Separator};
use lna_studio::mdlite::{self, Block, Span};
use lna_studio::texfmt;
use serde_json::Value;

const ACCENT: &str = "#8ab4f8";
const CODE_FG: &str = "#a0c8ff";
const CODE_BG: &str = "#33373f";

fn esc(s: &str) -> String {
    gtk4::glib::markup_escape_text(s).to_string()
}

/// Pango markup for a TeX expression.
pub fn math_markup(tex: &str) -> String {
    let mut out = String::from("<span font_family=\"serif\">");
    for run in texfmt::tex_to_runs(tex) {
        match run.shift {
            s if s > 0 => out.push_str(&format!("<sup>{}</sup>", esc(&run.text))),
            s if s < 0 => out.push_str(&format!("<sub>{}</sub>", esc(&run.text))),
            _ => out.push_str(&esc(&run.text)),
        }
    }
    out.push_str("</span>");
    out
}

/// Pango markup for one paragraph of inline Markdown.
pub fn inline_markup(text: &str) -> String {
    let mut out = String::new();
    for span in mdlite::parse_inline(text) {
        match span {
            Span::Text { text, bold, italic, strike } => {
                let mut s = esc(&text);
                if strike {
                    s = format!("<s>{s}</s>");
                }
                if italic {
                    s = format!("<i>{s}</i>");
                }
                if bold {
                    s = format!("<b>{s}</b>");
                }
                out.push_str(&s);
            }
            Span::Code(code) => out.push_str(&format!(
                "<span font_family=\"monospace\" foreground=\"{CODE_FG}\" background=\"{CODE_BG}\">{}</span>",
                esc(&code)
            )),
            Span::Math(tex) => out.push_str(&math_markup(&tex)),
            Span::Link { text, url } => out.push_str(&format!("<a href=\"{}\">{}</a>", esc(&url), esc(&text))),
            Span::Cite(n) => out.push_str(&format!("<sup><span foreground=\"{ACCENT}\"><b>[{n}]</b></span></sup>")),
        }
    }
    out
}

fn markup_label(markup: &str) -> Label {
    let l = Label::new(None);
    l.set_use_markup(true);
    l.set_markup(markup);
    l.set_wrap(true);
    l.set_wrap_mode(pango::WrapMode::WordChar);
    l.set_xalign(0.0);
    l.set_selectable(true);
    l
}

/// A label showing inline Markdown (titles, options, card faces).
pub fn inline_label(text: &str) -> Label {
    markup_label(&inline_markup(text))
}

/// A vertical box rendering a whole Markdown document.
pub fn markdown_widget(md: &str) -> GtkBox {
    let root = GtkBox::new(Orientation::Vertical, 6);
    for block in mdlite::parse(md) {
        root.append(&block_widget(&block));
    }
    root
}

fn block_widget(block: &Block) -> gtk4::Widget {
    match block {
        Block::Heading { level, text } => {
            let size = match level {
                1 => "x-large",
                2 => "large",
                _ => "medium",
            };
            let l = markup_label(&format!("<span size=\"{size}\" weight=\"bold\">{}</span>", inline_markup(text)));
            l.set_margin_top(4);
            l.upcast()
        }
        Block::Paragraph(text) => inline_label(text).upcast(),
        Block::ListItem { marker, depth, text, .. } => {
            let row = GtkBox::new(Orientation::Horizontal, 6);
            row.set_margin_start((*depth as i32) * 16);
            let m = Label::new(Some(marker));
            m.set_valign(gtk4::Align::Start);
            m.add_css_class("lna-muted");
            let body = inline_label(text);
            body.set_hexpand(true);
            row.append(&m);
            row.append(&body);
            row.upcast()
        }
        Block::Quote(text) => {
            let row = GtkBox::new(Orientation::Horizontal, 10);
            row.append(&Separator::new(Orientation::Vertical));
            let l = inline_label(text);
            l.add_css_class("lna-muted");
            row.append(&l);
            row.upcast()
        }
        Block::Code(code) => {
            let l = Label::new(None);
            l.set_markup(&format!("<span font_family=\"monospace\">{}</span>", esc(code)));
            l.set_xalign(0.0);
            l.set_selectable(true);
            l.set_margin_top(8);
            l.set_margin_bottom(8);
            l.set_margin_start(10);
            l.set_margin_end(10);
            let sw = ScrolledWindow::builder()
                .child(&l)
                .vscrollbar_policy(gtk4::PolicyType::Never)
                .build();
            sw.add_css_class("lna-card");
            sw.upcast()
        }
        Block::Math(tex) => {
            let l = markup_label(&format!("<span size=\"large\">{}</span>", math_markup(tex)));
            l.set_xalign(0.5);
            l.set_justify(gtk4::Justification::Center);
            l.upcast()
        }
        Block::Rule => Separator::new(Orientation::Horizontal).upcast(),
        Block::Table { header, rows } => {
            let grid = Grid::new();
            grid.set_column_spacing(14);
            grid.set_row_spacing(6);
            grid.add_css_class("lna-card");
            for (c, cell) in header.iter().enumerate() {
                grid.attach(&markup_label(&format!("<b>{}</b>", inline_markup(cell))), c as i32, 0, 1, 1);
            }
            for (r, row) in rows.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    grid.attach(&inline_label(cell), c as i32, r as i32 + 1, 1, 1);
                }
            }
            grid.upcast()
        }
    }
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

fn card() -> GtkBox {
    let b = GtkBox::new(Orientation::Vertical, 6);
    b.add_css_class("lna-card");
    b
}

/// Report section body plus its key takeaways.
pub fn report_section_widget(section: &Value, en: bool) -> GtkBox {
    let root = GtkBox::new(Orientation::Vertical, 8);
    root.append(&markdown_widget(section.get("body_md").and_then(Value::as_str).unwrap_or("")));
    let takeaways = strs(section.get("key_takeaways"));
    if !takeaways.is_empty() {
        let c = card();
        c.append(&markup_label(&format!("<b>{}</b>", tr(en, "readable.key_points"))));
        let md: Vec<String> = takeaways.iter().map(|t| format!("- {t}")).collect();
        c.append(&markdown_widget(&md.join("\n")));
        root.append(&c);
    }
    root
}

/// Readable views for quiz / flashcards / audio artifacts (instead of raw JSON).
pub fn readable_artifact_widget(kind: &str, payload: &Value, en: bool) -> Option<GtkBox> {
    let root = GtkBox::new(Orientation::Vertical, 12);
    match kind {
        "quiz" => {
            let qs = payload.get("questions").and_then(Value::as_array).cloned().unwrap_or_default();
            for (qi, q) in qs.iter().enumerate() {
                let c = card();
                c.append(&markdown_widget(&format!(
                    "**{}.** {}",
                    qi + 1,
                    q.get("prompt").and_then(Value::as_str).unwrap_or("")
                )));
                for (oi, opt) in strs(q.get("options")).iter().enumerate() {
                    let letter = (b'A' + (oi.min(25) as u8)) as char;
                    c.append(&inline_label(&format!("{letter}. {opt}")));
                }
                let answer = GtkBox::new(Orientation::Vertical, 4);
                answer.set_visible(false);
                let correct: Vec<String> = q
                    .get("answer_indices")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_u64)
                            .map(|i| ((b'A' + (i.min(25) as u8)) as char).to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut accepted = strs(q.get("accepted_answers"));
                accepted.extend(correct);
                if !accepted.is_empty() {
                    let label = tr(en, "readable.answer");
                    answer.append(&markdown_widget(&format!("{label}{}", accepted.join(" / "))));
                }
                for key in ["rubric", "explanation"] {
                    if let Some(t) = q.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                        let w = markdown_widget(t);
                        w.add_css_class("lna-muted");
                        answer.append(&w);
                    }
                }
                let btn = Button::with_label(tr(en, "readable.show_answer"));
                btn.add_css_class("lna-pill-btn");
                btn.set_halign(gtk4::Align::Start);
                let ans = answer.clone();
                btn.connect_clicked(move |b| {
                    let show = !ans.is_visible();
                    ans.set_visible(show);
                    b.set_label(tr(en, if show { "readable.hide_answer" } else { "readable.show_answer" }));
                });
                c.append(&btn);
                c.append(&answer);
                root.append(&c);
            }
        }
        "flashcards" => {
            let cards = payload.get("cards").and_then(Value::as_array).cloned().unwrap_or_default();
            let total = cards.len();
            for (i, cd) in cards.iter().enumerate() {
                let c = card();
                let front = markdown_widget(cd.get("front").and_then(Value::as_str).unwrap_or(""));
                let back = markdown_widget(cd.get("back").and_then(Value::as_str).unwrap_or(""));
                back.set_visible(false);
                let head = Label::new(Some(&format!("{} / {total}", i + 1)));
                head.set_xalign(0.0);
                head.add_css_class("lna-muted");
                c.append(&head);
                c.append(&front);
                if let Some(h) = cd.get("hint").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                    let hint = inline_label(&format!("{}{h}", tr(en, "readable.hint")));
                    hint.add_css_class("lna-muted");
                    c.append(&hint);
                }
                c.append(&back);
                let btn = Button::with_label(tr(en, "readable.flip"));
                btn.add_css_class("lna-pill-btn");
                btn.set_halign(gtk4::Align::Start);
                let (f, b) = (front.clone(), back.clone());
                btn.connect_clicked(move |_| {
                    let show_back = !b.is_visible();
                    b.set_visible(show_back);
                    f.set_visible(!show_back);
                });
                c.append(&btn);
                root.append(&c);
            }
        }
        "audio" => {
            for t in payload.get("turns").and_then(Value::as_array).cloned().unwrap_or_default() {
                let spk = markup_label(&format!(
                    "<span foreground=\"{ACCENT}\"><b>{}</b></span>",
                    esc(t.get("speaker").and_then(Value::as_str).unwrap_or(""))
                ));
                root.append(&spk);
                root.append(&markdown_widget(t.get("text").and_then(Value::as_str).unwrap_or("")));
            }
        }
        _ => return None,
    }
    Some(root)
}
