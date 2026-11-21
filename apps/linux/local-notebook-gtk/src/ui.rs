use crate::icons::{studio_icon_image, svg_image, SVG_BRAIN, SVG_CLOSE, SVG_FULLSCREEN, SVG_RING, SVG_SEARCH_DOC};
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, Entry, Grid, Label,
    MenuButton, Orientation, Paned, Popover, PositionType, ScrolledWindow, Stack, TextView,
};
use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

fn call_core(core: &Arc<CoreRuntime>, method: &str, params: Value) -> Result<Value, String> {
    let req = json!({ "id": "gtk", "method": method, "params": params });
    let raw = core.handle_json(&req.to_string());
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if let Some(err) = parsed.get("error") {
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("Error");
        return Err(msg.to_string());
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

struct UiState {
    core: Arc<CoreRuntime>,
    active_notebook_id: RefCell<String>,
    studio_fullscreen: RefCell<bool>,
    sources_box: GtkBox,
    chat_box: GtkBox,
    artifacts_box: GtkBox,
    summary_label: Label,
    status_label: Label,
    studio_stack: Stack,
    studio_detail_box: GtkBox,
    left_panel: GtkBox,
    center_panel: GtkBox,
    outer_paned: Paned,
    inner_paned: Paned,
}

pub fn build_main_window(app: &Application, core: Arc<CoreRuntime>) {
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

    // Top bar
    let header = GtkBox::new(Orientation::Horizontal, 10);
    let title_lbl = Label::new(Some("Local Notebook Agent"));
    title_lbl.set_hexpand(true);
    title_lbl.set_halign(gtk4::Align::Start);

    let new_nb_btn = Button::with_label("新建笔记本");
    new_nb_btn.add_css_class("lna-pill-btn");
    let embed_btn = Button::with_label("语义检索");
    embed_btn.add_css_class("lna-pill-btn");
    let settings_btn = Button::with_label("模型协议配置");
    settings_btn.add_css_class("lna-pill-btn");

    header.append(&title_lbl);
    header.append(&new_nb_btn);
    header.append(&embed_btn);
    header.append(&settings_btn);
    root.append(&header);

    // Left panel: Sources (downward popover)
    let left_panel = GtkBox::new(Orientation::Vertical, 10);
    left_panel.add_css_class("lna-panel");
    left_panel.set_width_request(280);
    let src_hdr = Label::new(Some("来源"));
    src_hdr.set_halign(gtk4::Align::Start);
    left_panel.append(&src_hdr);

    let src_actions = GtkBox::new(Orientation::Horizontal, 8);
    let add_src_btn = MenuButton::builder().label("+ 添加来源").build();
    add_src_btn.add_css_class("lna-pill-btn");
    let add_popover = Popover::new();
    add_popover.set_position(PositionType::Bottom); // Requirement 7: always downward
    let pop_list = GtkBox::new(Orientation::Vertical, 4);
    let add_paste_btn = Button::with_label("粘贴文本");
    let add_url_btn = Button::with_label("网页地址");
    pop_list.append(&add_paste_btn);
    pop_list.append(&add_url_btn);
    add_popover.set_child(Some(&pop_list));
    add_src_btn.set_popover(Some(&add_popover));

    let add_note_btn = Button::with_label("+ 创建笔记");
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

    let summary_label = Label::new(Some("请添加来源或直接提问。"));
    summary_label.set_wrap(true);
    summary_label.set_xalign(0.0);
    let summary_card = GtkBox::new(Orientation::Vertical, 6);
    summary_card.add_css_class("lna-card");
    summary_card.append(&summary_label);
    center_panel.append(&summary_card);

    let chat_box = GtkBox::new(Orientation::Vertical, 12);
    let chat_scroll = ScrolledWindow::builder()
        .child(&chat_box)
        .vexpand(true)
        .build();
    center_panel.append(&chat_scroll);

    let input_row = GtkBox::new(Orientation::Horizontal, 8);
    let chat_entry = Entry::builder()
        .placeholder_text("提问或创作内容...")
        .hexpand(true)
        .build();
    let send_btn = Button::with_label("发送");
    send_btn.add_css_class("lna-primary-btn");
    input_row.append(&chat_entry);
    input_row.append(&send_btn);
    center_panel.append(&input_row);

    // Right panel: Studio (Home 7-tile grid OR Inline Artifact Viewer)
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
        ("audio", "音频概览"),
        ("slides", "演示文稿"),
        ("mindmap", "思维导图"),
        ("report", "报告"),
        ("flashcards", "闪卡"),
        ("quiz", "测验"),
        ("infographic", "信息图"),
    ];

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

    let status_label = Label::new(Some("就绪"));
    status_label.add_css_class("lna-muted");
    status_label.set_halign(gtk4::Align::Start);
    root.append(&status_label);

    let state = Rc::new(UiState {
        core: core.clone(),
        active_notebook_id: RefCell::new(String::new()),
        studio_fullscreen: RefCell::new(false),
        sources_box,
        chat_box,
        artifacts_box,
        summary_label,
        status_label,
        studio_stack,
        studio_detail_box,
        left_panel,
        center_panel,
        outer_paned,
        inner_paned,
    });

    // Wire 7 Studio tiles (no video modality)
    for (idx, (kind, label)) in modalities.iter().enumerate() {
        let btn = Button::new();
        btn.add_css_class("lna-studio-tile");
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.append(&studio_icon_image(kind));
        row.append(&Label::new(Some(label)));
        btn.set_child(Some(&row));
        let st = state.clone();
        let k = (*kind).to_string();
        btn.connect_clicked(move |_| {
            let nid = st.active_notebook_id.borrow().clone();
            if nid.is_empty() { return; }
            let cfg = if k == "slides" { json!({ "pages": 12, "reviewOutline": true }) } else { json!({}) };
            match call_core(&st.core, "studio.create", json!({ "notebook_id": nid, "artifact_type": k, "config": cfg })) {
                Ok(_) => {
                    st.status_label.set_text("已提交 Studio 生成任务。");
                    refresh_all(&st);
                }
                Err(e) => st.status_label.set_text(&format!("生成失败: {e}")),
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
            if text.is_empty() { return; }
            entry.set_text("");
            let nid = st.active_notebook_id.borrow().clone();
            match call_core(&st.core, "chat.send", json!({ "notebook_id": nid, "text": text })) {
                Ok(_) => refresh_all(&st),
                Err(e) => st.status_label.set_text(&format!("对话失败: {e}")),
            }
        });
    }

    // Wire New Notebook
    {
        let st = state.clone();
        new_nb_btn.connect_clicked(move |_| {
            if let Ok(nb) = call_core(&st.core, "notebook.create", json!({ "title": "新笔记本" })) {
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
            let _ = call_core(&st.core, "source.add_text", json!({ "notebook_id": nid, "title": "粘贴笔记.txt", "text": "示例来源正文内容。" }));
            refresh_all(&st);
        });
    }
    {
        let st = state.clone();
        add_note_btn.connect_clicked(move |_| {
            let nid = st.active_notebook_id.borrow().clone();
            let _ = call_core(&st.core, "note.save", json!({ "notebook_id": nid, "title": "新笔记", "body_md": "" }));
            refresh_all(&st);
        });
    }

    // Wire Model Settings dialog (all 4 protocols)
    {
        let st = state.clone();
        let win = window.clone();
        settings_btn.connect_clicked(move |_| open_settings_dialog(&win, &st));
    }

    // Initialize first notebook
    if let Ok(list) = call_core(&core, "notebook.list", json!({})) {
        if let Some(id) = list.pointer("/notebooks/0/id").and_then(Value::as_str) {
            *state.active_notebook_id.borrow_mut() = id.to_string();
        } else if let Ok(nb) = call_core(&core, "notebook.create", json!({ "title": "未命名笔记本" })) {
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

fn refresh_all(st: &Rc<UiState>) {
    let nid = st.active_notebook_id.borrow().clone();
    if nid.is_empty() { return; }

    if let Ok(nb) = call_core(&st.core, "notebook.get", json!({ "notebook_id": nid })) {
        let sum = nb.get("summary_md").and_then(Value::as_str).unwrap_or("请添加来源或直接提问。");
        st.summary_label.set_text(sum);
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
                let lbl = Label::new(Some(title));
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                let cb = CheckButton::new();
                cb.set_active(selected);
                let st2 = st.clone();
                cb.connect_toggled(move |c| {
                    let _ = call_core(&st2.core, "source.set_selected", json!({ "source_id": sid, "selected": c.is_active() }));
                });
                row.append(&lbl);
                row.append(&cb);
                st.sources_box.append(&row);
            }
        }
    }

    // Chat messages with inconspicuous Thoughts button + Brain/Magnifier SVGs
    clear_box(&st.chat_box);
    if let Ok(res) = call_core(&st.core, "chat.history", json!({ "notebook_id": nid })) {
        if let Some(msgs) = res.get("messages").and_then(Value::as_array) {
            for m in msgs {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("assistant");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                let card = GtkBox::new(Orientation::Vertical, 6);
                let role_lbl = Label::new(Some(if role == "user" { "你" } else { "助手" }));
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
                            t_list.set_visible(false);
                            for th in thoughts {
                                let kind = th.get("kind").and_then(Value::as_str).unwrap_or("thought_stage");
                                let title = th.get("title").and_then(Value::as_str).unwrap_or("");
                                let detail = th.get("detail").and_then(Value::as_str).unwrap_or("");
                                let is_tool = matches!(kind, "retrieve" | "tool_search" | "tool_read_context" | "tool_list_sources" | "tool_call");

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
            for a in arr {
                let aid = a.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let kind = a.get("artifact_type").and_then(Value::as_str).unwrap_or("report");
                let title = a.get("title").and_then(Value::as_str).unwrap_or("产物");
                let status = a.get("status").and_then(Value::as_str).unwrap_or("PENDING");
                let btn = Button::new();
                btn.add_css_class("lna-card");
                let row = GtkBox::new(Orientation::Horizontal, 10);
                row.append(&studio_icon_image(kind));
                let col = GtkBox::new(Orientation::Vertical, 2);
                let t = Label::new(Some(title));
                t.set_xalign(0.0);
                let sub = Label::new(Some(status));
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

fn open_artifact_inline(st: &Rc<UiState>, artifact_id: &str) {
    let Ok(art) = call_core(&st.core, "studio.get", json!({ "artifact_id": artifact_id })) else {
        return;
    };
    clear_box(&st.studio_detail_box);

    // Header with Fullscreen & Close buttons (Requirement 3)
    let hdr = GtkBox::new(Orientation::Horizontal, 8);
    let title = art.get("title").and_then(Value::as_str).unwrap_or("产物");
    let tl = Label::new(Some(title));
    tl.set_hexpand(true);
    tl.set_xalign(0.0);

    let fs_btn = Button::new();
    fs_btn.set_child(Some(&svg_image(SVG_FULLSCREEN, 16)));
    fs_btn.set_tooltip_text(Some("全屏显示 / 还原"));
    let st_fs = st.clone();
    fs_btn.connect_clicked(move |_| {
        let mut fs = st_fs.studio_fullscreen.borrow_mut();
        *fs = !*fs;
        st_fs.left_panel.set_visible(!*fs);
        st_fs.center_panel.set_visible(!*fs);
    });

    let close_btn = Button::new();
    close_btn.set_child(Some(&svg_image(SVG_CLOSE, 16)));
    close_btn.set_tooltip_text(Some("关闭详情"));
    let st_cl = st.clone();
    close_btn.connect_clicked(move |_| {
        *st_cl.studio_fullscreen.borrow_mut() = false;
        st_cl.left_panel.set_visible(true);
        st_cl.center_panel.set_visible(true);
        st_cl.studio_stack.set_visible_child_name("home");
        st_cl.inner_paned.set_position(660);
    });

    hdr.append(&tl);
    hdr.append(&fs_btn);
    hdr.append(&close_btn);
    st.studio_detail_box.append(&hdr);

    let status = art.get("status").and_then(Value::as_str).unwrap_or("");
    let kind = art.get("artifact_type").and_then(Value::as_str).unwrap_or("");
    if status == "DRAFT_REVIEW" {
        // Requirement 5: Markdown outline preview/editor, never raw JSON
        let draft_md = art
            .get("draft")
            .and_then(Value::as_str)
            .unwrap_or("# 演示文稿提纲\n\n## 第 1 页：核心主题\n- 要点说明");
        let tv = TextView::new();
        tv.buffer().set_text(draft_md);
        tv.set_vexpand(true);
        let approve = Button::with_label("批准并生成演示文稿");
        approve.add_css_class("lna-primary-btn");
        let st_ap = st.clone();
        let aid = artifact_id.to_string();
        let buf = tv.buffer();
        approve.connect_clicked(move |_| {
            let (s, e) = buf.bounds();
            let text = buf.text(&s, &e, false).to_string();
            let _ = call_core(&st_ap.core, "studio.approve_draft", json!({ "artifact_id": aid, "draft": text }));
            refresh_all(&st_ap);
        });
        st.studio_detail_box.append(&ScrolledWindow::builder().child(&tv).vexpand(true).build());
        st.studio_detail_box.append(&approve);
    } else if kind == "report" {
        // Requirement 4: Report TOC + Content split view
        let split = GtkBox::new(Orientation::Horizontal, 12);
        split.set_vexpand(true);
        let toc = GtkBox::new(Orientation::Vertical, 4);
        toc.add_css_class("lna-card");
        toc.set_width_request(180);
        toc.append(&Label::new(Some("目录")));
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
        // Requirement 4: Infographic card canvas
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

    // Widen Studio panel and narrow Chat panel (Requirement 3)
    st.studio_stack.set_visible_child_name("detail");
    st.inner_paned.set_position(360);
}

fn open_settings_dialog(parent: &ApplicationWindow, st: &Rc<UiState>) {
    let win = gtk4::Window::builder()
        .title("模型协议配置")
        .transient_for(parent)
        .modal(true)
        .default_width(480)
        .default_height(420)
        .build();
    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    let proto_entry = Entry::builder().text("chat_completion").placeholder_text("chat_completion / responses / anthropic / gemini").build();
    let url_entry = Entry::builder().placeholder_text("Base URL (e.g. https://api.openai.com/v1)").build();
    let model_entry = Entry::builder().placeholder_text("Model ID").build();
    let key_entry = Entry::builder().placeholder_text("API Key (stored in OS Secret Service)").visibility(false).build();
    let save_btn = Button::with_label("保存并启用");
    save_btn.add_css_class("lna-primary-btn");

    vbox.append(&Label::new(Some("协议 (chat_completion / responses / anthropic / gemini)")));
    vbox.append(&proto_entry);
    vbox.append(&Label::new(Some("接口地址")));
    vbox.append(&url_entry);
    vbox.append(&Label::new(Some("模型 ID")));
    vbox.append(&model_entry);
    vbox.append(&Label::new(Some("API Key")));
    vbox.append(&key_entry);
    vbox.append(&save_btn);

    let st2 = st.clone();
    let w2 = win.clone();
    save_btn.connect_clicked(move |_| {
        let proto = proto_entry.text().to_string();
        let url = url_entry.text().to_string();
        let model = model_entry.text().to_string();
        let key = key_entry.text().to_string();
        if let Ok(prof) = call_core(&st2.core, "profile.upsert", json!({
            "display_name": format!("{proto} / {model}"),
            "protocol": proto,
            "base_url": url,
            "model_id": model,
            "activate": true
        })) {
            if let Some(kref) = prof.get("keychain_ref").and_then(Value::as_str) {
                if !key.is_empty() {
                    let _ = call_core(&st2.core, "secret.set", json!({ "keychain_ref": kref, "secret": key }));
                }
            }
            st2.status_label.set_text("模型配置已保存并启用。");
            w2.close();
        }
    });

    win.set_child(Some(&vbox));
    win.present();
}
