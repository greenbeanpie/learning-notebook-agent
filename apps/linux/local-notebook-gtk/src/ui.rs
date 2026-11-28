use crate::attach::{self, AttachBar};
use crate::i18n::{tr, user_error_advice, UiPreferences, trf};
use crate::icons::{studio_icon_image, svg_image, SVG_BRAIN, SVG_CLOSE, SVG_FULLSCREEN, SVG_RING, SVG_SEARCH_DOC};
use crate::mindmap_view::{build_mindmap_widget, new_memory, ActionFn, SharedMemory};
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DropDown, Entry, Expander, FlowBox, SelectionMode,
    Grid, Label, MenuButton, Notebook, Orientation, Paned, Popover, PositionType, ScrolledWindow,
    Stack, StringList, Switch, TextView,
};
use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn call_core(core: &Arc<CoreRuntime>, method: &str, params: Value) -> Result<Value, (String, String)> {
    let req = json!({ "id": "gtk", "method": method, "params": params });
    let raw = core.handle_json(&req.to_string());
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| (String::new(), e.to_string()))?;
    if let Some(err) = parsed.get("error") {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("").to_string();
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        return Err((code, msg));
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

/// A plain `http://` URL to anything but this machine.
fn is_plaintext_remote(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    }
    .to_ascii_lowercase();
    !(host == "localhost" || host == "::1" || host.starts_with("127."))
}

fn friendly_err(en: bool, err: (String, String)) -> String {
    // a model refused for missing tool calling / structured output says exactly what it lacks
    if err.0 == "CAPABILITY_MISSING" && !err.1.is_empty() {
        return trf(en, "studiocfg.this_app_only_works_with", &[&(err.1)]);
    }
    if let Some(adv) = user_error_advice(en, &err.0) {
        adv.to_string()
    } else if !err.1.is_empty() {
        err.1
    } else {
        tr(en, "studio.failed").to_string()
    }
}

/// "Detected inputs: image ✓ / audio ✗ / video ✗"
fn detected_line(en: bool, vision: bool, audio: bool, video: bool) -> String {
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    trf(en, "model.detected_inputs", &[&mark(vision), &mark(audio), &mark(video)])
}

/// The read-only detected-inputs line for a stored profile (`None` for a new one). The core
/// writes the detection result into the profile's capabilities once it has probed it.
fn detected_inputs_text(en: bool, profile: Option<&Value>) -> String {
    let caps = profile.and_then(|p| p.get("capabilities"));
    let probed = caps.and_then(|c| c.get("probed_at")).filter(|v| !v.is_null()).is_some();
    if !probed {
        return tr(en, "model.detected_not_yet").to_string();
    }
    let flag = |k: &str| caps.and_then(|c| c.get(k)).and_then(Value::as_bool).unwrap_or(false);
    detected_line(en, flag("supports_vision"), flag("supports_audio"), flag("supports_video"))
}

/// A token limit shown in its entry: empty when unlimited (stored as 0 or missing).
fn limit_text(v: Option<&Value>) -> String {
    v.and_then(Value::as_i64).filter(|n| *n > 0).map(|n| n.to_string()).unwrap_or_default()
}

/// A token limit read from its entry: empty, invalid or negative means 0 = unlimited.
fn parse_limit(text: &str) -> i64 {
    text.trim().parse::<i64>().unwrap_or(0).max(0)
}

/// The user-facing name of a backend progress stage code.
fn stage_label(en: bool, stage: &str) -> &'static str {
    tr(
        en,
        match stage {
            "COLLECT" => "studio.stage_collect",
            "PLAN" => "studio.stage_plan",
            "GENERATE" => "studio.stage_generate",
            "RENDER" => "studio.stage_render",
            _ => "studio.stage_processing",
        },
    )
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
    /// Pictures, files and links attached to the next chat message.
    attach: AttachBar,
    /// Current UI language, read by the attachment widgets.
    en_cell: Rc<Cell<bool>>,
    /// Unsent chat text and attachments of the notebooks that are not open.
    drafts: RefCell<HashMap<String, (String, Vec<attach::Item>)>>,
    /// The notebook whose draft is in the chat box right now.
    draft_owner: RefCell<String>,
    /// The generating artifact shown in the detail pane, updated by core events.
    live: RefCell<Option<LiveView>>,
    /// Every streamed thought delta received for the open generating artifact; survives re-fetches.
    thought_streams: RefCell<StreamLog>,
    /// Receives the `ffmpeg.*` core events (set by the AI model settings page).
    ffmpeg_hook: RefCell<Option<Rc<dyn Fn(&str, &Value)>>>,
    /// Receives the `profile.updated` core event (set while the AI model settings page is open).
    profile_hook: RefCell<Option<Rc<dyn Fn(&str, &Value)>>>,
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
        self.en_cell.set(en);
        self.attach.relabel();
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
    // "New notebook" sits at the bottom of the notebook list as its own button.
    let new_nb_btn = Button::with_label(tr(en, "nb.new"));
    new_nb_btn.add_css_class("lna-pill-btn");
    let nb_switch_outer = GtkBox::new(Orientation::Vertical, 6);
    nb_switch_outer.append(&nb_switch_popover_box);
    nb_switch_outer.append(&gtk4::Separator::new(Orientation::Horizontal));
    nb_switch_outer.append(&new_nb_btn);
    nb_switch_popover.set_child(Some(&nb_switch_outer));
    nb_switch_btn.set_popover(Some(&nb_switch_popover));

    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);

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

    let status_label = Label::new(Some(tr(en, "status.ready")));
    status_label.add_css_class("lna-muted");
    status_label.set_halign(gtk4::Align::Start);

    let en_cell = Rc::new(Cell::new(en));
    let attach_bar = {
        let en_c = en_cell.clone();
        let status_c = status_label.clone();
        AttachBar::new(
            window.upcast_ref::<gtk4::Window>(),
            Rc::new(move || en_c.get()),
            Rc::new(move |m| status_c.set_text(m)),
        )
    };
    let input_row = GtkBox::new(Orientation::Horizontal, 8);
    let chat_entry = Entry::builder()
        .placeholder_text(tr(en, "chat.placeholder"))
        .hexpand(true)
        .build();
    let send_btn = Button::with_label(tr(en, "chat.send"));
    send_btn.add_css_class("lna-primary-btn");
    input_row.append(attach_bar.button());
    input_row.append(&chat_entry);
    input_row.append(&send_btn);
    let input_col = GtkBox::new(Orientation::Vertical, 6);
    input_col.append(attach_bar.chips());
    input_col.append(&input_row);
    attach_bar.accept_drops(&input_col);
    attach_bar.accept_paste(&input_col);
    center_panel.append(&input_col);

    // Right panel: Studio
    let right_panel = GtkBox::new(Orientation::Vertical, 10);
    right_panel.add_css_class("lna-panel");
    right_panel.set_width_request(340);

    let studio_stack = Stack::new();
    studio_stack.set_vexpand(true);

    let studio_home = GtkBox::new(Orientation::Vertical, 10);
    let st_hdr = Label::new(Some(tr(en, "studio.title")));
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
        "lecture",
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
        attach: attach_bar.clone(),
        en_cell: en_cell.clone(),
        drafts: RefCell::new(HashMap::new()),
        draft_owner: RefCell::new(String::new()),
        live: RefCell::new(None),
        thought_streams: RefCell::new(StreamLog::default()),
        ffmpeg_hook: RefCell::new(None),
        profile_hook: RefCell::new(None),
    });
    state.apply_preferences();
    start_event_pump(&state);

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
                .title(trf(en, "studiocfg.generate_2", &[&k]))
                .default_width(480)
                .default_height(360)
                .modal(true)
                .build();
            let vbox = GtkBox::new(Orientation::Vertical, 10);
            vbox.set_margin_top(16);
            vbox.set_margin_bottom(16);
            vbox.set_margin_start(16);
            vbox.set_margin_end(16);

            let prompt_lbl = Label::new(Some(if k == "lecture" {
                tr(en, "studio.cfg.lecture_topic_header")
            } else {
                tr(en, "studio.cfg.topic_header")
            }));
            prompt_lbl.set_xalign(0.0);
            vbox.append(&prompt_lbl);

            let ctx_bar = {
                let en_c = st.en_cell.clone();
                let status_c = st.status_label.clone();
                AttachBar::new(&win, Rc::new(move || en_c.get()), Rc::new(move |m| status_c.set_text(m)))
            };

            let tv = gtk4::TextView::new();
            tv.set_wrap_mode(gtk4::WrapMode::WordChar);
            let tv_scroll = ScrolledWindow::builder()
                .child(&tv)
                .min_content_height(110)
                .vexpand(true)
                .build();
            vbox.append(&tv_scroll);

            let detail_combo = if k == "mindmap" || k == "knowledge_graph" {
                let dl = Label::new(Some(tr(en, "studio.cfg.detail_header")));
                dl.set_xalign(0.0);
                vbox.append(&dl);
                let dd = gtk4::DropDown::from_strings(&[tr(en, "studiocfg.mm_concise"), tr(en, "studiocfg.mm_standard"), tr(en, "studiocfg.mm_detailed")]);
                dd.set_selected(1);
                vbox.append(&dd);
                Some(dd)
            } else if k == "report" {
                let dl = Label::new(Some(tr(en, "studiocfg.depth")));
                dl.set_xalign(0.0);
                vbox.append(&dl);
                let dd = gtk4::DropDown::from_strings(&[tr(en, "studiocfg.report_brief"), tr(en, "studiocfg.report_standard"), tr(en, "studiocfg.report_deep")]);
                dd.set_selected(1);
                vbox.append(&dd);
                Some(dd)
            } else {
                None
            };

            const LECTURE_MODES: [&str; 4] = ["general", "preview", "review", "problem"];
            // lecture: mode, detail and whether the scope is confirmed first
            let lecture_opts = if k == "lecture" {
                let ml = Label::new(Some(tr(en, "studio.cfg.lecture_mode_header")));
                ml.set_xalign(0.0);
                vbox.append(&ml);
                let modes = LECTURE_MODES;
                let mode_labels: Vec<&str> = modes
                    .iter()
                    .map(|m| tr(en, &format!("studio.cfg.lecture_mode_{m}")))
                    .collect();
                let mode_dd = gtk4::DropDown::from_strings(&mode_labels);
                vbox.append(&mode_dd);
                let dl = Label::new(Some(tr(en, "studiocfg.detail")));
                dl.set_xalign(0.0);
                vbox.append(&dl);
                let level_dd = gtk4::DropDown::from_strings(&[
                    tr(en, "studio.cfg.level_auto"),
                    tr(en, "studiocfg.brief"),
                    tr(en, "studio.cfg.qty_standard"),
                    tr(en, "studiocfg.detailed"),
                ]);
                vbox.append(&level_dd);
                let confirm = CheckButton::with_label(tr(en, "studio.cfg.btn_lecture_confirm_scope"));
                confirm.set_active(true);
                vbox.append(&confirm);
                let hint = Label::new(Some(tr(en, "studio.cfg.lecture_hint")));
                hint.set_wrap(true);
                hint.set_xalign(0.0);
                hint.add_css_class("lna-muted");
                vbox.append(&hint);
                Some((mode_dd, level_dd, confirm))
            } else {
                None
            };

            let mut ref_checks: Vec<(String, CheckButton)> = Vec::new();
            let cur_nid = st.active_notebook_id.borrow().clone();
            if !cur_nid.is_empty() {
                if let Ok(list_v) = call_core(&st.core, "studio.list", json!({ "notebook_id": cur_nid })) {
                    let ready: Vec<(String, String)> = list_v
                        .get("artifacts")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter(|a| a.get("status").and_then(Value::as_str) == Some("READY"))
                                .map(|a| {
                                    (
                                        a.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                                        a.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                                    )
                                })
                                .filter(|(id, _)| !id.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();
                    if !ready.is_empty() {
                        let rl = Label::new(Some(tr(en, "studio.cfg.ref_artifacts")));
                        rl.set_xalign(0.0);
                        vbox.append(&rl);
                        let rbox = GtkBox::new(Orientation::Vertical, 4);
                        for (rid, rtitle) in ready {
                            let cb = CheckButton::with_label(&rtitle);
                            rbox.append(&cb);
                            ref_checks.push((rid, cb));
                        }
                        vbox.append(&rbox);
                    }
                }
            }
            let ctx_row = GtkBox::new(Orientation::Horizontal, 8);
            ctx_row.append(ctx_bar.button());
            let ctx_hint = Label::new(Some(tr(en, "ctx.cfg_hint")));
            ctx_hint.add_css_class("lna-muted");
            ctx_hint.set_wrap(true);
            ctx_hint.set_xalign(0.0);
            ctx_row.append(&ctx_hint);
            vbox.append(&ctx_row);
            vbox.append(ctx_bar.chips());
            ctx_bar.accept_drops(&vbox);
            ctx_bar.accept_paste(&vbox);

            let actions = GtkBox::new(Orientation::Horizontal, 8);
            actions.set_halign(gtk4::Align::End);
            let cancel_b = Button::with_label(tr(en, "common.cancel"));
            let ok_b = Button::with_label(tr(en, "studiocfg.generate"));
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
                    let ref_ids: Vec<String> = ref_checks
                        .iter()
                        .filter(|(_, cb)| cb.is_active())
                        .map(|(id, _)| id.clone())
                        .collect();
                    if !ref_ids.is_empty() {
                        obj.insert("ref_artifact_ids".into(), json!(ref_ids));
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
                if let (Some(obj), Some((mode_dd, level_dd, confirm))) = (cfg.as_object_mut(), lecture_opts.as_ref()) {
                    let modes = LECTURE_MODES;
                    let levels = ["auto", "brief", "standard", "detailed"];
                    let mode = modes.get(mode_dd.selected() as usize).copied().unwrap_or("general");
                    let level = levels.get(level_dd.selected() as usize).copied().unwrap_or("auto");
                    if let Some(extra) = crate::lecture_model::lecture_config(mode, level, confirm.is_active()).as_object() {
                        for (key, v) in extra {
                            obj.insert(key.clone(), v.clone());
                        }
                    }
                }
                let actual_type = if k_ok == "knowledge_graph" { "mindmap" } else { k_ok.as_str() };
                let en2 = st_ok.is_en();
                let mut params = json!({ "notebook_id": nid, "artifact_type": actual_type, "config": cfg });
                ctx_bar.add_to(&mut params);
                match call_core(&st_ok.core, "studio.create", params) {
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
            if text.is_empty() && st.attach.is_empty() {
                return;
            }
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() {
                return;
            }
            let en = st.is_en();
            let mut params = json!({ "notebook_id": nid, "text": text });
            st.attach.add_to(&mut params);
            match call_core(&st.core, "chat.send", params) {
                Ok(_) => {
                    // sent: only now are the box and the attachments cleared, so a failed send keeps them
                    entry.set_text("");
                    st.attach.clear();
                    refresh_all(&st)
                }
                Err(e) => st.status_label.set_text(&friendly_err(en, e)),
            }
        });
    }

    // Wire New Notebook (opens the create dialog)
    {
        let st = state.clone();
        new_nb_btn.connect_clicked(move |_| {
            if let Some(pop) = st.nb_switch_btn.popover() {
                pop.popdown();
            }
            open_create_notebook_dialog(&st);
        });
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
            if call_core(&st.core, "diagnostics.export", json!({ "output_path": out_path.to_string_lossy() })).is_ok() {
                let en = st.is_en();
                st.status_label.set_text(&trf(en, "art.diagnostics_exported_to", &[&(out_path.display())]));
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
                st.status_label.set_text(&trf(en, "art.exported_to", &[&(out_path.display())]));
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
                st.status_label.set_text(&trf(en, "art.exported_to", &[&(out_path.display())]));
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
    st.draft_owner.borrow_mut().clear();
    st.chat_entry.set_text("");
    st.attach.clear();
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

/// Keeps the unsent chat text and attachments per notebook: stores the draft of the notebook that
/// was open and shows the one of `nid`.
fn swap_chat_draft(st: &Rc<UiState>, nid: &str) {
    let owner = st.draft_owner.borrow().clone();
    if owner == nid {
        return;
    }
    if !owner.is_empty() {
        let text = st.chat_entry.text().to_string();
        let items = st.attach.snapshot();
        if text.is_empty() && items.is_empty() {
            st.drafts.borrow_mut().remove(&owner);
        } else {
            st.drafts.borrow_mut().insert(owner, (text, items));
        }
    }
    let next = st.drafts.borrow_mut().remove(nid);
    match next {
        Some((text, items)) => {
            st.chat_entry.set_text(&text);
            st.attach.restore(items);
        }
        None => {
            st.chat_entry.set_text("");
            st.attach.clear();
        }
    }
    *st.draft_owner.borrow_mut() = nid.to_string();
}

fn refresh_all(st: &Rc<UiState>) {
    let nid = st.active_notebook_id.borrow().clone();
    if nid.is_empty() {
        return;
    }
    swap_chat_draft(st, &nid);
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
                let btn_label = if id == nid { format!("✓ {title}") } else { title.to_string() };
                let btn = Button::with_label(&btn_label);
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
        st.summary_label.set_markup(&crate::md_view::inline_markup(sum));

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
                let status = s.get("status").and_then(Value::as_str).unwrap_or("READY");
                let ready = status == "READY";
                let lbl = Label::new(Some(&if ready {
                    title.to_string()
                } else {
                    format!("{title} · {}", tr(en, &format!("sources.status_{}", status.to_lowercase())))
                }));
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                row.append(&lbl);
                if ready {
                    let cb = CheckButton::new();
                    cb.set_active(selected);
                    let st2 = st.clone();
                    let sid_sel = sid.clone();
                    cb.connect_toggled(move |c| {
                        let _ = call_core(
                            &st2.core,
                            "source.set_selected",
                            json!({ "source_id": sid_sel, "selected": c.is_active() }),
                        );
                    });
                    row.append(&cb);
                } else {
                    // Not fully parsed (index and source guide included): it cannot be used or
                    // opened, the only action is deleting it.
                    row.set_opacity(0.7);
                    row.set_tooltip_text(Some(tr(en, "sources.not_ready_hint")));
                    let del = Button::with_label(tr(en, "sources.delete"));
                    let st_del = st.clone();
                    let sid_del = sid.clone();
                    del.connect_clicked(move |_| {
                        let _ = call_core(&st_del.core, "source.remove", json!({ "source_id": sid_del }));
                        refresh_all(&st_del);
                    });
                    row.append(&del);
                }
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
                            t_hdr.append(&Label::new(Some(tr(en, "studio.thoughts"))));
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

                if let Some(atts) = m.get("attachments") {
                    let st_p = st.clone();
                    let nid_p = nid.clone();
                    let promote: Rc<dyn Fn(&str)> = Rc::new(move |att_id| {
                        let en = st_p.is_en();
                        match call_core(
                            &st_p.core,
                            "attachment.promote",
                            json!({ "notebook_id": nid_p, "attachment_id": att_id }),
                        ) {
                            Ok(_) => {
                                st_p.status_label.set_text(tr(en, "ctx.promoted"));
                                refresh_all(&st_p);
                            }
                            Err(e) => st_p.status_label.set_text(&friendly_err(en, e)),
                        }
                    });
                    if let Some(row) = attach::attachments_row(atts, en, promote) {
                        card.append(&row);
                    }
                }
                if role == "user" {
                    let body_lbl = Label::new(Some(content));
                    body_lbl.set_wrap(true);
                    body_lbl.set_selectable(true);
                    body_lbl.set_xalign(0.0);
                    card.append(&body_lbl);
                } else {
                    card.append(&crate::md_view::markdown_widget(content));
                }
                st.chat_box.append(&card);
            }
        }
    }

    // Studio artifacts
    clear_box(&st.artifacts_box);
    if let Ok(res) = call_core(&st.core, "studio.list", json!({ "notebook_id": nid })) {
        if let Some(arr) = res.get("artifacts").and_then(Value::as_array) {
            for a in arr.iter() {
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


/// How often streamed thought text / previews are pushed to the widgets.
const THOUGHT_FLUSH: Duration = Duration::from_millis(200);
const PREVIEW_FLUSH: Duration = Duration::from_millis(1000);

/// The generating artifact currently open in the detail pane.
struct LiveView {
    artifact_id: String,
    kind: String,
    en: bool,
    stage_lbl: Label,
    panel: GtkBox,
    cards: LiveCards,
    /// Cards whose streamed text changed since the last flush.
    dirty: HashSet<i64>,
    tabs: Stack,
    switcher: gtk4::StackSwitcher,
    preview_host: GtkBox,
    preview_added: bool,
    pending_preview: Option<Value>,
    /// The preview that is on screen, to skip rebuilding an identical one.
    last_preview: Option<Value>,
    /// The user's expand / collapse state, zoom and scroll of the preview mind map.
    preview_memory: SharedMemory,
    last_thought_flush: Instant,
    last_preview_flush: Instant,
}

impl LiveView {
    fn add_thought(&mut self, d: &Value) {
        let Some(seq) = d.get("seq").and_then(Value::as_i64) else { return };
        if self.cards.cards.contains_key(&seq) || d.get("kind").and_then(Value::as_str) == Some("retry") {
            return;
        }
        if let Some(p) = self.cards.placeholder.take() {
            self.panel.remove(&p);
        }
        let (card, lc) = build_thought_card(d, self.en);
        self.panel.append(&card);
        self.cards.cards.insert(seq, lc);
        // Deltas may have arrived before the card existed.
        self.dirty.insert(seq);
    }

    /// Brings every card with new streamed text up to date. A card is always a function of the
    /// stored body and the cumulative delta log, so flushing twice, or flushing right after a
    /// re-fetch that already contains the text, never repeats anything.
    fn flush_thoughts(&mut self, log: &StreamLog) {
        self.last_thought_flush = Instant::now();
        if self.dirty.is_empty() {
            return;
        }
        let en = self.en;
        let aid = self.artifact_id.as_str();
        let cards = &mut self.cards.cards;
        self.dirty.retain(|seq| match cards.get_mut(seq) {
            Some(c) => {
                if let Some(text) = log.get(aid, *seq) {
                    sync_card(c, text, en);
                }
                false
            }
            None => true,
        });
    }

    fn flush_preview(&mut self) {
        let Some(pv) = self.pending_preview.take() else { return };
        self.last_preview_flush = Instant::now();
        if self.last_preview.as_ref() == Some(&pv) {
            return;
        }
        // Build the new view before the old one goes away; the view state comes from the memory.
        let widget = build_preview_widget(&pv, &self.kind, self.en, &self.preview_memory);
        clear_box(&self.preview_host);
        self.preview_host.append(&widget);
        if !self.preview_added {
            self.preview_added = true;
            self.tabs.add_titled(&self.preview_host, Some("preview"), tr(self.en, "art.tab_preview"));
            self.switcher.set_visible(true);
        }
        self.last_preview = Some(pv);
    }
}

/// Cumulative streamed thought text per card of one artifact (deltas carry no offsets, so the
/// full text is kept and cards are reconciled against it).
#[derive(Default)]
struct StreamLog {
    artifact_id: String,
    text: HashMap<i64, String>,
}

impl StreamLog {
    fn push(&mut self, artifact_id: &str, seq: i64, delta: &str) {
        if self.artifact_id != artifact_id {
            self.artifact_id = artifact_id.to_string();
            self.text.clear();
        }
        self.text.entry(seq).or_default().push_str(delta);
    }

    fn get(&self, artifact_id: &str, seq: i64) -> Option<&str> {
        if self.artifact_id == artifact_id {
            self.text.get(&seq).map(String::as_str)
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.artifact_id.clear();
        self.text.clear();
    }
}

/// The text a card should show given what it holds (`current`, from the database or earlier
/// flushes) and the cumulative streamed text: the streamed text wins only when it extends what
/// the card already has. Text the card already contains is never appended again.
fn stream_extension<'a>(current: &str, streamed: &'a str) -> Option<&'a str> {
    if streamed.len() > current.len() && streamed.starts_with(current) {
        Some(streamed)
    } else {
        None
    }
}

fn sync_card(c: &mut LiveCard, streamed: &str, en: bool) {
    let next = stream_extension(&c.full.borrow(), streamed);
    if let Some(text) = next {
        set_card_body(c, text, en);
    }
}

/// Subscribes to the core's event stream on a worker thread and applies studio events to the
/// open generating artifact on the GTK main thread (polled every 50 ms; thoughts are flushed to
/// the widgets at most every 200 ms and previews every second).
fn start_event_pump(st: &Rc<UiState>) {
    let (tx, rx) = std::sync::mpsc::channel::<lna_types::ApiEvent>();
    let mut sub = st.core.events.subscribe();
    std::thread::spawn(move || loop {
        match sub.blocking_recv() {
            Ok(ev) => {
                if (ev.event.starts_with("studio.") || ev.event.starts_with("ffmpeg.") || ev.event == "profile.updated")
                    && tx.send(ev).is_err()
                {
                    break;
                }
            }
            Err(e) => {
                if format!("{e:?}").contains("Closed") {
                    break;
                }
                // Lagged: events were dropped; the manual refresh button resyncs.
            }
        }
    });
    let st = st.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let mut finished: Option<String> = None;
        let ffmpeg_hook = st.ffmpeg_hook.borrow().clone();
        let profile_hook = st.profile_hook.borrow().clone();
        {
            let mut guard = st.live.borrow_mut();
            while let Ok(ev) = rx.try_recv() {
                if ev.event.starts_with("ffmpeg.") {
                    if let Some(hook) = &ffmpeg_hook {
                        hook(ev.event.as_str(), &ev.data);
                    }
                    continue;
                }
                if ev.event == "profile.updated" {
                    if let Some(hook) = &profile_hook {
                        hook(ev.event.as_str(), &ev.data);
                    }
                    continue;
                }
                let Some(live) = guard.as_mut() else { continue };
                let d = &ev.data;
                if d.get("artifact_id").and_then(Value::as_str) != Some(live.artifact_id.as_str()) {
                    continue;
                }
                match ev.event.as_str() {
                    "studio.thought" => live.add_thought(d),
                    "studio.thought_delta" => {
                        if let (Some(seq), Some(delta)) =
                            (d.get("seq").and_then(Value::as_i64), d.get("delta").and_then(Value::as_str))
                        {
                            st.thought_streams.borrow_mut().push(&live.artifact_id, seq, delta);
                            live.dirty.insert(seq);
                        }
                    }
                    "studio.preview" => {
                        if d.get("payload").map(|p| p.is_object()).unwrap_or(false) {
                            live.pending_preview = Some(d.clone());
                        }
                    }
                    "studio.status" => {
                        let status = d.get("status").and_then(Value::as_str).unwrap_or("");
                        if matches!(status, "READY" | "FAILED" | "CANCELLED") {
                            finished = Some(live.artifact_id.clone());
                        } else {
                            let stage = d.get("stage").and_then(Value::as_str).unwrap_or("PROCESSING");
                            let pct = (d.get("progress").and_then(Value::as_f64).unwrap_or(0.0) * 100.0).round() as i32;
                            live.stage_lbl.set_text(&format!(
                                "{} · {} ({pct}%)",
                                tr(live.en, "art.ai_workflow_in_progress"),
                                stage_label(live.en, stage)
                            ));
                        }
                    }
                    _ => {}
                }
            }
            if finished.is_none() {
                if let Some(live) = guard.as_mut() {
                    if live.last_thought_flush.elapsed() >= THOUGHT_FLUSH {
                        live.flush_thoughts(&st.thought_streams.borrow());
                    }
                    if live.pending_preview.is_some() && live.last_preview_flush.elapsed() >= PREVIEW_FLUSH {
                        live.flush_preview();
                    }
                }
            }
        }
        if let Some(id) = finished {
            // The result view replaces the progress view (open_artifact_inline drops the live state).
            open_artifact_inline(&st, &id);
            st.thought_streams.borrow_mut().clear();
            refresh_all(&st);
        }
        glib::ControlFlow::Continue
    });
}

/// Reasoning text longer than this is shown as its tail only, until the user asks for all of it.
const REASONING_TAIL_CHARS: usize = 8000;

fn tail_chars(text: &str, max: usize) -> Option<&str> {
    let total = text.chars().count();
    if total <= max {
        return None;
    }
    let skip = total - max;
    let byte = text.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    Some(&text[byte..])
}

/// One thought card of the process view, kept so live events can update it in place.
struct LiveCard {
    reasoning: bool,
    body_box: GtkBox,
    label: Option<Label>,
    more: Option<Button>,
    full: Rc<RefCell<String>>,
    shown_full: Rc<Cell<bool>>,
}

/// Thought cards of a process view, keyed by thought seq.
#[derive(Default)]
struct LiveCards {
    cards: HashMap<i64, LiveCard>,
    placeholder: Option<Label>,
}

/// Puts `body` into a card's label (creating it on first use). Reasoning text is capped to its
/// tail unless the user asked for everything.
fn set_card_body(c: &mut LiveCard, body: &str, en: bool) {
    *c.full.borrow_mut() = body.to_string();
    if body.is_empty() {
        return;
    }
    if c.label.is_none() {
        let b = Label::new(None);
        b.set_xalign(0.0);
        b.set_wrap(true);
        b.set_selectable(true);
        b.add_css_class("lna-muted");
        if c.reasoning {
            let more = Button::with_label(tr(en, "art.earlier_text_omitted_click_to"));
            more.add_css_class("lna-pill-btn");
            more.set_halign(gtk4::Align::Start);
            more.set_visible(false);
            let full = c.full.clone();
            let shown = c.shown_full.clone();
            let b_c = b.clone();
            more.connect_clicked(move |btn| {
                shown.set(true);
                b_c.set_text(&full.borrow());
                btn.set_visible(false);
            });
            c.body_box.append(&more);
            c.more = Some(more);
        }
        c.body_box.append(&b);
        c.label = Some(b);
    }
    let tail = if c.reasoning && !c.shown_full.get() { tail_chars(body, REASONING_TAIL_CHARS) } else { None };
    if let Some(l) = &c.label {
        l.set_text(tail.unwrap_or(body));
    }
    if let Some(m) = &c.more {
        m.set_visible(tail.is_some());
    }
}

fn build_thought_card(t: &Value, en: bool) -> (GtkBox, LiveCard) {
    let kind = t.get("kind").and_then(Value::as_str).unwrap_or("stage");
    let title = t.get("title").and_then(Value::as_str).unwrap_or("");
    let body = t.get("body").and_then(Value::as_str).unwrap_or("");
    let card = GtkBox::new(Orientation::Vertical, 4);
    card.add_css_class("lna-card");
    let tag = match kind {
        "tool" => tr(en, "art.tool"),
        "reasoning" => tr(en, "art.thinking"),
        "error" => tr(en, "art.error"),
        _ => tr(en, "art.step"),
    };
    let head_text = format!("{tag}{title}");
    let body_box = GtkBox::new(Orientation::Vertical, 4);
    let mut lc = LiveCard {
        reasoning: kind == "reasoning",
        body_box: body_box.clone(),
        label: None,
        more: None,
        full: Rc::new(RefCell::new(String::new())),
        shown_full: Rc::new(Cell::new(false)),
    };
    set_card_body(&mut lc, body, en);
    if kind == "reasoning" {
        let exp = Expander::new(Some(&head_text));
        exp.set_expanded(false);
        exp.set_child(Some(&body_box));
        card.append(&exp);
    } else {
        let head = Label::new(Some(&head_text));
        head.set_xalign(0.0);
        head.set_wrap(true);
        card.append(&head);
        card.append(&body_box);
    }
    (card, lc)
}

/// Process view ("执行过程"). Reasoning cards start collapsed; `running` is false for finished runs,
/// which get a note when the model produced no reasoning cards at all. `live` collects the cards
/// so streaming events can update them.
fn build_thoughts_panel(art: &Value, en: bool, running: bool, mut live: Option<&mut LiveCards>) -> GtkBox {
    let box_w = GtkBox::new(Orientation::Vertical, 6);
    let hdr = Label::new(Some(tr(en, "art.thoughts_header")));
    hdr.set_xalign(0.0);
    hdr.add_css_class("lna-muted");
    box_w.append(&hdr);
    let items: Vec<&Value> = art
        .get("thoughts")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|t| t.get("kind").and_then(Value::as_str) != Some("retry"))
                .collect()
        })
        .unwrap_or_default();
    if !running && !items.iter().any(|t| t.get("kind").and_then(Value::as_str) == Some("reasoning")) {
        let note = Label::new(Some(tr(en, "art.no_reasoning_note")));
        note.set_xalign(0.0);
        note.add_css_class("lna-muted");
        box_w.append(&note);
    }
    if items.is_empty() {
        let empty = Label::new(Some(tr(en, "art.waiting_for_ai_workflow_steps")));
        empty.set_xalign(0.0);
        empty.add_css_class("lna-muted");
        box_w.append(&empty);
        if let Some(l) = live.as_deref_mut() {
            l.placeholder = Some(empty);
        }
        return box_w;
    }
    for t in items {
        let (card, lc) = build_thought_card(t, en);
        if let (Some(l), Some(seq)) = (live.as_deref_mut(), t.get("seq").and_then(Value::as_i64)) {
            l.cards.insert(seq, lc);
        }
        box_w.append(&card);
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
    // only the notebook sources the generation actually read (the core works this out)
    let src_count = src_refs.len();

    let pill_label = if src_count > 0 {
        trf(en, "studio.view_prompt_sources", &[&src_count])
    } else {
        tr(en, "studio.view_prompt").to_string()
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

    if art.get("status").and_then(Value::as_str) == Some("READY") {
        let add_src_btn = Button::with_label(tr(en, "studio.add_to_sources"));
        add_src_btn.add_css_class("lna-pill-btn");
        let st_as = st.clone();
        let aid_as = artifact_id.to_string();
        let pop_as = pop.clone();
        add_src_btn.connect_clicked(move |_| {
            pop_as.popdown();
            let _ = call_core(&st_as.core, "studio.add_to_sources", json!({ "artifact_id": aid_as }));
            refresh_all(&st_as);
        });
        p_hdr.append(&add_src_btn);
    }
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

    // The user's own pictures and files given with the request
    let atts = art.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
    if !atts.is_empty() {
        let a_title = Label::new(Some(tr(en, "studio.attachments_header")));
        a_title.set_xalign(0.0);
        a_title.add_css_class("lna-muted");
        body.append(&a_title);
        let pics = GtkBox::new(Orientation::Horizontal, 8);
        for a in &atts {
            let path = a.get("path").and_then(Value::as_str).unwrap_or("");
            let name = a.get("name").and_then(Value::as_str).unwrap_or("");
            let mime = a.get("mime").and_then(Value::as_str).unwrap_or("");
            if mime.starts_with("image/") && std::path::Path::new(path).exists() {
                let pic = gtk4::Picture::for_filename(path);
                pic.set_size_request(96, 96);
                pic.set_tooltip_text(Some(name));
                pics.append(&pic);
            } else {
                let l = Label::new(Some(&format!("📄 {name}")));
                l.set_xalign(0.0);
                l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                body.append(&l);
            }
        }
        if pics.first_child().is_some() {
            body.append(&ScrolledWindow::builder().child(&pics).vscrollbar_policy(gtk4::PolicyType::Never).min_content_height(100).build());
        }
    }

    // Sources section
    let s_title = Label::new(Some(tr(en, "studio.sources_header")));
    s_title.set_xalign(0.0);
    s_title.add_css_class("lna-muted");
    body.append(&s_title);

    let chips_box = GtkBox::new(Orientation::Vertical, 6);
    if !src_refs.is_empty() {
        for sr in &src_refs {
            let stitle = sr.get("title").and_then(Value::as_str).unwrap_or("");
            let skind = sr
                .get("kind")
                .and_then(Value::as_str)
                .map(|k| k.to_uppercase())
                .unwrap_or_else(|| tr(en, "ctx.file").to_uppercase());
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
    // Re-opening the same generating artifact keeps the user's preview view state.
    let prev_memory = st
        .live
        .borrow()
        .as_ref()
        .filter(|l| l.artifact_id == artifact_id)
        .map(|l| l.preview_memory.clone());
    *st.live.borrow_mut() = None;

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

    let status = art.get("status").and_then(Value::as_str).unwrap_or("");

    hdr.append(&title_col);
    if matches!(status, "READY" | "FAILED" | "CANCELLED") {
        // same requirements, same sources, from scratch; confirmed in a popover because the
        // current content is replaced
        let confirm = GtkBox::new(Orientation::Vertical, 8);
        confirm.set_margin_top(10);
        confirm.set_margin_bottom(10);
        confirm.set_margin_start(10);
        confirm.set_margin_end(10);
        let msg = Label::new(Some(&trf(en, "studio.regenerate_confirm", &[&title])));
        msg.set_wrap(true);
        msg.set_max_width_chars(36);
        msg.set_xalign(0.0);
        let go = Button::with_label(tr(en, "studio.regenerate"));
        go.add_css_class("lna-primary-btn");
        go.set_halign(gtk4::Align::End);
        confirm.append(&msg);
        confirm.append(&go);
        let pop = Popover::new();
        pop.set_child(Some(&confirm));
        let regen = MenuButton::new();
        regen.set_label("↻");
        regen.set_tooltip_text(Some(tr(en, "studio.regenerate")));
        regen.set_popover(Some(&pop));
        let st_rg = st.clone();
        let aid_rg = artifact_id.to_string();
        go.connect_clicked(move |_| {
            pop.popdown();
            let _ = call_core(
                &st_rg.core,
                "studio.regenerate",
                json!({ "artifact_id": aid_rg, "use_selected_sources": false }),
            );
            refresh_all(&st_rg);
            open_artifact_inline(&st_rg, &aid_rg);
        });
        hdr.append(&regen);
    }
    hdr.append(&fs_btn);
    hdr.append(&close_btn);
    st.studio_detail_box.append(&hdr);

    let kind = art.get("artifact_type").and_then(Value::as_str).unwrap_or("");

    if status == "READY" && art.get("thoughts").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false) {
        let exp = Expander::new(Some(tr(en, "art.ai_workflow_thoughts")));
        let list = build_thoughts_panel(&art, en, false, None);
        exp.set_child(Some(&ScrolledWindow::builder().child(&list).min_content_height(160).max_content_height(280).build()));
        st.studio_detail_box.append(&exp);
    }

    if status == "PENDING" || status == "RUNNING" {
        let prog_box = GtkBox::new(Orientation::Vertical, 10);
        let stage = art.get("stage").and_then(Value::as_str).unwrap_or("PROCESSING");
        let pct = (art.get("progress").and_then(Value::as_f64).unwrap_or(0.0) * 100.0).round() as i32;
        let hdr_lbl = Label::new(Some(&format!(
            "{} · {} ({pct}%)",
            tr(en, "art.ai_workflow_in_progress"),
            stage_label(en, stage)
        )));
        hdr_lbl.set_xalign(0.0);
        prog_box.append(&hdr_lbl);
        let mut cards = LiveCards::default();
        let thoughts_list = build_thoughts_panel(&art, en, true, Some(&mut cards));
        let process_view = ScrolledWindow::builder().child(&thoughts_list).vexpand(true).min_content_height(260).build();
        // Unvalidated preview of the in-progress artifact: a second tab, only once a preview exists.
        let tabs = Stack::new();
        tabs.set_vexpand(true);
        tabs.add_titled(&process_view, Some("process"), tr(en, "art.tab_process"));
        let switcher = gtk4::StackSwitcher::new();
        switcher.set_stack(Some(&tabs));
        switcher.set_halign(gtk4::Align::Start);
        switcher.set_visible(false);
        let preview_host = GtkBox::new(Orientation::Vertical, 0);
        preview_host.set_vexpand(true);
        prog_box.append(&switcher);
        prog_box.append(&tabs);
        let cards_seqs: HashSet<i64> = cards.cards.keys().copied().collect();
        let mut live = LiveView {
            artifact_id: artifact_id.to_string(),
            kind: kind.to_string(),
            en,
            stage_lbl: hdr_lbl.clone(),
            panel: thoughts_list.clone(),
            cards,
            dirty: cards_seqs,
            tabs,
            switcher,
            preview_host,
            preview_added: false,
            pending_preview: None,
            last_preview: None,
            preview_memory: prev_memory.unwrap_or_else(new_memory),
            last_thought_flush: Instant::now(),
            last_preview_flush: Instant::now() - Duration::from_secs(10),
        };
        let preview = call_core(&st.core, "studio.preview.get", json!({ "artifact_id": artifact_id }))
            .ok()
            .filter(|v| v.get("payload").map(|p| p.is_object()).unwrap_or(false));
        if let Some(pv) = preview {
            live.pending_preview = Some(pv);
            live.flush_preview();
        }
        // Text streamed before this (re-)fetch that the stored bodies do not hold yet.
        live.flush_thoughts(&st.thought_streams.borrow());
        *st.live.borrow_mut() = Some(live);
        let refresh_btn = Button::with_label(tr(en, "art.refresh_progress"));
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
            .placeholder_text(tr(en, "art.enter_revision_prompt_to_refine"))
            .hexpand(true)
            .build();
        let rev_bar = {
            let en_c = st.en_cell.clone();
            let status_c = st.status_label.clone();
            AttachBar::new(
                st.window.upcast_ref::<gtk4::Window>(),
                Rc::new(move || en_c.get()),
                Rc::new(move |m| status_c.set_text(m)),
            )
        };
        let rev_btn = Button::with_label(tr(en, "art.outline_revise_btn"));
        rev_btn.add_css_class("lna-pill-btn");
        let st_rv = st.clone();
        let aid_rv = artifact_id.to_string();
        let buf_rv = tv.buffer();
        let rev_entry_c = rev_entry.clone();
        let rev_bar_c = rev_bar.clone();
        rev_btn.connect_clicked(move |_| {
            let mut instr = rev_entry_c.text().trim().to_string();
            if instr.is_empty() && rev_bar_c.is_empty() { return; }
            if instr.is_empty() {
                instr = tr(st_rv.is_en(), "ctx.revise_default").to_string();
            }
            let (s, e) = buf_rv.bounds();
            let cur_draft = buf_rv.text(&s, &e, false).to_string();
            let mut params = json!({ "artifact_id": aid_rv, "instruction": instr, "draft": cur_draft });
            rev_bar_c.add_to(&mut params);
            match call_core(&st_rv.core, "studio.revise_draft", params) {
                Ok(updated) => {
                    if let Some(new_md) = updated.get("draft").and_then(Value::as_str) {
                        buf_rv.set_text(new_md);
                        rev_entry_c.set_text("");
                        rev_bar_c.clear();
                    }
                }
                Err(e) => st_rv.status_label.set_text(&friendly_err(st_rv.is_en(), e)),
            }
        });
        rev_row.append(rev_bar.button());
        rev_row.append(&rev_entry);
        rev_row.append(&rev_btn);
        rev_bar.accept_drops(&rev_row);
        rev_bar.accept_paste(&rev_row);
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&tv).vexpand(true).build());
        st.studio_detail_box.append(rev_bar.chips());
        st.studio_detail_box.append(&rev_row);
        st.studio_detail_box.append(&approve);
    } else if kind == "mindmap" {
        let payload = art.get("payload").unwrap_or(&Value::Null);
        let is_kg = art.pointer("/config/mode").and_then(Value::as_str) == Some("knowledge_graph")
            || payload.get("kind").and_then(Value::as_str) == Some("knowledge_graph");
        let st_link = st.clone();
        let aid_refine = artifact_id.to_string();
        let on_action: ActionFn = Rc::new(move |action, concept| {
            let nid2 = st_link.active_notebook_id.borrow().clone();
            if nid2.is_empty() { return; }
            if let Some(node_id) = action.strip_prefix("refine:") {
                // Refinement runs model calls: keep it off the GTK thread and poll for the result.
                let en = st_link.is_en();
                st_link.status_label.set_text(&format!(
                    "{}{concept}",
                    tr(en, "art.refining")
                ));
                let (tx, rx) = std::sync::mpsc::channel();
                let core = st_link.core.clone();
                let params = json!({ "artifact_id": aid_refine, "node_id": node_id });
                std::thread::spawn(move || {
                    let _ = tx.send(call_core(&core, "studio.refine_node", params));
                });
                let st_done = st_link.clone();
                let aid_done = aid_refine.clone();
                let label = concept.to_string();
                gtk4::glib::timeout_add_local(Duration::from_millis(250), move || {
                    let Ok(res) = rx.try_recv() else {
                        return gtk4::glib::ControlFlow::Continue;
                    };
                    let en = st_done.is_en();
                    let msg = match res {
                        Ok(r) if r.get("refined").and_then(Value::as_bool) == Some(true) => {
                            let added = r.get("added").and_then(Value::as_u64).unwrap_or(0);
                            trf(en, "art.added_finer_point_s_under", &[&added, &label])
                        }
                        Ok(r) => {
                            let reason = r.get("reason").and_then(Value::as_str).unwrap_or("");
                            let head = trf(en, "art.is_already_a_minimal_concept", &[&label]);
                            if reason.is_empty() { head } else { format!("{head} — {reason}") }
                        }
                        Err(e) => friendly_err(en, e),
                    };
                    st_done.status_label.set_text(&msg);
                    refresh_all(&st_done);
                    open_artifact_inline(&st_done, &aid_done);
                    gtk4::glib::ControlFlow::Break
                });
                return;
            }
            match action {
                "quiz" => {
                    let _ = call_core(&st_link.core, "studio.create", json!({
                        "notebook_id": nid2, "artifact_type": "quiz",
                        "title": trf(st_link.is_en(), "ui.kg_quiz_title", &[&concept]),
                        "config": { "topic": concept, "count": 5 }
                    }));
                }
                "mindmap" => {
                    let _ = call_core(&st_link.core, "studio.create", json!({
                        "notebook_id": nid2, "artifact_type": "mindmap",
                        "title": trf(st_link.is_en(), "ui.kg_mindmap_title", &[&concept]),
                        "config": { "topic": concept }
                    }));
                }
                _ => {
                    let _ = call_core(&st_link.core, "chat.send", json!({
                        "notebook_id": nid2, "text": trf(st_link.is_en(), "ui.kg_explain_prompt", &[&concept])
                    }));
                }
            }
            refresh_all(&st_link);
        });
        st.studio_detail_box.append(&build_mindmap_widget(payload, en, is_kg, false, None, on_action));
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
                let tb = Button::with_label(&format!("{}. {}", i + 1, h));
                tb.add_css_class("lna-toc-btn");
                toc.append(&tb);
                let hl = crate::md_view::inline_label(h);
                hl.set_markup(&format!(
                    "<span size=\"large\" weight=\"bold\">{}</span>",
                    crate::md_view::inline_markup(h)
                ));
                body.append(&hl);
                body.append(&crate::md_view::report_section_widget(sec, en));
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
                let line = crate::md_view::inline_label(&format!("**{h}** — {m}\n{s}"));
                canvas.append(&line);
            }
        }
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&canvas).vexpand(true).build());
    } else if kind == "lecture" {
        st.studio_detail_box.append(&crate::lecture::build_lecture_player(
            &st.core,
            st.window.upcast_ref::<gtk4::Window>(),
            &art,
            en,
        ));
    } else if let Some(w) =
        crate::md_view::readable_artifact_widget(kind, art.get("payload").unwrap_or(&Value::Null), en)
    {
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&w).vexpand(true).build());
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
    let privacy_text = [tr(en, "privacy.line_local"), tr(en, "privacy.line_keys"), tr(en, "privacy.line_send"), tr(en, "privacy.line_embed")].join("\n");
    let body = Label::new(Some(privacy_text.as_str()));
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
    let theme_labels = [tr(en, "settings.theme_dark"), tr(en, "settings.theme_light"), tr(en, "settings.theme_system"), tr(en, "settings.theme_midnight"), tr(en, "settings.theme_forest"), tr(en, "settings.theme_warm_paper"), tr(en, "settings.theme_rose_dusk")];
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
            tr(en, "settings.follow_ui_language"),
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
            tr(en, "settings.continuous_scroll"),
            tr(en, "settings.one_question_at_a_time"),
        ])),
        None::<gtk4::Expression>,
    );
    quiz_view_dd.set_selected(if st.prefs.borrow().quiz_view_mode == "one_by_one" { 1 } else { 0 });

    let quiz_submit_dd = DropDown::new(
        Some(StringList::new(&[
            tr(en, "settings.manual_submit"),
            tr(en, "settings.auto_check_on_select"),
        ])),
        None::<gtk4::Expression>,
    );
    quiz_submit_dd.set_selected(if st.prefs.borrow().quiz_submit_mode == "auto" { 1 } else { 0 });

    // Lecture repair rounds per section ("lecture.limits"): 0 = no repair, no independent review.
    let lecture_rounds_spin = gtk4::SpinButton::with_range(0.0, 20.0, 1.0);
    lecture_rounds_spin.set_value(
        call_core(&st.core, "settings.get", json!({ "key": "lecture.limits" }))
            .ok()
            .and_then(|v| v.pointer("/value/repair_rounds").and_then(Value::as_u64))
            .map(|n| n.min(20) as f64)
            .unwrap_or(2.0),
    );

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
    let lecture_rounds_spin_c = lecture_rounds_spin.clone();
    save_prefs_btn.connect_clicked(move |_| {
        let _ = call_core(
            &st_p.core,
            "settings.set",
            json!({ "key": "lecture.limits", "value": { "repair_rounds": lecture_rounds_spin_c.value_as_int().clamp(0, 20) } }),
        );
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
    let parent_priv = parent.clone();
    let st_priv = st.clone();
    gen_privacy_btn.connect_clicked(move |_| open_privacy_dialog(&parent_priv, st_priv.is_en()));
    let st_diag = st.clone();
    gen_diag_btn.connect_clicked(move |_| {
        let out_dir = dirs::download_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let out_path = out_dir.join("local-notebook-diagnostics.zip");
        if call_core(&st_diag.core, "diagnostics.export", json!({ "output_path": out_path.to_string_lossy() })).is_ok() {
            st_diag.status_label.set_text(&trf(st_diag.is_en(), "art.diagnostics_exported_to", &[&out_path.display()]));
        }
    });
    gen_extra_row.append(&gen_privacy_btn);
    gen_extra_row.append(&gen_diag_btn);

    gen_box.append(&exp_row);
    gen_box.append(&sug_row);
    gen_box.append(&Label::new(Some(tr(en, "settings.default_studio_lang"))));
    gen_box.append(&studio_lang_dd);
    gen_box.append(&Label::new(Some(tr(en, "settings.quiz_view_mode"))));
    gen_box.append(&quiz_view_dd);
    gen_box.append(&Label::new(Some(tr(en, "settings.quiz_submit_mode"))));
    gen_box.append(&quiz_submit_dd);
    gen_box.append(&Label::new(Some(tr(en, "settings.lecture_repair_rounds"))));
    gen_box.append(&lecture_rounds_spin);
    let lecture_rounds_hint = Label::new(Some(tr(en, "settings.lecture_repair_rounds_hint")));
    lecture_rounds_hint.set_wrap(true);
    lecture_rounds_hint.set_xalign(0.0);
    lecture_rounds_hint.add_css_class("dim-label");
    gen_box.append(&lecture_rounds_hint);
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
    let profile_dd = DropDown::from_strings(&[tr(en, "model.new_profile")]);
    profile_dd.set_hexpand(true);
    let new_prof_btn = Button::with_label(tr(en, "model.new"));
    new_prof_btn.add_css_class("lna-pill-btn");
    let del_prof_btn = Button::with_label(tr(en, "studio.delete"));
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
        .placeholder_text(tr(en, "ui.base_url_placeholder"))
        .build();
    let model_row = GtkBox::new(Orientation::Horizontal, 8);
    let model_entry = Entry::builder().placeholder_text(tr(en, "model.model_id")).hexpand(true).build();
    let discover_btn = Button::with_label(tr(en, "model.fetch_models"));
    discover_btn.add_css_class("lna-pill-btn");
    model_row.append(&model_entry);
    model_row.append(&discover_btn);

    let discovered_models: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let discovered_dd = DropDown::from_strings(&[tr(en, "model.discovered_models")]);
    discovered_dd.set_visible(false);

    let key_entry = Entry::builder()
        .placeholder_text(tr(en, "ui.api_key"))
        .visibility(false)
        .build();

    let insecure_http_sw = Switch::new();
    let insecure_row = GtkBox::new(Orientation::Horizontal, 8);
    let insecure_lbl = Label::new(Some(tr(en, "model.allow_non_loopback_http")));
    insecure_lbl.set_hexpand(true);
    insecure_lbl.set_xalign(0.0);
    insecure_row.append(&insecure_lbl);
    insecure_row.append(&insecure_http_sw);

    let active_chat_sw = Switch::new();
    active_chat_sw.set_active(true);
    let active_row = GtkBox::new(Orientation::Horizontal, 8);
    let active_lbl = Label::new(Some(tr(en, "model.set_as_default_chat_model")));
    active_lbl.set_hexpand(true);
    active_lbl.set_xalign(0.0);
    active_row.append(&active_lbl);
    active_row.append(&active_chat_sw);

    // read-only: the core detects image/audio/video support by probing the profile
    let detected_lbl = Label::new(None);
    detected_lbl.set_xalign(0.0);
    detected_lbl.set_wrap(true);
    detected_lbl.add_css_class("lna-muted");

    // Advanced Options Expander
    let adv_exp = Expander::new(Some(tr(en, "model.advanced_options")));
    let adv_box = GtkBox::new(Orientation::Vertical, 8);
    let thinking_sw = Switch::new();
    thinking_sw.set_active(true);
    let thinking_row = GtkBox::new(Orientation::Horizontal, 8);
    let thinking_lbl = Label::new(Some(tr(en, "model.show_thinking_process")));
    thinking_lbl.set_hexpand(true);
    thinking_lbl.set_xalign(0.0);
    thinking_row.append(&thinking_lbl);
    thinking_row.append(&thinking_sw);

    let effort_ids = ["", "low", "medium", "high"];
    let effort_dd = DropDown::from_strings(&[tr(en, "tts.default_voice"), "low", "medium", "high"]);
    let ctx_entry = Entry::builder().text("32000").placeholder_text(tr(en, "ui.context_window")).build();
    // empty or 0 = unlimited
    let out_entry = Entry::builder().placeholder_text(tr(en, "model.unlimited_hint")).build();
    let par_entry = Entry::builder().placeholder_text(tr(en, "model.unlimited_hint")).tooltip_text(tr(en, "model.max_parallel_tip")).build();
    let out_hint = Label::new(Some(tr(en, "model.unlimited_hint")));
    out_hint.set_xalign(0.0);
    out_hint.add_css_class("dim-label");
    let par_hint = Label::new(Some(tr(en, "model.unlimited_hint")));
    par_hint.set_xalign(0.0);
    par_hint.add_css_class("dim-label");
    let price_in_entry = Entry::builder().placeholder_text(tr(en, "ui.input_price")).build();
    let price_out_entry = Entry::builder().placeholder_text(tr(en, "ui.output_price")).build();

    let extra_headers_rows: Rc<RefCell<Vec<(Entry, Entry)>>> = Rc::new(RefCell::new(Vec::new()));
    // "the form changed": re-evaluates whether Apply has anything to apply. Bound further
    // down, once every field exists.
    let form_changed_hook: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let notify_changed: Rc<dyn Fn()> = {
        let hook = form_changed_hook.clone();
        Rc::new(move || {
            let f = hook.borrow().clone();
            if let Some(f) = f {
                f();
            }
        })
    };
    let headers_list = GtkBox::new(Orientation::Vertical, 6);
    let headers_hdr = GtkBox::new(Orientation::Horizontal, 8);
    let headers_lbl = Label::new(Some(tr(en, "model.extra_headers")));
    headers_lbl.set_hexpand(true);
    headers_lbl.set_xalign(0.0);
    let add_hdr_btn = Button::with_label(tr(en, "model.add_header"));
    add_hdr_btn.add_css_class("lna-pill-btn");
    let rows_add = extra_headers_rows.clone();
    let list_add = headers_list.clone();
    let nc_add = notify_changed.clone();
    add_hdr_btn.connect_clicked(move |_| {
        let row_box = GtkBox::new(Orientation::Horizontal, 6);
        let k_ent = Entry::builder().placeholder_text(tr(en, "model.header_name")).hexpand(true).build();
        let v_ent = Entry::builder().placeholder_text(tr(en, "model.value")).hexpand(true).build();
        let del_btn = Button::with_label(tr(en, "studio.delete"));
        del_btn.add_css_class("lna-pill-btn");
        row_box.append(&k_ent);
        row_box.append(&v_ent);
        row_box.append(&del_btn);
        list_add.append(&row_box);
        rows_add.borrow_mut().push((k_ent.clone(), v_ent.clone()));
        for ent in [&k_ent, &v_ent] {
            let nc = nc_add.clone();
            ent.connect_changed(move |_| nc());
        }
        let rows_del = rows_add.clone();
        let list_del = list_add.clone();
        let row_del = row_box.clone();
        let nc = nc_add.clone();
        del_btn.connect_clicked(move |_| {
            rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
            list_del.remove(&row_del);
            nc();
        });
    });
    headers_hdr.append(&headers_lbl);
    headers_hdr.append(&add_hdr_btn);

    adv_box.append(&thinking_row);
    adv_box.append(&Label::new(Some(tr(en, "model.reasoning_effort"))));
    adv_box.append(&effort_dd);
    adv_box.append(&Label::new(Some(tr(en, "model.context_window_max_output_tokens"))));
    adv_box.append(&ctx_entry);
    adv_box.append(&out_entry);
    adv_box.append(&out_hint);
    adv_box.append(&Label::new(Some(tr(en, "model.max_parallel"))));
    adv_box.append(&par_entry);
    adv_box.append(&par_hint);
    adv_box.append(&price_in_entry);
    adv_box.append(&price_out_entry);
    adv_box.append(&headers_hdr);
    adv_box.append(&headers_list);
    adv_exp.set_child(Some(&adv_box));

    // Media Processing & Per-Modality Model Assignment Card
    let media_card = GtkBox::new(Orientation::Vertical, 8);
    media_card.add_css_class("lna-card");
    let av_row = GtkBox::new(Orientation::Horizontal, 8);
    let av_lbl = Label::new(Some(tr(en, "route.enable_native_audio_video_processing")));
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
    let vision_mod_dd = DropDown::from_strings(&[tr(en, "route.auto_prefer_default_chat")]);
    let audio_mod_dd = DropDown::from_strings(&[tr(en, "route.auto_prefer_default_chat")]);
    let video_mod_dd = DropDown::from_strings(&[tr(en, "route.auto_prefer_default_chat")]);
    let routing_summary_lbl = Label::new(None);
    routing_summary_lbl.set_xalign(0.0);
    routing_summary_lbl.set_wrap(true);
    routing_summary_lbl.add_css_class("lna-muted");

    media_card.append(&av_row);
    media_card.append(&Label::new(Some(tr(en, "route.vision_image_pdf"))));
    media_card.append(&vision_mod_dd);
    media_card.append(&Label::new(Some(tr(en, "route.audio_2"))));
    media_card.append(&audio_mod_dd);
    media_card.append(&Label::new(Some(tr(en, "route.video_2"))));
    media_card.append(&video_mod_dd);
    media_card.append(&routing_summary_lbl);

    let model_status_lbl = Label::new(None);
    model_status_lbl.set_xalign(0.0);
    model_status_lbl.set_wrap(true);
    model_status_lbl.add_css_class("lna-muted");

    let model_actions = GtkBox::new(Orientation::Horizontal, 8);
    let save_model_btn = Button::with_label(tr(en, "settings.apply"));
    save_model_btn.add_css_class("lna-primary-btn");
    let test_model_btn = Button::with_label(tr(en, "model.test_connection"));
    test_model_btn.add_css_class("lna-pill-btn");
    let clear_key_btn = Button::with_label(tr(en, "model.clear_key"));
    clear_key_btn.add_css_class("lna-pill-btn");
    model_actions.append(&test_model_btn);
    model_actions.append(&clear_key_btn);

    let presets_val = call_core(&st.core, "profile.presets", json!({}))
        .ok()
        .and_then(|v| v.get("presets").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut preset_labels: Vec<String> = vec![tr(en, "model.select_provider_preset").into()];
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
    // preset id per dropdown slot; slot 0 is "none" ("")
    let preset_ids: Rc<Vec<String>> = Rc::new(
        std::iter::once(String::new())
            .chain(presets_val.iter().map(|pr| pr.get("id").and_then(Value::as_str).unwrap_or("").to_string()))
            .collect(),
    );
    // set_selected fires the preset handler, which fills provider defaults; this suppresses it
    // while a stored profile is loaded into the form
    let preset_quiet: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let shrink_sw = Switch::new();
    let shrink_row = GtkBox::new(Orientation::Horizontal, 8);
    let shrink_lbl = Label::new(Some(tr(en, "model.shrink_media")));
    shrink_lbl.set_hexpand(true);
    shrink_lbl.set_xalign(0.0);
    shrink_row.append(&shrink_lbl);
    shrink_row.append(&shrink_sw);
    let shrink_hint = Label::new(Some(tr(en, "model.shrink_media_hint")));
    shrink_hint.set_wrap(true);
    shrink_hint.set_xalign(0.0);
    shrink_hint.add_css_class("dim-label");
    let shrink_box = GtkBox::new(Orientation::Vertical, 4);
    shrink_box.append(&shrink_row);
    shrink_box.append(&shrink_hint);
    shrink_box.set_visible(false);

    // ffmpeg row under the shrink switch: the app-managed copy, its status, download and remove
    let ffmpeg_status_lbl = Label::new(None);
    ffmpeg_status_lbl.set_xalign(0.0);
    ffmpeg_status_lbl.set_wrap(true);
    let ffmpeg_dl_btn = Button::with_label(tr(en, "ffmpeg.download"));
    ffmpeg_dl_btn.add_css_class("lna-pill-btn");
    let ffmpeg_rm_btn = Button::with_label(tr(en, "ffmpeg.remove"));
    ffmpeg_rm_btn.add_css_class("lna-pill-btn");
    let ffmpeg_btns = GtkBox::new(Orientation::Horizontal, 8);
    ffmpeg_btns.append(&ffmpeg_dl_btn);
    ffmpeg_btns.append(&ffmpeg_rm_btn);
    let ffmpeg_msg_lbl = Label::new(None);
    ffmpeg_msg_lbl.set_xalign(0.0);
    ffmpeg_msg_lbl.set_wrap(true);
    ffmpeg_msg_lbl.add_css_class("lna-muted");
    shrink_box.append(&ffmpeg_status_lbl);
    shrink_box.append(&ffmpeg_btns);
    shrink_box.append(&ffmpeg_msg_lbl);

    // true while our own ffmpeg.install runs; it disables the buttons and enables progress text
    let ffmpeg_busy: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let ffmpeg_refresh: Rc<dyn Fn()> = {
        let core = st.core.clone();
        let busy = ffmpeg_busy.clone();
        let status_lbl = ffmpeg_status_lbl.clone();
        let dl_btn = ffmpeg_dl_btn.clone();
        let rm_btn = ffmpeg_rm_btn.clone();
        Rc::new(move || match call_core(&core, "ffmpeg.status", json!({})) {
            Ok(s) => {
                let installed = s.get("installed").and_then(Value::as_bool).unwrap_or(false);
                let from_app = s.get("source").and_then(Value::as_str) == Some("app");
                let supported = s.get("supported").and_then(Value::as_bool).unwrap_or(false);
                let is_busy = busy.get();
                let mut text = if !installed {
                    tr(en, "ffmpeg.not_installed").to_string()
                } else if from_app {
                    tr(en, "ffmpeg.installed_app").to_string()
                } else {
                    tr(en, "ffmpeg.installed_system").to_string()
                };
                if let Some(v) = s.get("version").and_then(Value::as_str).filter(|v| !v.is_empty()) {
                    text = format!("{text} · {v}");
                }
                if !supported {
                    text.push('\n');
                    text.push_str(tr(en, "ffmpeg.unsupported"));
                }
                status_lbl.set_text(&text);
                dl_btn.set_sensitive(!is_busy && !installed && supported);
                rm_btn.set_visible(installed && from_app);
                rm_btn.set_sensitive(!is_busy);
            }
            Err(e) => status_lbl.set_text(&friendly_err(en, e)),
        })
    };
    // download progress arrives as core events on the "ffmpeg" topic (see start_event_pump)
    {
        let busy = ffmpeg_busy.clone();
        let msg_lbl = ffmpeg_msg_lbl.clone();
        let hook: Rc<dyn Fn(&str, &Value)> = Rc::new(move |event: &str, data: &Value| {
            if event != "ffmpeg.download_progress" || !busy.get() {
                return;
            }
            const MIB: f64 = 1024.0 * 1024.0;
            let done = data.get("downloaded").and_then(Value::as_f64).unwrap_or(0.0) / MIB;
            let line = match data.get("total").and_then(Value::as_f64).filter(|t| *t > 0.0) {
                Some(total) => trf(en, "ffmpeg.downloading", &[&format!("{done:.1}"), &format!("{:.1}", total / MIB)]),
                None => trf(en, "ffmpeg.downloading_unknown", &[&format!("{done:.1}")]),
            };
            msg_lbl.set_text(&line);
        });
        *st.ffmpeg_hook.borrow_mut() = Some(hook);
    }
    // "profile.updated" comes after the core's background capability probe and after profile.test.
    // Only the cached profile list and the read-only detected-inputs label are refreshed here:
    // populate_profile_form is deliberately not called, so unsaved edits, saved_form and the
    // dirty/Save state are left alone.
    {
        let core = st.core.clone();
        let profiles_state = profiles_state.clone();
        let current_profile_id = current_profile_id.clone();
        let detected_lbl = detected_lbl.clone();
        let hook: Rc<dyn Fn(&str, &Value)> = Rc::new(move |_event: &str, _data: &Value| {
            let fresh = call_core(&core, "profile.list", json!({}))
                .ok()
                .and_then(|v| v.get("profiles").and_then(Value::as_array).cloned());
            let Some(fresh) = fresh else { return };
            if let Ok(mut cached) = profiles_state.try_borrow_mut() {
                *cached = fresh;
            }
            let Some(pid) = current_profile_id.borrow().clone() else { return };
            let Ok(cached) = profiles_state.try_borrow() else { return };
            if let Some(p) = cached.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(pid.as_str())) {
                detected_lbl.set_text(&detected_inputs_text(en, Some(p)));
            }
        });
        *st.profile_hook.borrow_mut() = Some(hook);
    }
    {
        let core = st.core.clone();
        let busy = ffmpeg_busy.clone();
        let msg_lbl = ffmpeg_msg_lbl.clone();
        let refresh = ffmpeg_refresh.clone();
        ffmpeg_dl_btn.connect_clicked(move |_| {
            if busy.get() {
                return;
            }
            busy.set(true);
            msg_lbl.set_text(&trf(en, "ffmpeg.downloading_unknown", &[&"0.0"]));
            refresh();
            // ffmpeg.install blocks until the download ends, so it runs off the GTK thread
            let (tx, rx) = std::sync::mpsc::channel();
            let core_w = core.clone();
            std::thread::spawn(move || {
                let _ = tx.send(call_core(&core_w, "ffmpeg.install", json!({})));
            });
            let busy_done = busy.clone();
            let msg_done = msg_lbl.clone();
            let refresh_done = refresh.clone();
            glib::timeout_add_local(Duration::from_millis(250), move || {
                let res = match rx.try_recv() {
                    Ok(r) => r,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err((String::new(), String::new())),
                };
                busy_done.set(false);
                match res {
                    Ok(_) => msg_done.set_text(tr(en, "ffmpeg.download_ok")),
                    Err(e) => msg_done.set_text(&trf(en, "ffmpeg.download_failed", &[&friendly_err(en, e)])),
                }
                refresh_done();
                glib::ControlFlow::Break
            });
        });
    }
    {
        let core = st.core.clone();
        let busy = ffmpeg_busy.clone();
        let msg_lbl = ffmpeg_msg_lbl.clone();
        let refresh = ffmpeg_refresh.clone();
        ffmpeg_rm_btn.connect_clicked(move |_| {
            if busy.get() {
                return;
            }
            match call_core(&core, "ffmpeg.remove", json!({})) {
                Ok(_) => msg_lbl.set_text(tr(en, "ffmpeg.removed")),
                Err(e) => msg_lbl.set_text(&friendly_err(en, e)),
            }
            refresh();
        });
    }
    ffmpeg_refresh();
    // the selected preset id ("" when none)
    let selected_preset: Rc<dyn Fn() -> String> = {
        let ids = preset_ids.clone();
        let dd = preset_dd.clone();
        Rc::new(move || ids.get(dd.selected() as usize).cloned().unwrap_or_default())
    };
    // the shrink option only exists for MiMo, which is the only preset that needs it
    let sync_shrink: Rc<dyn Fn()> = {
        let selected_preset = selected_preset.clone();
        let shrink_box = shrink_box.clone();
        Rc::new(move || shrink_box.set_visible(selected_preset() == "mimo"))
    };

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

            let auto_lbl = tr(en, "route.auto_prefer_default_chat");
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
            let chat_name = chat_m.and_then(|p| p.get("display_name").and_then(Value::as_str)).unwrap_or(tr(en, "route.not_configured"));
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
            let route_off = tr(en, "route.disabled");
            let summary = trf(
                en,
                "route.summary",
                &[
                    &chat_name,
                    &pick_mod(&vp, "supports_vision", tr(en, "route.vision_none")),
                    &(if av_on { pick_mod(&ap, "supports_audio", tr(en, "route.unassigned")) } else { route_off.to_string() }),
                    &(if av_on { pick_mod(&vidp, "supports_video", tr(en, "route.unassigned")) } else { route_off.to_string() }),
                ],
            );
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

    // What the form held when it was last loaded or applied; Apply is enabled only when the
    // form differs from it.
    let saved_form: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let form_snapshot: Rc<dyn Fn() -> String> = {
        let proto_dd = proto_dd.clone();
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let key_entry = key_entry.clone();
        let insecure_http_sw = insecure_http_sw.clone();
        let active_chat_sw = active_chat_sw.clone();
        let thinking_sw = thinking_sw.clone();
        let effort_dd = effort_dd.clone();
        let selected_preset = selected_preset.clone();
        let shrink_sw = shrink_sw.clone();
        let entries = [
            ctx_entry.clone(),
            out_entry.clone(),
            par_entry.clone(),
            price_in_entry.clone(),
            price_out_entry.clone(),
        ];
        let rows = extra_headers_rows.clone();
        Rc::new(move || {
            let mut headers: Vec<String> = rows
                .borrow()
                .iter()
                .filter_map(|(k, v)| {
                    let k = k.text().trim().to_string();
                    (!k.is_empty()).then(|| format!("{k}={}", v.text().trim()))
                })
                .collect();
            headers.sort();
            let mut parts = vec![
                proto_dd.selected().to_string(),
                url_entry.text().trim().to_string(),
                model_entry.text().trim().to_string(),
                if key_entry.text().is_empty() { String::new() } else { "key".into() },
                insecure_http_sw.is_active().to_string(),
                active_chat_sw.is_active().to_string(),
                thinking_sw.is_active().to_string(),
                effort_dd.selected().to_string(),
                selected_preset(),
                // what would be sent: shrink only counts for MiMo
                (selected_preset() == "mimo" && shrink_sw.is_active()).to_string(),
            ];
            parts.extend(entries.iter().map(|e| e.text().trim().to_string()));
            parts.push(headers.join("\n"));
            parts.join("\u{1f}")
        })
    };
    let form_dirty: Rc<dyn Fn() -> bool> = {
        let current_profile_id = current_profile_id.clone();
        let saved_form = saved_form.clone();
        let form_snapshot = form_snapshot.clone();
        Rc::new(move || {
            current_profile_id.borrow().is_none() || form_snapshot() != *saved_form.borrow()
        })
    };
    let sync_model_btns: Rc<dyn Fn()> = {
        let form_dirty = form_dirty.clone();
        let current_profile_id = current_profile_id.clone();
        let current_keychain_ref = current_keychain_ref.clone();
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let save_model_btn = save_model_btn.clone();
        let test_model_btn = test_model_btn.clone();
        let clear_key_btn = clear_key_btn.clone();
        let del_prof_btn = del_prof_btn.clone();
        Rc::new(move || {
            let complete = !url_entry.text().trim().is_empty() && !model_entry.text().trim().is_empty();
            let has_profile = current_profile_id.borrow().is_some();
            save_model_btn.set_sensitive(complete && form_dirty());
            test_model_btn.set_sensitive(has_profile);
            clear_key_btn.set_sensitive(has_profile && !current_keychain_ref.borrow().is_empty());
            del_prof_btn.set_sensitive(has_profile);
        })
    };
    *form_changed_hook.borrow_mut() = Some(sync_model_btns.clone());
    let mark_form_saved: Rc<dyn Fn()> = {
        let saved_form = saved_form.clone();
        let form_snapshot = form_snapshot.clone();
        let sync = sync_model_btns.clone();
        Rc::new(move || {
            let snap = form_snapshot();
            *saved_form.borrow_mut() = snap;
            sync();
        })
    };

    let populate_profile_form = {
        let mark_form_saved = mark_form_saved.clone();
        let nc_p = notify_changed.clone();
        let preset_dd_p = preset_dd.clone();
        let preset_ids_p = preset_ids.clone();
        let preset_quiet_p = preset_quiet.clone();
        let sync_shrink_p = sync_shrink.clone();
        let shrink_sw_p = shrink_sw.clone();
        let current_profile_id = current_profile_id.clone();
        let current_keychain_ref = current_keychain_ref.clone();
        let proto_dd = proto_dd.clone();
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let key_entry = key_entry.clone();
        let insecure_http_sw = insecure_http_sw.clone();
        let active_chat_sw = active_chat_sw.clone();
        let thinking_sw = thinking_sw.clone();
        let effort_dd = effort_dd.clone();
        let ctx_entry = ctx_entry.clone();
        let out_entry = out_entry.clone();
        let par_entry = par_entry.clone();
        let price_in_entry = price_in_entry.clone();
        let price_out_entry = price_out_entry.clone();
        let rows_p = extra_headers_rows.clone();
        let list_p = headers_list.clone();
        let model_status_lbl = model_status_lbl.clone();
        let detected_lbl_p = detected_lbl.clone();
        let ffmpeg_refresh_p = ffmpeg_refresh.clone();
        move |p_opt: Option<&Value>| {
            rows_p.borrow_mut().clear();
            clear_box(&list_p);
            key_entry.set_text("");
            if let Some(p) = p_opt {
                *current_profile_id.borrow_mut() = p.get("id").and_then(Value::as_str).map(String::from);
                *current_keychain_ref.borrow_mut() = p.get("keychain_ref").and_then(Value::as_str).unwrap_or("").to_string();
                let pr = p.get("protocol").and_then(Value::as_str).unwrap_or("chat_completion");
                proto_dd.set_selected(proto_ids.iter().position(|&x| x == pr).unwrap_or(0) as u32);
                let stored_preset = p.get("preset").and_then(Value::as_str).unwrap_or("");
                preset_quiet_p.set(true);
                preset_dd_p.set_selected(preset_ids_p.iter().position(|x| x == stored_preset).unwrap_or(0) as u32);
                preset_quiet_p.set(false);
                sync_shrink_p();
                shrink_sw_p.set_active(p.get("shrink_media").and_then(Value::as_bool).unwrap_or(false));
                url_entry.set_text(p.get("base_url").and_then(Value::as_str).unwrap_or(""));
                model_entry.set_text(p.get("model_id").and_then(Value::as_str).unwrap_or(""));
                // the core does not store this flag; a saved plaintext remote URL was confirmed back then
                let saved_url = p.get("base_url").and_then(Value::as_str).unwrap_or("");
                insecure_http_sw.set_active(
                    p.get("allow_insecure_http").and_then(Value::as_bool).unwrap_or(false)
                        || is_plaintext_remote(saved_url),
                );
                active_chat_sw.set_active(p.get("is_active").and_then(Value::as_bool).unwrap_or(false));
                thinking_sw.set_active(p.get("show_thinking").and_then(Value::as_bool).unwrap_or(true));
                let eff = p.get("effort").and_then(Value::as_str).unwrap_or("");
                effort_dd.set_selected(effort_ids.iter().position(|&x| x == eff).unwrap_or(0) as u32);
                ctx_entry.set_text(&p.get("context_window").and_then(Value::as_i64).unwrap_or(32000).to_string());
                out_entry.set_text(&limit_text(p.get("max_output_tokens")));
                par_entry.set_text(&limit_text(p.get("max_parallel")));
                price_in_entry.set_text(&p.get("input_price_per_m").and_then(Value::as_f64).map(|x| x.to_string()).unwrap_or_default());
                price_out_entry.set_text(&p.get("output_price_per_m").and_then(Value::as_f64).map(|x| x.to_string()).unwrap_or_default());
                if let Some(hdrs) = p.get("extra_headers").and_then(Value::as_object) {
                    for (hk, hv) in hdrs {
                        let row_box = GtkBox::new(Orientation::Horizontal, 6);
                        let k_ent = Entry::builder().text(hk).hexpand(true).build();
                        let v_ent = Entry::builder().text(hv.as_str().unwrap_or("")).hexpand(true).build();
                        let del_btn = Button::with_label(tr(en, "studio.delete"));
                        del_btn.add_css_class("lna-pill-btn");
                        row_box.append(&k_ent);
                        row_box.append(&v_ent);
                        row_box.append(&del_btn);
                        list_p.append(&row_box);
                        rows_p.borrow_mut().push((k_ent.clone(), v_ent.clone()));
                        for ent in [&k_ent, &v_ent] {
                            let nc = nc_p.clone();
                            ent.connect_changed(move |_| nc());
                        }
                        let rows_del = rows_p.clone();
                        let list_del = list_p.clone();
                        let row_del = row_box.clone();
                        let nc = nc_p.clone();
                        del_btn.connect_clicked(move |_| {
                            rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
                            list_del.remove(&row_del);
                            nc();
                        });
                    }
                }
                let has_sec = p.get("has_secret").and_then(Value::as_bool).unwrap_or(false);
                model_status_lbl.set_text(if has_sec {
                    tr(en, "model.key_status_saved_in_system")
                } else {
                    tr(en, "model.key_status_not_saved")
                });
            } else {
                *current_profile_id.borrow_mut() = None;
                *current_keychain_ref.borrow_mut() = String::new();
                proto_dd.set_selected(0);
                preset_quiet_p.set(true);
                preset_dd_p.set_selected(0);
                preset_quiet_p.set(false);
                sync_shrink_p();
                shrink_sw_p.set_active(false);
                url_entry.set_text("");
                model_entry.set_text("");
                insecure_http_sw.set_active(false);
                active_chat_sw.set_active(false);
                thinking_sw.set_active(true);
                effort_dd.set_selected(0);
                ctx_entry.set_text("32000");
                out_entry.set_text("");
                par_entry.set_text("");
                price_in_entry.set_text("");
                price_out_entry.set_text("");
                model_status_lbl.set_text(tr(en, "model.editing_new_profile_choose_a"));
            }
            detected_lbl_p.set_text(&detected_inputs_text(en, p_opt));
            ffmpeg_refresh_p();
            mark_form_saved();
        }
    };
    // every edit re-evaluates whether there is something to apply
    for e in [&url_entry, &model_entry, &key_entry, &ctx_entry, &out_entry, &par_entry, &price_in_entry, &price_out_entry] {
        let s = sync_model_btns.clone();
        e.connect_changed(move |_| s());
    }
    for sw in [&insecure_http_sw, &active_chat_sw, &thinking_sw, &shrink_sw] {
        let s = sync_model_btns.clone();
        sw.connect_active_notify(move |_| s());
    }
    for dd in [&proto_dd, &effort_dd] {
        let s = sync_model_btns.clone();
        dd.connect_selected_notify(move |_| s());
    }

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
            let mut labels = vec![tr(en, "model.new_profile").to_string()];
            let mut target_idx = 0usize;
            for (i, p) in profs.iter().enumerate() {
                let id = p.get("id").and_then(Value::as_str).unwrap_or("");
                let name = p.get("display_name").and_then(Value::as_str).unwrap_or("");
                let act = if p.get("is_active").and_then(Value::as_bool).unwrap_or(false) {
                    tr(en, "model.default")
                } else {
                    ""
                };
                let disabled = if p.get("eligible").and_then(Value::as_bool) == Some(false) {
                    tr(en, "model.disabled_tag")
                } else {
                    ""
                };
                labels.push(format!("{name}{act}{disabled}"));
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
        let model_status_lbl = model_status_lbl.clone();
        // a second click within a few seconds confirms the delete
        let armed: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        del_prof_btn.connect_clicked(move |btn| {
            let pid = current_profile_id.borrow().clone();
            let Some(id) = pid else { return };
            if armed.borrow().as_deref() != Some(id.as_str()) {
                *armed.borrow_mut() = Some(id.clone());
                btn.set_label(tr(en, "model.confirm_delete"));
                let armed2 = armed.clone();
                let btn2 = btn.clone();
                glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
                    let still = armed2.borrow().as_deref() == Some(id.as_str());
                    if still {
                        *armed2.borrow_mut() = None;
                        btn2.set_label(tr(en, "studio.delete"));
                    }
                });
                return;
            }
            *armed.borrow_mut() = None;
            btn.set_label(tr(en, "studio.delete"));
            match call_core(&core, "profile.delete", json!({ "profile_id": id })) {
                Ok(_) => {
                    reload(None);
                    model_status_lbl.set_text(tr(en, "model.profile_deleted"));
                }
                Err(e) => model_status_lbl.set_text(&friendly_err(en, e)),
            }
        });
    }

    {
        let proto_dd = proto_dd.clone();
        let url_p = url_entry.clone();
        let model_p = model_entry.clone();
        let ctx_p = ctx_entry.clone();
        let out_p = out_entry.clone();
        let rows_p = extra_headers_rows.clone();
        let list_p = headers_list.clone();
        let nc_pr = notify_changed.clone();
        let quiet = preset_quiet.clone();
        let sync_shrink = sync_shrink.clone();
        preset_dd.connect_selected_notify(move |dd| {
            // a stored profile is being loaded: its values are already in the form
            if quiet.get() {
                return;
            }
            // the chosen preset stays selected; editing any field afterwards does not clear it
            sync_shrink();
            let idx = dd.selected() as usize;
            if idx == 0 {
                nc_pr();
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
            if pr.get("max_output_tokens").is_some() {
                out_p.set_text(&limit_text(pr.get("max_output_tokens")));
            }
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
                    let del_btn = Button::with_label(tr(en, "studio.delete"));
                    del_btn.add_css_class("lna-pill-btn");
                    row_box.append(&k_ent);
                    row_box.append(&v_ent);
                    row_box.append(&del_btn);
                    list_p.append(&row_box);
                    rows_p.borrow_mut().push((k_ent.clone(), v_ent.clone()));
                    for ent in [&k_ent, &v_ent] {
                        let nc = nc_pr.clone();
                        ent.connect_changed(move |_| nc());
                    }
                    let rows_del = rows_p.clone();
                    let list_del = list_p.clone();
                    let row_del = row_box.clone();
                    let nc = nc_pr.clone();
                    del_btn.connect_clicked(move |_| {
                        rows_del.borrow_mut().retain(|(k, _)| k != &k_ent);
                        list_del.remove(&row_del);
                        nc();
                    });
                }
            }
            nc_pr();
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
                    model_status_lbl.set_text(&trf(en, "model.found_models", &[&(list.len())]));
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

    model_box.append(&model_actions);
    model_box.append(&model_status_lbl);
    model_box.append(&Label::new(Some(tr(en, "model.saved_profiles"))));
    model_box.append(&profile_row);
    model_box.append(&Label::new(Some(tr(en, "model.provider_preset"))));
    model_box.append(&preset_dd);
    model_box.append(&shrink_box);
    model_box.append(&Label::new(Some(tr(en, "model.protocol"))));
    model_box.append(&proto_dd);
    model_box.append(&Label::new(Some(tr(en, "model.base_url"))));
    model_box.append(&url_entry);
    model_box.append(&Label::new(Some(tr(en, "model.model_id"))));
    model_box.append(&model_row);
    model_box.append(&discovered_dd);
    model_box.append(&Label::new(Some(tr(en, "ui.api_key"))));
    model_box.append(&key_entry);
    model_box.append(&insecure_row);
    model_box.append(&active_row);
    model_box.append(&detected_lbl);
    model_box.append(&adv_exp);
    model_box.append(&media_card);

    let st_m = st.clone();
    let rows_save = extra_headers_rows.clone();
    let current_profile_id_s = current_profile_id.clone();
    let reload_s = reload_profile_dropdown.clone();
    let save_media_s = save_media_settings.clone();
    let model_status_s = model_status_lbl.clone();
    let selected_preset_s = selected_preset.clone();
    let shrink_sw_s2 = shrink_sw.clone();
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
        // empty or 0 = unlimited
        let mo: i64 = parse_limit(&out_entry.text());
        let mp: i64 = parse_limit(&par_entry.text());
        let en_now = st_m.is_en();
        let preset = selected_preset_s();
        let shrink = preset == "mimo" && shrink_sw_s2.is_active();
        let mut payload = json!({
            "display_name": format!("{proto} / {model}"),
            "protocol": proto,
            "preset": preset,
            "shrink_media": shrink,
            "base_url": url,
            "model_id": model,
            "allow_insecure_http": insecure_http_sw.is_active(),
            "activate": active_chat_sw.is_active(),
            "show_thinking": thinking_sw.is_active(),
            "context_window": cw,
            "max_output_tokens": mo,
            "max_parallel": mp,
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
        // saving only stores; capabilities are measured by "test connection"
        if !key.is_empty() {
            payload["api_key"] = Value::String(key.clone());
        }
        match call_core(&st_m.core, "profile.upsert", payload) {
            Ok(prof) => {
                let new_id = prof.get("id").and_then(Value::as_str).map(String::from);
                if active_chat_sw.is_active() {
                    if let Some(ref nid) = new_id {
                        let _ = call_core(&st_m.core, "profile.activate", json!({ "profile_id": nid }));
                    }
                }
                save_media_s();
                // reloads the stored profile into the form, which also marks it applied
                reload_s(new_id.as_deref());
                model_status_s.set_text(tr(en_now, "model.saved_checked"));
                st_m.status_label.set_text(tr(en_now, "settings.saved"));
            }
            Err(e) => model_status_s.set_text(&friendly_err(en_now, e)),
        }
    });

    {
        let core = st.core.clone();
        let current_profile_id = current_profile_id.clone();
        let model_status_lbl = model_status_lbl.clone();
        let form_dirty = form_dirty.clone();
        let reload = reload_profile_dropdown.clone();
        let detected_lbl = detected_lbl.clone();
        test_model_btn.connect_clicked(move |_| {
            let Some(pid) = current_profile_id.borrow().clone() else {
                model_status_lbl.set_text(tr(en, "model.save_profile_first"));
                return;
            };
            // the test runs against what is stored, so unapplied edits would not be tested
            if form_dirty() {
                model_status_lbl.set_text(tr(en, "model.apply_before_test"));
                return;
            }
            let mut detected: Option<Value> = None;
            let msg = match call_core(&core, "profile.test", json!({ "profile_id": pid })) {
                Ok(res) => {
                    let reply = res.get("reply").and_then(Value::as_str).unwrap_or("OK");
                    detected = res.get("detected").cloned();
                    let mut msg = trf(en, "model.connected", &[&reply]);
                    for f in res.get("media_failed").and_then(Value::as_array).into_iter().flatten() {
                        let kind = match f.get("kind").and_then(Value::as_str) {
                            Some("vision") => tr(en, "model.vision_images_pdf"),
                            Some("audio") => tr(en, "model.audio"),
                            _ => tr(en, "model.video"),
                        };
                        let detail = f.get("detail").and_then(Value::as_str).unwrap_or("");
                        msg.push('\n');
                        msg.push_str(&trf(en, "model.media_probe_failed", &[&kind, &detail]));
                    }
                    msg
                }
                Err(e) => friendly_err(en, e),
            };
            // the test may have switched off inputs the model does not accept; the form was
            // unchanged before the test, so reloading the stored profile loses nothing
            reload(Some(pid.as_str()));
            // the test result is the freshest detection; it overrides what the reload showed
            if let Some(d) = detected {
                let flag = |k: &str| d.get(k).and_then(Value::as_bool).unwrap_or(false);
                detected_lbl.set_text(&detected_line(en, flag("vision"), flag("audio"), flag("video")));
            }
            model_status_lbl.set_text(&msg);
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
            match call_core(&core, "secret.clear", json!({ "keychain_ref": kref })) {
                Ok(_) => model_status_lbl.set_text(tr(en, "model.api_key_cleared")),
                Err(e) => model_status_lbl.set_text(&friendly_err(en, e)),
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
        trf(en, "emb.status_line", &[&tr(en, if active { "emb.state_on" } else { "emb.state_off" }), &ready, &total])
    } else {
        tr(en, "emb.model_status").into()
    };
    let emb_status = Label::new(Some(&emb_status_text));
    emb_status.set_xalign(0.0);
    let emb_actions = GtkBox::new(Orientation::Horizontal, 8);
    let dl_btn = Button::with_label(tr(en, "emb.download_enable"));
    dl_btn.add_css_class("lna-primary-btn");
    let cancel_dl_btn = Button::with_label(tr(en, "common.cancel"));
    cancel_dl_btn.add_css_class("lna-pill-btn");
    let rebuild_btn = Button::with_label(tr(en, "emb.rebuild_index"));
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

    // "Save" lives in a footer bar (outside the scrolling form) and only applies to the models tab
    tabs.set_vexpand(true);
    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_halign(gtk4::Align::End);
    footer.set_margin_top(8);
    footer.set_margin_bottom(12);
    footer.set_margin_start(16);
    footer.set_margin_end(16);
    footer.append(&save_model_btn);
    footer.set_visible(tabs.current_page() == Some(2));
    {
        let footer = footer.clone();
        tabs.connect_switch_page(move |_, _, idx| footer.set_visible(idx == 2));
    }
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.append(&tabs);
    root.append(&footer);
    win.set_child(Some(&root));
    {
        // the page is gone: stop routing core events into its widgets
        let st_close = st.clone();
        win.connect_close_request(move |_| {
            *st_close.profile_hook.borrow_mut() = None;
            glib::Propagation::Proceed
        });
    }
    win.present();
}

/// Read-only rendering of an unvalidated in-progress payload (`studio.preview.get` result).
/// Mind maps reuse the graph view; everything else is a plain list, falling back to truncated JSON.
fn build_preview_widget(pv: &Value, artifact_type: &str, en: bool, memory: &SharedMemory) -> GtkBox {
    let wrap = GtkBox::new(Orientation::Vertical, 6);
    wrap.set_vexpand(true);
    let note = Label::new(Some(tr(en, "art.preview_unvalidated")));
    note.set_xalign(0.0);
    note.add_css_class("lna-muted");
    wrap.append(&note);

    let stage = pv.get("stage").and_then(Value::as_str).unwrap_or("");
    let payload = pv.get("payload").unwrap_or(&Value::Null);

    if stage == "mindmap_passes" || artifact_type == "mindmap" || artifact_type == "knowledge_graph" {
        let noop: ActionFn = Rc::new(|_, _| {});
        let w = build_mindmap_widget(payload, en, artifact_type == "knowledge_graph", true, Some(memory.clone()), noop);
        w.set_size_request(-1, 320);
        wrap.append(&w);
        return wrap;
    }

    let list = GtkBox::new(Orientation::Vertical, 8);
    let lines = preview_lines(stage, artifact_type, payload);
    if lines.is_empty() {
        let json = serde_json::to_string_pretty(payload).unwrap_or_default();
        let shown: String = json.chars().take(4000).collect();
        let l = Label::new(Some(&shown));
        l.set_xalign(0.0);
        l.set_wrap(true);
        l.set_selectable(true);
        l.add_css_class("lna-muted");
        list.append(&l);
    } else {
        for (bold, text) in lines {
            let l = Label::new(None);
            if bold {
                l.set_markup(&format!("<b>{}</b>", gtk4::glib::markup_escape_text(&text)));
            } else {
                l.set_text(&text);
                l.add_css_class("lna-muted");
            }
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_selectable(true);
            list.append(&l);
        }
    }
    wrap.append(&ScrolledWindow::builder().child(&list).vexpand(true).min_content_height(260).build());
    wrap
}

fn preview_item_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Object(_) => ["text", "label", "title", "heading", "value", "content"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// Flattens a preview payload into (bold, text) lines. An empty result means "show raw JSON".
fn preview_lines(stage: &str, artifact_type: &str, p: &Value) -> Vec<(bool, String)> {
    const MAX_ITEMS: usize = 400;
    const MAX_BODY: usize = 700;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let arr = |v: &Value, k: &str| -> Vec<Value> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().take(MAX_ITEMS).cloned().collect())
            .unwrap_or_default()
    };
    let clip = |t: String, max: usize| -> String {
        if t.chars().count() <= max {
            t
        } else {
            format!("{}…", t.chars().take(max).collect::<String>())
        }
    };
    let mut out: Vec<(bool, String)> = Vec::new();

    if stage == "lecture_passes" || artifact_type == "lecture" || p.get("outline").is_some() {
        let outline = p.get("outline").cloned().unwrap_or(Value::Null);
        let done = p.get("done").cloned().unwrap_or(Value::Null);
        let t = s(&outline, "title");
        if !t.is_empty() {
            out.push((true, t));
        }
        for (i, sec) in arr(&outline, "sections").iter().enumerate() {
            let key = format!("s{}", i + 1);
            let st = s(sec, "title");
            match done.get(&key).filter(|c| c.is_object()) {
                Some(content) => {
                    out.push((true, format!("✓ {}. {}", i + 1, st)));
                    for (n, slide) in arr(content, "slides").iter().enumerate() {
                        out.push((false, format!("   {}. {}", n + 1, s(slide, "title"))));
                    }
                }
                None => out.push((false, format!("{}. {}", i + 1, st))),
            }
        }
        return out;
    }

    let title = s(p, "title");
    if !title.is_empty() {
        out.push((true, title));
    }
    let subtitle = s(p, "subtitle");
    if !subtitle.is_empty() {
        out.push((false, subtitle));
    }
    let kind = if ["quiz", "slides", "report", "flashcards", "infographic", "audio"].contains(&artifact_type) {
        artifact_type
    } else if p.get("questions").is_some() {
        "quiz"
    } else if p.get("slides").is_some() {
        "slides"
    } else if p.get("cards").is_some() {
        "flashcards"
    } else if p.get("blocks").is_some() {
        "infographic"
    } else if p.get("turns").is_some() {
        "audio"
    } else if p.get("sections").is_some() {
        "report"
    } else {
        ""
    };
    match kind {
        "quiz" => {
            for (n, q) in arr(p, "questions").iter().enumerate() {
                out.push((true, format!("{}. {}", n + 1, s(q, "prompt"))));
                for (i, o) in q.get("options").and_then(Value::as_array).into_iter().flatten().enumerate() {
                    let letter = (b'A' + (i.min(25) as u8)) as char;
                    out.push((false, format!("   {}. {}", letter, preview_item_text(o))));
                }
            }
        }
        "slides" => {
            for (n, sl) in arr(p, "slides").iter().enumerate() {
                let t = s(sl, "title");
                out.push((true, format!("{}. {}", n + 1, if t.is_empty() { s(sl, "layout") } else { t })));
                for b in sl.get("bullets").and_then(Value::as_array).into_iter().flatten() {
                    let bt = preview_item_text(b);
                    if !bt.is_empty() {
                        out.push((false, format!("   • {bt}")));
                    }
                }
            }
        }
        "report" => {
            for sec in arr(p, "sections") {
                out.push((true, s(&sec, "heading")));
                let body = s(&sec, "body_md");
                if !body.is_empty() {
                    out.push((false, clip(body, MAX_BODY)));
                }
            }
        }
        "flashcards" => {
            for (n, c) in arr(p, "cards").iter().enumerate() {
                out.push((true, format!("{}. {}", n + 1, s(c, "front"))));
                let back = s(c, "back");
                if !back.is_empty() {
                    out.push((false, format!("   {back}")));
                }
            }
        }
        "infographic" => {
            for b in arr(p, "blocks") {
                let head = s(&b, "heading");
                let metric = s(&b, "metric");
                out.push((true, if metric.is_empty() { head } else { format!("{head}  ·  {metric}") }));
                let sum = s(&b, "summary");
                if !sum.is_empty() {
                    out.push((false, sum));
                }
            }
        }
        "audio" => {
            for t in arr(p, "turns") {
                let sp = s(&t, "speaker");
                let tx = s(&t, "text");
                out.push((false, if sp.is_empty() { tx } else { format!("{sp}: {tx}") }));
            }
        }
        _ => return Vec::new(),
    }
    out
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn streamed_text_extends_an_empty_or_shorter_body() {
        assert_eq!(stream_extension("", "abcd"), Some("abcd"));
        assert_eq!(stream_extension("ab", "abcd"), Some("abcd"));
    }

    #[test]
    fn text_the_card_already_has_is_not_applied_again() {
        // A re-fetch that already holds the finished body, then late deltas of that same text.
        let mut log = StreamLog::default();
        log.push("a1", 3, "ab");
        log.push("a1", 3, "cd");
        assert_eq!(stream_extension("abcd", log.get("a1", 3).unwrap()), None);
        assert_eq!(stream_extension("abcdef", log.get("a1", 3).unwrap()), None);
        // Repeated flushes of the same state change nothing either.
        assert_eq!(stream_extension("abcd", "abcd"), None);
    }

    #[test]
    fn log_is_per_artifact_and_cleared() {
        let mut log = StreamLog::default();
        log.push("a1", 1, "x");
        assert_eq!(log.get("a1", 1), Some("x"));
        assert_eq!(log.get("a2", 1), None);
        log.push("a2", 1, "y");
        assert_eq!(log.get("a1", 1), None);
        assert_eq!(log.get("a2", 1), Some("y"));
        log.clear();
        assert_eq!(log.get("a2", 1), None);
    }
}
