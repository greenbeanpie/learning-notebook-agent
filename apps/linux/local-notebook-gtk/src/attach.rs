//! "+ Add context": pictures, one-off reference files and web links attached to the next message
//! of a text input (chat, Studio request, outline revision, lecture question). Files are sent as
//! they are; the core stores them and the agent reads them on demand.
use crate::i18n::tr;
use base64::Engine as _;
use gtk4::prelude::*;
use gtk4::{gdk, gio, Box as GtkBox, Button, Entry, FlowBox, Label, MenuButton, Orientation, Popover, PositionType, SelectionMode};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Pictures, files and links per message (same limit as the core).
pub const MAX_ITEMS: usize = 6;
const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;
const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Something attached to the next message.
#[derive(Clone)]
pub struct Item {
    /// `image` | `file` | `link`
    pub kind: &'static str,
    pub name: String,
    pub b64: String,
    pub url: Option<String>,
    pub size: u64,
    pub thumb: Option<gdk::Texture>,
}

fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

/// Short label of a link: host and path.
fn link_label(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.trim_end_matches('/').to_string()
}

pub fn valid_link(url: &str) -> bool {
    let u = url.trim();
    (u.starts_with("http://") || u.starts_with("https://")) && u.len() > 8 && !u.contains(char::is_whitespace)
}

/// Replaces `{0}` and `{1}` of a localized string.
pub fn fmt(template: &str, args: &[&str]) -> String {
    let mut s = template.to_string();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

#[derive(Clone)]
pub struct AttachBar {
    items: Rc<RefCell<Vec<Item>>>,
    chips: FlowBox,
    button: MenuButton,
    parent: gtk4::Window,
    en: Rc<dyn Fn() -> bool>,
    notice: Rc<dyn Fn(&str)>,
    // native dialogs must outlive the call that opens them
    keep: Rc<RefCell<Vec<gtk4::FileChooserNative>>>,
}

impl AttachBar {
    /// `en` tells the current UI language; `notice` shows a short message to the user.
    pub fn new(parent: &gtk4::Window, en: Rc<dyn Fn() -> bool>, notice: Rc<dyn Fn(&str)>) -> Self {
        let chips = FlowBox::new();
        chips.set_selection_mode(SelectionMode::None);
        chips.set_row_spacing(6);
        chips.set_column_spacing(6);
        chips.set_homogeneous(false);
        chips.set_visible(false);

        let button = MenuButton::builder().label("+").build();
        button.add_css_class("lna-pill-btn");
        button.set_tooltip_text(Some(tr(en(), "ctx.add")));

        let bar = AttachBar {
            items: Rc::new(RefCell::new(Vec::new())),
            chips,
            button,
            parent: parent.clone(),
            en,
            notice,
            keep: Rc::new(RefCell::new(Vec::new())),
        };
        bar.build_menu();
        bar
    }

    fn en(&self) -> bool {
        (self.en)()
    }

    /// The "+" button that opens the menu.
    pub fn button(&self) -> &MenuButton {
        &self.button
    }

    /// The row of attached items; empty (and hidden) when nothing is attached.
    pub fn chips(&self) -> &FlowBox {
        &self.chips
    }

    pub fn is_empty(&self) -> bool {
        self.items.borrow().is_empty()
    }

    pub fn snapshot(&self) -> Vec<Item> {
        self.items.borrow().clone()
    }

    pub fn restore(&self, items: Vec<Item>) {
        *self.items.borrow_mut() = items;
        self.refresh();
    }

    pub fn clear(&self) {
        self.items.borrow_mut().clear();
        self.refresh();
    }

    /// Puts the attachments into a request as the core expects them (`images`, `files`, `links`).
    pub fn add_to(&self, params: &mut Value) {
        let (mut images, mut files, mut links) = (Vec::new(), Vec::new(), Vec::new());
        for it in self.items.borrow().iter() {
            match it.kind {
                "link" => links.push(json!(it.url.clone().unwrap_or_default())),
                "file" => files.push(json!({ "name": it.name, "data_base64": it.b64 })),
                _ => images.push(json!({ "name": it.name, "data_base64": it.b64 })),
            }
        }
        if let Some(obj) = params.as_object_mut() {
            if !images.is_empty() {
                obj.insert("images".into(), Value::Array(images));
            }
            if !files.is_empty() {
                obj.insert("files".into(), Value::Array(files));
            }
            if !links.is_empty() {
                obj.insert("links".into(), Value::Array(links));
            }
        }
    }

    fn build_menu(&self) {
        let en = self.en();
        let pop = Popover::new();
        pop.set_position(PositionType::Top);
        let list = GtkBox::new(Orientation::Vertical, 2);
        let title = Label::new(Some(tr(en, "ctx.add")));
        title.add_css_class("lna-muted");
        title.set_xalign(0.0);
        list.append(&title);
        let mk = |label: &str| {
            let b = Button::with_label(label);
            b.add_css_class("lna-ghost-btn");
            b
        };
        let (b_img, b_file, b_link) = (mk(tr(en, "ctx.image")), mk(tr(en, "ctx.file")), mk(tr(en, "ctx.link")));
        list.append(&b_img);
        list.append(&b_file);
        list.append(&b_link);
        pop.set_child(Some(&list));
        self.button.set_popover(Some(&pop));

        for (btn, images_only) in [(&b_img, true), (&b_file, false)] {
            let me = self.clone();
            let pop = pop.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                me.pick(images_only);
            });
        }
        let me = self.clone();
        b_link.connect_clicked(move |_| {
            pop.popdown();
            me.ask_link();
        });
    }

    /// Re-applies the localized menu labels (the language can change while the app runs).
    pub fn relabel(&self) {
        self.button.set_tooltip_text(Some(tr(self.en(), "ctx.add")));
        self.build_menu();
        self.refresh();
    }

    fn pick(&self, images_only: bool) {
        let en = self.en();
        let dlg = gtk4::FileChooserNative::new(
            Some(tr(en, if images_only { "ctx.image" } else { "ctx.file" })),
            Some(&self.parent),
            gtk4::FileChooserAction::Open,
            None,
            None,
        );
        dlg.set_select_multiple(true);
        if images_only {
            let f = gtk4::FileFilter::new();
            f.set_name(Some(tr(en, "ctx.image")));
            for e in IMAGE_EXTS {
                f.add_suffix(e);
            }
            dlg.add_filter(&f);
        }
        let me = self.clone();
        dlg.connect_response(move |d, resp| {
            if resp == gtk4::ResponseType::Accept {
                let files = d.files();
                let paths: Vec<std::path::PathBuf> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                me.add_paths(&paths);
            }
            me.keep.borrow_mut().clear();
        });
        dlg.show();
        self.keep.borrow_mut().push(dlg);
    }

    fn ask_link(&self) {
        let en = self.en();
        let win = gtk4::Window::builder()
            .title(tr(en, "ctx.link_title"))
            .transient_for(&self.parent)
            .modal(true)
            .default_width(460)
            .build();
        let vbox = GtkBox::new(Orientation::Vertical, 10);
        vbox.set_margin_top(16);
        vbox.set_margin_bottom(16);
        vbox.set_margin_start(16);
        vbox.set_margin_end(16);
        let entry = Entry::builder().placeholder_text("https://").hexpand(true).build();
        let err = Label::new(None);
        err.set_xalign(0.0);
        err.add_css_class("lna-muted");
        let add = Button::with_label(tr(en, "ctx.link_add"));
        add.add_css_class("suggested-action");
        add.set_halign(gtk4::Align::End);
        vbox.append(&entry);
        vbox.append(&err);
        vbox.append(&add);
        win.set_child(Some(&vbox));

        let submit = {
            let me = self.clone();
            let entry = entry.clone();
            let win = win.clone();
            let err = err.clone();
            move || {
                let url = entry.text().trim().to_string();
                if !valid_link(&url) {
                    err.set_text(tr(me.en(), "ctx.link_bad"));
                    return;
                }
                me.add_link(&url);
                win.close();
            }
        };
        let s1 = submit.clone();
        add.connect_clicked(move |_| s1());
        entry.connect_activate(move |_| submit());
        win.present();
    }

    pub fn add_link(&self, url: &str) {
        if self.items.borrow().len() >= MAX_ITEMS {
            self.too_many();
            return;
        }
        self.items.borrow_mut().push(Item {
            kind: "link",
            name: link_label(url),
            b64: String::new(),
            url: Some(url.to_string()),
            size: 0,
            thumb: None,
        });
        self.refresh();
    }

    fn too_many(&self) {
        (self.notice)(&fmt(tr(self.en(), "ctx.too_many"), &[&MAX_ITEMS.to_string()]));
    }

    /// Adds files from disk; pictures become pictures, everything else a reference file.
    pub fn add_paths(&self, paths: &[std::path::PathBuf]) {
        for p in paths {
            if self.items.borrow().len() >= MAX_ITEMS {
                self.too_many();
                break;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            let image = is_image_name(&name);
            let limit = if image { MAX_IMAGE_BYTES } else { MAX_FILE_BYTES };
            let read = std::fs::metadata(p).map_err(|e| e.to_string()).and_then(|m| {
                if m.len() == 0 || m.len() > limit {
                    Err(fmt(tr(self.en(), "ctx.too_large"), &[&name, &(limit / 1024 / 1024).to_string()]))
                } else {
                    std::fs::read(p).map_err(|e| e.to_string())
                }
            });
            match read {
                Ok(bytes) => {
                    let thumb = if image {
                        gdk::Texture::from_bytes(&gtk4::glib::Bytes::from(&bytes)).ok()
                    } else {
                        None
                    };
                    if image && thumb.is_none() {
                        (self.notice)(&fmt(tr(self.en(), "ctx.bad_image"), &[&name]));
                        continue;
                    }
                    self.items.borrow_mut().push(Item {
                        kind: if image { "image" } else { "file" },
                        name,
                        b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        url: None,
                        size: bytes.len() as u64,
                        thumb,
                    });
                }
                Err(e) => (self.notice)(&fmt(tr(self.en(), "ctx.read_failed"), &[&name, &e])),
            }
        }
        self.refresh();
    }

    /// Lets files dropped on `widget` be attached.
    pub fn accept_drops(&self, widget: &impl IsA<gtk4::Widget>) {
        let target = gtk4::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let me = self.clone();
        target.connect_drop(move |_, value, _, _| {
            if let Ok(list) = value.get::<gdk::FileList>() {
                let paths: Vec<_> = list.files().iter().filter_map(|f| f.path()).collect();
                me.add_paths(&paths);
                return true;
            }
            false
        });
        widget.add_controller(target);
    }

    /// Ctrl+V inside `widget`: copied files (a file manager puts the path next to the file, which a text field
    /// would paste) and pictures (screenshots, "Copy Image") are attached; plain text pastes as usual.
    pub fn accept_paste(&self, widget: &impl IsA<gtk4::Widget>) {
        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let me = self.clone();
        let host = widget.clone().upcast::<gtk4::Widget>();
        keys.connect_key_pressed(move |_, key, _, state| {
            let ctrl_v = state.contains(gdk::ModifierType::CONTROL_MASK) && (key == gdk::Key::v || key == gdk::Key::V);
            if !ctrl_v {
                return gtk4::glib::Propagation::Proceed;
            }
            let clipboard = host.clipboard();
            let formats = clipboard.formats();
            if formats.contain_type(gdk::FileList::static_type()) {
                let me = me.clone();
                clipboard.read_value_async(
                    gdk::FileList::static_type(),
                    gtk4::glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |res| {
                        if let Ok(value) = res {
                            if let Ok(list) = value.get::<gdk::FileList>() {
                                let paths: Vec<_> = list.files().iter().filter_map(|f| f.path()).collect();
                                me.add_paths(&paths);
                            }
                        }
                    },
                );
                return gtk4::glib::Propagation::Stop;
            }
            // A picture, unless the clipboard also carries text (copying spreadsheet cells puts both on it).
            if formats.contain_type(gdk::Texture::static_type()) && !formats.contain_type(String::static_type()) {
                let me = me.clone();
                clipboard.read_texture_async(gio::Cancellable::NONE, move |res| {
                    if let Ok(Some(texture)) = res {
                        me.add_texture(&texture);
                    }
                });
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        widget.add_controller(keys);
    }

    /// Attaches a picture that came from the clipboard as a PNG.
    fn add_texture(&self, texture: &gdk::Texture) {
        if self.items.borrow().len() >= MAX_ITEMS {
            self.too_many();
            return;
        }
        let png = texture.save_to_png_bytes();
        let bytes: &[u8] = png.as_ref();
        let stamp = gtk4::glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%Y%m%d-%H%M%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let name = format!("{} {}.png", tr(self.en(), "img.pasted"), stamp);
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            (self.notice)(&fmt(tr(self.en(), "ctx.too_large"), &[&name, &(MAX_IMAGE_BYTES / 1024 / 1024).to_string()]));
            return;
        }
        self.items.borrow_mut().push(Item {
            kind: "image",
            name,
            b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            url: None,
            size: bytes.len() as u64,
            thumb: Some(texture.clone()),
        });
        self.refresh();
    }

    fn refresh(&self) {
        while let Some(c) = self.chips.first_child() {
            self.chips.remove(&c);
        }
        let items = self.items.borrow().clone();
        self.chips.set_visible(!items.is_empty());
        let en = self.en();
        for (i, it) in items.iter().enumerate() {
            let chip = GtkBox::new(Orientation::Horizontal, 6);
            chip.add_css_class("lna-card");
            if let Some(t) = &it.thumb {
                let pic = gtk4::Picture::for_paintable(t);
                pic.set_size_request(44, 44);
                pic.set_content_fit(gtk4::ContentFit::Cover);
                chip.append(&pic);
            } else {
                chip.append(&Label::new(Some(if it.kind == "link" { "🔗" } else { "📄" })));
            }
            let text = if it.kind == "file" {
                format!("{} · {}", it.name, size_text(it.size))
            } else {
                it.name.clone()
            };
            let lbl = Label::new(Some(&text));
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            lbl.set_max_width_chars(28);
            chip.append(&lbl);
            let x = Button::with_label("×");
            x.add_css_class("lna-ghost-btn");
            x.set_tooltip_text(Some(tr(en, "ctx.remove")));
            let me = self.clone();
            x.connect_clicked(move |_| {
                let mut v = me.items.borrow_mut();
                if i < v.len() {
                    v.remove(i);
                }
                drop(v);
                me.refresh();
            });
            chip.append(&x);
            self.chips.append(&chip);
        }
    }
}

/// Opens a stored attachment (file) or a link with the system default application.
pub fn open_attachment(att: &Value) {
    let target = if att.get("kind").and_then(Value::as_str) == Some("link") {
        att.get("url").and_then(Value::as_str).map(str::to_string)
    } else {
        att.get("path")
            .and_then(Value::as_str)
            .map(|p| gio::File::for_path(p).uri().to_string())
    };
    if let Some(uri) = target {
        let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
    }
}

/// The attachments of a stored message: pictures as thumbnails, files and links as chips. Clicking
/// one opens it; `on_promote` adds it to the sources.
pub fn attachments_row(atts: &Value, en: bool, on_promote: Rc<dyn Fn(&str)>) -> Option<FlowBox> {
    let arr = atts.as_array().filter(|a| !a.is_empty())?;
    let row = FlowBox::new();
    row.set_selection_mode(SelectionMode::None);
    row.set_row_spacing(6);
    row.set_column_spacing(6);
    row.set_homogeneous(false);
    row.set_halign(gtk4::Align::Start);
    for a in arr {
        let kind = a.get("kind").and_then(Value::as_str).unwrap_or("image");
        let name = a.get("name").and_then(Value::as_str).unwrap_or("");
        let id = a.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let chip = GtkBox::new(Orientation::Horizontal, 6);
        chip.add_css_class("lna-card");
        let open = Button::new();
        open.add_css_class("lna-ghost-btn");
        let inner = GtkBox::new(Orientation::Horizontal, 6);
        let path = a.get("path").and_then(Value::as_str).unwrap_or("");
        if kind == "image" && Path::new(path).exists() {
            let pic = gtk4::Picture::for_filename(path);
            pic.set_size_request(120, 90);
            pic.set_content_fit(gtk4::ContentFit::Contain);
            inner.append(&pic);
        } else {
            inner.append(&Label::new(Some(if kind == "link" { "🔗" } else { "📄" })));
            let shown = if kind == "link" {
                a.get("url").and_then(Value::as_str).map(link_label).unwrap_or_else(|| name.to_string())
            } else {
                let bytes = a.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                format!("{name} · {}", size_text(bytes))
            };
            let l = Label::new(Some(&shown));
            l.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            l.set_max_width_chars(30);
            inner.append(&l);
        }
        open.set_child(Some(&inner));
        let att = a.clone();
        open.connect_clicked(move |_| open_attachment(&att));
        chip.append(&open);
        if kind != "image" {
            let promote = Button::with_label(tr(en, "ctx.promote"));
            promote.add_css_class("lna-ghost-btn");
            let cb = on_promote.clone();
            promote.connect_clicked(move |_| cb(&id));
            chip.append(&promote);
        }
        row.append(&chip);
    }
    Some(row)
}
