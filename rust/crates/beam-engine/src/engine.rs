//! Owner: EA. The engine's lifecycle (`beamtel.py:98-178, 12136-12671`): the launch gate,
//! the Shift-held-at-startup check, signals, the DOM dump writer, `_broadcast_ui_settings`,
//! `_apply_live_config`, `_config_watcher`, `_run_engine` (every subsystem started in the
//! Python's order), the game launch, `_run_update_flow`, and the parts of `main()` and
//! `BeamTelFrame._on_close` that are not window code.
//!
//! How `src-tauri` (U2) drives it, mirroring `main()`:
//!
//! ```ignore
//! beam_engine::engine::sample_shift_at_startup();       // first statement of main()
//! beam_engine::engine::install_signal_handlers();
//! // ... build the window, install the UiSink ...
//! let engine = beam_engine::engine::start_engine_thread();   // "beamtel-engine"
//! // after the window is up (wx.CallAfter):
//! beam_engine::engine::run_update_flow(&*sink);          // releases LAUNCH_GATE exactly once
//! // on close: flush the panels' pending saves, then
//! beam_engine::engine::close_sequence(engine);           // STOP.set(), join(timeout=2.0)
//! // after the event loop has ended:
//! beam_engine::engine::after_main_loop();                // LEARN_OFF, STOP.set(), speech atexit
//! ```

use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use beam_speech::speech;
use serde_json::{Map, Value, json};

use crate::convert::{ConfigMap, json_to_pyops};
use crate::listeners;
use crate::state::{
    self, State, drift_announcer, driving_assistant, hill_climb_challenge, road_diagnostics, state,
};
use crate::timer::{Event, STOP, Timer, spawn_daemon};
use crate::ui_sink::UiSink;
use crate::{browsers, console, keyboard, menus, speech_out, ui_bridge, units, updater};
use beam_audio::controller::AudioController;
use beam_base::config::{default_config_engine, load_config_engine};
use beam_base::gear_report;
use beam_base::pyval::{from_json_map, to_json_map};
use beam_road::assistant_route::suppressed_toast;

// ---- Launch gate ---------------------------------------------------------------------------

/// `LAUNCH_GATE`: the game launch waits on it until the updater has answered.
pub static LAUNCH_GATE: Event = Event::new();
/// `LAUNCH_ALLOWED`, read once immediately after the wait.
pub static LAUNCH_ALLOWED: AtomicBool = AtomicBool::new(true);
/// `LAUNCH_SUPPRESSED_BY_SHIFT`, sampled in `main()`, tested at the gate.
pub static LAUNCH_SUPPRESSED_BY_SHIFT: AtomicBool = AtomicBool::new(false);

pub const VK_LSHIFT: i32 = 0xA0;
pub const VK_RSHIFT: i32 = 0xA1;
/// How long the engine waits for the updater before launching as configured.
pub const LAUNCH_GATE_TIMEOUT_S: f64 = 180.0;

/// `_shift_held_at_startup()`: either physical Shift key is down right now.
pub fn shift_held_at_startup() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        [VK_LSHIFT, VK_RSHIFT].iter().any(|&vk| {
            // SAFETY: GetAsyncKeyState takes a virtual-key code and only reads key state.
            let s = unsafe { GetAsyncKeyState(vk) };
            (s as u16 & 0x8000) != 0
        })
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The first statement of `main()`: sample Shift and log it.
pub fn sample_shift_at_startup() {
    let held = shift_held_at_startup();
    LAUNCH_SUPPRESSED_BY_SHIFT.store(held, Ordering::SeqCst);
    if held {
        log::info!("Shift held at startup: automatic game launch disabled for this session.");
    }
}

/// `LaunchGate`: the updater's view of the deferred launch.
#[derive(Debug, Default, Clone, Copy)]
pub struct LaunchGate;

impl updater::Gate for LaunchGate {
    fn allow(&self) {
        LAUNCH_ALLOWED.store(true, Ordering::SeqCst);
        LAUNCH_GATE.set();
    }
    fn deny(&self) {
        LAUNCH_ALLOWED.store(false, Ordering::SeqCst);
        LAUNCH_GATE.set();
    }
}

// ---- Signals ---------------------------------------------------------------------------------

/// `_handle_sigint`: Ctrl+C / Ctrl+Break set `STOP` (and nothing else).
pub fn install_signal_handlers() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
        };
        use windows::core::BOOL;
        unsafe extern "system" fn handler(ctrl: u32) -> BOOL {
            if ctrl == CTRL_C_EVENT || ctrl == CTRL_BREAK_EVENT {
                STOP.set();
                return BOOL(1);
            }
            BOOL(0)
        }
        // SAFETY: registers a 'static handler function; it only sets an Event.
        if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(handler), true) } {
            log::debug!("SetConsoleCtrlHandler failed: {e}");
        }
    }
}

// ---- DOM dump ----------------------------------------------------------------------------------

/// `DOM_DUMP_PATH`.
pub fn dom_dump_path() -> PathBuf {
    beam_base::paths::config_dir().join("dom_dump.log")
}

/// `_on_dom_dump_received(lines)`.
pub fn on_dom_dump_received(lines: Vec<String>) {
    let path = dom_dump_path();
    let result = std::fs::File::create(&path).and_then(|mut f| {
        for line in &lines {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        Ok(())
    });
    match result {
        Ok(()) => log::info!(
            "DOM dump written to {} ({} lines)",
            path.display(),
            lines.len()
        ),
        Err(e) => log::error!("Failed to write DOM dump: {e}"),
    }
}

// ---- Config helpers (Python's cfg.get / bool / int / float) -----------------------------------

/// beamtel.py `load_config()`, as JSON. The real loader reports only I/O failures it could
/// not recover from; those fall back to the defaults for this call, as Python's OSError path does.
pub fn load_config() -> ConfigMap {
    match load_config_engine() {
        Ok(d) => to_json_map(&d),
        Err(e) => {
            log::warn!("Config could not be loaded ({e}); using defaults.");
            to_json_map(&default_config_engine())
        }
    }
}

/// `gear_report.settings_from_config(cfg)`; panics where Python raises.
fn gear_settings(cfg: &ConfigMap) -> gear_report::GearSettings {
    gear_report::settings_from_config(&from_json_map(cfg))
        .unwrap_or_else(|e| panic!("gear_report.settings_from_config: {e:?}"))
}

/// `audio_controller.apply_config(cfg)`; panics where Python raises.
fn apply_audio_config(ac: &Arc<AudioController>, cfg: &ConfigMap) {
    if let Err(e) = ac.apply_config(&from_json_map(cfg)) {
        panic!("AudioController.apply_config: {e:?}");
    }
}

/// Python truthiness of a config value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn get<'a>(cfg: &'a ConfigMap, key: &str, default: &'a Value) -> &'a Value {
    cfg.get(key).unwrap_or(default)
}

fn get_bool(cfg: &ConfigMap, key: &str, default: bool) -> bool {
    py_truthy(get(cfg, key, &Value::Bool(default)))
}

fn get_str(cfg: &ConfigMap, key: &str, default: &str) -> String {
    match cfg.get(key) {
        None => default.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// `int(x)` of a config value. Panics where Python raises (a non-numeric string).
fn py_int_value(v: &Value, key: &str) -> i64 {
    match v {
        Value::Bool(b) => *b as i64,
        Value::Number(n) => n
            .as_i64()
            .unwrap_or_else(|| units::py_int(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => beam_base::pyfmt::py_strip(s)
            .parse::<i64>()
            .unwrap_or_else(|_| {
                panic!("ValueError: invalid literal for int() with base 10: {s:?} ({key})")
            }),
        _ => panic!("TypeError: int() argument must be a string or a number ({key})"),
    }
}

/// `float(x)` of a config value. Panics where Python raises.
fn py_float_value(v: &Value, key: &str) -> f64 {
    match v {
        Value::Bool(b) => *b as i64 as f64,
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => beam_base::pyfmt::py_float_parse(s).unwrap_or_else(|| {
            panic!("ValueError: could not convert string to float: {s:?} ({key})")
        }),
        _ => panic!("TypeError: float() argument must be a string or a real number ({key})"),
    }
}

/// A raw numeric config value kept for later arithmetic (`speed_announce_interval`,
/// `scanner_distance_callout_interval`). Python stores whatever the file held and raises
/// later; the Rust keeps the previous value and logs instead (engine-map bug 4).
fn get_number_or_keep(cfg: &ConfigMap, key: &str, default: f64, previous: f64) -> f64 {
    match cfg.get(key) {
        None => default,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(previous),
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(other) => {
            log::warning_non_number(key, other);
            previous
        }
    }
}

mod log {
    pub use ::log::*;
    pub fn warning_non_number(key: &str, v: &serde_json::Value) {
        ::log::warn!("Config value {key} is not a number ({v}); keeping the previous value.");
    }
}

/// `{k: cfg.get(k, DEFAULT_CONFIG.get(k)) for k in DEFAULT_CONFIG if k.startswith("ai_describer_")}`.
fn ai_describer_settings_from(cfg: &ConfigMap) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, dv) in to_json_map(&default_config_engine()) {
        if k.starts_with("ai_describer_") {
            let v = cfg.get(&k).cloned().unwrap_or(dv);
            out.insert(k, v);
        }
    }
    out
}

/// The settings `_run_engine` applies before speech starts (beamtel.py:12271-12295).
fn apply_startup_settings(st: &mut State, cfg: &ConfigMap) {
    units::set_units_mode(&get_str(cfg, "units", "imperial"));
    let s = &mut st.cfg;
    s.oil_chime_enabled = get_bool(cfg, "oil_chime_enabled", true);
    s.announce_turn_signals = get_bool(cfg, "announce_turn_signals", true);
    s.announce_speed = get_bool(cfg, "announce_speed", true);
    s.speed_announce_interval = get_number_or_keep(
        cfg,
        "speed_announce_interval",
        25.0,
        s.speed_announce_interval,
    );
    s.gear_report_settings = gear_settings(cfg);
    s.announce_clickspot_actions = get_bool(cfg, "announce_clickspot_actions", false);
    s.scanner_distance_callout_enabled = get_bool(cfg, "scanner_distance_callout_enabled", false);
    s.scanner_distance_callout_interval = get_number_or_keep(
        cfg,
        "scanner_distance_callout_interval",
        10.0,
        s.scanner_distance_callout_interval,
    );
    s.announce_implement_proximity = get_bool(cfg, "implement_proximity_speech", true);
    s.announce_cannon_shot = get_bool(cfg, "cannon_shot_readout", true);
    s.announce_binding_learn_description = get_bool(cfg, "binding_learn_speak_description", false);
    s.road_follow_guidance_enabled = get_bool(cfg, "road_follow_guidance_enabled", true);
    s.road_junction_speech_enabled = get_bool(cfg, "road_junction_speech_enabled", true);
    s.road_stop_sign_speech_enabled = get_bool(cfg, "road_stop_sign_speech_enabled", true);
    s.road_traffic_light_speech_enabled = get_bool(cfg, "road_traffic_light_speech_enabled", true);
    s.road_speed_limit_speech_enabled = get_bool(cfg, "road_speed_limit_speech_enabled", true);
    s.road_surface_speech_enabled = get_bool(cfg, "road_surface_speech_enabled", true);
    drift_announcer().set_enabled(get_bool(cfg, "drift_speech_enabled", true));
    s.road_junction_earcon_enabled = get_bool(cfg, "road_junction_earcon_enabled", true);
    s.road_include_private = get_bool(cfg, "road_include_private", false);
    s.obstacle_warning_sensitivity = get_str(cfg, "obstacle_warning_sensitivity", "normal");
    s.ui_nav_hold_suppression = get_bool(cfg, "ui_nav_hold_suppression", true);
}

/// The block `_apply_live_config` runs under `state_lock` (beamtel.py:12176-12219).
fn apply_live_settings(st: &mut State, cfg: &ConfigMap) {
    units::set_units_mode(&get_str(cfg, "units", "imperial"));
    st.cfg.oil_chime_enabled = get_bool(cfg, "oil_chime_enabled", true);
    st.tel.protocol_mode = get_str(cfg, "telemetry_protocol", "outgauge");
    st.cfg.compass_highlight_enabled = get_bool(cfg, "compass_highlight_enabled", true);
    st.cfg.compass_highlight_nth_click = py_int_value(
        get(cfg, "compass_highlight_nth_click", &json!(6)),
        "compass_highlight_nth_click",
    );
    st.cfg.compass_click_interval_deg = py_float_value(
        get(cfg, "compass_click_interval", &json!(15.0)),
        "compass_click_interval",
    );
    let s = &mut st.cfg;
    s.announce_turn_signals = get_bool(cfg, "announce_turn_signals", true);
    s.announce_speed = get_bool(cfg, "announce_speed", true);
    s.speed_announce_interval = get_number_or_keep(
        cfg,
        "speed_announce_interval",
        25.0,
        s.speed_announce_interval,
    );
    s.gear_report_settings = gear_settings(cfg);
    s.announce_clickspot_actions = get_bool(cfg, "announce_clickspot_actions", false);
    s.scanner_distance_callout_enabled = get_bool(cfg, "scanner_distance_callout_enabled", false);
    s.scanner_distance_callout_interval = get_number_or_keep(
        cfg,
        "scanner_distance_callout_interval",
        10.0,
        s.scanner_distance_callout_interval,
    );
    s.announce_implement_proximity = get_bool(cfg, "implement_proximity_speech", true);
    s.announce_cannon_shot = get_bool(cfg, "cannon_shot_readout", true);
    s.announce_binding_learn_description = get_bool(cfg, "binding_learn_speak_description", false);
    s.road_follow_guidance_enabled = get_bool(cfg, "road_follow_guidance_enabled", true);
    s.road_junction_speech_enabled = get_bool(cfg, "road_junction_speech_enabled", true);
    s.road_stop_sign_speech_enabled = get_bool(cfg, "road_stop_sign_speech_enabled", true);
    s.road_traffic_light_speech_enabled = get_bool(cfg, "road_traffic_light_speech_enabled", true);
    s.road_speed_limit_speech_enabled = get_bool(cfg, "road_speed_limit_speech_enabled", true);
    s.road_surface_speech_enabled = get_bool(cfg, "road_surface_speech_enabled", true);
    drift_announcer().set_enabled(get_bool(cfg, "drift_speech_enabled", true));
    s.road_junction_earcon_enabled = get_bool(cfg, "road_junction_earcon_enabled", true);
    s.road_include_private = get_bool(cfg, "road_include_private", false);
    s.obstacle_warning_sensitivity = get_str(cfg, "obstacle_warning_sensitivity", "normal");
    s.ui_nav_hold_suppression = get_bool(cfg, "ui_nav_hold_suppression", true);
    s.ai_describer_provider = get_str(cfg, "ai_describer_provider", "gemini");
    s.ai_describer_settings = ai_describer_settings_from(cfg);
    s.ai_describer_disable_ui_toggle = get_bool(cfg, "ai_describer_disable_ui_toggle", false);
}

// ---- _broadcast_ui_settings / _apply_live_config / _config_watcher ------------------------

/// `_broadcast_ui_settings()`: push the UI-side settings to the mod's JS runtime.
pub fn broadcast_ui_settings() {
    let payload = {
        let st = state();
        json!({"type": "settings", "ui_nav_hold_suppression": st.cfg.ui_nav_hold_suppression})
    };
    if let Err(p) = catch_unwind(|| ui_bridge::broadcast(&payload)) {
        log::error!("Failed to broadcast UI settings: {}", panic_text(&p));
    }
}

/// `_apply_live_config(audio_controller)`: reload the config and apply every setting.
pub fn apply_live_config(audio_controller: Option<Arc<AudioController>>) {
    let cfg = match catch_unwind(load_config) {
        Ok(c) => c,
        Err(p) => {
            log::warn!("Config reload failed: {}", panic_text(&p));
            return;
        }
    };
    {
        let mut st = state();
        apply_live_settings(&mut st, &cfg);
    }
    if let Some(ac) = &audio_controller {
        apply_audio_config(ac, &cfg);
        // Outside state_lock: loading a set reads and resamples a file.
        ac.select_hrtf_set(
            cfg.get("hrtf_set").and_then(Value::as_str).unwrap_or(""),
            &beam_base::paths::hrtf_base_dir(),
            cfg.get("hrtf_elevation_deg").and_then(Value::as_f64),
            cfg.get("hrtf_equalise").and_then(Value::as_bool),
        );
    }
    listeners::road::send_road_configuration();
    if state().obstacle.obstacle_mode_active {
        listeners::obstacle::send_obstacle_configuration();
    }
    // Outside state_lock: rebuilding the backend can take long enough to stall others.
    speech::configure(&Value::Object(cfg.clone()));
    broadcast_ui_settings();
    log::info!("Configuration reloaded.");
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(beam_base::paths::config_path())
        .and_then(|m| m.modified())
        .ok()
}

/// `_config_watcher(stop_event)`: poll the config's mtime once a second, hot-reload on change.
pub fn config_watcher(stop_event: &Event) {
    let mut last_mtime = config_mtime();
    while !stop_event.wait_secs(1.0) {
        let Some(mtime) = config_mtime() else {
            continue;
        };
        if Some(mtime) != last_mtime {
            last_mtime = Some(mtime);
            apply_live_config(state::audio_controller_ref());
        }
    }
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string())
}

// ---- The game launch -------------------------------------------------------------------------

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `subprocess.run([...tasklist...], capture_output=True, text=True, timeout=5,
/// creationflags=CREATE_NO_WINDOW).stdout`.
fn tasklist_stdout() -> Result<String, String> {
    let system_root = std::env::var("SystemRoot")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| r"C:\Windows".to_string());
    let exe = PathBuf::from(system_root)
        .join("System32")
        .join("tasklist.exe");
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["/FI", "IMAGENAME eq BeamNG.drive.x64.exe", "/NH"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let _ = std::io::Read::read_to_end(out, &mut buf);
        }
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Command '['{}', '/FI', 'IMAGENAME eq BeamNG.drive.x64.exe', '/NH']' timed out after 5 seconds",
                    exe.display()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let bytes = reader.join().unwrap_or_default();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `winreg.QueryValueEx(OpenKey(HKLM, path), "InstallPath")[0]`.
#[cfg(windows)]
fn steam_install_path(reg_path: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let sub = HSTRING::from(reg_path);
    let name = HSTRING::from("InstallPath");
    let mut size: u32 = 0;
    // SAFETY: first call only asks for the size; all pointers are valid for the call.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &sub,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if r.is_err() || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` holds `size` bytes, and RegGetValueW writes at most that many.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &sub,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(not(windows))]
fn steam_install_path(_reg_path: &str) -> Option<String> {
    None
}

/// The launch block of `_run_engine` after the gate (beamtel.py:12349-12395).
fn launch_beamng(cfg: &ConfigMap) -> Result<(), String> {
    let stdout = tasklist_stdout()?;
    if stdout.contains("BeamNG.drive.x64.exe") {
        log::info!("BeamNG.drive is already running, skipping launch.");
        return Ok(());
    }
    let mut steam_path = None;
    for reg_path in [r"SOFTWARE\Valve\Steam", r"SOFTWARE\WOW6432Node\Valve\Steam"] {
        if let Some(p) = steam_install_path(reg_path) {
            steam_path = Some(p);
            break;
        }
    }
    let steam_path = steam_path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "Steam installation not found in registry.".to_string())?;
    let steam_exe = PathBuf::from(steam_path).join("steam.exe");
    let renderer = get_str(cfg, "beamng_renderer", "d3d");
    let gfx_flag = if renderer == "vulkan" { "vk" } else { "dx11" };
    std::process::Command::new(steam_exe)
        .args(["-applaunch", "284160", "-gfx", gfx_flag])
        .spawn()
        .map_err(|e| e.to_string())?;
    log::info!("Launched BeamNG.drive via Steam (-applaunch, {gfx_flag}).");
    Ok(())
}

// ---- _run_engine --------------------------------------------------------------------------

fn start_listener(name: &str, f: impl FnOnce() + Send + 'static) {
    spawn_daemon(name, f);
}

/// `_run_engine()`: the telemetry engine, run on the "beamtel-engine" thread.
pub fn run_engine() {
    let cfg = load_config();
    {
        let mut st = state();
        apply_startup_settings(&mut st, &cfg);
    }

    let cfg_value = Value::Object(cfg.clone());
    if !speech::init(Some(&cfg_value)) {
        log::warn!("No speech backend available; callouts will be silent.");
    } else {
        log::info!("Speech backend: {}", speech::describe_capabilities());
    }

    let audio_controller = AudioController::new();
    apply_audio_config(&audio_controller, &cfg);
    audio_controller.start();
    state::set_audio_controller_ref(Some(Arc::clone(&audio_controller)));

    apply_live_config(Some(Arc::clone(&audio_controller)));

    // Controller actions arrive on the bridge thread even without keyboard hooks.
    keyboard::start_command_worker();
    ui_bridge::register_accessibility_action_callback(Arc::new(menus::on_accessibility_action));

    start_listener("config-watcher", || config_watcher(&STOP));

    hill_climb_challenge()
        .set_finalize_callback(Some(Arc::new(listeners::hill_climb::hill_climb_finalized)));
    let ws_stop =
        match ui_bridge::start_server_in_thread(Arc::new(|text: &str, interrupt: bool| {
            let da = driving_assistant();
            let following = da.snapshot().following_route;
            if !suppressed_toast(text, following, da.seconds_since_arrival()) {
                speech_out::say(text, interrupt, false, "ui_bridge");
            }
        })) {
            Ok(stop) => Some(stop),
            Err(e) => {
                log::error!("Failed to start NVDA WS/HTTP bridge: {e}");
                None
            }
        };

    ui_bridge::register_dom_dump_callback(Arc::new(on_dom_dump_received));
    ui_bridge::register_loading_state_callback(Arc::new(speech_out::on_loading_state_changed));
    ui_bridge::register_settings_request_callback(Arc::new(broadcast_ui_settings));
    ui_bridge::register_screen_context_callback(Arc::new(browsers::on_screen_context));
    ui_bridge::register_page_text_callback(Arc::new(browsers::on_page_text));
    ui_bridge::register_challenge_event_callback(Arc::new(
        listeners::hill_climb::on_hill_climb_event,
    ));
    // `register_drift_event_callback(DRIFT_ANNOUNCER.handle)`; the bridge logs what the
    // handler raises as "drift_event callback error: ...", which the closure does here.
    ui_bridge::register_drift_event_callback(Arc::new(|data: &Value| {
        if let Err(e) = drift_announcer().handle(&json_to_pyops(data)) {
            log::error!("drift_event callback error: {e}");
        }
    }));

    // Wait for the updater's answer before touching the game; the timeout is the failure path.
    if !LAUNCH_GATE.wait_secs(LAUNCH_GATE_TIMEOUT_S) {
        log::warn!("Updater did not answer within 180s; launching as configured.");
    }
    if !LAUNCH_ALLOWED.load(Ordering::SeqCst) {
        log::info!("Game launch skipped: an update is being applied.");
    } else if LAUNCH_SUPPRESSED_BY_SHIFT.load(Ordering::SeqCst) {
        log::info!("Game launch skipped: Shift held at startup.");
    } else if get_bool(&cfg, "launch_beamng", false) {
        if let Err(e) = launch_beamng(&cfg) {
            log::warn!("Failed to launch BeamNG.drive: {e}");
        }
    }

    keyboard::install_hotkeys(Arc::clone(&audio_controller));

    use listeners::*;
    let ac = || Arc::clone(&audio_controller);
    start_listener("ui-listener", || ui_toast::ui_listener(&STOP));
    let a = ac();
    start_listener("scanner-listener", move || {
        scanner::scanner_listener(a, &STOP)
    });
    start_listener("driving-assistant", || {
        driving_assistant().run(|timeout| STOP.wait(Some(timeout)))
    });
    start_listener("scanner-callout", || {
        scanner::scanner_callout_thread_fn(&STOP)
    });
    let a = ac();
    start_listener("obstacle-listener", move || {
        obstacle::obstacle_listener(a, &STOP)
    });
    let a = ac();
    start_listener("road-listener", move || road::road_listener(a, &STOP));
    let a = ac();
    start_listener("implement-listener", move || {
        implement::implement_listener(a, &STOP)
    });
    let a = ac();
    start_listener("terrain-scan-listener", move || {
        terrain_scan::terrain_scan_listener(a, &STOP)
    });
    start_listener("cannon-shot-listener", || {
        cannon::cannon_shot_listener(&STOP)
    });
    start_listener("trailer-angle-listener", || {
        trailer::trailer_angle_listener(&STOP)
    });
    let a = ac();
    start_listener("camera-lineup-listener", move || {
        lineup::camera_lineup_listener(a, &STOP)
    });
    start_listener("wheel-report-listener", || {
        wheel_report::wheel_report_listener(&STOP)
    });
    start_listener("route-beacon-listener", || {
        route_beacon::route_beacon_listener(&STOP)
    });
    start_listener("fuel-station-listener", || {
        fuel::fuel_station_listener(&STOP)
    });
    let a = ac();
    start_listener("camera-listener", move || camera::camera_listener(a, &STOP));
    let a = ac();
    start_listener("nodegrab-listener", move || {
        nodegrab::nodegrab_listener(a, &STOP)
    });
    let a = ac();
    start_listener("clickspot-listener", move || {
        clickspot::clickspot_listener(a, &STOP)
    });
    start_listener("bindings-listener", || {
        vehicle_bindings::vehicle_bindings_listener(&STOP)
    });
    // beamtel may have started after the game spawned a vehicle and missed its push.
    vehicle_bindings::send_vehicle_bindings_cmd("REQUEST");
    start_listener("binding-learn-listener", || {
        binding_learn::binding_learn_listener(&STOP)
    });
    start_listener("binding-learn-keepalive", || {
        binding_learn::binding_learn_keepalive_thread_fn(&STOP)
    });
    start_listener("environment-listener", || {
        environment::environment_listener(&STOP)
    });
    start_listener("audio-debug-listener", || {
        audio_debug::audio_debug_listener(&STOP)
    });
    start_listener("vehicle-info-listener", || {
        vehicle_info::vehicle_info_listener(&STOP)
    });
    // beamtel may have started after the level loaded.
    browsers::send_env_cmd("REQUEST");
    start_listener("slot-listener", || slots::slot_listener(&STOP));
    let _slot_status = Timer::start(2.0, || crate::ai_layer::send_slot_command("SLOT_STATUS"));
    start_listener("console-listener", || console::console_listener(&STOP));
    let _ctxlist = Timer::start(2.0, || console::send_console_command("CTXLIST"));

    // MCP automation server, off by default; a failure to bind only logs.
    let mut mcp_stop: Option<Box<dyn FnOnce() + Send>> = None;
    if get_bool(&cfg, "mcp_server_enabled", false) {
        let started = catch_unwind(AssertUnwindSafe(
            || -> Result<Box<dyn FnOnce() + Send>, String> {
                crate::mcp_server::init(mcp_deps());
                console::register_console_tap(Some(Arc::new(crate::mcp_server::console_tap)));
                let port = cfg
                    .get("mcp_server_port")
                    .map(|v| py_int_value(v, "mcp_server_port"))
                    .unwrap_or(4481);
                crate::mcp_server::start(u16::try_from(port).map_err(|e| e.to_string())?)
            },
        ));
        match started {
            Ok(Ok(stop)) => mcp_stop = Some(stop),
            Ok(Err(e)) => log::error!("Failed to start MCP server: {e}"),
            Err(p) => log::error!("Failed to start MCP server: {}", panic_text(&p)),
        }
    }

    let spawner_ac = Arc::clone(&audio_controller);
    let spawner = catch_unwind(AssertUnwindSafe(move || {
        crate::vehicle_spawner::init(crate::vehicle_spawner::InitArgs {
            say_fn: Arc::new(speech_out::say),
            is_focused_fn: Arc::new(crate::win_util::is_beamng_focused),
            get_slots_fn: Arc::new(|| state::slots().vehicle_slots.clone()),
            close_others_fn: Arc::new(|| keyboard::release_arrow_owners("vehicle spawner opened")),
            ping_fn: Arc::new(move |rung, az, pitch| {
                spawner_ac.trigger_placement_ping(rung, az, pitch)
            }),
            request_info_fn: Arc::new(browsers::request_vehicle_info),
        });
        crate::vehicle_spawner::start();
    }));
    if let Err(p) = spawner {
        log::error!("Failed to start vehicle spawner: {}", panic_text(&p));
    }

    let run = catch_unwind(AssertUnwindSafe(|| {
        crate::telemetry::telemetry_loop(Arc::clone(&audio_controller), "0.0.0.0", 4444, &STOP)
    }));

    // finally:
    if hill_climb_challenge().is_capturing() {
        listeners::road::send_road_command("CAPTURE_OFF");
    }
    let _ = hill_climb_challenge().shutdown();
    if road_diagnostics()
        .status()
        .get("active")
        .is_some_and(|v| v.truthy())
    {
        listeners::road::send_road_command("DIAG_OFF");
        let _ = road_diagnostics().stop();
    }
    STOP.set();
    audio_controller.stop();
    if let Some(stop) = ws_stop {
        stop();
    }
    // Drop the tap before tearing the server down.
    console::register_console_tap(None);
    if let Some(stop) = mcp_stop {
        stop();
    }

    match run {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::error!("Telemetry loop ended with an error: {e}"),
        Err(p) => resume_unwind(p),
    }
}

/// The `mcp_server.Deps(...)` `_run_engine` builds.
fn mcp_deps() -> crate::mcp_server::Deps {
    crate::mcp_server::Deps {
        say_fn: Arc::new(|text, interrupt| speech_out::say(text, interrupt, true, "mcp")),
        get_speech_log_fn: Arc::new(speech_out::get_speech_log),
        snapshot_state_fn: Arc::new(state::mcp_snapshot_state),
        load_config_fn: Arc::new(load_config),
        send_console_command_fn: Arc::new(console::send_console_command),
        send_udp_fn: crate::mcp_server::make_udp_sender(),
        press_command_fn: Arc::new(crate::f9_commands::mcp_press_command),
        f9_help: crate::f9_commands::F9_HELP.clone(),
        world_active_fn: Arc::new(state::world_is_active),
        capture_png_fn: Arc::new(crate::capture::capture_scene_png),
        road_diagnostic_fn: Arc::new(listeners::road::road_diagnostic_control),
        assistant_diagnostic_fn: Arc::new(listeners::road::assistant_diagnostic_control),
        version: "1.0".to_string(),
    }
}

// ---- main() / _on_close / _run_update_flow ------------------------------------------------

/// Start `_run_engine` on the "beamtel-engine" thread.
pub fn start_engine_thread() -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("beamtel-engine".to_string())
        .spawn(run_engine)
}

/// `_run_update_flow(frame)`: the startup update check, then release the launch gate.
/// Scheduled by the window after it is up; releases `LAUNCH_GATE` exactly once.
pub fn run_update_flow(sink: &dyn UiSink) {
    let r = catch_unwind(AssertUnwindSafe(|| {
        updater::run_startup_flow(sink, &LaunchGate, false)
    }));
    if let Err(p) = r {
        log::error!("Update check failed to run: {}", panic_text(&p));
        updater::Gate::allow(&LaunchGate);
    }
}

/// The engine half of `BeamTelFrame._on_close` after the panels have flushed:
/// drop the console sink, `STOP.set()`, join the engine for at most 2 s.
pub fn close_sequence(engine: Option<JoinHandle<()>>) {
    crate::ui_sink::clear_ui_sink();
    STOP.set();
    if let Some(h) = engine {
        let deadline = Instant::now() + Duration::from_secs_f64(2.0);
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        }
    }
}

/// The end of `main()`, after the event loop: learn mode off, `STOP.set()`, and the
/// speech library's exit hook (Prism's atexit, which the Python got from the wheel).
pub fn after_main_loop() {
    if state().bindings.binding_learn_active {
        listeners::binding_learn::send_binding_learn_cmd("LEARN_OFF");
    }
    STOP.set();
    speech::atexit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness_and_coercions_follow_python() {
        assert!(!py_truthy(&json!(null)));
        assert!(!py_truthy(&json!(0)));
        assert!(!py_truthy(&json!(0.0)));
        assert!(!py_truthy(&json!("")));
        assert!(py_truthy(&json!("false")));
        assert!(py_truthy(&json!([0])));
        assert_eq!(py_int_value(&json!(6.9), "k"), 6);
        assert_eq!(py_int_value(&json!(" 7 "), "k"), 7);
        assert_eq!(py_int_value(&json!(true), "k"), 1);
        assert_eq!(py_float_value(&json!("15"), "k"), 15.0);
    }

    #[test]
    fn live_settings_apply_like_python() {
        let _g = crate::units::TEST_UNITS_LOCK.lock();
        let mut cfg = ConfigMap::new();
        cfg.insert("units".into(), json!("metric"));
        cfg.insert("telemetry_protocol".into(), json!("extended"));
        cfg.insert("compass_click_interval".into(), json!(30));
        cfg.insert("speed_announce_interval".into(), json!("oops"));
        cfg.insert("ai_describer_model".into(), json!("m"));
        let mut st = State::default();
        apply_live_settings(&mut st, &cfg);
        assert_eq!(units::units_mode(), "metric");
        assert_eq!(st.tel.protocol_mode, "extended");
        assert_eq!(st.cfg.compass_click_interval_deg, 30.0);
        assert_eq!(st.cfg.compass_highlight_nth_click, 6);
        assert_eq!(st.cfg.speed_announce_interval, 25.0);
        assert_eq!(
            st.cfg.ai_describer_settings["ai_describer_model"],
            json!("m")
        );
        assert_eq!(
            st.cfg.ai_describer_settings["ai_describer_openai_model"],
            json!("gpt-5.6-terra")
        );
        let mut empty = State::default();
        apply_live_settings(&mut empty, &ConfigMap::new());
        assert_eq!(empty.tel.protocol_mode, "outgauge");
        units::set_units_mode("imperial");
    }

    #[test]
    fn launch_gate_allow_and_deny() {
        use updater::Gate;
        LaunchGate.deny();
        assert!(LAUNCH_GATE.is_set());
        assert!(!LAUNCH_ALLOWED.load(Ordering::SeqCst));
        LaunchGate.allow();
        assert!(LAUNCH_ALLOWED.load(Ordering::SeqCst));
        LAUNCH_GATE.clear();
    }
}
