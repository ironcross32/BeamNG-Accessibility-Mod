//! Every Tauri command in `rust/ui/CONTRACT.md`. Commands that take arguments take one,
//! named `args`. Anything that can block (a dialog, the network, a config write with its
//! retry loop, speech) runs on a blocking thread so the page keeps processing events.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use beam_base::config;
use beam_engine::console;
use beam_engine::ui_sink::UiSink;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tauri::State;

use crate::describer::{self, SaveArgs, ValidateArgs};
use crate::dialogs::{self, MessageBoxArgs};
use crate::logs_menu;
use crate::panel;
use crate::shell::Shell;

type Cmd<T> = Result<T, String>;

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown error".to_string()
    }
}

/// Run blocking work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Cmd<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

// ---- Startup and shared -------------------------------------------------------------------

#[tauri::command]
pub fn app_info(shell: State<'_, Arc<Shell>>) -> Value {
    json!({
        "version": beam_engine::updater::APP_VERSION,
        "title": shell.mode.title(),
        "mode": shell.mode.name(),
    })
}

#[tauri::command]
pub fn ui_choices() -> Value {
    crate::choices::ui_choices()
}

#[tauri::command]
pub async fn message_box(shell: State<'_, Arc<Shell>>, args: MessageBoxArgs) -> Cmd<String> {
    let owner = shell.hwnd();
    let msg = args.to_message_box();
    blocking(move || dialogs::answer_name(dialogs::show(owner, &msg)).to_string()).await
}

#[tauri::command]
pub fn frontend_ready(shell: State<'_, Arc<Shell>>) -> Value {
    shell.inner().frontend_ready()
}

#[tauri::command]
pub fn close_ready(shell: State<'_, Arc<Shell>>) {
    shell.inner().finish_close();
}

#[tauri::command]
pub fn exit_app(shell: State<'_, Arc<Shell>>) {
    shell.inner().begin_close();
}

// ---- Configuration panel ------------------------------------------------------------------

#[tauri::command]
pub async fn config_load(shell: State<'_, Arc<Shell>>) -> Cmd<Value> {
    let cfg = blocking(config::load_config_configurator)
        .await?
        .map_err(|e| e.to_string())?;
    let values = panel::control_values(&cfg)?;
    *shell.panel.lock() = cfg;
    Ok(values)
}

#[tauri::command]
pub async fn config_save_panel(
    shell: State<'_, Arc<Shell>>,
    args: Map<String, Value>,
) -> Cmd<Value> {
    let shell = Arc::clone(shell.inner());
    blocking(move || {
        let path = beam_base::paths::config_path();
        let mut snap = shell.panel.lock();
        panel::save_panel_at(&path, &mut snap, &args)
    })
    .await
}

#[tauri::command]
pub fn config_reset(shell: State<'_, Arc<Shell>>) -> Cmd<Value> {
    let cfg = config::default_config();
    let values = panel::control_values(&cfg)?;
    *shell.panel.lock() = cfg;
    Ok(values)
}

#[tauri::command]
pub async fn speech_list_backends() -> Cmd<Vec<String>> {
    Ok(
        blocking(|| catch_unwind(beam_speech::speech::list_backends).unwrap_or_default())
            .await
            .unwrap_or_default(),
    )
}

#[derive(Debug, Deserialize)]
pub struct BackendArgs {
    pub backend: String,
}

#[tauri::command]
pub async fn speech_list_voices(args: BackendArgs) -> Cmd<Value> {
    let backend = if args.backend.is_empty() {
        beam_speech::speech::AUTO_BACKEND.to_string()
    } else {
        args.backend
    };
    let r = blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            beam_speech::speech::list_voices(&backend)
        }))
    })
    .await?;
    let (mut voices, feats) = r.unwrap_or_default();
    // sorted(names, key=str.lower): stable.
    voices.sort_by_key(|v| v.to_lowercase());
    let features = feats.map(|f| {
        json!({
            "supports_set_voice": f.supports_set_voice(),
            "supports_set_rate": f.supports_set_rate(),
            "supports_set_volume": f.supports_set_volume(),
            "supports_braille": f.supports_braille(),
        })
    });
    Ok(json!({"voices": voices, "features": features}))
}

#[derive(Debug, Deserialize)]
pub struct SpeechTestArgs {
    pub backend: String,
    pub voice: String,
    pub rate: f64,
    pub volume: f64,
}

#[tauri::command]
pub async fn speech_test(args: SpeechTestArgs) -> Cmd<Value> {
    let r = blocking(move || {
        let backend = if args.backend.is_empty() {
            beam_speech::speech::AUTO_BACKEND.to_string()
        } else {
            args.backend
        };
        catch_unwind(AssertUnwindSafe(|| {
            beam_speech::speech::test_speak(
                &backend,
                &args.voice,
                Some(args.rate),
                Some(args.volume),
                "This is a test of the current speech configuration.",
            )
        }))
        .unwrap_or_else(|p| (false, Some(panic_text(&*p))))
    })
    .await?;
    let (ok, err) = r;
    let error = if ok {
        Value::Null
    } else {
        json!(format!("Speech test failed: {}", err.unwrap_or_default()))
    };
    Ok(json!({"ok": ok, "error": error}))
}

#[tauri::command]
pub async fn audio_list_devices() -> Cmd<Vec<String>> {
    Ok(blocking(|| {
        catch_unwind(beam_audio::device::list_wasapi_output_devices).unwrap_or_default()
    })
    .await
    .unwrap_or_default())
}

fn audio_result(r: Result<(), String>) -> Value {
    match r {
        Ok(()) => json!({"ok": true, "error": null}),
        Err(e) => json!({"ok": false, "error": format!("Failed to play audio: {e}")}),
    }
}

#[derive(Debug, Deserialize)]
pub struct ToneArgs {
    pub freq_hz: f64,
    pub level_dbfs: f64,
}

#[tauri::command]
pub async fn audio_test_tone(args: ToneArgs) -> Cmd<Value> {
    let buf = crate::tones::test_tone(args.freq_hz, args.level_dbfs);
    let r = blocking(move || beam_audio::device::play_buffer(buf, crate::tones::SR)).await?;
    Ok(audio_result(r))
}

#[derive(Debug, Deserialize)]
pub struct GearPreviewArgs {
    pub root: String,
    pub octave: i64,
    pub scale: String,
    pub level_dbfs: f64,
}

#[tauri::command]
pub async fn audio_preview_gear_tones(args: GearPreviewArgs) -> Cmd<Value> {
    let r = blocking(move || {
        let buf = catch_unwind(|| {
            crate::tones::gear_preview(&args.root, args.octave, &args.scale, args.level_dbfs)
        })
        .map_err(|p| panic_text(&*p))?;
        beam_audio::device::play_buffer(buf, crate::tones::SR)
    })
    .await?;
    Ok(audio_result(r))
}

// ---- AI Describer panel -------------------------------------------------------------------

#[tauri::command]
pub async fn describer_load(shell: State<'_, Arc<Shell>>) -> Cmd<Value> {
    let cfg = blocking(config::load_config_configurator)
        .await?
        .map_err(|e| e.to_string())?;
    let state = describer::state(&cfg);
    *shell.describer.lock() = cfg;
    Ok(state)
}

#[tauri::command]
pub async fn describer_save(shell: State<'_, Arc<Shell>>, args: SaveArgs) -> Cmd<Value> {
    let shell = Arc::clone(shell.inner());
    blocking(move || {
        let path = beam_base::paths::config_path();
        let mut snap = shell.describer.lock();
        describer::save_at(&path, &mut snap, &args)
    })
    .await
}

#[tauri::command]
pub async fn describer_validate_key(args: ValidateArgs) -> Cmd<Value> {
    blocking(move || describer::validate(&args)).await
}

// ---- Main tab ------------------------------------------------------------------------------

#[tauri::command]
pub fn console_history(shell: State<'_, Arc<Shell>>) -> Vec<String> {
    shell.history.lock().clone()
}

#[derive(Debug, Deserialize)]
pub struct ConsoleRunArgs {
    pub index: i64,
    pub command: String,
}

#[tauri::command]
pub async fn console_run(shell: State<'_, Arc<Shell>>, args: ConsoleRunArgs) -> Cmd<Value> {
    let shell = Arc::clone(shell.inner());
    blocking(move || {
        let history = {
            let mut h = shell.history.lock();
            if h.last() != Some(&args.command) {
                h.push(args.command.clone());
                let start = h.len().saturating_sub(console::CONSOLE_HISTORY_MAX);
                h.drain(..start);
                console::save_console_history(&h);
            }
            h.clone()
        };
        console::send_console_command(&format!("EXEC|{}|{}", args.index, args.command));
        json!({"history": history})
    })
    .await
}

#[tauri::command]
pub fn console_refresh_contexts() {
    console::send_console_command("CTXLIST");
}

#[derive(Debug, Deserialize)]
pub struct LogStreamArgs {
    pub enabled: bool,
}

#[tauri::command]
pub fn console_set_log_stream(args: LogStreamArgs) {
    console::send_console_command(if args.enabled { "LOGON" } else { "LOGOFF" });
}

#[derive(Debug, Deserialize)]
pub struct PopupArgs {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

#[tauri::command]
pub async fn logs_menu_popup(shell: State<'_, Arc<Shell>>, args: PopupArgs) -> Cmd<()> {
    let Some(hwnd) = shell.hwnd() else {
        return Ok(());
    };
    let Some(app) = shell.app().cloned() else {
        return Ok(());
    };
    let rows = logs_menu::entries(&logs_menu::LogPaths::current());
    let (x, y) = (
        (args.x * args.scale).round() as i32,
        (args.y * args.scale).round() as i32,
    );
    let (tx, rx) = std::sync::mpsc::channel();
    // TrackPopupMenu must run on the thread that owns the window.
    let posted = app.run_on_main_thread(move || {
        let _ = tx.send(logs_menu::popup(hwnd, x, y, &rows));
    });
    if let Err(e) = posted {
        log::error!("could not show the log menu: {e}");
        return Ok(());
    }
    blocking(move || {
        if let Ok(Some(choice)) = rx.recv() {
            logs_menu::handle_choice(choice, &|m| dialogs::show(Some(hwnd), m));
        }
    })
    .await
}

#[tauri::command]
pub async fn install_mod(shell: State<'_, Arc<Shell>>) -> Cmd<bool> {
    let shell = Arc::clone(shell.inner());
    blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            beam_engine::mod_install::install_mod_interactive(&*shell as &dyn UiSink)
        }))
        .map_err(|p| panic_text(&*p))
    })
    .await?
}

#[tauri::command]
pub async fn check_updates(shell: State<'_, Arc<Shell>>) -> Cmd<()> {
    let shell = Arc::clone(shell.inner());
    blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            beam_engine::updater::run_startup_flow(
                &*shell as &dyn UiSink,
                &beam_engine::updater::NullGate,
                true,
            )
        }))
        .map_err(|p| {
            let e = panic_text(&*p);
            log::error!("Manual update check failed: {e}");
            e
        })
    })
    .await?
}

#[tauri::command]
pub fn update_cancel(shell: State<'_, Arc<Shell>>) {
    shell.cancel_progress();
}
