//! The narrated lecture ("讲解") player: the shared deck page in lecture mode inside a WebKitGTK
//! view, driven sentence by sentence with the system speech engine (speech-dispatcher or
//! espeak-ng) or the core's cached cloud voice, plus captions, checkpoints, interruption
//! questions and adaptations of the slides ahead.
use crate::attach::AttachBar;
use crate::i18n::{tr, trf};
use crate::lecture_model::{
    build_sequence, deck_html, js_string, next_rate, payload_with_figures, step_before, LecSentence,
};
use crate::ui::call_core;
use gtk4::prelude::*;
use gtk4::{gio, glib, Box as GtkBox, Button, CheckButton, Entry, Label, MenuButton, Orientation, Popover, ScrolledWindow};
use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use webkit6::prelude::*;

/// A question of the learner and the teacher's answer.
#[derive(Clone, Default)]
struct Qa {
    question: String,
    images: Vec<String>,
    answer: String,
    speak: String,
    pending: bool,
    error: String,
    suggest_kind: String,
    suggest_reason: String,
}

struct Player {
    core: Arc<CoreRuntime>,
    en: bool,
    window: gtk4::Window,
    artifact_id: String,
    web: webkit6::WebView,
    payload: Value,
    output_dir: String,
    seq: Vec<LecSentence>,
    pos: usize,
    playing: bool,
    captions: bool,
    rate: f64,
    engine: String,
    policy: String,
    web_ready: bool,
    /// Bumped whenever playback is paused or restarted; a loop of an older generation stops.
    generation: u64,
    speech: Option<gio::Subprocess>,
    media: Option<gtk4::MediaFile>,
    checkpoint: Option<LecSentence>,
    qa: Vec<Qa>,
    adapting: bool,
    adaptation: Option<Value>,
    // widgets
    play_btn: Button,
    counter: Label,
    rate_btn: Button,
    status: Label,
    adapt_row: GtkBox,
    ask_panel: GtkBox,
    checkpoint_box: GtkBox,
    qa_box: GtkBox,
}

type Shared = Rc<RefCell<Player>>;

fn t(p: &Shared, key: &str) -> &'static str {
    tr(p.borrow().en, key)
}

fn js(p: &Shared, script: &str) {
    let pl = p.borrow();
    if pl.web_ready {
        pl.web.evaluate_javascript(script, None, None, gio::Cancellable::NONE, |_| {});
    }
}

fn caption(p: &Shared, text: &str) {
    let on = p.borrow().captions;
    js(p, &format!("lecture.caption({})", if on { js_string(text) } else { "''".into() }));
}

fn show_position(p: &Shared) {
    let (slide, step, text) = {
        let pl = p.borrow();
        let Some(s) = pl.seq.get(pl.pos) else { return };
        (s.slide, step_before(&pl.seq, pl.pos), s.text.clone())
    };
    js(p, &format!("lecture.show({slide},{step})"));
    caption(p, &text);
}

fn update_counter(p: &Shared) {
    let pl = p.borrow();
    let total = pl.payload.get("slides").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let cur = pl.seq.get(pl.pos).map(|s| s.slide).unwrap_or(0) + 1;
    pl.counter.set_text(&format!("{cur}/{total}"));
    pl.play_btn.set_label(if pl.playing { "⏸" } else { "▶" });
    pl.rate_btn.set_label(&format!("{}×", pl.rate));
}

fn set_status(p: &Shared, msg: &str) {
    p.borrow().status.set_text(msg);
}

fn save_progress(p: &Shared) {
    let (core, id, pos_id) = {
        let pl = p.borrow();
        let Some(s) = pl.seq.get(pl.pos) else { return };
        (pl.core.clone(), pl.artifact_id.clone(), s.id.clone())
    };
    glib::spawn_future_local(async move {
        let _ = gio::spawn_blocking(move || {
            call_core(&core, "studio.save_progress", json!({ "artifact_id": id, "state": { "position": pos_id } }))
        })
        .await;
    });
}

/// Calls the core off the GTK thread.
async fn core_call(en: bool, core: Arc<CoreRuntime>, method: &'static str, params: Value) -> Result<Value, String> {
    match gio::spawn_blocking(move || call_core(&core, method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err((code, msg))) => Err(if !msg.is_empty() {
            msg
        } else if !code.is_empty() {
            code
        } else {
            tr(en, "err.unknown").to_string()
        }),
        Err(_) => Err(tr(en, "err.unknown").to_string()),
    }
}

// ---- speech -----------------------------------------------------------------------------------

fn stop_audio(p: &Shared) {
    let mut pl = p.borrow_mut();
    if let Some(proc) = pl.speech.take() {
        proc.force_exit();
    }
    if let Some(m) = pl.media.take() {
        m.pause();
    }
}

async fn speak_system(p: &Shared, text: &str, gen: u64) -> bool {
    let rate = p.borrow().rate;
    let pct = (((rate - 1.0) * 100.0) as i32).clamp(-100, 100);
    let wpm = ((175.0 * rate) as i32).max(80);
    let attempts: Vec<Vec<String>> = vec![
        vec!["spd-say".into(), "--wait".into(), "-r".into(), pct.to_string(), text.into()],
        vec!["espeak-ng".into(), "-s".into(), wpm.to_string(), text.into()],
    ];
    for argv in attempts {
        let refs: Vec<&std::ffi::OsStr> = argv.iter().map(|a| std::ffi::OsStr::new(a.as_str())).collect();
        let Ok(proc) = gio::Subprocess::newv(&refs, gio::SubprocessFlags::STDERR_SILENCE) else {
            continue;
        };
        p.borrow_mut().speech = Some(proc.clone());
        let done = proc.wait_future().await;
        if p.borrow().generation != gen {
            return false;
        }
        p.borrow_mut().speech = None;
        if done.is_ok() {
            return true;
        }
    }
    // no speech engine: keep the captions up for as long as the sentence would take to read
    let ms = ((text.chars().count() as f64) * 180.0 / rate.max(0.5)) as u64;
    glib::timeout_future(Duration::from_millis(ms.clamp(1500, 20000))).await;
    p.borrow().generation == gen
}

async fn speak_file(p: &Shared, path: &str, gen: u64) -> bool {
    let media = gtk4::MediaFile::for_filename(path);
    media.play();
    p.borrow_mut().media = Some(media.clone());
    loop {
        glib::timeout_future(Duration::from_millis(120)).await;
        if p.borrow().generation != gen {
            return false;
        }
        if media.is_ended() || media.error().is_some() {
            p.borrow_mut().media = None;
            return true;
        }
    }
}

async fn speak(p: &Shared, text: &str, gen: u64) -> bool {
    if text.trim().is_empty() {
        glib::timeout_future(Duration::from_millis(3000)).await;
        return p.borrow().generation == gen;
    }
    if p.borrow().engine == "cloud" {
        let (core, en) = {
            let pl = p.borrow();
            (pl.core.clone(), pl.en)
        };
        if let Ok(r) = core_call(en, core, "tts.synthesize", json!({ "text": text })).await {
            if let Some(path) = r.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return speak_file(p, path, gen).await;
            }
        }
    }
    speak_system(p, text, gen).await
}

// ---- playback ---------------------------------------------------------------------------------

fn pause(p: &Shared) {
    {
        let mut pl = p.borrow_mut();
        pl.generation += 1;
        pl.playing = false;
    }
    stop_audio(p);
    js(p, "lecture.stopLaser()");
    update_counter(p);
    save_progress(p);
}

fn play(p: &Shared) {
    if p.borrow().seq.is_empty() {
        return;
    }
    let gen = {
        let mut pl = p.borrow_mut();
        pl.generation += 1;
        pl.playing = true;
        pl.checkpoint = None;
        pl.generation
    };
    update_counter(p);
    let p2 = p.clone();
    glib::spawn_future_local(async move {
        run_loop(p2, gen).await;
    });
}

async fn run_loop(p: Shared, gen: u64) {
    let mut shown: Option<usize> = None;
    loop {
        let s = {
            let pl = p.borrow();
            if pl.generation != gen || pl.pos >= pl.seq.len() {
                return;
            }
            pl.seq[pl.pos].clone()
        };
        if shown != Some(s.slide) {
            let step = {
                let pl = p.borrow();
                step_before(&pl.seq, pl.pos)
            };
            js(&p, &format!("lecture.show({},{step})", s.slide));
            shown = Some(s.slide);
            update_counter(&p);
            save_progress(&p);
        }
        let mut lasers = Vec::new();
        for cue in &s.cues {
            match cue.get("type").and_then(Value::as_str) {
                Some("step") => js(&p, &format!("lecture.step({})", cue.get("n").and_then(Value::as_i64).unwrap_or(0))),
                Some("laser") => lasers.push(cue.to_string()),
                _ => {}
            }
        }
        caption(&p, &s.text);
        // the page times each pointer cue by where its words fall in the sentence
        if !lasers.is_empty() {
            let rate = p.borrow().rate;
            js(&p, &format!("lecture.lasers([{}],{},{rate})", lasers.join(","), js_string(&s.text)));
        }
        let ok = speak(&p, &s.text, gen).await;
        if !ok || p.borrow().generation != gen {
            return;
        }
        if s.last_of_slide && s.checkpoint.is_some() {
            {
                let mut pl = p.borrow_mut();
                pl.playing = false;
                pl.checkpoint = Some(s.clone());
            }
            update_counter(&p);
            show_checkpoint(&p, &s);
            p.borrow().ask_panel.set_visible(true);
            return;
        }
        let last = {
            let pl = p.borrow();
            pl.pos >= pl.seq.len() - 1
        };
        if last {
            p.borrow_mut().playing = false;
            update_counter(&p);
            caption(&p, t(&p, "lec.ended"));
            save_progress(&p);
            return;
        }
        p.borrow_mut().pos += 1;
    }
}

fn jump_to(p: &Shared, new_pos: i64) {
    let was = p.borrow().playing;
    pause(p);
    {
        let mut pl = p.borrow_mut();
        let max = pl.seq.len().saturating_sub(1) as i64;
        pl.pos = new_pos.clamp(0, max) as usize;
    }
    show_position(p);
    update_counter(p);
    if was {
        play(p);
    }
}

fn slide_start(p: &Shared, delta: i64) -> i64 {
    let pl = p.borrow();
    let (Some(cur), Some(last)) = (pl.seq.get(pl.pos), pl.seq.last()) else { return 0 };
    let target = (cur.slide as i64 + delta).clamp(0, last.slide as i64) as usize;
    pl.seq.iter().position(|s| s.slide == target).unwrap_or(0) as i64
}

// ---- questions and adaptations ----------------------------------------------------------------

fn show_checkpoint(p: &Shared, s: &LecSentence) {
    let pl = p.borrow();
    let en = pl.en;
    let b = &pl.checkpoint_box;
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
    let Some(cp) = s.checkpoint.as_ref() else {
        b.set_visible(false);
        return;
    };
    let attempt = cp.get("kind").and_then(Value::as_str) == Some("attempt");
    let question = cp.get("question").and_then(Value::as_str).unwrap_or("").to_string();
    b.set_visible(true);
    let title = Label::new(Some(tr(en, if attempt { "lec.attempt" } else { "lec.checkpoint" })));
    title.set_xalign(0.0);
    title.add_css_class("lna-muted");
    b.append(&title);
    let q = Label::new(Some(&question));
    q.set_wrap(true);
    q.set_xalign(0.0);
    b.append(&q);
    let answer = Entry::builder().placeholder_text(tr(en, "lec.your_answer")).build();
    if attempt {
        b.append(&answer);
    }
    let row = GtkBox::new(Orientation::Horizontal, 8);
    if attempt {
        let submit = Button::with_label(tr(en, "lec.submit"));
        submit.add_css_class("suggested-action");
        let (p2, q2, a2) = (p.clone(), question.clone(), answer.clone());
        submit.connect_clicked(move |_| {
            let text = tr(p2.borrow().en, "lec.attempt_answer_fmt")
                .replace("{0}", &q2)
                .replace("{1}", a2.text().as_str());
            a2.set_text("");
            ask(&p2, text, false, None);
        });
        row.append(&submit);
    }
    let cont = Button::with_label(tr(en, "lec.continue"));
    let p3 = p.clone();
    cont.connect_clicked(move |_| {
        {
            let mut pl = p3.borrow_mut();
            pl.checkpoint = None;
            pl.checkpoint_box.set_visible(false);
            if pl.pos + 1 < pl.seq.len() {
                pl.pos += 1;
            }
        }
        play(&p3);
    });
    row.append(&cont);
    b.append(&row);
}

fn render_qa(p: &Shared) {
    let (en, qa) = {
        let pl = p.borrow();
        (pl.en, pl.qa.clone())
    };
    let b = p.borrow().qa_box.clone();
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
    for (i, e) in qa.iter().enumerate() {
        let col = GtkBox::new(Orientation::Vertical, 4);
        for path in &e.images {
            let pic = gtk4::Picture::for_filename(path);
            pic.set_size_request(120, 80);
            pic.set_content_fit(gtk4::ContentFit::Contain);
            pic.set_halign(gtk4::Align::Start);
            col.append(&pic);
        }
        let q = Label::new(Some(&e.question));
        q.set_wrap(true);
        q.set_xalign(0.0);
        q.add_css_class("heading");
        col.append(&q);
        if e.pending {
            col.append(&Label::new(Some(tr(en, "lec.thinking"))));
        } else if !e.error.is_empty() {
            let l = Label::new(Some(&e.error));
            l.set_wrap(true);
            l.set_xalign(0.0);
            l.add_css_class("error");
            col.append(&l);
        } else {
            let a = crate::md_view::markdown_widget(&e.answer);
            col.append(&a);
            let speak_btn = Button::with_label(tr(en, "lec.speak_answer"));
            speak_btn.add_css_class("lna-ghost-btn");
            speak_btn.set_halign(gtk4::Align::Start);
            let (p2, text) = (p.clone(), if e.speak.is_empty() { e.answer.clone() } else { e.speak.clone() });
            speak_btn.connect_clicked(move |_| {
                pause(&p2);
                let gen = p2.borrow().generation;
                let (p3, text) = (p2.clone(), text.clone());
                glib::spawn_future_local(async move {
                    speak(&p3, &text, gen).await;
                });
            });
            col.append(&speak_btn);
            if !e.suggest_kind.is_empty() {
                let kind_label = format!("lec.kind_{}", e.suggest_kind);
                let head = Label::new(Some(&trf(en, "lecture.label_value", &[&tr(en, "lec.adapt_suggest"), &tr(en, &kind_label)])));
                head.set_xalign(0.0);
                head.add_css_class("heading");
                col.append(&head);
                if !e.suggest_reason.is_empty() {
                    let r = Label::new(Some(&e.suggest_reason));
                    r.set_wrap(true);
                    r.set_xalign(0.0);
                    col.append(&r);
                }
                let row = GtkBox::new(Orientation::Horizontal, 8);
                let apply = Button::with_label(tr(en, "lec.adapt_apply"));
                apply.add_css_class("suggested-action");
                let skip = Button::with_label(tr(en, "lec.adapt_skip"));
                let (p2, kind, reason, question) =
                    (p.clone(), e.suggest_kind.clone(), e.suggest_reason.clone(), e.question.clone());
                apply.connect_clicked(move |_| {
                    p2.borrow_mut().qa[i].suggest_kind.clear();
                    render_qa(&p2);
                    let slide_id = {
                        let pl = p2.borrow();
                        pl.seq.get(pl.pos).map(|s| s.slide_id.clone()).unwrap_or_default()
                    };
                    adapt(&p2, slide_id, kind.clone(), reason.clone(), question.clone());
                });
                let p4 = p.clone();
                skip.connect_clicked(move |_| {
                    p4.borrow_mut().qa[i].suggest_kind.clear();
                    render_qa(&p4);
                });
                row.append(&apply);
                row.append(&skip);
                col.append(&row);
            }
        }
        b.append(&col);
    }
}

fn apply_artifact(p: &Shared, art: &Value) {
    let Some(payload) = art.get("payload").filter(|v| v.is_object()).cloned() else { return };
    let (cur_id, cur_slide) = {
        let pl = p.borrow();
        (pl.seq.get(pl.pos).map(|s| s.id.clone()), pl.seq.get(pl.pos).map(|s| s.slide_id.clone()))
    };
    {
        let mut pl = p.borrow_mut();
        if let Some(d) = art.get("output_dir").and_then(Value::as_str).filter(|d| !d.is_empty()) {
            pl.output_dir = d.to_string();
        }
        pl.seq = build_sequence(&payload);
        pl.payload = payload.clone();
        pl.pos = pl
            .seq
            .iter()
            .position(|s| Some(&s.id) == cur_id.as_ref())
            .or_else(|| pl.seq.iter().position(|s| Some(&s.slide_id) == cur_slide.as_ref()))
            .unwrap_or(0);
    }
    let (slides, slide, step) = {
        let pl = p.borrow();
        let with_figs = payload_with_figures(&pl.payload, &pl.output_dir);
        (
            with_figs.get("slides").cloned().unwrap_or_else(|| json!([])),
            pl.seq.get(pl.pos).map(|s| s.slide).unwrap_or(0),
            step_before(&pl.seq, pl.pos),
        )
    };
    js(p, &format!("lecture.setSlides({slides},{slide},{step})"));
    update_counter(p);
}

fn render_adaptation(p: &Shared) {
    let (en, adapting, adaptation, row) = {
        let pl = p.borrow();
        (pl.en, pl.adapting, pl.adaptation.clone(), pl.adapt_row.clone())
    };
    while let Some(c) = row.first_child() {
        row.remove(&c);
    }
    if adapting {
        row.append(&Label::new(Some(tr(en, "lec.adapting"))));
    } else if adaptation.is_some() {
        let b = Button::with_label(tr(en, "lec.adapted"));
        b.add_css_class("lna-pill-btn");
        let p2 = p.clone();
        b.connect_clicked(move |_| show_adaptation_details(&p2));
        row.append(&b);
    }
}

fn show_adaptation_details(p: &Shared) {
    let (en, a, parent) = {
        let pl = p.borrow();
        (pl.en, pl.adaptation.clone().unwrap_or(Value::Null), pl.window.clone())
    };
    let kind = a.get("kind").and_then(Value::as_str).unwrap_or("");
    let win = gtk4::Window::builder()
        .title(tr(en, &format!("lec.kind_{kind}")))
        .transient_for(&parent)
        .modal(true)
        .default_width(420)
        .build();
    let v = GtkBox::new(Orientation::Vertical, 8);
    v.set_margin_top(16);
    v.set_margin_bottom(16);
    v.set_margin_start(16);
    v.set_margin_end(16);
    if let Some(reason) = a.get("reason").and_then(Value::as_str).filter(|r| !r.is_empty()) {
        let l = Label::new(Some(&trf(en, "lecture.label_value", &[&tr(en, "lec.reason"), &reason])));
        l.set_wrap(true);
        l.set_xalign(0.0);
        v.append(&l);
    }
    let ids: Vec<String> = a
        .get("inserted")
        .and_then(Value::as_array)
        .map(|x| x.iter().filter_map(|i| i.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let titles: Vec<String> = a
        .get("inserted_titles")
        .and_then(Value::as_array)
        .map(|x| x.iter().filter_map(|i| i.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if !ids.is_empty() {
        let h = Label::new(Some(tr(en, "lec.inserted")));
        h.set_xalign(0.0);
        h.add_css_class("heading");
        v.append(&h);
        for (i, id) in ids.iter().enumerate() {
            let b = Button::with_label(titles.get(i).unwrap_or(id));
            b.add_css_class("lna-ghost-btn");
            let (p2, id2, win2) = (p.clone(), id.clone(), win.clone());
            b.connect_clicked(move |_| {
                win2.close();
                let at = p2.borrow().seq.iter().position(|s| s.slide_id == id2);
                if let Some(at) = at {
                    jump_to(&p2, at as i64);
                }
            });
            v.append(&b);
        }
    }
    if let Some(removed) = a.get("removed_titles").and_then(Value::as_array).filter(|r| !r.is_empty()) {
        let h = Label::new(Some(tr(en, "lec.replaced")));
        h.set_xalign(0.0);
        h.add_css_class("heading");
        v.append(&h);
        let names: Vec<&str> = removed.iter().filter_map(Value::as_str).collect();
        let l = Label::new(Some(&names.join(tr(en, "lecture.list_sep"))));
        l.set_wrap(true);
        l.set_xalign(0.0);
        v.append(&l);
    }
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_halign(gtk4::Align::End);
    let restore = Button::with_label(tr(en, "lec.restore_original"));
    let undo_btn = Button::with_label(tr(en, "lec.undo"));
    undo_btn.add_css_class("suggested-action");
    for (btn, all) in [(&restore, true), (&undo_btn, false)] {
        let (p2, win2) = (p.clone(), win.clone());
        btn.connect_clicked(move |_| {
            win2.close();
            undo(&p2, all);
        });
        row.append(btn);
    }
    v.append(&row);
    win.set_child(Some(&v));
    win.present();
}

fn adapt(p: &Shared, slide_id: String, kind: String, reason: String, question: String) {
    {
        p.borrow_mut().adapting = true;
    }
    render_adaptation(p);
    let (core, id, en) = {
        let pl = p.borrow();
        (pl.core.clone(), pl.artifact_id.clone(), pl.en)
    };
    let p2 = p.clone();
    glib::spawn_future_local(async move {
        let res = core_call(
            en,
            core,
            "lecture.adapt",
            json!({ "artifact_id": id, "slide_id": slide_id, "kind": kind, "reason": reason, "question": question }),
        )
        .await;
        match res {
            Ok(r) => {
                if let Some(art) = r.get("artifact") {
                    apply_artifact(&p2, art);
                }
                p2.borrow_mut().adaptation = r.get("adaptation").filter(|a| a.is_object()).cloned();
            }
            Err(e) => {
                let msg = trf(p2.borrow().en, "lecture.label_value", &[&t(&p2, "lec.adapt_failed"), &e]);
                set_status(&p2, &msg);
            }
        }
        p2.borrow_mut().adapting = false;
        render_adaptation(&p2);
    });
}

fn undo(p: &Shared, all: bool) {
    let (core, id, en, adaptation_id) = {
        let pl = p.borrow();
        (
            pl.core.clone(),
            pl.artifact_id.clone(),
            pl.en,
            pl.adaptation.as_ref().and_then(|a| a.get("id")).and_then(Value::as_str).map(String::from),
        )
    };
    let mut params = json!({ "artifact_id": id, "all": all });
    if let (false, Some(a)) = (all, adaptation_id.filter(|a| !a.is_empty())) {
        params["adaptation_id"] = json!(a);
    }
    let p2 = p.clone();
    glib::spawn_future_local(async move {
        match core_call(en, core, "lecture.undo", params).await {
            Ok(r) => {
                if let Some(art) = r.get("artifact") {
                    apply_artifact(&p2, art);
                }
                p2.borrow_mut().adaptation = None;
                render_adaptation(&p2);
            }
            Err(e) => set_status(&p2, &e),
        }
    });
}

/// Asks the teacher a question about the sentence being spoken; `attach` carries attachments.
fn ask(p: &Shared, question: String, allow_adapt: bool, attach: Option<&AttachBar>) {
    let Some(s) = ({
        let pl = p.borrow();
        pl.seq.get(pl.pos).cloned()
    }) else {
        return;
    };
    if question.trim().is_empty() && attach.is_none_or(|a| a.is_empty()) {
        return;
    }
    if p.borrow().playing {
        pause(p);
    }
    let idx = {
        let mut pl = p.borrow_mut();
        pl.qa.push(Qa { question: question.clone(), pending: true, ..Default::default() });
        pl.qa.len() - 1
    };
    render_qa(p);
    let history: Vec<Value> = p
        .borrow()
        .qa
        .iter()
        .rev()
        .take(5)
        .rev()
        .filter(|e| !e.pending)
        .flat_map(|e| {
            [
                json!({ "role": "user", "text": e.question }),
                json!({ "role": "assistant", "text": e.answer }),
            ]
        })
        .collect();
    let (core, id, en) = {
        let pl = p.borrow();
        (pl.core.clone(), pl.artifact_id.clone(), pl.en)
    };
    let mut params = json!({
        "artifact_id": id, "slide_id": s.slide_id, "sentence_id": s.id,
        "question": question, "history": history,
    });
    if let Some(a) = attach {
        a.add_to(&mut params);
    }
    let p2 = p.clone();
    glib::spawn_future_local(async move {
        let res = core_call(en, core, "lecture.ask", params).await;
        let mut adapt_args: Option<(String, String, String)> = None;
        {
            let mut pl = p2.borrow_mut();
            let policy = pl.policy.clone();
            let Some(e) = pl.qa.get_mut(idx) else { return };
            e.pending = false;
            match res {
                Ok(r) => {
                    let ad = r.get("adapt").cloned().unwrap_or(Value::Null);
                    let needed = ad.get("needed").and_then(Value::as_bool).unwrap_or(false) && allow_adapt && policy != "never";
                    e.images = r
                        .get("attachments")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|x| x.get("path").and_then(Value::as_str)).filter(|x| !x.is_empty()).map(String::from).collect())
                        .unwrap_or_default();
                    e.answer = r.get("answer").and_then(Value::as_str).unwrap_or("").to_string();
                    e.speak = r.get("speak").and_then(Value::as_str).unwrap_or("").to_string();
                    e.suggest_reason = ad.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
                    let kind = ad.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
                    if needed && policy == "ask" {
                        e.suggest_kind = kind.clone();
                    }
                    if needed && policy == "auto" {
                        adapt_args = Some((kind, e.suggest_reason.clone(), e.question.clone()));
                    }
                }
                Err(err) => e.error = err,
            }
        }
        render_qa(&p2);
        if let Some((kind, reason, q)) = adapt_args {
            adapt(&p2, s.slide_id.clone(), kind, reason, q);
        }
    });
}

// ---- widget -----------------------------------------------------------------------------------

/// Builds the player for a lecture artifact (the full `studio.get` result).
pub fn build_lecture_player(core: &Arc<CoreRuntime>, window: &gtk4::Window, art: &Value, en: bool) -> GtkBox {
    let payload = art.get("payload").cloned().unwrap_or(Value::Null);
    let seq = build_sequence(&payload);
    let saved = art.pointer("/progress_state/position").and_then(Value::as_str).unwrap_or("");
    let pos = seq.iter().position(|s| s.id == saved).unwrap_or(0);

    let ucm = webkit6::UserContentManager::new();
    ucm.register_script_message_handler("lna", None);
    let web = webkit6::WebView::builder().user_content_manager(&ucm).vexpand(true).hexpand(true).build();
    web.set_size_request(-1, 260);

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_vexpand(true);
    root.append(&web);

    let play_btn = Button::with_label("▶");
    play_btn.add_css_class("suggested-action");
    let counter = Label::new(None);
    let rate_btn = Button::with_label("1×");
    let status = Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("error");
    let adapt_row = GtkBox::new(Orientation::Horizontal, 6);
    let ask_panel = GtkBox::new(Orientation::Vertical, 8);
    ask_panel.add_css_class("lna-card");
    ask_panel.set_visible(false);
    let checkpoint_box = GtkBox::new(Orientation::Vertical, 6);
    checkpoint_box.set_visible(false);
    let qa_box = GtkBox::new(Orientation::Vertical, 10);

    let p: Shared = Rc::new(RefCell::new(Player {
        core: core.clone(),
        en,
        window: window.clone(),
        artifact_id: art.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        web: web.clone(),
        payload: payload.clone(),
        output_dir: art.get("output_dir").and_then(Value::as_str).unwrap_or("").to_string(),
        seq,
        pos,
        playing: false,
        captions: true,
        rate: 1.0,
        engine: "system".into(),
        policy: "auto".into(),
        web_ready: false,
        generation: 0,
        speech: None,
        media: None,
        checkpoint: None,
        qa: Vec::new(),
        adapting: false,
        adaptation: None,
        play_btn: play_btn.clone(),
        counter: counter.clone(),
        rate_btn: rate_btn.clone(),
        status: status.clone(),
        adapt_row: adapt_row.clone(),
        ask_panel: ask_panel.clone(),
        checkpoint_box: checkpoint_box.clone(),
        qa_box: qa_box.clone(),
    }));

    // the page tells when it is ready
    {
        let p2 = p.clone();
        ucm.connect_script_message_received(Some("lna"), move |_, value| {
            let text = value.to_str();
            let ready = serde_json::from_str::<Value>(text.as_str())
                .ok()
                .and_then(|v| v.get("type").and_then(Value::as_str).map(|t| t == "ready"))
                .unwrap_or(false);
            if ready {
                p2.borrow_mut().web_ready = true;
                show_position(&p2);
            }
        });
    }

    // controls
    let controls = GtkBox::new(Orientation::Horizontal, 6);
    let mk = |label: &str, tip: &str| {
        let b = Button::with_label(label);
        b.set_tooltip_text(Some(tip));
        b
    };
    let prev_slide = mk("⏮", tr(en, "lec.prev_slide"));
    let prev_sent = mk("⏪", tr(en, "lec.prev_sentence"));
    let next_sent = mk("⏩", tr(en, "lec.next_sentence"));
    let next_slide = mk("⏭", tr(en, "lec.next_slide"));
    play_btn.set_tooltip_text(Some(tr(en, "lec.play")));
    rate_btn.set_tooltip_text(Some(tr(en, "lec.speed")));
    for b in [&prev_slide, &prev_sent, &play_btn, &next_sent, &next_slide] {
        controls.append(b);
    }
    controls.append(&counter);
    let grow = GtkBox::new(Orientation::Horizontal, 0);
    grow.set_hexpand(true);
    controls.append(&grow);
    controls.append(&rate_btn);
    root.append(&controls);

    {
        let p2 = p.clone();
        prev_slide.connect_clicked(move |_| {
            let to = slide_start(&p2, -1);
            jump_to(&p2, to);
        });
        let p2 = p.clone();
        prev_sent.connect_clicked(move |_| {
            let to = p2.borrow().pos as i64 - 1;
            jump_to(&p2, to);
        });
        let p2 = p.clone();
        play_btn.connect_clicked(move |_| {
            if p2.borrow().playing {
                pause(&p2);
            } else {
                play(&p2);
            }
        });
        let p2 = p.clone();
        next_sent.connect_clicked(move |_| {
            let to = p2.borrow().pos as i64 + 1;
            jump_to(&p2, to);
        });
        let p2 = p.clone();
        next_slide.connect_clicked(move |_| {
            let to = slide_start(&p2, 1);
            jump_to(&p2, to);
        });
        let p2 = p.clone();
        rate_btn.connect_clicked(move |_| {
            {
                let mut pl = p2.borrow_mut();
                pl.rate = next_rate(pl.rate);
            }
            update_counter(&p2);
        });
    }

    // options: captions, ask, adaptation policy
    let options = GtkBox::new(Orientation::Horizontal, 8);
    let captions_chk = CheckButton::with_label(tr(en, "lec.captions"));
    captions_chk.set_active(true);
    let ask_btn = Button::with_label(tr(en, "lec.ask"));
    ask_btn.add_css_class("suggested-action");
    let policy_btn = MenuButton::builder().label(tr(en, "lec.adapt_policy")).build();
    let policy_pop = Popover::new();
    let policy_list = GtkBox::new(Orientation::Vertical, 2);
    for (code, key) in [("auto", "lec.policy_auto"), ("ask", "lec.policy_ask"), ("never", "lec.policy_never")] {
        let b = Button::with_label(tr(en, key));
        b.add_css_class("lna-ghost-btn");
        let (p2, pop2) = (p.clone(), policy_pop.clone());
        b.connect_clicked(move |_| {
            pop2.popdown();
            p2.borrow_mut().policy = code.to_string();
            let core = p2.borrow().core.clone();
            glib::spawn_future_local(async move {
                let _ = core_call(
                    en,
                    core,
                    "settings.set",
                    json!({ "key": "lecture.prefs", "value": { "adapt_policy": code } }),
                )
                .await;
            });
        });
        policy_list.append(&b);
    }
    policy_pop.set_child(Some(&policy_list));
    policy_btn.set_popover(Some(&policy_pop));
    options.append(&captions_chk);
    options.append(&ask_btn);
    options.append(&policy_btn);
    options.append(&adapt_row);
    root.append(&options);
    root.append(&status);

    {
        let p2 = p.clone();
        captions_chk.connect_toggled(move |c| {
            p2.borrow_mut().captions = c.is_active();
            let text = {
                let pl = p2.borrow();
                pl.seq.get(pl.pos).map(|s| s.text.clone()).unwrap_or_default()
            };
            caption(&p2, &text);
        });
        let p2 = p.clone();
        ask_btn.connect_clicked(move |_| {
            pause(&p2);
            p2.borrow().ask_panel.set_visible(true);
        });
    }

    // question panel
    let ask_head = GtkBox::new(Orientation::Horizontal, 8);
    let ask_title = Label::new(Some(tr(en, "lec.qa_title")));
    ask_title.add_css_class("heading");
    ask_title.set_hexpand(true);
    ask_title.set_xalign(0.0);
    let ask_close = Button::with_label("×");
    ask_close.add_css_class("lna-ghost-btn");
    ask_head.append(&ask_title);
    ask_head.append(&ask_close);
    ask_panel.append(&ask_head);
    ask_panel.append(&checkpoint_box);
    ask_panel.append(&ScrolledWindow::builder().child(&qa_box).min_content_height(120).max_content_height(240).propagate_natural_height(true).build());
    {
        let panel = ask_panel.clone();
        ask_close.connect_clicked(move |_| panel.set_visible(false));
    }

    let status_for_attach = status.clone();
    let en_cell = Rc::new(Cell::new(en));
    let attach = AttachBar::new(
        window,
        Rc::new(move || en_cell.get()),
        Rc::new(move |m| status_for_attach.set_text(m)),
    );
    let entry = Entry::builder().placeholder_text(tr(en, "lec.ask_placeholder")).hexpand(true).build();
    let send = Button::with_label(tr(en, "lec.send"));
    send.add_css_class("suggested-action");
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.append(attach.button());
    row.append(&entry);
    row.append(&send);
    ask_panel.append(attach.chips());
    ask_panel.append(&row);
    attach.accept_drops(&row);
    attach.accept_paste(&row);
    {
        let submit = {
            let (p2, entry2, attach2) = (p.clone(), entry.clone(), attach.clone());
            move || {
                let mut q = entry2.text().trim().to_string();
                if q.is_empty() && attach2.is_empty() {
                    return;
                }
                if q.is_empty() {
                    q = tr(p2.borrow().en, "ctx.ask_default").to_string();
                }
                entry2.set_text("");
                ask(&p2, q, true, Some(&attach2));
                attach2.clear();
            }
        };
        let s1 = submit.clone();
        send.connect_clicked(move |_| s1());
        entry.connect_activate(move |_| submit());
    }
    root.append(&ask_panel);

    // settings, then the page
    {
        let p2 = p.clone();
        let (core2, web2) = (core.clone(), web.clone());
        glib::spawn_future_local(async move {
            if let Ok(s) = core_call(en, core2.clone(), "tts.get_settings", json!({})).await {
                p2.borrow_mut().engine = if s.get("engine").and_then(Value::as_str) == Some("cloud") { "cloud" } else { "system" }.into();
            }
            if let Ok(r) = core_call(en, core2, "settings.get", json!({ "key": "lecture.prefs" })).await {
                if let Some(pol) = r.pointer("/value/adapt_policy").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    p2.borrow_mut().policy = pol.to_string();
                }
            }
            let bridge = "<script>window.chrome={webview:{postMessage:function(m){window.webkit.messageHandlers.lna.postMessage(m)}}};</script>";
            let html = {
                let pl = p2.borrow();
                deck_html(
                    include_str!("../assets/deck_player.html"),
                    &payload_with_figures(&pl.payload, &pl.output_dir),
                    pl.en,
                    bridge,
                )
            };
            web2.load_html(&html, None);
            update_counter(&p2);
        });
    }

    // stop speaking and remember the position when the player goes away
    {
        let p2 = p.clone();
        root.connect_destroy(move |_| {
            pause(&p2);
        });
    }
    root
}
