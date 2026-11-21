//! BEAM application shell: the Tauri window and everything the wx frame did.
//!
//! The engine (`beam_engine`) knows nothing about Tauri; this crate implements its
//! `UiSink` ([`shell::Shell`]) and exposes the commands and events listed in
//! `rust/ui/CONTRACT.md`. `--configurator` runs the standalone configurator
//! (`configurator.py main()`): the same page in configurator mode, no engine.

use std::sync::Arc;

use tauri::{LogicalSize, Manager, RunEvent, WindowEvent};

pub mod choices;
pub mod commands;
pub mod console_view;
pub mod describer;
pub mod dialogs;
pub mod logs_menu;
pub mod panel;
pub mod shell;
pub mod tones;

use shell::{Mode, Shell};

/// Build and run the application. Returns when the window has closed.
pub fn run() {
    let mode = Mode::from_args(std::env::args());
    if mode == Mode::Main {
        // bnh_logger at import, then main()'s first statement: sample Shift.
        beam_base::logger::init();
        beam_engine::engine::sample_shift_at_startup();
        beam_engine::engine::install_signal_handlers();
    }

    let shell = Shell::new(mode);
    let setup_shell = Arc::clone(&shell);

    let built = tauri::Builder::default()
        .manage(Arc::clone(&shell))
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::ui_choices,
            commands::message_box,
            commands::frontend_ready,
            commands::close_ready,
            commands::exit_app,
            commands::config_load,
            commands::config_save_panel,
            commands::config_reset,
            commands::speech_list_backends,
            commands::speech_list_voices,
            commands::speech_test,
            commands::audio_list_devices,
            commands::audio_test_tone,
            commands::audio_preview_gear_tones,
            commands::describer_load,
            commands::describer_save,
            commands::describer_validate_key,
            commands::console_history,
            commands::console_run,
            commands::console_refresh_contexts,
            commands::console_set_log_stream,
            commands::logs_menu_popup,
            commands::install_mod,
            commands::check_updates,
            commands::update_cancel,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // The page flushes its debounced saves first; close_ready finishes.
                api.prevent_close();
                window.state::<Arc<Shell>>().inner().begin_close();
            }
        })
        .setup(move |app| {
            let window = app
                .get_webview_window("main")
                .ok_or("the main window was not created")?;
            window.set_title(&mode.title())?;
            if mode == Mode::Configurator {
                window.set_min_size(Some(LogicalSize::new(580.0, 940.0)))?;
                window.set_size(LogicalSize::new(600.0, 940.0))?;
                window.center()?;
            }
            let hwnd = window.hwnd().ok().map(|h| h.0 as isize);
            setup_shell.attach(app.handle().clone(), hwnd);
            if mode == Mode::Main {
                beam_engine::ui_sink::set_ui_sink(Arc::clone(&setup_shell) as _);
                setup_shell.install_log_sink();
                match beam_engine::engine::start_engine_thread() {
                    Ok(h) => *setup_shell.engine.lock() = Some(h),
                    Err(e) => log::error!("could not start the engine thread: {e}"),
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(e) => {
            log::error!("error while starting BEAM: {e}");
            eprintln!("error while starting BEAM: {e}");
            std::process::exit(1);
        }
    };
    app.run(move |_handle, event| {
        if let RunEvent::Exit = event {
            if mode == Mode::Main {
                // The rest of main() after MainLoop: learn mode off, STOP, speech atexit.
                beam_engine::engine::after_main_loop();
            }
        }
    });
}
