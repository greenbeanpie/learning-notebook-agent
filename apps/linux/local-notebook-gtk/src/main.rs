mod attach;
mod i18n;
mod lecture;
mod lecture_model;
mod icons;
mod md_view;
mod mindmap_view;
mod slides_view;
mod ui;

use gtk4::gdk::Display;
use gtk4::prelude::*;
use gtk4::{Application, CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};
use lna_api::CoreRuntime;
use std::path::PathBuf;
use std::sync::Arc;

fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LocalNotebook")
}

fn main() {
    let app = Application::builder()
        .application_id("com.localnotebook.agent")
        .build();

    let data_dir = default_data_dir();
    let core: Arc<CoreRuntime> =
        CoreRuntime::open(data_dir, true).expect("Failed to initialize Local Notebook Core");

    app.connect_startup(|_| {
        let provider = CssProvider::new();
        provider.load_from_data(include_str!("style.css"));
        if let Some(display) = Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });

    let core_activate = core.clone();
    app.connect_activate(move |app| {
        ui::build_main_window(app, core_activate.clone());
    });

    let core_shutdown = core.clone();
    app.connect_shutdown(move |_| {
        core_shutdown.shutdown();
    });

    app.run();
}
