//! BEAM application shell: the Tauri window and everything the wx frame did.
//!
//! The engine (`beam_engine`) knows nothing about Tauri; this crate implements its
//! `UiSink` and exposes the commands and events listed in `rust/ui/CONTRACT.md`.

pub mod commands;
pub mod configurator_mode;
pub mod console_view;
pub mod dialogs;
pub mod events;
pub mod logs_menu;
pub mod sink;

/// Build and run the application. Returns when the last window closes.
pub fn run() {
    let result = tauri::Builder::default().run(tauri::generate_context!());
    if let Err(e) = result {
        // No console in release: there is nowhere better to report this yet.
        eprintln!("error while running BEAM: {e}");
        std::process::exit(1);
    }
}
