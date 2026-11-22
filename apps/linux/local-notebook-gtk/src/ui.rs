use crate::i18n::{tr, user_error_advice, UiPreferences};
use crate::icons::{studio_icon_image, svg_image, SVG_BRAIN, SVG_CLOSE, SVG_FULLSCREEN, SVG_RING, SVG_SEARCH_DOC};
use crate::mindmap_view::build_mindmap_widget;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DropDown, Entry, Expander, FlowBox, SelectionMode,
    Grid, Label, MenuButton, Notebook, Orientation, Paned, Popover, PositionType, ScrolledWindow,
    Stack, StringList, Switch, TextView,
};
use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn call_core(core: &Arc<CoreRuntime>, method: &str, params: Value) -> Result<Value, (String, String)> {
    let req = json!({ "id": "gtk", "method": method, "params": params });
    let raw = core.handle_json(&req.to_string());
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| (String::new(), e.to_string()))?;
    if let Some(err) = parsed.get("error") {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("").to_string();
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("Error").to_string();
        return Err((code, msg));
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

fn friendly_err(en: bool, err: (String, String)) -> String {
    if let Some(adv) = user_error_advice(en, &err.0) {
        adv.to_string()
    } else if !err.1.is_empty() {
        err.1
    } else {
        tr(en, "studio.failed").to_string()
    }
}

struct UiState {
    window: ApplicationWindow,
    core: Arc<CoreRuntime>,
    prefs: RefCell<UiPreferences>,
    active_notebook_id: RefCell<String>,
    studio_fullscreen: RefCell<bool>,
    sources_box: GtkBox,
    chat_box: GtkBox,
    suggested_box: FlowBox,
    artifacts_box: GtkBox,
    summary_label: Label,
    status_label: Label,
    studio_stack: Stack,
    studio_detail_box: GtkBox,
    left_panel: GtkBox,
    center_panel: GtkBox,
    outer_paned: Paned,
    inner_paned: Paned,
    // Localizable labels
    new_nb_btn: Button,
    settings_btn: Button,
    src_hdr: Label,
    add_src_btn: MenuButton,
    add_note_btn: Button,
    chat_entry: Entry,
    send_btn: Button,
    tile_labels: Vec<(String, Label)>,
}

impl UiState {
    fn is_en(&self) -> bool {
        self.prefs.borrow().is_english()
    }

    fn apply_preferences(&self) {
        let p = self.prefs.borrow().clone();
        let en = p.is_english();
        if p.theme == "light" {
            self.window.add_css_class("lna-light");
        } else {
            self.window.remove_css_class("lna-light");
        }
        self.new_nb_btn.set_label(tr(en, "nb.new"));
        self.settings_btn.set_label(tr(en, "settings.title"));
        self.src_hdr.set_text(tr(en, "sources.title"));
        self.add_src_btn.set_label(tr(en, "sources.add"));
        self.add_note_btn.set_label(tr(en, "sources.create_note"));
        self.chat_entry.set_placeholder_text(Some(tr(en, "chat.placeholder")));
        self.send_btn.set_label(tr(en, "chat.send"));
        for (kind, lbl) in &self.tile_labels {
            lbl.set_text(tr(en, &format!("studio.{kind}")));
        }
    }
}

pub fn build_main_window(app: &Application, core: Arc<CoreRuntime>) {
    let prefs = call_core(&core, "settings.get", json!({ "key": "ui.preferences" }))
        .ok()
        .and_then(|r| r.get("value").cloned())
        .map(|v| UiPreferences::from_json(&v))
        .unwrap_or_default();
    let en = prefs.is_english();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Local Notebook Agent")
        .default_width(1360)
        .default_height(860)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 10);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(14);
    root.set_margin_end(14);

    // Top bar (unified Settings entry — Model & Embedding config live inside Settings)
    let header = GtkBox::new(Orientation::Horizontal, 10);
    let title_lbl = Label::new(Some("Local Notebook Agent"));
    title_lbl.set_hexpand(true);
    title_lbl.set_halign(gtk4::Align::Start);

    let new_nb_btn = Button::with_label(tr(en, "nb.new"));
    new_nb_btn.add_css_class("lna-pill-btn");
    let settings_btn = Button::with_label(tr(en, "settings.title"));
    settings_btn.add_css_class("lna-pill-btn");

    header.append(&title_lbl);
    header.append(&new_nb_btn);
    header.append(&settings_btn);
    root.append(&header);

    // Left panel: Sources
    let left_panel = GtkBox::new(Orientation::Vertical, 10);
    left_panel.add_css_class("lna-panel");
    left_panel.set_width_request(280);
    let src_hdr = Label::new(Some(tr(en, "sources.title")));
    src_hdr.set_halign(gtk4::Align::Start);
    left_panel.append(&src_hdr);

    let src_actions = GtkBox::new(Orientation::Horizontal, 8);
    let add_src_btn = MenuButton::builder().label(tr(en, "sources.add")).build();
    add_src_btn.add_css_class("lna-pill-btn");
    let add_popover = Popover::new();
    add_popover.set_position(PositionType::Bottom);
    let pop_list = GtkBox::new(Orientation::Vertical, 4);
    let add_paste_btn = Button::with_label(tr(en, "sources.add_paste"));
    let add_url_btn = Button::with_label(tr(en, "sources.add_url"));
    pop_list.append(&add_paste_btn);
    pop_list.append(&add_url_btn);
    add_popover.set_child(Some(&pop_list));
    add_src_btn.set_popover(Some(&add_popover));

    let add_note_btn = Button::with_label(tr(en, "sources.create_note"));
    add_note_btn.add_css_class("lna-pill-btn");
    src_actions.append(&add_src_btn);
    src_actions.append(&add_note_btn);
    left_panel.append(&src_actions);

    let sources_box = GtkBox::new(Orientation::Vertical, 6);
    let sources_scroll = ScrolledWindow::builder()
        .child(&sources_box)
        .vexpand(true)
        .build();
    left_panel.append(&sources_scroll);

    // Center panel: Chat
    let center_panel = GtkBox::new(Orientation::Vertical, 10);
    center_panel.add_css_class("lna-panel");
    center_panel.set_hexpand(true);

    let summary_label = Label::new(Some(tr(en, "chat.empty_summary")));
    summary_label.set_wrap(true);
    summary_label.set_xalign(0.0);
    let suggested_box = FlowBox::new();
    suggested_box.set_selection_mode(SelectionMode::None);
    suggested_box.set_row_spacing(6);
    suggested_box.set_column_spacing(8);
    suggested_box.set_homogeneous(false);
    let summary_card = GtkBox::new(Orientation::Vertical, 8);
    summary_card.add_css_class("lna-card");
    summary_card.append(&summary_label);
    summary_card.append(&suggested_box);
    center_panel.append(&summary_card);

    let chat_box = GtkBox::new(Orientation::Vertical, 12);
    let chat_scroll = ScrolledWindow::builder()
        .child(&chat_box)
        .vexpand(true)
        .build();
    center_panel.append(&chat_scroll);

    let input_row = GtkBox::new(Orientation::Horizontal, 8);
    let chat_entry = Entry::builder()
        .placeholder_text(tr(en, "chat.placeholder"))
        .hexpand(true)
        .build();
    let send_btn = Button::with_label(tr(en, "chat.send"));
    send_btn.add_css_class("lna-primary-btn");
    input_row.append(&chat_entry);
    input_row.append(&send_btn);
    center_panel.append(&input_row);

    // Right panel: Studio
    let right_panel = GtkBox::new(Orientation::Vertical, 10);
    right_panel.add_css_class("lna-panel");
    right_panel.set_width_request(340);

    let studio_stack = Stack::new();
    studio_stack.set_vexpand(true);

    let studio_home = GtkBox::new(Orientation::Vertical, 10);
    let st_hdr = Label::new(Some("Studio"));
    st_hdr.set_halign(gtk4::Align::Start);
    studio_home.append(&st_hdr);

    let tile_grid = Grid::new();
    tile_grid.set_row_spacing(8);
    tile_grid.set_column_spacing(8);
    tile_grid.set_column_homogeneous(true);

    let modalities = [
        "audio",
        "slides",
        "mindmap",
        "report",
        "flashcards",
        "quiz",
        "infographic",
        "knowledge_graph",
    ];
    let mut tile_labels = Vec::new();

    let artifacts_box = GtkBox::new(Orientation::Vertical, 8);
    let artifacts_scroll = ScrolledWindow::builder()
        .child(&artifacts_box)
        .vexpand(true)
        .build();
    studio_home.append(&tile_grid);
    studio_home.append(&artifacts_scroll);

    let studio_detail_box = GtkBox::new(Orientation::Vertical, 10);
    studio_stack.add_named(&studio_home, Some("home"));
    studio_stack.add_named(&studio_detail_box, Some("detail"));
    studio_stack.set_visible_child_name("home");
    right_panel.append(&studio_stack);

    let inner_paned = Paned::new(Orientation::Horizontal);
    inner_paned.set_start_child(Some(&center_panel));
    inner_paned.set_end_child(Some(&right_panel));
    inner_paned.set_position(660);

    let outer_paned = Paned::new(Orientation::Horizontal);
    outer_paned.set_start_child(Some(&left_panel));
    outer_paned.set_end_child(Some(&inner_paned));
    outer_paned.set_position(300);
    outer_paned.set_vexpand(true);
    root.append(&outer_paned);

    let status_label = Label::new(Some(tr(en, "status.ready")));
    status_label.add_css_class("lna-muted");
    status_label.set_halign(gtk4::Align::Start);
    root.append(&status_label);

    for (idx, kind) in modalities.iter().enumerate() {
        let lbl = Label::new(Some(tr(en, &format!("studio.{kind}"))));
        tile_labels.push(((*kind).to_string(), lbl));
        let _ = idx;
    }

    let state = Rc::new(UiState {
        window: window.clone(),
        core: core.clone(),
        prefs: RefCell::new(prefs),
        active_notebook_id: RefCell::new(String::new()),
        studio_fullscreen: RefCell::new(false),
        sources_box,
        chat_box,
        suggested_box,
        artifacts_box,
        summary_label,
        status_label,
        studio_stack,
        studio_detail_box,
        left_panel,
        center_panel,
        outer_paned,
        inner_paned,
        new_nb_btn: new_nb_btn.clone(),
        settings_btn: settings_btn.clone(),
        src_hdr,
        add_src_btn,
        add_note_btn: add_note_btn.clone(),
        chat_entry: chat_entry.clone(),
        send_btn: send_btn.clone(),
        tile_labels,
    });
    state.apply_preferences();

    // Wire 7 Studio tiles
    for (idx, (kind, lbl)) in state.tile_labels.iter().enumerate() {
        let btn = Button::new();
        btn.add_css_class("lna-studio-tile");
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.append(&studio_icon_image(kind));
        row.append(lbl);
        btn.set_child(Some(&row));
        let st = state.clone();
        let k = kind.clone();
        btn.connect_clicked(move |_| {
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() {
                return;
            }
            let dlang = st.prefs.borrow().default_studio_lang.clone();
            let mut cfg = if k == "slides" {
                json!({ "pages": 8, "reviewOutline": true })
            } else if k == "knowledge_graph" {
                json!({ "mode": "knowledge_graph" })
            } else {
                json!({})
            };
            let actual_type = if k == "knowledge_graph" { "mindmap" } else { k.as_str() };
            if !dlang.is_empty() {
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert("lang".into(), json!(dlang));
                }
            }
            let en = st.is_en();
            match call_core(
                &st.core,
                "studio.create",
                json!({ "notebook_id": nid, "artifact_type": actual_type, "config": cfg }),
            ) {
                Ok(_) => {
                    st.status_label.set_text(tr(en, "studio.submitted"));
                    refresh_all(&st);
                }
                Err(e) => st.status_label.set_text(&friendly_err(en, e)),
            }
        });
        tile_grid.attach(&btn, (idx % 2) as i32, (idx / 2) as i32, 1, 1);
    }

    // Wire Chat send
    {
        let st = state.clone();
        let entry = chat_entry.clone();
        send_btn.connect_clicked(move |_| {
            let text = entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            entry.set_text("");
            let nid = st.active_notebook_id.borrow().clone();
            let en = st.is_en();
            match call_core(&st.core, "chat.send", json!({ "notebook_id": nid, "text": text })) {
                Ok(_) => refresh_all(&st),
                Err(e) => st.status_label.set_text(&friendly_err(en, e)),
            }
        });
    }

    // Wire New Notebook
    {
        let st = state.clone();
        new_nb_btn.connect_clicked(move |_| {
            let en = st.is_en();
            if let Ok(nb) = call_core(&st.core, "notebook.create", json!({ "title": tr(en, "nb.new") })) {
                if let Some(id) = nb.get("id").and_then(Value::as_str) {
                    *st.active_notebook_id.borrow_mut() = id.to_string();
                    refresh_all(&st);
                }
            }
        });
    }

    // Wire Add Paste Text & Add Note
    {
        let st = state.clone();
        let pop = add_popover.clone();
        add_paste_btn.connect_clicked(move |_| {
            pop.popdown();
            let nid = st.active_notebook_id.borrow().clone();
            let _ = call_core(
                &st.core,
                "source.add_text",
                json!({ "notebook_id": nid, "title": "pasted-note.txt", "text": "Sample source content." }),
            );
            refresh_all(&st);
        });
    }
    {
        let st = state.clone();
        add_note_btn.connect_clicked(move |_| {
            let nid = st.active_notebook_id.borrow().clone();
            let _ = call_core(
                &st.core,
                "note.save",
                json!({ "notebook_id": nid, "title": "Note", "body_md": "" }),
            );
            refresh_all(&st);
        });
    }

    // Wire Unified Settings dialog
    {
        let st = state.clone();
        let win = window.clone();
        settings_btn.connect_clicked(move |_| open_settings_dialog(&win, &st));
    }

    // Initialize first notebook
    if let Ok(list) = call_core(&core, "notebook.list", json!({})) {
        if let Some(id) = list.pointer("/notebooks/0/id").and_then(Value::as_str) {
            *state.active_notebook_id.borrow_mut() = id.to_string();
        } else if let Ok(nb) = call_core(&core, "notebook.create", json!({ "title": tr(en, "nb.untitled") })) {
            if let Some(id) = nb.get("id").and_then(Value::as_str) {
                *state.active_notebook_id.borrow_mut() = id.to_string();
            }
        }
    }
    refresh_all(&state);

    window.set_child(Some(&root));
    window.present();
}

fn clear_box(b: &GtkBox) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

/// Smoothly animates pane positions when expanding or shrinking Studio fullscreen (~180ms cubic ease-out).
fn animate_fullscreen_toggle(st: &Rc<UiState>, to_fullscreen: bool) {
    let animate = st.prefs.borrow().fullscreen_animation;
    if !animate {
        st.left_panel.set_visible(!to_fullscreen);
        st.center_panel.set_visible(!to_fullscreen);
        if !to_fullscreen {
            st.outer_paned.set_position(300);
            st.inner_paned.set_position(360);
        }
        return;
    }

    let start_outer = if to_fullscreen { st.outer_paned.position() } else { 0 };
    let start_inner = if to_fullscreen { st.inner_paned.position() } else { 0 };
    let target_outer = if to_fullscreen { 0 } else { 300 };
    let target_inner = if to_fullscreen { 0 } else { 360 };

    st.left_panel.set_visible(true);
    st.center_panel.set_visible(true);

    let started = Instant::now();
    let duration_ms = 185.0_f64;
    let st_anim = st.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let t = (started.elapsed().as_secs_f64() * 1000.0 / duration_ms).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - t).powi(3);
        let cur_outer = (start_outer as f64 + (target_outer - start_outer) as f64 * ease) as i32;
        let cur_inner = (start_inner as f64 + (target_inner - start_inner) as f64 * ease) as i32;
        st_anim.outer_paned.set_position(cur_outer.max(0));
        st_anim.inner_paned.set_position(cur_inner.max(0));
        if t >= 1.0 {
            st_anim.left_panel.set_visible(!to_fullscreen);
            st_anim.center_panel.set_visible(!to_fullscreen);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn refresh_all(st: &Rc<UiState>) {
    let nid = st.active_notebook_id.borrow().clone();
    if nid.is_empty() {
        return;
    }
    let en = st.is_en();

    if let Ok(nb) = call_core(&st.core, "notebook.get", json!({ "notebook_id": nid })) {
        let sum = nb
            .get("summary_md")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(tr(en, "chat.empty_summary"));
        st.summary_label.set_text(sum);

        while let Some(child) = st.suggested_box.first_child() {
            st.suggested_box.remove(&child);
        }
        if st.prefs.borrow().show_suggested_questions {
            if let Some(arr) = nb.get("suggested_questions").and_then(Value::as_array) {
                for q in arr.iter().take(6) {
                    if let Some(qs) = q.as_str() {
                        let btn = Button::with_label(qs);
                        btn.add_css_class("lna-pill-btn");
                        let st_q = st.clone();
                        let qtext = qs.to_string();
                        btn.connect_clicked(move |_| {
                            let nid2 = st_q.active_notebook_id.borrow().clone();
                            if call_core(&st_q.core, "chat.send", json!({ "notebook_id": nid2, "text": qtext })).is_ok() {
                                refresh_all(&st_q);
                            }
                        });
                        st.suggested_box.insert(&btn, -1);
                    }
                }
            }
        }
    }

    // Sources
    clear_box(&st.sources_box);
    if let Ok(res) = call_core(&st.core, "source.list", json!({ "notebook_id": nid })) {
        if let Some(arr) = res.get("sources").and_then(Value::as_array) {
            for s in arr {
                let sid = s.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let title = s.get("title").and_then(Value::as_str).unwrap_or("");
                let selected = s.get("is_selected").and_then(Value::as_bool).unwrap_or(true);
                let row = GtkBox::new(Orientation::Horizontal, 8);
                row.add_css_class("lna-source-row");
                let lbl = Label::new(Some(title));
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                let cb = CheckButton::new();
                cb.set_active(selected);
                let st2 = st.clone();
                cb.connect_toggled(move |c| {
                    let _ = call_core(
                        &st2.core,
                        "source.set_selected",
                        json!({ "source_id": sid, "selected": c.is_active() }),
                    );
                });
                row.append(&lbl);
                row.append(&cb);
                st.sources_box.append(&row);
                if s.get("recommend_knowledge_graph").and_then(Value::as_bool).unwrap_or(false) {
                    let kg_row = GtkBox::new(Orientation::Horizontal, 6);
                    kg_row.add_css_class("lna-kg-banner");
                    let kg_lbl = Label::new(Some(tr(en, "kg.recommended")));
                    kg_lbl.set_hexpand(true);
                    kg_lbl.set_xalign(0.0);
                    let kg_btn = Button::with_label(tr(en, "kg.build_btn"));
                    kg_btn.add_css_class("lna-primary-btn");
                    let st_kg = st.clone();
                    let s_title = title.to_string();
                    kg_btn.connect_clicked(move |_| {
                        let nid2 = st_kg.active_notebook_id.borrow().clone();
                        let _ = call_core(
                            &st_kg.core,
                            "studio.create",
                            json!({
                                "notebook_id": nid2,
                                "artifact_type": "mindmap",
                                "title": format!("{s_title} · {}", tr(st_kg.is_en(), "studio.knowledge_graph")),
                                "config": { "mode": "knowledge_graph", "topic": s_title }
                            }),
                        );
                        refresh_all(&st_kg);
                    });
                    kg_row.append(&kg_lbl);
                    kg_row.append(&kg_btn);
                    st.sources_box.append(&kg_row);
                }
            }
        }
    }

    // Chat messages
    clear_box(&st.chat_box);
    let expand_thoughts = st.prefs.borrow().expand_thoughts_by_default;
    if let Ok(res) = call_core(&st.core, "chat.history", json!({ "notebook_id": nid })) {
        if let Some(msgs) = res.get("messages").and_then(Value::as_array) {
            for m in msgs {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("assistant");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                let card = GtkBox::new(Orientation::Vertical, 6);
                let role_lbl = Label::new(Some(if role == "user" {
                    tr(en, "chat.you")
                } else {
                    tr(en, "chat.assistant")
                }));
                role_lbl.add_css_class("lna-muted");
                role_lbl.set_halign(gtk4::Align::Start);
                card.append(&role_lbl);

                if role == "assistant" {
                    if let Some(thoughts) = m.get("thoughts").and_then(Value::as_array) {
                        if !thoughts.is_empty() {
                            let t_btn = Button::new();
                            t_btn.add_css_class("lna-thoughts-btn");
                            let t_hdr = GtkBox::new(Orientation::Horizontal, 6);
                            t_hdr.append(&svg_image(SVG_RING, 14));
                            t_hdr.append(&Label::new(Some("Thoughts")));
                            t_btn.set_child(Some(&t_hdr));
                            t_btn.set_halign(gtk4::Align::Start);

                            let t_list = GtkBox::new(Orientation::Vertical, 8);
                            t_list.set_visible(expand_thoughts);
                            for th in thoughts {
                                let kind = th.get("kind").and_then(Value::as_str).unwrap_or("thought_stage");
                                if kind == "retry" {
                                    continue;
                                }
                                let title = th.get("title").and_then(Value::as_str).unwrap_or("");
                                let detail = th.get("detail").and_then(Value::as_str).unwrap_or("");
                                let is_tool = matches!(
                                    kind,
                                    "retrieve" | "tool_search" | "tool_read_context" | "tool_list_sources" | "tool_call"
                                );

                                let item = GtkBox::new(Orientation::Vertical, 4);
                                let hrow = GtkBox::new(Orientation::Horizontal, 8);
                                hrow.append(&svg_image(if is_tool { SVG_SEARCH_DOC } else { SVG_BRAIN }, 16));
                                let tl = Label::new(Some(title));
                                tl.set_xalign(0.0);
                                hrow.append(&tl);
                                item.append(&hrow);
                                if !is_tool && !detail.is_empty() {
                                    let dl = Label::new(Some(detail));
                                    dl.set_wrap(true);
                                    dl.set_xalign(0.0);
                                    dl.add_css_class("lna-thought-guide");
                                    item.append(&dl);
                                }
                                t_list.append(&item);
                            }
                            let list_ref = t_list.clone();
                            t_btn.connect_clicked(move |_| {
                                list_ref.set_visible(!list_ref.is_visible());
                            });
                            card.append(&t_btn);
                            card.append(&t_list);
                        }
                    }
                }

                let body_lbl = Label::new(Some(content));
                body_lbl.set_wrap(true);
                body_lbl.set_selectable(true);
                body_lbl.set_xalign(0.0);
                card.append(&body_lbl);
                st.chat_box.append(&card);
            }
        }
    }

    // Studio artifacts
    clear_box(&st.artifacts_box);
    if let Ok(res) = call_core(&st.core, "studio.list", json!({ "notebook_id": nid })) {
        if let Some(arr) = res.get("artifacts").and_then(Value::as_array) {
            for a in arr.iter().rev() {
                let aid = a.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let kind = a.get("artifact_type").and_then(Value::as_str).unwrap_or("report");
                let title = a.get("title").and_then(Value::as_str).unwrap_or("Artifact");
                let status = a.get("status").and_then(Value::as_str).unwrap_or("PENDING");
                let btn = Button::new();
                btn.add_css_class("lna-card");
                let row = GtkBox::new(Orientation::Horizontal, 10);
                row.append(&studio_icon_image(kind));
                let col = GtkBox::new(Orientation::Vertical, 2);
                let t = Label::new(Some(title));
                t.set_xalign(0.0);
                let sub = Label::new(Some(&format!("{} · {}", tr(en, &format!("studio.{kind}")), status)));
                sub.add_css_class("lna-muted");
                sub.set_xalign(0.0);
                col.append(&t);
                col.append(&sub);
                row.append(&col);
                btn.set_child(Some(&row));

                let st2 = st.clone();
                btn.connect_clicked(move |_| open_artifact_inline(&st2, &aid));
                st.artifacts_box.append(&btn);
            }
        }
    }
}

fn build_prompt_sources_button(st: &Rc<UiState>, art: &Value, artifact_id: &str) -> MenuButton {
    let en = st.is_en();
    let prompt_text = art
        .pointer("/config/topic")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let src_refs = art
        .get("source_refs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let src_count = if !src_refs.is_empty() {
        src_refs.len()
    } else {
        art.get("source_ids").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0)
    };

    let pill_label = if src_count > 0 {
        if en {
            format!("View prompt and {src_count} sources")
        } else {
            format!("查看提示和 {src_count} 个来源")
        }
    } else {
        if en { "View prompt".to_string() } else { "查看提示".to_string() }
    };

    let btn = MenuButton::builder().label(&pill_label).build();
    btn.add_css_class("lna-pill-btn");
    btn.set_halign(gtk4::Align::Start);

    let pop = Popover::new();
    pop.set_position(PositionType::Bottom);
    let body = GtkBox::new(Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_width_request(340);

    // Prompt header + Regenerate & Copy
    let p_hdr = GtkBox::new(Orientation::Horizontal, 8);
    let p_title = Label::new(Some(tr(en, "studio.prompt_header")));
    p_title.set_hexpand(true);
    p_title.set_xalign(0.0);
    let regen_btn = Button::with_label("↻");
    regen_btn.set_tooltip_text(Some(tr(en, "studio.regenerate")));
    regen_btn.add_css_class("lna-pill-btn");
    let st_rg = st.clone();
    let aid_rg = artifact_id.to_string();
    let cfg_rg = art.get("config").cloned().unwrap_or_else(|| json!({}));
    let pop_rg = pop.clone();
    regen_btn.connect_clicked(move |_| {
        pop_rg.popdown();
        let _ = call_core(
            &st_rg.core,
            "studio.regenerate",
            json!({ "artifact_id": aid_rg, "config": cfg_rg, "use_selected_sources": true }),
        );
        refresh_all(&st_rg);
    });

    let copy_btn = Button::with_label("📋");
    copy_btn.set_tooltip_text(Some(tr(en, "studio.copy_prompt")));
    copy_btn.add_css_class("lna-pill-btn");
    let copy_text = if prompt_text.is_empty() {
        art.get("title").and_then(Value::as_str).unwrap_or("").to_string()
    } else {
        prompt_text.clone()
    };
    let win_cp = st.window.clone();
    copy_btn.connect_clicked(move |_| {
        win_cp.clipboard().set_text(&copy_text);
    });

    p_hdr.append(&p_title);
    p_hdr.append(&regen_btn);
    p_hdr.append(&copy_btn);
    body.append(&p_hdr);

    let p_body = Label::new(Some(if prompt_text.is_empty() {
        tr(en, "studio.prompt_empty")
    } else {
        &prompt_text
    }));
    p_body.set_wrap(true);
    p_body.set_xalign(0.0);
    p_body.set_selectable(true);
    body.append(&p_body);

    // Sources section
    let s_title = Label::new(Some(tr(en, "studio.sources_header")));
    s_title.set_xalign(0.0);
    s_title.add_css_class("lna-muted");
    body.append(&s_title);

    let chips_box = GtkBox::new(Orientation::Vertical, 6);
    if !src_refs.is_empty() {
        for sr in &src_refs {
            let stitle = sr.get("title").and_then(Value::as_str).unwrap_or("");
            let skind = sr.get("kind").and_then(Value::as_str).unwrap_or("file").to_uppercase();
            let chip = Label::new(Some(&format!("[{skind}] {stitle}")));
            chip.add_css_class("lna-source-chip");
            chip.set_xalign(0.0);
            chip.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            chips_box.append(&chip);
        }
    } else {
        let empty = Label::new(Some(tr(en, "studio.sources_empty")));
        empty.add_css_class("lna-muted");
        empty.set_xalign(0.0);
        chips_box.append(&empty);
    }
    body.append(&chips_box);

    pop.set_child(Some(&body));
    btn.set_popover(Some(&pop));
    btn
}

fn open_artifact_inline(st: &Rc<UiState>, artifact_id: &str) {
    let Ok(art) = call_core(&st.core, "studio.get", json!({ "artifact_id": artifact_id })) else {
        return;
    };
    let en = st.is_en();
    clear_box(&st.studio_detail_box);

    // Header with title, prompt/sources pill, fullscreen & close buttons
    let hdr = GtkBox::new(Orientation::Horizontal, 8);
    let title_col = GtkBox::new(Orientation::Vertical, 4);
    title_col.set_hexpand(true);
    let title = art.get("title").and_then(Value::as_str).unwrap_or("Artifact");
    let tl = Label::new(Some(title));
    tl.set_xalign(0.0);
    title_col.append(&tl);
    title_col.append(&build_prompt_sources_button(st, &art, artifact_id));

    let fs_btn = Button::new();
    fs_btn.set_child(Some(&svg_image(SVG_FULLSCREEN, 16)));
    fs_btn.set_tooltip_text(Some(tr(en, "studio.fullscreen")));
    let st_fs = st.clone();
    fs_btn.connect_clicked(move |_| {
        let next = {
            let mut fs = st_fs.studio_fullscreen.borrow_mut();
            *fs = !*fs;
            *fs
        };
        animate_fullscreen_toggle(&st_fs, next);
    });

    let close_btn = Button::new();
    close_btn.set_child(Some(&svg_image(SVG_CLOSE, 16)));
    close_btn.set_tooltip_text(Some(tr(en, "studio.close")));
    let st_cl = st.clone();
    close_btn.connect_clicked(move |_| {
        *st_cl.studio_fullscreen.borrow_mut() = false;
        st_cl.left_panel.set_visible(true);
        st_cl.center_panel.set_visible(true);
        st_cl.studio_stack.set_visible_child_name("home");
        st_cl.outer_paned.set_position(300);
        st_cl.inner_paned.set_position(660);
    });

    hdr.append(&title_col);
    hdr.append(&fs_btn);
    hdr.append(&close_btn);
    st.studio_detail_box.append(&hdr);

    let status = art.get("status").and_then(Value::as_str).unwrap_or("");
    let kind = art.get("artifact_type").and_then(Value::as_str).unwrap_or("");

    if status == "FAILED" {
        let err_box = GtkBox::new(Orientation::Vertical, 10);
        err_box.add_css_class("lna-card");
        let code = art.get("error_code").and_then(Value::as_str).unwrap_or("");
        let msg = user_error_advice(en, code).unwrap_or(tr(en, "studio.failed"));
        let msg_lbl = Label::new(Some(msg));
        msg_lbl.set_wrap(true);
        msg_lbl.set_xalign(0.0);
        err_box.append(&msg_lbl);

        let retry_btn = Button::with_label(tr(en, "studio.retry"));
        retry_btn.add_css_class("lna-primary-btn");
        retry_btn.set_halign(gtk4::Align::Start);
        let st_r = st.clone();
        let aid_r = artifact_id.to_string();
        retry_btn.connect_clicked(move |_| {
            let _ = call_core(&st_r.core, "studio.retry", json!({ "artifact_id": aid_r }));
            refresh_all(&st_r);
        });
        err_box.append(&retry_btn);

        let error_log = art
            .get("error_log")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                format!(
                    "ArtifactId: {}\nErrorCode: {}\nDetail: {}",
                    artifact_id,
                    code,
                    art.get("error_detail").and_then(Value::as_str).unwrap_or("")
                )
            });
        let log_exp = Expander::new(Some(tr(en, "studio.diag_log")));
        let log_box = GtkBox::new(Orientation::Vertical, 6);
        let copy_log = Button::with_label(tr(en, "studio.copy_log"));
        copy_log.add_css_class("lna-pill-btn");
        copy_log.set_halign(gtk4::Align::Start);
        let log_for_cp = error_log.clone();
        let win_cp = st.window.clone();
        copy_log.connect_clicked(move |_| win_cp.clipboard().set_text(&log_for_cp));
        let log_lbl = Label::new(Some(&error_log));
        log_lbl.set_selectable(true);
        log_lbl.set_wrap(true);
        log_lbl.set_xalign(0.0);
        log_box.append(&copy_log);
        log_box.append(&ScrolledWindow::builder().child(&log_lbl).min_content_height(160).build());
        log_exp.set_child(Some(&log_box));
        err_box.append(&log_exp);

        st.studio_detail_box.append(&err_box);
    } else if status == "DRAFT_REVIEW" {
        let draft_md = art.get("draft").and_then(Value::as_str).unwrap_or("");
        let tv = TextView::new();
        tv.buffer().set_text(draft_md);
        tv.set_vexpand(true);
        let approve = Button::with_label(tr(en, "studio.approve_draft"));
        approve.add_css_class("lna-primary-btn");
        let st_ap = st.clone();
        let aid = artifact_id.to_string();
        let buf = tv.buffer();
        approve.connect_clicked(move |_| {
            let (s, e) = buf.bounds();
            let text = buf.text(&s, &e, false).to_string();
            let _ = call_core(
                &st_ap.core,
                "studio.approve_draft",
                json!({ "artifact_id": aid, "draft": text }),
            );
            refresh_all(&st_ap);
        });
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&tv).vexpand(true).build());
        st.studio_detail_box.append(&approve);
    } else if kind == "mindmap" {
        let payload = art.get("payload").unwrap_or(&Value::Null);
        let is_kg = art.pointer("/config/mode").and_then(Value::as_str) == Some("knowledge_graph")
            || payload.get("kind").and_then(Value::as_str) == Some("knowledge_graph");
        let st_link = st.clone();
        let on_action: Rc<dyn Fn(&str, &str)> = Rc::new(move |action, concept| {
            let nid2 = st_link.active_notebook_id.borrow().clone();
            if nid2.is_empty() { return; }
            match action {
                "quiz" => {
                    let _ = call_core(&st_link.core, "studio.create", json!({
                        "notebook_id": nid2, "artifact_type": "quiz",
                        "title": format!("Quiz: {concept}"),
                        "config": { "topic": concept, "count": 5 }
                    }));
                }
                "mindmap" => {
                    let _ = call_core(&st_link.core, "studio.create", json!({
                        "notebook_id": nid2, "artifact_type": "mindmap",
                        "title": format!("Mind Map: {concept}"),
                        "config": { "topic": concept }
                    }));
                }
                _ => {
                    let _ = call_core(&st_link.core, "chat.send", json!({
                        "notebook_id": nid2, "text": format!("Explain: {concept}")
                    }));
                }
            }
            refresh_all(&st_link);
        });
        st.studio_detail_box.append(&build_mindmap_widget(payload, en, is_kg, on_action));
    } else if kind == "report" {
        let split = GtkBox::new(Orientation::Horizontal, 12);
        split.set_vexpand(true);
        let toc = GtkBox::new(Orientation::Vertical, 4);
        toc.add_css_class("lna-card");
        toc.set_width_request(180);
        toc.append(&Label::new(Some(tr(en, "studio.toc"))));
        let body = GtkBox::new(Orientation::Vertical, 12);
        if let Some(secs) = art.pointer("/payload/sections").and_then(Value::as_array) {
            for (i, sec) in secs.iter().enumerate() {
                let h = sec.get("heading").and_then(Value::as_str).unwrap_or("");
                let b = sec.get("body_md").and_then(Value::as_str).unwrap_or("");
                let tb = Button::with_label(&format!("{}. {}", i + 1, h));
                tb.add_css_class("lna-toc-btn");
                toc.append(&tb);
                let hl = Label::new(Some(h));
                hl.set_xalign(0.0);
                let bl = Label::new(Some(b));
                bl.set_wrap(true);
                bl.set_xalign(0.0);
                body.append(&hl);
                body.append(&bl);
            }
        }
        split.append(&ScrolledWindow::builder().child(&toc).build());
        split.append(&ScrolledWindow::builder().child(&body).hexpand(true).vexpand(true).build());
        st.studio_detail_box.append(&split);
    } else if kind == "infographic" {
        let canvas = GtkBox::new(Orientation::Vertical, 10);
        canvas.add_css_class("lna-infographic-canvas");
        let sub = art.pointer("/payload/subtitle").and_then(Value::as_str).unwrap_or("");
        canvas.append(&Label::new(Some(sub)));
        if let Some(blocks) = art.pointer("/payload/blocks").and_then(Value::as_array) {
            for blk in blocks {
                let h = blk.get("heading").and_then(Value::as_str).unwrap_or("");
                let m = blk.get("metric").and_then(Value::as_str).unwrap_or("");
                let s = blk.get("summary").and_then(Value::as_str).unwrap_or("");
                let line = Label::new(Some(&format!("{h} — {m}\n{s}")));
                line.set_wrap(true);
                line.set_xalign(0.0);
                canvas.append(&line);
            }
        }
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&canvas).vexpand(true).build());
    } else {
        let preview = serde_json::to_string_pretty(art.get("payload").unwrap_or(&Value::Null)).unwrap_or_default();
        let lbl = Label::new(Some(&preview));
        lbl.set_wrap(true);
        lbl.set_selectable(true);
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&lbl).vexpand(true).build());
    }

    // Feedback bar for READY artifacts
    if status == "READY" {
        let fb_row = GtkBox::new(Orientation::Horizontal, 8);
        let good = Button::with_label(tr(en, "studio.good"));
        good.add_css_class("lna-pill-btn");
        let bad = Button::with_label(tr(en, "studio.bad"));
        bad.add_css_class("lna-pill-btn");
        let st_g = st.clone();
        let aid_g = artifact_id.to_string();
        good.connect_clicked(move |_| {
            let _ = call_core(&st_g.core, "studio.set_feedback", json!({ "artifact_id": aid_g, "feedback": 1 }));
        });
        let st_b = st.clone();
        let aid_b = artifact_id.to_string();
        bad.connect_clicked(move |_| {
            let _ = call_core(&st_b.core, "studio.set_feedback", json!({ "artifact_id": aid_b, "feedback": -1 }));
        });
        fb_row.append(&good);
        fb_row.append(&bad);
        st.studio_detail_box.append(&fb_row);
    }

    st.studio_stack.set_visible_child_name("detail");
    st.inner_paned.set_position(360);
}

fn open_settings_dialog(parent: &ApplicationWindow, st: &Rc<UiState>) {
    let en = st.is_en();
    let win = gtk4::Window::builder()
        .title(tr(en, "settings.title"))
        .transient_for(parent)
        .modal(true)
        .default_width(540)
        .default_height(500)
        .build();

    let tabs = Notebook::new();

    // --- Tab 1: Appearance ---
    let app_box = GtkBox::new(Orientation::Vertical, 12);
    app_box.set_margin_top(16);
    app_box.set_margin_bottom(16);
    app_box.set_margin_start(16);
    app_box.set_margin_end(16);

    let lang_dd = DropDown::new(
        Some(StringList::new(&["auto", "zh-CN", "en-US"])),
        None::<gtk4::Expression>,
    );
    lang_dd.set_selected(match st.prefs.borrow().language.as_str() {
        "zh-CN" => 1,
        "en-US" => 2,
        _ => 0,
    });

    let theme_dd = DropDown::new(
        Some(StringList::new(&["dark", "light"])),
        None::<gtk4::Expression>,
    );
    theme_dd.set_selected(if st.prefs.borrow().theme == "light" { 1 } else { 0 });

    let fs_anim_row = GtkBox::new(Orientation::Horizontal, 10);
    let fs_lbl = Label::new(Some(tr(en, "settings.fullscreen_anim")));
    fs_lbl.set_hexpand(true);
    fs_lbl.set_xalign(0.0);
    let fs_sw = Switch::new();
    fs_sw.set_active(st.prefs.borrow().fullscreen_animation);
    fs_anim_row.append(&fs_lbl);
    fs_anim_row.append(&fs_sw);

    app_box.append(&Label::new(Some(tr(en, "settings.language"))));
    app_box.append(&lang_dd);
    app_box.append(&Label::new(Some(tr(en, "settings.theme"))));
    app_box.append(&theme_dd);
    app_box.append(&fs_anim_row);

    // --- Tab 2: General ---
    let gen_box = GtkBox::new(Orientation::Vertical, 12);
    gen_box.set_margin_top(16);
    gen_box.set_margin_bottom(16);
    gen_box.set_margin_start(16);
    gen_box.set_margin_end(16);

    let exp_row = GtkBox::new(Orientation::Horizontal, 10);
    let exp_lbl = Label::new(Some(tr(en, "settings.expand_thoughts")));
    exp_lbl.set_hexpand(true);
    exp_lbl.set_xalign(0.0);
    let exp_sw = Switch::new();
    exp_sw.set_active(st.prefs.borrow().expand_thoughts_by_default);
    exp_row.append(&exp_lbl);
    exp_row.append(&exp_sw);

    let sug_row = GtkBox::new(Orientation::Horizontal, 10);
    let sug_lbl = Label::new(Some(tr(en, "settings.show_suggestions")));
    sug_lbl.set_hexpand(true);
    sug_lbl.set_xalign(0.0);
    let sug_sw = Switch::new();
    sug_sw.set_active(st.prefs.borrow().show_suggested_questions);
    sug_row.append(&sug_lbl);
    sug_row.append(&sug_sw);

    let save_prefs_btn = Button::with_label(tr(en, "settings.save"));
    save_prefs_btn.add_css_class("lna-primary-btn");
    let st_p = st.clone();
    let lang_dd_c = lang_dd.clone();
    let theme_dd_c = theme_dd.clone();
    let fs_sw_c = fs_sw.clone();
    let exp_sw_c = exp_sw.clone();
    let sug_sw_c = sug_sw.clone();
    save_prefs_btn.connect_clicked(move |_| {
        {
            let mut p = st_p.prefs.borrow_mut();
            p.language = match lang_dd_c.selected() {
                1 => "zh-CN".into(),
                2 => "en-US".into(),
                _ => "auto".into(),
            };
            p.theme = if theme_dd_c.selected() == 1 { "light".into() } else { "dark".into() };
            p.fullscreen_animation = fs_sw_c.is_active();
            p.expand_thoughts_by_default = exp_sw_c.is_active();
            p.show_suggested_questions = sug_sw_c.is_active();
            let _ = call_core(
                &st_p.core,
                "settings.set",
                json!({ "key": "ui.preferences", "value": p.to_json() }),
            );
        }
        st_p.apply_preferences();
        let en_now = st_p.is_en();
        let nid_tr = st_p.active_notebook_id.borrow().clone();
        if !nid_tr.is_empty() {
            let target_lang = if en_now { "en-US" } else { "zh-CN" };
            let _ = call_core(&st_p.core, "i18n.translate_notebook_content", json!({ "notebook_id": nid_tr, "target_lang": target_lang }));
        }
        st_p.status_label.set_text(tr(en_now, "settings.saved"));
        refresh_all(&st_p);
    });

    gen_box.append(&exp_row);
    gen_box.append(&sug_row);
    gen_box.append(&save_prefs_btn);

    // --- Tab 3: AI Models (moved from top bar) ---
    let model_box = GtkBox::new(Orientation::Vertical, 10);
    model_box.set_margin_top(16);
    model_box.set_margin_bottom(16);
    model_box.set_margin_start(16);
    model_box.set_margin_end(16);

    let proto_entry = Entry::builder()
        .text("chat_completion")
        .placeholder_text("chat_completion / responses / anthropic / gemini")
        .build();
    let url_entry = Entry::builder()
        .placeholder_text("Base URL (e.g. https://api.openai.com/v1)")
        .build();
    let model_entry = Entry::builder().placeholder_text("Model ID").build();
    let key_entry = Entry::builder()
        .placeholder_text("API Key")
        .visibility(false)
        .build();
    let save_model_btn = Button::with_label(tr(en, "settings.save"));
    save_model_btn.add_css_class("lna-primary-btn");

    model_box.append(&Label::new(Some("Protocol (chat_completion / responses / anthropic / gemini)")));
    model_box.append(&proto_entry);
    model_box.append(&Label::new(Some("Base URL")));
    model_box.append(&url_entry);
    model_box.append(&Label::new(Some("Model ID")));
    model_box.append(&model_entry);
    model_box.append(&Label::new(Some("API Key")));
    model_box.append(&key_entry);
    model_box.append(&save_model_btn);

    let st_m = st.clone();
    let w_m = win.clone();
    save_model_btn.connect_clicked(move |_| {
        let proto = proto_entry.text().to_string();
        let url = url_entry.text().to_string();
        let model = model_entry.text().to_string();
        let key = key_entry.text().to_string();
        let en_now = st_m.is_en();
        if let Ok(prof) = call_core(
            &st_m.core,
            "profile.upsert",
            json!({
                "display_name": format!("{proto} / {model}"),
                "protocol": proto,
                "base_url": url,
                "model_id": model,
                "activate": true
            }),
        ) {
            if let Some(kref) = prof.get("keychain_ref").and_then(Value::as_str) {
                if !key.is_empty() {
                    let _ = call_core(&st_m.core, "secret.set", json!({ "keychain_ref": kref, "secret": key }));
                }
            }
            st_m.status_label.set_text(tr(en_now, "settings.saved"));
            w_m.close();
        }
    });

    // --- Tab 4: Semantic Search ---
    let emb_box = GtkBox::new(Orientation::Vertical, 10);
    emb_box.set_margin_top(16);
    emb_box.set_margin_bottom(16);
    emb_box.set_margin_start(16);
    emb_box.set_margin_end(16);
    let emb_status = Label::new(Some(if en { "Local embedding model status" } else { "本地语义检索模型状态" }));
    emb_status.set_xalign(0.0);
    let dl_btn = Button::with_label(if en { "Download & Enable" } else { "下载并启用" });
    dl_btn.add_css_class("lna-primary-btn");
    let st_e = st.clone();
    dl_btn.connect_clicked(move |_| {
        let _ = call_core(&st_e.core, "embedding.download", json!({}));
    });
    emb_box.append(&emb_status);
    emb_box.append(&dl_btn);

    tabs.append_page(&app_box, Some(&Label::new(Some(tr(en, "settings.tab_appearance")))));
    tabs.append_page(&gen_box, Some(&Label::new(Some(tr(en, "settings.tab_general")))));
    tabs.append_page(&model_box, Some(&Label::new(Some(tr(en, "settings.tab_models")))));
    tabs.append_page(&emb_box, Some(&Label::new(Some(tr(en, "settings.tab_embedding")))));

    win.set_child(Some(&tabs));
    win.present();
}
