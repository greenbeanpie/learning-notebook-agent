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
    title_lbl: Entry,
    nb_switch_btn: MenuButton,
    nb_switch_popover_box: GtkBox,
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
        for cls in [
            "lna-light",
            "lna-midnight",
            "lna-forest",
            "lna-warm-paper",
            "lna-rose-dusk",
            "lna-font-14",
            "lna-font-15",
            "lna-font-17",
        ] {
            self.window.remove_css_class(cls);
        }
        let resolved_theme = if p.theme == "system" {
            let prefer_dark = gtk4::Settings::default()
                .map(|s| s.is_gtk_application_prefer_dark_theme())
                .unwrap_or(true);
            if prefer_dark { "dark" } else { "light" }
        } else {
            p.theme.as_str()
        };
        match resolved_theme {
            "light" => self.window.add_css_class("lna-light"),
            "midnight" => self.window.add_css_class("lna-midnight"),
            "forest" => self.window.add_css_class("lna-forest"),
            "warm_paper" => self.window.add_css_class("lna-warm-paper"),
            "rose_dusk" => self.window.add_css_class("lna-rose-dusk"),
            _ => {}
        }
        match p.font_size {
            14 => self.window.add_css_class("lna-font-14"),
            17 => self.window.add_css_class("lna-font-17"),
            _ => self.window.add_css_class("lna-font-15"),
        }
        self.nb_switch_btn.set_label(tr(en, "nb.switch"));
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

    // Top bar (synced with macOS & Windows: Logo + Title + Notebook Switcher + New + Settings + More)
    let header = GtkBox::new(Orientation::Horizontal, 10);
    let logo_img = svg_image(include_str!("../assets/logo.svg"), 26);
    // Editable notebook title: click to rename in place (Enter / focus loss commits, Esc reverts).
    let title_lbl = Entry::builder()
        .has_frame(false)
        .placeholder_text(tr(en, "nb.none_hint"))
        .tooltip_text(tr(en, "nb.title_edit_hint"))
        .width_chars(14)
        .max_width_chars(40)
        .build();
    title_lbl.add_css_class("lna-title-entry");

    let nb_switch_btn = MenuButton::builder().label(tr(en, "nb.switch")).build();
    nb_switch_btn.add_css_class("lna-ghost-btn");
    let nb_switch_popover = Popover::new();
    nb_switch_popover.set_position(PositionType::Bottom);
    let nb_switch_popover_box = GtkBox::new(Orientation::Vertical, 4);
    nb_switch_popover.set_child(Some(&nb_switch_popover_box));
    nb_switch_btn.set_popover(Some(&nb_switch_popover));

    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);

    let new_nb_btn = Button::with_label(tr(en, "nb.new"));
    new_nb_btn.add_css_class("lna-pill-btn");
    let settings_btn = Button::with_label(tr(en, "settings.title"));
    settings_btn.add_css_class("lna-pill-btn");

    let more_btn = MenuButton::builder().label("⋯").build();
    more_btn.add_css_class("lna-pill-btn");
    let more_popover = Popover::new();
    more_popover.set_position(PositionType::Bottom);
    let more_list = GtkBox::new(Orientation::Vertical, 4);
    let rename_nb_btn = Button::with_label(tr(en, "nb.rename"));
    let clear_chat_btn = Button::with_label(tr(en, "nb.clear_chat"));
    let export_lna_btn = Button::with_label(tr(en, "nb.export_lna"));
    let export_md_btn = Button::with_label(tr(en, "nb.export_md"));
    let import_lna_btn = Button::with_label(tr(en, "nb.import_lna"));
    let export_diag_btn = Button::with_label(tr(en, "nb.export_diag"));
    let privacy_btn = Button::with_label(tr(en, "nb.privacy"));
    let delete_nb_btn = Button::with_label(tr(en, "nb.delete"));
    for b in [
        &rename_nb_btn,
        &clear_chat_btn,
        &export_lna_btn,
        &export_md_btn,
        &import_lna_btn,
        &export_diag_btn,
        &privacy_btn,
        &delete_nb_btn,
    ] {
        b.add_css_class("lna-ghost-btn");
        more_list.append(b);
    }
    more_popover.set_child(Some(&more_list));
    more_btn.set_popover(Some(&more_popover));

    header.append(&logo_img);
    header.append(&title_lbl);
    header.append(&nb_switch_btn);
    header.append(&spacer);
    header.append(&new_nb_btn);
    header.append(&settings_btn);
    header.append(&more_btn);
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
        title_lbl: title_lbl.clone(),
        nb_switch_btn: nb_switch_btn.clone(),
        nb_switch_popover_box: nb_switch_popover_box.clone(),
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
            let nid_check = st.active_notebook_id.borrow().clone();
            if nid_check.is_empty() {
                return;
            }
            let en = st.is_en();
            let win = gtk4::Window::builder()
                .title(if en { format!("Generate {k}") } else { format!("生成配置 · {k}") })
                .default_width(480)
                .default_height(360)
                .modal(true)
                .build();
            let vbox = GtkBox::new(Orientation::Vertical, 10);
            vbox.set_margin_top(16);
            vbox.set_margin_bottom(16);
            vbox.set_margin_start(16);
            vbox.set_margin_end(16);

            let prompt_lbl = Label::new(Some(if en {
                "Prompt / Focus & Scope (Optional)"
            } else {
                "生成提示与范围要求（可选）"
            }));
            prompt_lbl.set_xalign(0.0);
            vbox.append(&prompt_lbl);

            let tv = gtk4::TextView::new();
            tv.set_wrap_mode(gtk4::WrapMode::WordChar);
            let tv_scroll = ScrolledWindow::builder()
                .child(&tv)
                .min_content_height(110)
                .vexpand(true)
                .build();
            vbox.append(&tv_scroll);

            let detail_combo = if k == "mindmap" || k == "knowledge_graph" {
                let dl = Label::new(Some(if en { "Detail Level" } else { "详尽程度" }));
                dl.set_xalign(0.0);
                vbox.append(&dl);
                let dd = gtk4::DropDown::from_strings(if en {
                    &["Concise (2-3 levels)", "Standard (3-4 levels)", "Detailed (4-5 levels)"]
                } else {
                    &["简洁（2~3 层概览）", "标准（3~4 层均衡）", "详尽（4~5 层细分）"]
                });
                dd.set_selected(1);
                vbox.append(&dd);
                Some(dd)
            } else if k == "report" {
                let dl = Label::new(Some(if en { "Depth" } else { "报告深度" }));
                dl.set_xalign(0.0);
                vbox.append(&dl);
                let dd = gtk4::DropDown::from_strings(if en {
                    &["Brief", "Standard", "Deep"]
                } else {
                    &["简明摘要", "标准报告", "深度解析"]
                });
                dd.set_selected(1);
                vbox.append(&dd);
                Some(dd)
            } else {
                None
            };

            let actions = GtkBox::new(Orientation::Horizontal, 8);
            actions.set_halign(gtk4::Align::End);
            let cancel_b = Button::with_label(if en { "Cancel" } else { "取消" });
            let ok_b = Button::with_label(if en { "Generate" } else { "开始生成" });
            ok_b.add_css_class("suggested-action");
            actions.append(&cancel_b);
            actions.append(&ok_b);
            vbox.append(&actions);
            win.set_child(Some(&vbox));

            let win_c = win.clone();
            cancel_b.connect_clicked(move |_| win_c.close());

            let st_ok = st.clone();
            let k_ok = k.clone();
            let win_ok = win.clone();
            ok_b.connect_clicked(move |_| {
                let buf = tv.buffer();
                let topic_txt = buf.text(&buf.start_iter(), &buf.end_iter(), false).trim().to_string();
                let nid = st_ok.active_notebook_id.borrow().clone();
                let dlang = st_ok.prefs.borrow().default_studio_lang.clone();
                let mut cfg = if k_ok == "quiz" {
                    json!({ "count": 5, "difficulty": "mixed", "types": ["single", "multi"] })
                } else if k_ok == "flashcards" {
                    json!({ "count": 10, "difficulty": "mixed" })
                } else if k_ok == "slides" {
                    json!({ "pages": 8, "reviewOutline": true })
                } else if k_ok == "knowledge_graph" {
                    json!({ "mode": "knowledge_graph" })
                } else {
                    json!({})
                };
                if let Some(obj) = cfg.as_object_mut() {
                    if !topic_txt.is_empty() {
                        obj.insert("topic".into(), json!(topic_txt));
                    }
                    if !dlang.is_empty() {
                        obj.insert("lang".into(), json!(dlang));
                    }
                    if let Some(dd) = detail_combo.as_ref() {
                        let sel = dd.selected();
                        if k_ok == "mindmap" || k_ok == "knowledge_graph" {
                            let dv = match sel { 0 => "concise", 2 => "detailed", _ => "standard" };
                            obj.insert("detail".into(), json!(dv));
                        } else if k_ok == "report" {
                            let dv = match sel { 0 => "brief", 2 => "deep", _ => "standard" };
                            obj.insert("depth".into(), json!(dv));
                        }
                    }
                }
                let actual_type = if k_ok == "knowledge_graph" { "mindmap" } else { k_ok.as_str() };
                let en2 = st_ok.is_en();
                match call_core(
                    &st_ok.core,
                    "studio.create",
                    json!({ "notebook_id": nid, "artifact_type": actual_type, "config": cfg }),
                ) {
                    Ok(_) => {
                        st_ok.status_label.set_text(tr(en2, "studio.submitted"));
                        refresh_all(&st_ok);
                    }
                    Err(e) => st_ok.status_label.set_text(&friendly_err(en2, e)),
                }
                win_ok.close();
            });
            win.present();
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

    // Wire New Notebook (opens the create dialog)
    {
        let st = state.clone();
        new_nb_btn.connect_clicked(move |_| open_create_notebook_dialog(&st));
    }

    // Wire inline title rename
    {
        let st = state.clone();
        title_lbl.connect_activate(move |_| {
            commit_title(&st);
            st.chat_entry.grab_focus();
        });
        let st = state.clone();
        let fc = gtk4::EventControllerFocus::new();
        fc.connect_leave(move |_| commit_title(&st));
        title_lbl.add_controller(fc);
        let st = state.clone();
        let kc = gtk4::EventControllerKey::new();
        kc.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                revert_title(&st);
                st.chat_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        title_lbl.add_controller(kc);
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

    // Wire notebook overflow menu actions
    {
        let st = state.clone();
        let pop = more_popover.clone();
        clear_chat_btn.connect_clicked(move |_| {
            pop.popdown();
            let nid = st.active_notebook_id.borrow().clone();
            if !nid.is_empty() {
                let _ = call_core(&st.core, "chat.clear", json!({ "notebook_id": nid }));
                refresh_all(&st);
            }
        });
    }
    {
        let st = state.clone();
        let pop = more_popover.clone();
        delete_nb_btn.connect_clicked(move |_| {
            pop.popdown();
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() { return; }
            let _ = call_core(&st.core, "notebook.delete", json!({ "notebook_id": nid }));
            if let Ok(list) = call_core(&st.core, "notebook.list", json!({})) {
                if let Some(id) = list.pointer("/notebooks/0/id").and_then(Value::as_str) {
                    *st.active_notebook_id.borrow_mut() = id.to_string();
                    refresh_all(&st);
                } else {
                    enter_empty_state(&st);
                    open_create_notebook_dialog(&st);
                }
            }
        });
    }
    {
        let st = state.clone();
        let pop = more_popover.clone();
        export_diag_btn.connect_clicked(move |_| {
            pop.popdown();
            let out_dir = dirs::download_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let out_path = out_dir.join("local-notebook-diagnostics.zip");
            if let Ok(_) = call_core(&st.core, "diagnostics.export", json!({ "output_path": out_path.to_string_lossy() })) {
                let en = st.is_en();
                st.status_label.set_text(&if en {
                    format!("Diagnostics exported to {}", out_path.display())
                } else {
                    format!("诊断日志已导出至 {}", out_path.display())
                });
            }
        });
    }
    {
        let st = state.clone();
        let win = window.clone();
        let pop = more_popover.clone();
        privacy_btn.connect_clicked(move |_| {
            pop.popdown();
            open_privacy_dialog(&win, st.is_en());
        });
    }
    {
        let st = state.clone();
        let win = window.clone();
        let pop = more_popover.clone();
        rename_nb_btn.connect_clicked(move |_| {
            pop.popdown();
            if st.active_notebook_id.borrow().is_empty() { return; }
            st.title_lbl.grab_focus();
            st.title_lbl.select_region(0, -1);
        });
    }
    {
        let st = state.clone();
        let pop = more_popover.clone();
        export_lna_btn.connect_clicked(move |_| {
            pop.popdown();
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() { return; }
            let out_dir = dirs::download_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let out_path = out_dir.join(format!("notebook-{nid}.lna"));
            if call_core(&st.core, "notebook.export", json!({ "notebook_id": nid, "output_path": out_path.to_string_lossy(), "format": "lna" })).is_ok() {
                let en = st.is_en();
                st.status_label.set_text(&if en {
                    format!("Exported to {}", out_path.display())
                } else {
                    format!("已导出至 {}", out_path.display())
                });
            }
        });
    }
    {
        let st = state.clone();
        let pop = more_popover.clone();
        export_md_btn.connect_clicked(move |_| {
            pop.popdown();
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() { return; }
            let out_dir = dirs::download_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let out_path = out_dir.join(format!("notebook-{nid}.md"));
            if call_core(&st.core, "notebook.export", json!({ "notebook_id": nid, "output_path": out_path.to_string_lossy(), "format": "markdown" })).is_ok() {
                let en = st.is_en();
                st.status_label.set_text(&if en {
                    format!("Exported to {}", out_path.display())
                } else {
                    format!("已导出至 {}", out_path.display())
                });
            }
        });
    }
    {
        let st = state.clone();
        let win = window.clone();
        let pop = more_popover.clone();
        import_lna_btn.connect_clicked(move |_| {
            pop.popdown();
            let en = st.is_en();
            let dlg = gtk4::FileChooserNative::new(
                Some(tr(en, "nb.import_lna")),
                Some(&win),
                gtk4::FileChooserAction::Open,
                None,
                None,
            );
            let st2 = st.clone();
            dlg.connect_response(move |d, resp| {
                if resp == gtk4::ResponseType::Accept {
                    if let Some(file) = d.file() {
                        if let Some(p) = file.path() {
                            if let Ok(res) = call_core(&st2.core, "notebook.import", json!({ "input_path": p.to_string_lossy() })) {
                                if let Some(new_id) = res.get("notebook_id").and_then(Value::as_str) {
                                    *st2.active_notebook_id.borrow_mut() = new_id.to_string();
                                    refresh_all(&st2);
                                }
                            }
                        }
                    }
                }
            });
            dlg.show();
        });
    }

    // Wire Unified Settings dialog
    {
        let st = state.clone();
        let win = window.clone();
        settings_btn.connect_clicked(move |_| open_settings_dialog(&win, &st));
    }

    // Initialize first notebook
    let mut need_create = false;
    if let Ok(list) = call_core(&core, "notebook.list", json!({})) {
        if let Some(id) = list.pointer("/notebooks/0/id").and_then(Value::as_str) {
            *state.active_notebook_id.borrow_mut() = id.to_string();
        } else {
            need_create = true;
        }
    }
    refresh_all(&state);

    window.set_child(Some(&root));
    window.present();

    if need_create {
        enter_empty_state(&state);
        let st = state.clone();
        glib::idle_add_local_once(move || open_create_notebook_dialog(&st));
    }
}

/// No notebook exists: clear the workspace so the user is asked to create one.
fn enter_empty_state(st: &Rc<UiState>) {
    st.active_notebook_id.borrow_mut().clear();
    st.title_lbl.set_text("");
    clear_box(&st.nb_switch_popover_box);
    clear_box(&st.sources_box);
    clear_box(&st.chat_box);
    clear_box(&st.artifacts_box);
    while let Some(c) = st.suggested_box.first_child() {
        st.suggested_box.remove(&c);
    }
    st.summary_label.set_text("");
}

fn current_notebook_title(st: &Rc<UiState>) -> String {
    let nid = st.active_notebook_id.borrow().clone();
    if nid.is_empty() {
        return String::new();
    }
    call_core(&st.core, "notebook.get", json!({ "notebook_id": nid }))
        .ok()
        .and_then(|nb| nb.get("title").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn revert_title(st: &Rc<UiState>) {
    let cur = current_notebook_title(st);
    st.title_lbl.set_text(&cur);
}

fn commit_title(st: &Rc<UiState>) {
    let nid = st.active_notebook_id.borrow().clone();
    if nid.is_empty() {
        return;
    }
    let name = st.title_lbl.text().trim().to_string();
    let cur = current_notebook_title(st);
    if name.is_empty() {
        st.title_lbl.set_text(&cur);
        return;
    }
    if name != cur {
        let _ = call_core(&st.core, "notebook.rename", json!({ "notebook_id": nid, "title": name }));
        refresh_all(st);
    }
}

fn rebuild_file_list(list: &GtkBox, files: &Rc<RefCell<Vec<String>>>) {
    clear_box(list);
    for path in files.borrow().iter() {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let lbl = Label::new(Some(&name));
        lbl.set_hexpand(true);
        lbl.set_xalign(0.0);
        lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        lbl.set_tooltip_text(Some(path));
        let rm = Button::with_label("✕");
        rm.add_css_class("lna-ghost-btn");
        let (list2, files2, path2) = (list.clone(), files.clone(), path.clone());
        rm.connect_clicked(move |_| {
            files2.borrow_mut().retain(|p| p != &path2);
            rebuild_file_list(&list2, &files2);
        });
        row.append(&lbl);
        row.append(&rm);
        list.append(&row);
    }
}

/// Modal "New Notebook" dialog: name, a note that details are AI-generated, optional source files.
fn open_create_notebook_dialog(st: &Rc<UiState>) {
    let en = st.is_en();
    let dlg = gtk4::Window::builder()
        .title(tr(en, "nb.new"))
        .transient_for(&st.window)
        .modal(true)
        .default_width(440)
        .build();
    let bx = GtkBox::new(Orientation::Vertical, 10);
    bx.set_margin_top(16);
    bx.set_margin_bottom(16);
    bx.set_margin_start(16);
    bx.set_margin_end(16);

    let name_lbl = Label::new(Some(tr(en, "nb.dlg_name")));
    name_lbl.set_xalign(0.0);
    let name_entry = Entry::builder().placeholder_text(tr(en, "nb.dlg_name_ph")).activates_default(true).build();
    let hint = Label::new(Some(tr(en, "nb.dlg_ai_hint")));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("lna-muted");
    let src_lbl = Label::new(Some(tr(en, "nb.dlg_sources")));
    src_lbl.set_xalign(0.0);
    let upload_btn = Button::with_label(tr(en, "nb.dlg_add_files"));
    upload_btn.set_halign(gtk4::Align::Start);
    let file_list = GtkBox::new(Orientation::Vertical, 4);
    let files: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let btn_row = GtkBox::new(Orientation::Horizontal, 8);
    btn_row.set_halign(gtk4::Align::End);
    let cancel_btn = Button::with_label(tr(en, "common.cancel"));
    let create_btn = Button::with_label(tr(en, "common.create"));
    create_btn.add_css_class("lna-primary-btn");
    create_btn.set_sensitive(false);
    btn_row.append(&cancel_btn);
    btn_row.append(&create_btn);

    for w in [
        name_lbl.upcast_ref::<gtk4::Widget>(),
        name_entry.upcast_ref(),
        hint.upcast_ref(),
        src_lbl.upcast_ref(),
        upload_btn.upcast_ref(),
        file_list.upcast_ref(),
        btn_row.upcast_ref(),
    ] {
        bx.append(w);
    }
    dlg.set_child(Some(&bx));
    dlg.set_default_widget(Some(&create_btn));

    {
        let create_btn = create_btn.clone();
        name_entry.connect_changed(move |e| create_btn.set_sensitive(!e.text().trim().is_empty()));
    }
    {
        let dlg = dlg.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }
    {
        let (win, file_list, files) = (dlg.clone(), file_list.clone(), files.clone());
        upload_btn.connect_clicked(move |_| {
            let chooser = gtk4::FileChooserNative::new(
                Some(tr(en, "nb.dlg_add_files")),
                Some(&win),
                gtk4::FileChooserAction::Open,
                None,
                None,
            );
            chooser.set_select_multiple(true);
            let keep = chooser.clone(); // keep the native dialog alive until it responds
            let (file_list, files) = (file_list.clone(), files.clone());
            chooser.connect_response(move |d, resp| {
                let _keep = &keep;
                if resp == gtk4::ResponseType::Accept {
                    let model = d.files();
                    for i in 0..model.n_items() {
                        if let Some(f) = model.item(i).and_then(|o| o.downcast::<gtk4::gio::File>().ok()) {
                            if let Some(p) = f.path() {
                                let p = p.to_string_lossy().to_string();
                                if !files.borrow().contains(&p) {
                                    files.borrow_mut().push(p);
                                }
                            }
                        }
                    }
                    rebuild_file_list(&file_list, &files);
                }
            });
            chooser.show();
        });
    }
    {
        let (st, dlg, files, name_entry) = (st.clone(), dlg.clone(), files.clone(), name_entry.clone());
        create_btn.connect_clicked(move |_| {
            let title = name_entry.text().trim().to_string();
            if title.is_empty() {
                return;
            }
            let en = st.is_en();
            match call_core(&st.core, "notebook.create", json!({ "title": title })) {
                Ok(nb) => {
                    if let Some(id) = nb.get("id").and_then(Value::as_str) {
                        *st.active_notebook_id.borrow_mut() = id.to_string();
                        refresh_all(&st);
                        let paths = files.borrow().clone();
                        if !paths.is_empty() {
                            match call_core(&st.core, "source.add_files", json!({ "notebook_id": id, "paths": paths })) {
                                Ok(_) => refresh_all(&st),
                                Err(e) => st.status_label.set_text(&friendly_err(en, e)),
                            }
                        }
                    }
                    dlg.close();
                }
                Err(e) => st.status_label.set_text(&friendly_err(en, e)),
            }
        });
    }
    dlg.present();
    name_entry.grab_focus();
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

    // Update notebook switcher popover & current title
    clear_box(&st.nb_switch_popover_box);
    if let Ok(list) = call_core(&st.core, "notebook.list", json!({})) {
        if let Some(arr) = list.get("notebooks").and_then(Value::as_array) {
            for nb_item in arr {
                let id = nb_item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let title = nb_item.get("title").and_then(Value::as_str).unwrap_or(tr(en, "nb.untitled"));
                if id == nid {
                    st.title_lbl.set_text(title);
                }
                let btn = Button::with_label(if id == nid {
                    format!("✓ {title}")
                } else {
                    title.to_string()
                });
                btn.add_css_class("lna-ghost-btn");
                let st2 = st.clone();
                btn.connect_clicked(move |_| {
                    *st2.active_notebook_id.borrow_mut() = id.clone();
                    if let Some(pop) = st2.nb_switch_btn.popover() {
                        pop.popdown();
                    }
                    refresh_all(&st2);
                });
                st.nb_switch_popover_box.append(&btn);
            }
        }
    }

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


fn build_thoughts_panel(art: &Value, en: bool) -> GtkBox {
    let box_w = GtkBox::new(Orientation::Vertical, 6);
    if let Some(arr) = art.get("thoughts").and_then(Value::as_array) {
        let items: Vec<&Value> = arr
            .iter()
            .filter(|t| t.get("kind").and_then(Value::as_str) != Some("retry"))
            .collect();
        if items.is_empty() {
            let empty = Label::new(Some(if en {
                "Waiting for AI workflow steps..."
            } else {
                "正在等待 AI 工作步骤与思考记录…"
            }));
            empty.set_xalign(0.0);
            empty.add_css_class("lna-muted");
            box_w.append(&empty);
        } else {
            for t in items {
                let kind = t.get("kind").and_then(Value::as_str).unwrap_or("stage");
                let title = t.get("title").and_then(Value::as_str).unwrap_or("");
                let body = t.get("body").and_then(Value::as_str).unwrap_or("");
                let card = GtkBox::new(Orientation::Vertical, 4);
                card.add_css_class("lna-card");
                let tag = match kind {
                    "tool" => if en { "[Tool] " } else { "[工具调用] " },
                    "reasoning" => if en { "[Thinking] " } else { "[思考总结] " },
                    "error" => if en { "[Error] " } else { "[错误] " },
                    _ => if en { "[Step] " } else { "[步骤] " },
                };
                let head = Label::new(Some(&format!("{tag}{title}")));
                head.set_xalign(0.0);
                head.set_wrap(true);
                card.append(&head);
                if !body.is_empty() {
                    let b = Label::new(Some(body));
                    b.set_xalign(0.0);
                    b.set_wrap(true);
                    b.set_selectable(true);
                    b.add_css_class("lna-muted");
                    card.append(&b);
                }
                box_w.append(&card);
            }
        }
    }
    box_w
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

    if status == "READY" {
        if art.get("thoughts").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false) {
            let exp = Expander::new(Some(if en { "AI Workflow & Thoughts" } else { "查看 AI 工具调用与思考过程" }));
            let list = build_thoughts_panel(&art, en);
            exp.set_child(Some(&ScrolledWindow::builder().child(&list).min_content_height(160).max_content_height(280).build()));
            st.studio_detail_box.append(&exp);
        }
    }

    if status == "PENDING" || status == "RUNNING" {
        let prog_box = GtkBox::new(Orientation::Vertical, 10);
        let stage = art.get("stage").and_then(Value::as_str).unwrap_or("PROCESSING");
        let pct = (art.get("progress").and_then(Value::as_f64).unwrap_or(0.0) * 100.0).round() as i32;
        let hdr_lbl = Label::new(Some(&format!(
            "{} · {} ({pct}%)",
            if en { "AI Workflow in Progress" } else { "AI 正在执行工作流" },
            stage
        )));
        hdr_lbl.set_xalign(0.0);
        prog_box.append(&hdr_lbl);
        let thoughts_list = build_thoughts_panel(&art, en);
        prog_box.append(&ScrolledWindow::builder().child(&thoughts_list).vexpand(true).min_content_height(260).build());
        let refresh_btn = Button::with_label(if en { "Refresh Progress" } else { "刷新实时进度" });
        refresh_btn.add_css_class("lna-pill-btn");
        refresh_btn.set_halign(gtk4::Align::Start);
        let st_ref = st.clone();
        let aid_ref = artifact_id.to_string();
        refresh_btn.connect_clicked(move |_| {
            open_artifact_inline(&st_ref, &aid_ref);
        });
        prog_box.append(&refresh_btn);
        st.studio_detail_box.append(&prog_box);
    } else if status == "FAILED" {
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
        let rev_row = GtkBox::new(Orientation::Horizontal, 6);
        let rev_entry = Entry::builder()
            .placeholder_text(if en {
                "Enter revision prompt to refine outline..."
            } else {
                "输入调整要求（如：拆分第3部分、补充案例），可反复让 AI 调整…"
            })
            .hexpand(true)
            .build();
        let rev_btn = Button::with_label(if en { "Revise with AI" } else { "AI 调整提纲" });
        rev_btn.add_css_class("lna-pill-btn");
        let st_rv = st.clone();
        let aid_rv = artifact_id.to_string();
        let buf_rv = tv.buffer();
        let rev_entry_c = rev_entry.clone();
        rev_btn.connect_clicked(move |_| {
            let instr = rev_entry_c.text().to_string();
            if instr.trim().is_empty() { return; }
            let (s, e) = buf_rv.bounds();
            let cur_draft = buf_rv.text(&s, &e, false).to_string();
            if let Ok(updated) = call_core(
                &st_rv.core,
                "studio.revise_draft",
                json!({ "artifact_id": aid_rv, "instruction": instr.trim(), "draft": cur_draft }),
            ) {
                if let Some(new_md) = updated.get("draft").and_then(Value::as_str) {
                    buf_rv.set_text(new_md);
                    rev_entry_c.set_text("");
                }
            }
        });
        rev_row.append(&rev_entry);
        rev_row.append(&rev_btn);
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&tv).vexpand(true).build());
        st.studio_detail_box.append(&rev_row);
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
    } else if kind == "slides" {
        let payload = art.get("payload").unwrap_or(&Value::Null);
        st.studio_detail_box.append(&crate::slides_view::build_slides_widget(payload, en));
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

fn open_privacy_dialog(parent: &ApplicationWindow, en: bool) {
    let dlg = gtk4::Window::builder()
        .title(tr(en, "nb.privacy"))
        .transient_for(parent)
        .modal(true)
        .default_width(480)
        .default_height(320)
        .build();
    let bx = GtkBox::new(Orientation::Vertical, 12);
    bx.set_margin_top(18);
    bx.set_margin_bottom(18);
    bx.set_margin_start(18);
    bx.set_margin_end(18);
    let body = Label::new(Some(if en {
        "• All notebooks, sources, embeddings, and chat history are stored locally on this device.\n\
         • API keys are stored in the system keyring (Secret Service).\n\
         • Only queries and selected excerpts sent during chat or Studio generation leave your device to your configured AI endpoint.\n\
         • Local semantic search embeddings run completely offline."
    } else {
        "• 所有笔记本、来源资料、本地向量索引与对话记录均存储在本地设备。\n\
         • API Key 保存在系统密钥环（Secret Service）中。\n\
         • 仅在发起对话、Studio 生成或使用云端视觉/音视频模型时，相关内容会发送至你配置的 AI 接口。\n\
         • 本地语义检索嵌入模型完全离线运行，不上传任何数据。"
    }));
    body.set_wrap(true);
    body.set_xalign(0.0);
    let close_btn = Button::with_label(tr(en, "studio.close"));
    close_btn.add_css_class("lna-primary-btn");
    let d2 = dlg.clone();
    close_btn.connect_clicked(move |_| d2.close());
    bx.append(&body);
    bx.append(&close_btn);
    dlg.set_child(Some(&bx));
    dlg.present();
}

fn open_settings_dialog(parent: &ApplicationWindow, st: &Rc<UiState>) {
    let en = st.is_en();
    let win = gtk4::Window::builder()
        .title(tr(en, "settings.title"))
        .transient_for(parent)
        .modal(true)
        .default_width(640)
        .default_height(620)
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

    let theme_ids = ["dark", "light", "system", "midnight", "forest", "warm_paper", "rose_dusk"];
    let theme_labels = if en {
        [
            "Obsidian Dark (Default)",
            "Paper Light",
            "Follow System",
            "Midnight Indigo",
            "Nordic Forest",
            "Warm Parchment",
            "Rose Dusk",
        ]
    } else {
        [
            "黑曜深色（默认）",
            "纸感浅色",
            "跟随系统",
            "午夜靛蓝",
            "北欧森绿",
            "暖纸羊皮",
            "暮色蔷薇",
        ]
    };
    let theme_dd = DropDown::new(
        Some(StringList::new(&theme_labels)),
        None::<gtk4::Expression>,
    );
    let cur_theme_idx = theme_ids
        .iter()
        .position(|&id| id == st.prefs.borrow().theme)
        .unwrap_or(0) as u32;
    theme_dd.set_selected(cur_theme_idx);

    let font_sizes = [14, 15, 17];
    let font_dd = DropDown::new(
        Some(StringList::new(&["14 px", "15 px", "17 px"])),
        None::<gtk4::Expression>,
    );
    let cur_font_idx = font_sizes
        .iter()
        .position(|&s| s == st.prefs.borrow().font_size)
        .unwrap_or(1) as u32;
    font_dd.set_selected(cur_font_idx);

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
    app_box.append(&Label::new(Some(tr(en, "settings.font_size"))));
    app_box.append(&font_dd);
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

    let studio_lang_dd = DropDown::new(
        Some(StringList::new(&[
            if en { "Follow UI language" } else { "跟随界面语言" },
            "简体中文",
            "English",
        ])),
        None::<gtk4::Expression>,
    );
    studio_lang_dd.set_selected(match st.prefs.borrow().default_studio_lang.as_str() {
        "zh-CN" => 1,
        "en-US" => 2,
        _ => 0,
    });

    let quiz_view_dd = DropDown::new(
        Some(StringList::new(&[
            if en { "Continuous scroll" } else { "连续滚动显示" },
            if en { "One question at a time" } else { "逐题展示" },
        ])),
        None::<gtk4::Expression>,
    );
    quiz_view_dd.set_selected(if st.prefs.borrow().quiz_view_mode == "one_by_one" { 1 } else { 0 });

    let quiz_submit_dd = DropDown::new(
        Some(StringList::new(&[
            if en { "Manual submit" } else { "手动提交答案" },
            if en { "Auto check on select" } else { "选择即自动判题" },
        ])),
        None::<gtk4::Expression>,
    );
    quiz_submit_dd.set_selected(if st.prefs.borrow().quiz_submit_mode == "auto" { 1 } else { 0 });

    let save_prefs_btn = Button::with_label(tr(en, "settings.save"));
    save_prefs_btn.add_css_class("lna-primary-btn");
    let st_p = st.clone();
    let lang_dd_c = lang_dd.clone();
    let theme_dd_c = theme_dd.clone();
    let font_dd_c = font_dd.clone();
    let fs_sw_c = fs_sw.clone();
    let exp_sw_c = exp_sw.clone();
    let sug_sw_c = sug_sw.clone();
    let studio_lang_dd_c = studio_lang_dd.clone();
    let quiz_view_dd_c = quiz_view_dd.clone();
    let quiz_submit_dd_c = quiz_submit_dd.clone();
    save_prefs_btn.connect_clicked(move |_| {
        {
            let mut p = st_p.prefs.borrow_mut();
            p.language = match lang_dd_c.selected() {
                1 => "zh-CN".into(),
                2 => "en-US".into(),
                _ => "auto".into(),
            };
            p.theme = theme_ids
                .get(theme_dd_c.selected() as usize)
                .copied()
                .unwrap_or("dark")
                .into();
            p.font_size = font_sizes
                .get(font_dd_c.selected() as usize)
                .copied()
                .unwrap_or(15);
            p.fullscreen_animation = fs_sw_c.is_active();
            p.expand_thoughts_by_default = exp_sw_c.is_active();
            p.show_suggested_questions = sug_sw_c.is_active();
            p.default_studio_lang = match studio_lang_dd_c.selected() {
                1 => "zh-CN".into(),
                2 => "en-US".into(),
                _ => String::new(),
            };
            p.quiz_view_mode = if quiz_view_dd_c.selected() == 1 {
                "one_by_one".into()
            } else {
                "continuous".into()
            };
            p.quiz_submit_mode = if quiz_submit_dd_c.selected() == 1 {
                "auto".into()
            } else {
                "manual".into()
            };
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

    let save_app_btn = Button::with_label(tr(en, "settings.save"));
    save_app_btn.add_css_class("lna-primary-btn");
    let save_prefs_btn_clone = save_prefs_btn.clone();
    save_app_btn.connect_clicked(move |_| {
        save_prefs_btn_clone.emit_clicked();
    });
    app_box.append(&save_app_btn);

    let gen_extra_row = GtkBox::new(Orientation::Horizontal, 8);
    let gen_privacy_btn = Button::with_label(tr(en, "nb.privacy"));
    gen_privacy_btn.add_css_class("lna-pill-btn");
    let gen_diag_btn = Button::with_label(tr(en, "nb.export_diag"));
    gen_diag_btn.add_css_class("lna-pill-btn");
    let win_priv = win.clone();
    let st_priv = st.clone();
    gen_privacy_btn.connect_clicked(move |_| open_privacy_dialog(parent, st_priv.is_en()));
    let _ = win_priv;
    let st_diag = st.clone();
    gen_diag_btn.connect_clicked(move |_| {
        let out_dir = dirs::download_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let out_path = out_dir.join("local-notebook-diagnostics.zip");
        if call_core(&st_diag.core, "diagnostics.export", json!({ "output_path": out_path.to_string_lossy() })).is_ok() {
            st_diag.status_label.set_text(&format!("Diagnostics: {}", out_path.display()));
        }
    });
    gen_extra_row.append(&gen_privacy_btn);
    gen_extra_row.append(&gen_diag_btn);

    gen_box.append(&exp_row);
    gen_box.append(&sug_row);
    gen_box.append(&Label::new(Some(tr(en, "settings.default_studio_lang"))));
    gen_box.append(&studio_lang_dd);
    gen_box.append(&Label::new(Some(if en { "Quiz view mode" } else { "测验显示方式" })));
    gen_box.append(&quiz_view_dd);
    gen_box.append(&Label::new(Some(if en { "Quiz submit mode" } else { "测验提交方式" })));
    gen_box.append(&quiz_submit_dd);
    gen_box.append(&gen_extra_row);
    gen_box.append(&save_prefs_btn);

    // --- Tab 3: AI Models (Full parity: Profiles, Presets, Discover, Modality Routing, Advanced, Test, Clear Key) ---
    let model_box = GtkBox::new(Orientation::Vertical, 10);
    model_box.set_margin_top(16);
    model_box.set_margin_bottom(16);
    model_box.set_margin_start(16);
    model_box.set_margin_end(16);

    let profiles_state: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(
        call_core(&st.core, "profile.list", json!({}))
            .ok()
            .and_then(|v| v.get("profiles").and_then(Value::as_array).cloned())
            .unwrap_or_default(),
    ));
    let current_profile_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let current_keychain_ref: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let profile_row = GtkBox::new(Orientation::Horizontal, 8);
    let profile_dd = DropDown::from_strings(&[if en { "— New Profile —" } else { "— 新建配置 —" }]);
    profile_dd.set_hexpand(true);
    let new_prof_btn = Button::with_label(if en { "New" } else { "新建" });
    new_prof_btn.add_css_class("lna-pill-btn");
    let del_prof_btn = Button::with_label(if en { "Delete" } else { "删除" });
    del_prof_btn.add_css_class("lna-pill-btn");
    profile_row.append(&profile_dd);
    profile_row.append(&new_prof_btn);
    profile_row.append(&del_prof_btn);

    let proto_ids = ["chat_completion", "responses", "anthropic", "gemini"];
    let proto_dd = DropDown::from_strings(&[
        "chat_completion (OpenAI Compatible)",
        "responses (OpenAI Responses)",
        "anthropic (Anthropic Messages)",
        "gemini (Gemini GenerateContent)",
    ]);
    let url_entry = Entry::builder()
        .placeholder_text("Base URL (e.g. https://api.openai.com/v1)")
        .build();
    let model_row = GtkBox::new(Orientation::Horizontal, 8);
    let model_entry = Entry::builder().placeholder_text("Model ID").hexpand(true).build();
    let discover_btn = Button::with_label(if en { "Fetch Models" } else { "获取模型列表" });
    discover_btn.add_css_class("lna-pill-btn");
    model_row.append(&model_entry);
    model_row.append(&discover_btn);

    let discovered_models: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let discovered_dd = DropDown::from_strings(&[if en { "Discovered models..." } else { "已获取的模型列表…" }]);
    discovered_dd.set_visible(false);

    let key_entry = Entry::builder()
        .placeholder_text("API Key")
        .visibility(false)
        .build();

    let insecure_http_sw = Switch::new();
    let insecure_row = GtkBox::new(Orientation::Horizontal, 8);
    let insecure_lbl = Label::new(Some(if en { "Allow non-loopback HTTP" } else { "允许非回环 HTTP 明文传输" }));
    insecure_lbl.set_hexpand(true);
    insecure_lbl.set_xalign(0.0);
    insecure_row.append(&insecure_lbl);
    insecure_row.append(&insecure_http_sw);

    let active_chat_sw = Switch::new();
    active_chat_sw.set_active(true);
    let active_row = GtkBox::new(Orientation::Horizontal, 8);
    let active_lbl = Label::new(Some(if en { "Set as default chat model" } else { "设为当前默认对话模型" }));
    active_lbl.set_hexpand(true);
    active_lbl.set_xalign(0.0);
    active_row.append(&active_lbl);
    active_row.append(&active_chat_sw);

    let mod_row = GtkBox::new(Orientation::Horizontal, 14);
    let vision_chk = CheckButton::with_label(if en { "Vision (images/PDF)" } else { "图文视觉" });
    let audio_chk = CheckButton::with_label(if en { "Audio" } else { "音频" });
    let video_chk = CheckButton::with_label(if en { "Video" } else { "视频" });
    mod_row.append(&vision_chk);
    mod_row.append(&audio_chk);
    mod_row.append(&video_chk);

    // Advanced Options Expander
    let adv_exp = Expander::new(Some(if en { "Advanced Options" } else { "高级选项" }));
    let adv_box = GtkBox::new(Orientation::Vertical, 8);
    let thinking_sw = Switch::new();
    thinking_sw.set_active(true);
    let thinking_row = GtkBox::new(Orientation::Horizontal, 8);
    let thinking_lbl = Label::new(Some(if en { "Show thinking process" } else { "显示模型思考过程" }));
    thinking_lbl.set_hexpand(true);
    thinking_lbl.set_xalign(0.0);
    thinking_row.append(&thinking_lbl);
    thinking_row.append(&thinking_sw);

    let effort_ids = ["", "low", "medium", "high"];
    let effort_dd = DropDown::from_strings(&[if en { "Default" } else { "默认" }, "low", "medium", "high"]);
    let ctx_entry = Entry::builder().text("32000").placeholder_text("Context Window").build();
    let out_entry = Entry::builder().text("8000").placeholder_text("Max Output Tokens").build();
    let price_in_entry = Entry::builder().placeholder_text("Input price ($/1M)").build();
    let price_out_entry = Entry::builder().placeholder_text("Output price ($/1M)").build();

    let extra_headers_rows: Rc<RefCell<Vec<(Entry, Entry)>>> = Rc::new(RefCell::new(Vec::new()));
    let headers_list = GtkBox::new(Orientation::Vertical, 6);
    let headers_hdr = GtkBox::new(Orientation::Horizontal, 8);
    let headers_lbl = Label::new(Some(if en { "Extra Headers" } else { "额外请求头" }));
    headers_lbl.set_hexpand(true);
    headers_lbl.set_xalign(0.0);
    let add_hdr_btn = Button::with_label(if en { "+ Add Header" } else { "+ 添加请求头" });
    add_hdr_btn.add_css_class("lna-pill-btn");
    let rows_add = extra_headers_rows.clone();
    let list_add = headers_list.clone();
    add_hdr_btn.connect_clicked(move |_| {
        let row_box = GtkBox::new(Orientation::Horizontal, 6);
        let k_ent = Entry::builder().placeholder_text(if en { "Header name" } else { "请求头名称" }).hexpand(true).build();
        let v_ent = Entry::builder().placeholder_text(if en { "Value" } else { "请求头值" }).hexpand(true).build();
        let del_btn = Button::with_label(if en { "Delete" } else { "删除" });
        del_btn.add_css_class("lna-pill-btn");
        row_box.append(&k_ent);
        row_box.append(&v_ent);
        row_box.append(&del_btn);
        list_add.append(&row_box);
        rows_add.borrow_mut().push((k_ent.clone(), v_ent.clone()));
        let rows_del = rows_add.clone();
        let list_del = list_add.clone();
        let row_del = row_box.clone();
        del_btn.connect_clicked(move |_| {
            rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
            list_del.remove(&row_del);
        });
    });
    headers_hdr.append(&headers_lbl);
    headers_hdr.append(&add_hdr_btn);

    adv_box.append(&thinking_row);
    adv_box.append(&Label::new(Some(if en { "Reasoning Effort" } else { "推理强度" })));
    adv_box.append(&effort_dd);
    adv_box.append(&Label::new(Some(if en { "Context Window / Max Output Tokens" } else { "上下文窗口 / 最大输出 Token" })));
    adv_box.append(&ctx_entry);
    adv_box.append(&out_entry);
    adv_box.append(&price_in_entry);
    adv_box.append(&price_out_entry);
    adv_box.append(&headers_hdr);
    adv_box.append(&headers_list);
    adv_exp.set_child(Some(&adv_box));

    // Media Processing & Per-Modality Model Assignment Card
    let media_card = GtkBox::new(Orientation::Vertical, 8);
    media_card.add_css_class("lna-card");
    let av_row = GtkBox::new(Orientation::Horizontal, 8);
    let av_lbl = Label::new(Some(if en {
        "Enable native audio/video processing"
    } else {
        "允许处理音视频文件（直接交给原生支持音视频的 AI 处理）"
    }));
    av_lbl.set_hexpand(true);
    av_lbl.set_xalign(0.0);
    av_lbl.set_wrap(true);
    let av_sw = Switch::new();
    av_row.append(&av_lbl);
    av_row.append(&av_sw);

    let media_val = call_core(&st.core, "settings.get", json!({ "key": "media_processing" }))
        .ok()
        .and_then(|r| r.get("value").cloned())
        .unwrap_or(Value::Null);
    let av_init = media_val.get("audio_video_enabled").and_then(Value::as_bool).unwrap_or(true);
    av_sw.set_active(av_init);

    let vision_prof_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![String::new()]));
    let audio_prof_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![String::new()]));
    let video_prof_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![String::new()]));
    let vision_mod_dd = DropDown::from_strings(&[if en { "Auto (prefer default chat)" } else { "自动（优先默认对话模型）" }]);
    let audio_mod_dd = DropDown::from_strings(&[if en { "Auto (prefer default chat)" } else { "自动（优先默认对话模型）" }]);
    let video_mod_dd = DropDown::from_strings(&[if en { "Auto (prefer default chat)" } else { "自动（优先默认对话模型）" }]);
    let routing_summary_lbl = Label::new(None);
    routing_summary_lbl.set_xalign(0.0);
    routing_summary_lbl.set_wrap(true);
    routing_summary_lbl.add_css_class("lna-muted");

    media_card.append(&av_row);
    media_card.append(&Label::new(Some(if en { "Vision (Image/PDF)" } else { "图文处理模型" })));
    media_card.append(&vision_mod_dd);
    media_card.append(&Label::new(Some(if en { "Audio" } else { "音频处理模型" })));
    media_card.append(&audio_mod_dd);
    media_card.append(&Label::new(Some(if en { "Video" } else { "视频处理模型" })));
    media_card.append(&video_mod_dd);
    media_card.append(&routing_summary_lbl);

    let model_status_lbl = Label::new(None);
    model_status_lbl.set_xalign(0.0);
    model_status_lbl.set_wrap(true);
    model_status_lbl.add_css_class("lna-muted");

    let model_actions = GtkBox::new(Orientation::Horizontal, 8);
    let save_model_btn = Button::with_label(tr(en, "settings.save"));
    save_model_btn.add_css_class("lna-primary-btn");
    let test_model_btn = Button::with_label(if en { "Test Connection" } else { "测试连接" });
    test_model_btn.add_css_class("lna-pill-btn");
    let clear_key_btn = Button::with_label(if en { "Clear Key" } else { "清除密钥" });
    clear_key_btn.add_css_class("lna-pill-btn");
    model_actions.append(&save_model_btn);
    model_actions.append(&test_model_btn);
    model_actions.append(&clear_key_btn);

    let presets_val = call_core(&st.core, "profile.presets", json!({}))
        .ok()
        .and_then(|v| v.get("presets").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut preset_labels: Vec<String> = vec![if en {
        "— Select Provider Preset —".into()
    } else {
        "— 选择 AI 提供商预设（快速填充）—".into()
    }];
    for pr in &presets_val {
        let lbl = if en {
            pr.get("name_en").and_then(Value::as_str)
        } else {
            pr.get("name").and_then(Value::as_str)
        }
        .unwrap_or("");
        preset_labels.push(lbl.to_string());
    }
    let preset_label_refs: Vec<&str> = preset_labels.iter().map(String::as_str).collect();
    let preset_dd = DropDown::from_strings(&preset_label_refs);

    let refresh_modality_dropdowns = {
        let profiles_state = profiles_state.clone();
        let vision_prof_ids = vision_prof_ids.clone();
        let audio_prof_ids = audio_prof_ids.clone();
        let video_prof_ids = video_prof_ids.clone();
        let vision_mod_dd = vision_mod_dd.clone();
        let audio_mod_dd = audio_mod_dd.clone();
        let video_mod_dd = video_mod_dd.clone();
        let av_sw = av_sw.clone();
        let routing_summary_lbl = routing_summary_lbl.clone();
        let core = st.core.clone();
        move || {
            let mv = call_core(&core, "settings.get", json!({ "key": "media_processing" }))
                .ok()
                .and_then(|r| r.get("value").cloned())
                .unwrap_or(Value::Null);
            let vp = mv.get("vision_profile_id").and_then(Value::as_str).unwrap_or("").to_string();
            let ap = mv.get("audio_profile_id").and_then(Value::as_str).unwrap_or("").to_string();
            let vidp = mv.get("video_profile_id").and_then(Value::as_str).unwrap_or("").to_string();
            let av_on = av_sw.is_active();
            audio_mod_dd.set_sensitive(av_on);
            video_mod_dd.set_sensitive(av_on);

            let auto_lbl = if en { "Auto (prefer default chat)" } else { "自动（优先默认对话模型）" };
            let profs = profiles_state.borrow();
            let fill_dd = |dd: &DropDown, ids_rc: &Rc<RefCell<Vec<String>>>, cap: &str, selected_id: &str| {
                let mut labels = vec![auto_lbl.to_string()];
                let mut ids = vec![String::new()];
                for p in profs.iter() {
                    if p.pointer(&format!("/capabilities/{cap}")).and_then(Value::as_bool).unwrap_or(false) {
                        ids.push(p.get("id").and_then(Value::as_str).unwrap_or("").to_string());
                        labels.push(p.get("display_name").and_then(Value::as_str).unwrap_or("").to_string());
                    }
                }
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                dd.set_model(Some(&StringList::new(&refs)));
                let sel_idx = ids.iter().position(|x| x == selected_id).unwrap_or(0) as u32;
                dd.set_selected(sel_idx);
                *ids_rc.borrow_mut() = ids;
            };
            fill_dd(&vision_mod_dd, &vision_prof_ids, "supports_vision", &vp);
            fill_dd(&audio_mod_dd, &audio_prof_ids, "supports_audio", &ap);
            fill_dd(&video_mod_dd, &video_prof_ids, "supports_video", &vidp);

            let chat_m = profs.iter().find(|p| p.get("is_active").and_then(Value::as_bool).unwrap_or(false));
            let chat_name = chat_m.and_then(|p| p.get("display_name").and_then(Value::as_str)).unwrap_or(if en { "Not configured" } else { "未配置" });
            let pick_mod = |exp_id: &str, cap: &str, empty_lbl: &str| -> String {
                if !exp_id.is_empty() {
                    if let Some(m) = profs.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(exp_id) && p.pointer(&format!("/capabilities/{cap}")).and_then(Value::as_bool).unwrap_or(false)) {
                        return m.get("display_name").and_then(Value::as_str).unwrap_or(empty_lbl).to_string();
                    }
                }
                if let Some(cm) = chat_m {
                    if cm.pointer(&format!("/capabilities/{cap}")).and_then(Value::as_bool).unwrap_or(false) {
                        return cm.get("display_name").and_then(Value::as_str).unwrap_or(empty_lbl).to_string();
                    }
                }
                profs.iter()
                    .find(|p| p.pointer(&format!("/capabilities/{cap}")).and_then(Value::as_bool).unwrap_or(false))
                    .and_then(|p| p.get("display_name").and_then(Value::as_str))
                    .unwrap_or(empty_lbl)
                    .to_string()
            };
            let summary = if en {
                format!(
                    "Current model routing:\n• Chat: {}\n• Vision: {}\n• Audio: {}\n• Video: {}",
                    chat_name,
                    pick_mod(&vp, "supports_vision", "None (local OCR fallback)"),
                    if av_on { pick_mod(&ap, "supports_audio", "Not assigned") } else { "Disabled".into() },
                    if av_on { pick_mod(&vidp, "supports_video", "Not assigned") } else { "Disabled".into() },
                )
            } else {
                format!(
                    "当前多模型分配：\n• 默认对话：{}\n• 图文处理：{}\n• 音频处理：{}\n• 视频处理：{}",
                    chat_name,
                    pick_mod(&vp, "supports_vision", "未指定（将尝试本地 OCR）"),
                    if av_on { pick_mod(&ap, "supports_audio", "未指定") } else { "已关闭".into() },
                    if av_on { pick_mod(&vidp, "supports_video", "未指定") } else { "已关闭".into() },
                )
            };
            routing_summary_lbl.set_text(&summary);
        }
    };

    let save_media_settings = {
        let core = st.core.clone();
        let av_sw = av_sw.clone();
        let vision_mod_dd = vision_mod_dd.clone();
        let audio_mod_dd = audio_mod_dd.clone();
        let video_mod_dd = video_mod_dd.clone();
        let vision_prof_ids = vision_prof_ids.clone();
        let audio_prof_ids = audio_prof_ids.clone();
        let video_prof_ids = video_prof_ids.clone();
        move || {
            let mut obj = serde_json::Map::new();
            obj.insert("audio_video_enabled".into(), Value::Bool(av_sw.is_active()));
            if let Some(id) = vision_prof_ids.borrow().get(vision_mod_dd.selected() as usize).filter(|s| !s.is_empty()) {
                obj.insert("vision_profile_id".into(), Value::String(id.clone()));
            }
            if let Some(id) = audio_prof_ids.borrow().get(audio_mod_dd.selected() as usize).filter(|s| !s.is_empty()) {
                obj.insert("audio_profile_id".into(), Value::String(id.clone()));
            }
            if let Some(id) = video_prof_ids.borrow().get(video_mod_dd.selected() as usize).filter(|s| !s.is_empty()) {
                obj.insert("video_profile_id".into(), Value::String(id.clone()));
            }
            let _ = call_core(&core, "settings.set", json!({ "key": "media_processing", "value": Value::Object(obj) }));
        }
    };

    {
        let sm = save_media_settings.clone();
        let rm = refresh_modality_dropdowns.clone();
        av_sw.connect_state_set(move |_, _| {
            sm();
            rm();
            glib::Propagation::Proceed
        });
    }
    for dd in [&vision_mod_dd, &audio_mod_dd, &video_mod_dd] {
        let sm = save_media_settings.clone();
        let rm = refresh_modality_dropdowns.clone();
        dd.connect_selected_notify(move |_| {
            sm();
            rm();
        });
    }

    let populate_profile_form = {
        let current_profile_id = current_profile_id.clone();
        let current_keychain_ref = current_keychain_ref.clone();
        let proto_dd = proto_dd.clone();
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let key_entry = key_entry.clone();
        let insecure_http_sw = insecure_http_sw.clone();
        let active_chat_sw = active_chat_sw.clone();
        let vision_chk = vision_chk.clone();
        let audio_chk = audio_chk.clone();
        let video_chk = video_chk.clone();
        let thinking_sw = thinking_sw.clone();
        let effort_dd = effort_dd.clone();
        let ctx_entry = ctx_entry.clone();
        let out_entry = out_entry.clone();
        let price_in_entry = price_in_entry.clone();
        let price_out_entry = price_out_entry.clone();
        let rows_p = extra_headers_rows.clone();
        let list_p = headers_list.clone();
        let model_status_lbl = model_status_lbl.clone();
        move |p_opt: Option<&Value>| {
            rows_p.borrow_mut().clear();
            clear_box(&list_p);
            key_entry.set_text("");
            if let Some(p) = p_opt {
                *current_profile_id.borrow_mut() = p.get("id").and_then(Value::as_str).map(String::from);
                *current_keychain_ref.borrow_mut() = p.get("keychain_ref").and_then(Value::as_str).unwrap_or("").to_string();
                let pr = p.get("protocol").and_then(Value::as_str).unwrap_or("chat_completion");
                proto_dd.set_selected(proto_ids.iter().position(|&x| x == pr).unwrap_or(0) as u32);
                url_entry.set_text(p.get("base_url").and_then(Value::as_str).unwrap_or(""));
                model_entry.set_text(p.get("model_id").and_then(Value::as_str).unwrap_or(""));
                insecure_http_sw.set_active(p.get("allow_insecure_http").and_then(Value::as_bool).unwrap_or(false));
                active_chat_sw.set_active(p.get("is_active").and_then(Value::as_bool).unwrap_or(false));
                vision_chk.set_active(p.pointer("/capabilities/supports_vision").and_then(Value::as_bool).unwrap_or(false));
                audio_chk.set_active(p.pointer("/capabilities/supports_audio").and_then(Value::as_bool).unwrap_or(false));
                video_chk.set_active(p.pointer("/capabilities/supports_video").and_then(Value::as_bool).unwrap_or(false));
                thinking_sw.set_active(p.get("show_thinking").and_then(Value::as_bool).unwrap_or(true));
                let eff = p.get("effort").and_then(Value::as_str).unwrap_or("");
                effort_dd.set_selected(effort_ids.iter().position(|&x| x == eff).unwrap_or(0) as u32);
                ctx_entry.set_text(&p.get("context_window").and_then(Value::as_i64).unwrap_or(32000).to_string());
                out_entry.set_text(&p.get("max_output_tokens").and_then(Value::as_i64).unwrap_or(8000).to_string());
                price_in_entry.set_text(&p.get("input_price_per_m").and_then(Value::as_f64).map(|x| x.to_string()).unwrap_or_default());
                price_out_entry.set_text(&p.get("output_price_per_m").and_then(Value::as_f64).map(|x| x.to_string()).unwrap_or_default());
                if let Some(hdrs) = p.get("extra_headers").and_then(Value::as_object) {
                    for (hk, hv) in hdrs {
                        let row_box = GtkBox::new(Orientation::Horizontal, 6);
                        let k_ent = Entry::builder().text(hk).hexpand(true).build();
                        let v_ent = Entry::builder().text(hv.as_str().unwrap_or("")).hexpand(true).build();
                        let del_btn = Button::with_label(if en { "Delete" } else { "删除" });
                        del_btn.add_css_class("lna-pill-btn");
                        row_box.append(&k_ent);
                        row_box.append(&v_ent);
                        row_box.append(&del_btn);
                        list_p.append(&row_box);
                        rows_p.borrow_mut().push((k_ent.clone(), v_ent.clone()));
                        let rows_del = rows_p.clone();
                        let list_del = list_p.clone();
                        let row_del = row_box.clone();
                        del_btn.connect_clicked(move |_| {
                            rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
                            list_del.remove(&row_del);
                        });
                    }
                }
                let has_sec = p.get("has_secret").and_then(Value::as_bool).unwrap_or(false);
                model_status_lbl.set_text(if has_sec {
                    if en { "Key status: saved in system keyring." } else { "密钥状态：已保存在系统密钥环。" }
                } else {
                    if en { "Key status: not saved." } else { "密钥状态：未保存。" }
                });
            } else {
                *current_profile_id.borrow_mut() = None;
                *current_keychain_ref.borrow_mut() = String::new();
                proto_dd.set_selected(0);
                url_entry.set_text("");
                model_entry.set_text("");
                insecure_http_sw.set_active(false);
                active_chat_sw.set_active(false);
                vision_chk.set_active(false);
                audio_chk.set_active(false);
                video_chk.set_active(false);
                thinking_sw.set_active(true);
                effort_dd.set_selected(0);
                ctx_entry.set_text("32000");
                out_entry.set_text("8000");
                price_in_entry.set_text("");
                price_out_entry.set_text("");
                model_status_lbl.set_text(if en {
                    "Editing new profile. Choose a preset or fill in details and save."
                } else {
                    "正在编辑新配置，可选择预设或手动填写后保存。"
                });
            }
        }
    };

    let reload_profile_dropdown = {
        let profiles_state = profiles_state.clone();
        let profile_dd = profile_dd.clone();
        let core = st.core.clone();
        let populate = populate_profile_form.clone();
        let refresh_mods = refresh_modality_dropdowns.clone();
        move |select_id: Option<&str>| {
            let profs = call_core(&core, "profile.list", json!({}))
                .ok()
                .and_then(|v| v.get("profiles").and_then(Value::as_array).cloned())
                .unwrap_or_default();
            let mut labels = vec![if en { "— New Profile —".to_string() } else { "— 新建配置 —".to_string() }];
            let mut target_idx = 0usize;
            for (i, p) in profs.iter().enumerate() {
                let id = p.get("id").and_then(Value::as_str).unwrap_or("");
                let name = p.get("display_name").and_then(Value::as_str).unwrap_or("");
                let act = if p.get("is_active").and_then(Value::as_bool).unwrap_or(false) {
                    if en { " (Default)" } else { "（默认对话）" }
                } else {
                    ""
                };
                labels.push(format!("{name}{act}"));
                if let Some(sid) = select_id {
                    if id == sid {
                        target_idx = i + 1;
                    }
                } else if p.get("is_active").and_then(Value::as_bool).unwrap_or(false) {
                    target_idx = i + 1;
                }
            }
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            profile_dd.set_model(Some(&StringList::new(&refs)));
            profile_dd.set_selected(target_idx as u32);
            *profiles_state.borrow_mut() = profs;
            let borrowed = profiles_state.borrow();
            if target_idx > 0 {
                populate(borrowed.get(target_idx - 1));
            } else {
                populate(None);
            }
            drop(borrowed);
            refresh_mods();
        }
    };
    reload_profile_dropdown(None);

    {
        let profiles_state = profiles_state.clone();
        let populate = populate_profile_form.clone();
        profile_dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let borrowed = profiles_state.borrow();
            if idx == 0 {
                populate(None);
            } else {
                populate(borrowed.get(idx - 1));
            }
        });
    }
    {
        let profile_dd = profile_dd.clone();
        let populate = populate_profile_form.clone();
        new_prof_btn.connect_clicked(move |_| {
            profile_dd.set_selected(0);
            populate(None);
        });
    }
    {
        let core = st.core.clone();
        let current_profile_id = current_profile_id.clone();
        let reload = reload_profile_dropdown.clone();
        del_prof_btn.connect_clicked(move |_| {
            let pid = current_profile_id.borrow().clone();
            if let Some(id) = pid {
                let _ = call_core(&core, "profile.delete", json!({ "profile_id": id }));
                reload(None);
            }
        });
    }

    {
        let proto_dd = proto_dd.clone();
        let url_p = url_entry.clone();
        let model_p = model_entry.clone();
        let ctx_p = ctx_entry.clone();
        let out_p = out_entry.clone();
        let v_p = vision_chk.clone();
        let a_p = audio_chk.clone();
        let vid_p = video_chk.clone();
        let rows_p = extra_headers_rows.clone();
        let list_p = headers_list.clone();
        preset_dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            if idx == 0 {
                return;
            }
            let Some(pr) = presets_val.get(idx - 1) else {
                return;
            };
            if let Some(p) = pr.get("protocol").and_then(Value::as_str) {
                proto_dd.set_selected(proto_ids.iter().position(|&x| x == p).unwrap_or(0) as u32);
            }
            if let Some(u) = pr.get("base_url").and_then(Value::as_str) {
                url_p.set_text(u);
            }
            if let Some(m) = pr.get("default_model").and_then(Value::as_str) {
                model_p.set_text(m);
            }
            if let Some(cw) = pr.get("context_window").and_then(Value::as_i64) {
                ctx_p.set_text(&cw.to_string());
            }
            if let Some(mo) = pr.get("max_output_tokens").and_then(Value::as_i64) {
                out_p.set_text(&mo.to_string());
            }
            v_p.set_active(pr.get("supports_vision").and_then(Value::as_bool).unwrap_or(false));
            a_p.set_active(pr.get("supports_audio").and_then(Value::as_bool).unwrap_or(false));
            vid_p.set_active(pr.get("supports_video").and_then(Value::as_bool).unwrap_or(false));
            rows_p.borrow_mut().clear();
            clear_box(&list_p);
            if let Some(hdrs) = pr.get("extra_headers").and_then(Value::as_object) {
                for (hk, hv) in hdrs {
                    let row_box = GtkBox::new(Orientation::Horizontal, 6);
                    let k_ent = Entry::builder().text(hk).hexpand(true).build();
                    let v_ent = Entry::builder()
                        .text(hv.as_str().unwrap_or(""))
                        .hexpand(true)
                        .build();
                    let del_btn = Button::with_label(if en { "Delete" } else { "删除" });
                    del_btn.add_css_class("lna-pill-btn");
                    row_box.append(&k_ent);
                    row_box.append(&v_ent);
                    row_box.append(&del_btn);
                    list_p.append(&row_box);
                    rows_p.borrow_mut().push((k_ent.clone(), v_ent.clone()));
                    let rows_del = rows_p.clone();
                    let list_del = list_p.clone();
                    let row_del = row_box.clone();
                    del_btn.connect_clicked(move |_| {
                        rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
                        list_del.remove(&row_del);
                    });
                }
            }
        });
    }

    {
        let core = st.core.clone();
        let current_profile_id = current_profile_id.clone();
        let proto_dd = proto_dd.clone();
        let url_entry = url_entry.clone();
        let key_entry = key_entry.clone();
        let discovered_models = discovered_models.clone();
        let discovered_dd = discovered_dd.clone();
        let model_status_lbl = model_status_lbl.clone();
        discover_btn.connect_clicked(move |_| {
            let pid = current_profile_id.borrow().clone().unwrap_or_default();
            let proto = proto_ids.get(proto_dd.selected() as usize).copied().unwrap_or("chat_completion");
            match call_core(
                &core,
                "profile.discover_models",
                json!({
                    "profile_id": pid,
                    "base_url": url_entry.text().trim(),
                    "protocol": proto,
                    "api_key": key_entry.text().trim(),
                }),
            ) {
                Ok(res) => {
                    let list: Vec<String> = res
                        .get("models")
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(String::from)).collect())
                        .unwrap_or_default();
                    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
                    discovered_dd.set_model(Some(&StringList::new(&refs)));
                    discovered_dd.set_visible(!list.is_empty());
                    model_status_lbl.set_text(&if en {
                        format!("Found {} models.", list.len())
                    } else {
                        format!("找到 {} 个模型。", list.len())
                    });
                    *discovered_models.borrow_mut() = list;
                }
                Err(e) => model_status_lbl.set_text(&friendly_err(en, e)),
            }
        });
    }
    {
        let discovered_models = discovered_models.clone();
        let model_entry = model_entry.clone();
        discovered_dd.connect_selected_notify(move |dd| {
            if let Some(m) = discovered_models.borrow().get(dd.selected() as usize) {
                model_entry.set_text(m);
            }
        });
    }

    model_box.append(&Label::new(Some(if en { "Saved Profiles" } else { "已保存的配置" })));
    model_box.append(&profile_row);
    model_box.append(&Label::new(Some(if en { "Provider Preset" } else { "AI 提供商预设" })));
    model_box.append(&preset_dd);
    model_box.append(&Label::new(Some(if en { "Protocol" } else { "协议" })));
    model_box.append(&proto_dd);
    model_box.append(&Label::new(Some("Base URL")));
    model_box.append(&url_entry);
    model_box.append(&Label::new(Some("Model ID")));
    model_box.append(&model_row);
    model_box.append(&discovered_dd);
    model_box.append(&Label::new(Some("API Key")));
    model_box.append(&key_entry);
    model_box.append(&insecure_row);
    model_box.append(&active_row);
    model_box.append(&mod_row);
    model_box.append(&adv_exp);
    model_box.append(&media_card);
    model_box.append(&model_actions);
    model_box.append(&model_status_lbl);

    let st_m = st.clone();
    let rows_save = extra_headers_rows.clone();
    let current_profile_id_s = current_profile_id.clone();
    let reload_s = reload_profile_dropdown.clone();
    let save_media_s = save_media_settings.clone();
    let model_status_s = model_status_lbl.clone();
    save_model_btn.connect_clicked(move |_| {
        let mut hdr_map = serde_json::Map::new();
        for (k_ent, v_ent) in rows_save.borrow().iter() {
            let k = k_ent.text().trim().to_string();
            if !k.is_empty() {
                hdr_map.insert(k, Value::String(v_ent.text().trim().to_string()));
            }
        }
        let proto = proto_ids.get(proto_dd.selected() as usize).copied().unwrap_or("chat_completion");
        let url = url_entry.text().trim().to_string();
        let model = model_entry.text().trim().to_string();
        let key = key_entry.text().to_string();
        let eff = effort_ids.get(effort_dd.selected() as usize).copied().unwrap_or("");
        let cw: i64 = ctx_entry.text().trim().parse().unwrap_or(32000);
        let mo: i64 = out_entry.text().trim().parse().unwrap_or(8000);
        let en_now = st_m.is_en();
        let mut payload = json!({
            "display_name": format!("{proto} / {model}"),
            "protocol": proto,
            "base_url": url,
            "model_id": model,
            "allow_insecure_http": insecure_http_sw.is_active(),
            "activate": active_chat_sw.is_active(),
            "show_thinking": thinking_sw.is_active(),
            "context_window": cw,
            "max_output_tokens": mo,
            "supports_vision": vision_chk.is_active(),
            "supports_audio": audio_chk.is_active(),
            "supports_video": video_chk.is_active(),
            "extra_headers": Value::Object(hdr_map),
        });
        if !eff.is_empty() {
            payload["effort"] = Value::String(eff.to_string());
        }
        if let Ok(pi) = price_in_entry.text().trim().parse::<f64>() {
            payload["input_price_per_m"] = json!(pi);
        }
        if let Ok(po) = price_out_entry.text().trim().parse::<f64>() {
            payload["output_price_per_m"] = json!(po);
        }
        if let Some(pid) = current_profile_id_s.borrow().as_ref() {
            payload["id"] = Value::String(pid.clone());
        }
        match call_core(&st_m.core, "profile.upsert", payload) {
            Ok(prof) => {
                let new_id = prof.get("id").and_then(Value::as_str).map(String::from);
                if let Some(kref) = prof.get("keychain_ref").and_then(Value::as_str) {
                    if !key.is_empty() {
                        let _ = call_core(&st_m.core, "secret.set", json!({ "keychain_ref": kref, "secret": key }));
                    }
                }
                if active_chat_sw.is_active() {
                    if let Some(ref nid) = new_id {
                        let _ = call_core(&st_m.core, "profile.activate", json!({ "profile_id": nid }));
                    }
                }
                save_media_s();
                reload_s(new_id.as_deref());
                model_status_s.set_text(tr(en_now, "settings.saved"));
                st_m.status_label.set_text(tr(en_now, "settings.saved"));
            }
            Err(e) => model_status_s.set_text(&friendly_err(en_now, e)),
        }
    });

    {
        let core = st.core.clone();
        let current_profile_id = current_profile_id.clone();
        let model_status_lbl = model_status_lbl.clone();
        test_model_btn.connect_clicked(move |_| {
            let Some(pid) = current_profile_id.borrow().clone() else {
                model_status_lbl.set_text(if en { "Save profile first." } else { "请先保存配置。" });
                return;
            };
            match call_core(&core, "profile.test", json!({ "profile_id": pid })) {
                Ok(res) => {
                    let reply = res.get("reply").and_then(Value::as_str).unwrap_or("OK");
                    model_status_lbl.set_text(&if en { format!("Connected: {reply}") } else { format!("连接成功：{reply}") });
                }
                Err(e) => model_status_lbl.set_text(&friendly_err(en, e)),
            }
        });
    }
    {
        let core = st.core.clone();
        let current_keychain_ref = current_keychain_ref.clone();
        let model_status_lbl = model_status_lbl.clone();
        clear_key_btn.connect_clicked(move |_| {
            let kref = current_keychain_ref.borrow().clone();
            if kref.is_empty() {
                return;
            }
            if call_core(&core, "secret.clear", json!({ "keychain_ref": kref })).is_ok() {
                model_status_lbl.set_text(if en { "API key cleared." } else { "密钥已清除。" });
            }
        });
    }

    let model_scroll = ScrolledWindow::builder().child(&model_box).vexpand(true).build();

    // --- Tab 4: Semantic Search ---
    let emb_box = GtkBox::new(Orientation::Vertical, 10);
    emb_box.set_margin_top(16);
    emb_box.set_margin_bottom(16);
    emb_box.set_margin_start(16);
    emb_box.set_margin_end(16);
    let emb_status_text = if let Ok(es) = call_core(&st.core, "embedding.status", json!({})) {
        let active = es.get("active").and_then(Value::as_bool).unwrap_or(false);
        let ready = es.get("ready_chunks").and_then(Value::as_i64).unwrap_or(0);
        let total = es.get("total_chunks").and_then(Value::as_i64).unwrap_or(0);
        if en {
            format!("Status: {} ({ready}/{total} chunks indexed)", if active { "Enabled" } else { "Not enabled" })
        } else {
            format!("状态：{}（已索引 {ready}/{total} 个片段）", if active { "已启用" } else { "未启用" })
        }
    } else if en {
        "Local embedding model status".into()
    } else {
        "本地语义检索模型状态".into()
    };
    let emb_status = Label::new(Some(&emb_status_text));
    emb_status.set_xalign(0.0);
    let emb_actions = GtkBox::new(Orientation::Horizontal, 8);
    let dl_btn = Button::with_label(if en { "Download & Enable" } else { "下载并启用" });
    dl_btn.add_css_class("lna-primary-btn");
    let cancel_dl_btn = Button::with_label(if en { "Cancel" } else { "取消" });
    cancel_dl_btn.add_css_class("lna-pill-btn");
    let rebuild_btn = Button::with_label(if en { "Rebuild Index" } else { "重建索引" });
    rebuild_btn.add_css_class("lna-pill-btn");
    let st_e = st.clone();
    dl_btn.connect_clicked(move |_| {
        let _ = call_core(&st_e.core, "embedding.download", json!({}));
    });
    let st_c = st.clone();
    cancel_dl_btn.connect_clicked(move |_| {
        let _ = call_core(&st_c.core, "embedding.download_cancel", json!({}));
    });
    let st_rb = st.clone();
    rebuild_btn.connect_clicked(move |_| {
        let _ = call_core(&st_rb.core, "embedding.rebuild", json!({ "force": true }));
    });
    emb_actions.append(&dl_btn);
    emb_actions.append(&cancel_dl_btn);
    emb_actions.append(&rebuild_btn);
    emb_box.append(&emb_status);
    emb_box.append(&emb_actions);

    tabs.append_page(&app_box, Some(&Label::new(Some(tr(en, "settings.tab_appearance")))));
    tabs.append_page(&gen_box, Some(&Label::new(Some(tr(en, "settings.tab_general")))));
    tabs.append_page(&model_scroll, Some(&Label::new(Some(tr(en, "settings.tab_models")))));
    tabs.append_page(&emb_box, Some(&Label::new(Some(tr(en, "settings.tab_embedding")))));

    win.set_child(Some(&tabs));
    win.present();
}
