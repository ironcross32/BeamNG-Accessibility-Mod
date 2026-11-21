//! `diagnostic/controller_menu_sim.py`: offline simulation of the controller Accessibility
//! menu, with the same checks in the same order.
//!
//! As in the Python, the catalogs, F9 command execution and clickspot activation are
//! replaced by recorders, and nothing needs a controller, audio device, keyboard hook or
//! the game. The checks that exercise other workstreams' code rather than the navigator
//! (clickspot SNAP/EXEC sequencing, the clickspot browser's Enter, action announcements,
//! the config defaults and configuration UI, the Python listener's packet strings, the UI
//! bridge's action filter) belong to those modules' own tests and are listed at the end of
//! this file. The clickspot list states the navigator reads are set the way
//! `_clickspot_set_off` / `_clickspot_set_pending` / `_clickspot_begin_list` /
//! `_clickspot_append_row` leave them.

use std::sync::Arc;

use beam_engine::menus::{
    self, ACCESSIBILITY_SCREENS, FunctionAction, FunctionItem, StatusEntry, StatusMetric,
};
use beam_engine::speech_out::{self, get_speech_log};
use beam_engine::state::{State, state};
use beam_engine::timer::time_time;
use beam_engine::units::{PyVal, TELEMETRY_HAS_AIR};
use beam_testutil::repo_root;
use parking_lot::Mutex;

const OG_TURBO: i64 = 1 << 13;

/// An F9 command as `_invoke_f9_command` receives it.
type Cmd = (String, bool, bool, bool);
type CmdRef<'a> = (&'a str, bool, bool, bool);

struct Sim {
    failures: Vec<String>,
}

impl Sim {
    fn check(&mut self, label: &str, condition: bool, detail: impl std::fmt::Debug) {
        println!("{}{label}", if condition { "OK   " } else { "FAIL " });
        if !condition {
            self.failures.push(format!("{label}: {detail:?}"));
        }
    }
}

fn metric(label: &str, value: &'static str) -> StatusEntry {
    StatusEntry::Row(StatusMetric {
        label: label.to_string(),
        category: None,
        get_value: Arc::new(move |_| (PyVal::Str(value.to_string()), "units".to_string())),
        is_available: Arc::new(|_| true),
    })
}

fn item(
    label: &str,
    category: &str,
    command: &'static str,
    is_available: impl Fn(&State) -> bool + Send + Sync + 'static,
) -> FunctionItem {
    FunctionItem {
        label: label.to_string(),
        category: category.to_string(),
        is_available: Arc::new(is_available),
        action: FunctionAction::Command(command, false, false, false),
    }
}

fn last_spoken() -> String {
    get_speech_log(None, Some(1), None, None, false)
        .entries
        .last()
        .map(|e| e.text.clone())
        .unwrap_or_default()
}

fn nav(action: &str) {
    menus::navigate_accessibility_menu(action);
}

fn screen() -> i64 {
    state().menu.current_accessibility_screen_index
}

// The clickspot list states, as the E4 helpers leave them.
fn clickspot_set_off() {
    let mut st = state();
    st.clickspot.clickspot_mode_active = false;
    st.clickspot.clickspot_trigger_list.clear();
    st.clickspot.clickspot_list_loading = false;
    st.clickspot.clickspot_trigger_building = None;
    st.clickspot.clickspot_expected_count = None;
    st.menu.current_clickspot_index = 0;
}

fn clickspot_set_pending() {
    let mut st = state();
    st.clickspot.clickspot_mode_active = true;
    st.clickspot.clickspot_trigger_list.clear();
    st.clickspot.clickspot_list_loading = true;
    st.clickspot.clickspot_trigger_building = None;
    st.clickspot.clickspot_expected_count = None;
    st.menu.current_clickspot_index = 0;
}

/// `_clickspot_begin_list(n)` followed by every row arriving.
fn clickspot_publish(rows: &[(i64, i64, &str)]) {
    let mut st = state();
    st.clickspot.clickspot_mode_active = true;
    st.clickspot.clickspot_trigger_list = rows
        .iter()
        .map(|(c, t, n)| (*c, *t, n.to_string()))
        .collect();
    st.clickspot.clickspot_list_loading = false;
    st.clickspot.clickspot_trigger_building = None;
    st.clickspot.clickspot_expected_count = None;
    st.menu.current_clickspot_index = 0;
}

#[test]
fn controller_menu_sim() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("beam_e5_sim_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before anything in this test binary reads the environment.
        unsafe { std::env::set_var("LOCALAPPDATA", &dir) };
    });
    let _s = speech_out::TEST_SERIAL.lock();
    speech_out::reset_for_tests();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    *state() = State::default();

    let mut sim = Sim {
        failures: Vec::new(),
    };
    let commands: Arc<Mutex<Vec<Cmd>>> = Arc::new(Mutex::new(Vec::new()));
    let activations: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let optional = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let c2 = Arc::clone(&commands);
    menus::set_f9_invoker_override(Some(Arc::new(move |n: &str, c, s, a| {
        c2.lock().push((n.to_string(), c, s, a))
    })));
    let a2 = Arc::clone(&activations);
    menus::set_clickspot_activator_override(Some(Arc::new(move |i| a2.lock().push(i))));
    menus::set_status_metrics_override(Some(vec![
        metric("Status one", "one"),
        metric("Status two", "two"),
        metric("Status three", "three"),
    ]));
    let opt = Arc::clone(&optional);
    menus::set_function_items_override(Some(vec![
        item("Alpha one", "Alpha", "a", |_| true),
        item("Alpha two", "Alpha", "b", |_| true),
        item("Beta one", "Beta", "c", |_| true),
        item("Beta two", "Beta", "d", |_| true),
        item("Optional one", "Optional", "e", move |_| {
            opt.load(std::sync::atomic::Ordering::SeqCst)
        }),
    ]));
    let last_command = || commands.lock().last().cloned();
    let cmd = |n: &str, c: bool, s: bool, a: bool| Some((n.to_string(), c, s, a));
    clickspot_set_off();

    sim.check(
        "Status is the startup screen",
        ACCESSIBILITY_SCREENS[screen() as usize] == "Status",
        screen(),
    );

    nav("status_down");
    nav("next_menu");
    sim.check(
        "Next screen selects Functions and announces category",
        screen() == 1 && last_spoken() == "Functions: Alpha: Alpha one",
        last_spoken(),
    );
    nav("status_down");
    nav("next_menu");
    sim.check(
        "Next screen selects Click spots after Functions",
        screen() == 2 && last_spoken() == "Click spots: Turn on clickspot detection",
        last_spoken(),
    );
    nav("activate");
    sim.check(
        "Click spots enable item dispatches the existing toggle",
        last_command() == cmd("c", true, true, false),
        last_command(),
    );
    nav("next_menu");
    sim.check(
        "Three screens wrap and preserve the Status cursor",
        screen() == 0
            && state().menu.current_status_metric_index == 1
            && last_spoken() == "Status: Status two, two units",
        last_spoken(),
    );
    nav("previous_menu");
    sim.check(
        "Previous screen wraps from Status to Click spots",
        screen() == 2 && last_spoken() == "Click spots: Turn on clickspot detection",
        last_spoken(),
    );
    nav("previous_menu");
    sim.check(
        "Previous screen preserves the Functions cursor",
        screen() == 1
            && state().menu.current_functions_item_index == 1
            && last_spoken() == "Functions: Alpha: Alpha two",
        last_spoken(),
    );

    state().menu.current_functions_item_index = 0;
    nav("status_up");
    sim.check(
        "Functions wraps upward",
        state().menu.current_functions_item_index == 4 && last_spoken() == "Optional: Optional one",
        last_spoken(),
    );
    nav("status_down");
    sim.check(
        "Functions wraps downward",
        state().menu.current_functions_item_index == 0 && last_spoken() == "Alpha: Alpha one",
        last_spoken(),
    );

    state().menu.current_functions_item_index = 1;
    nav("status_down");
    let forward_boundary = last_spoken();
    nav("status_up");
    sim.check(
        "Category boundaries announce in both directions",
        forward_boundary == "Beta: Beta one" && last_spoken() == "Alpha: Alpha two",
        (&forward_boundary, last_spoken()),
    );
    let cursor_before = state().menu.current_functions_item_index;
    nav("status_repeat");
    sim.check(
        "Functions Repeat reads without moving",
        state().menu.current_functions_item_index == cursor_before && last_spoken() == "Alpha two",
        last_spoken(),
    );
    nav("activate");
    sim.check(
        "Functions Activate dispatches the selected command",
        last_command() == cmd("b", false, false, false)
            && state().menu.current_functions_item_index == cursor_before,
        last_command(),
    );

    nav("previous_menu");
    state().menu.current_status_metric_index = 0;
    nav("status_up");
    sim.check(
        "Status wraps upward",
        state().menu.current_status_metric_index == 2
            && last_spoken() == "Status three, three units",
        last_spoken(),
    );
    nav("status_down");
    sim.check(
        "Status wraps downward",
        state().menu.current_status_metric_index == 0 && last_spoken() == "Status one, one units",
        last_spoken(),
    );
    nav("status_repeat");
    sim.check(
        "Status Repeat reads only the value",
        last_spoken() == "one units",
        last_spoken(),
    );
    nav("activate");
    sim.check(
        "Status Activate repeats only the value",
        last_spoken() == "one units",
        last_spoken(),
    );

    nav("next_menu");
    optional.store(false, std::sync::atomic::Ordering::SeqCst);
    state().menu.current_functions_item_index = 999;
    nav("status_repeat");
    let idx = state().menu.current_functions_item_index;
    sim.check(
        "Availability changes safely normalize the cursor",
        (0..4).contains(&idx),
        idx,
    );

    nav("next_menu");
    clickspot_set_pending();
    nav("status_repeat");
    let command_count = commands.lock().len();
    let activation_count = activations.lock().len();
    nav("activate");
    sim.check(
        "Loading placeholder is announced and non-mutating",
        last_spoken() == "Detecting click spots"
            && commands.lock().len() == command_count
            && activations.lock().len() == activation_count,
        last_spoken(),
    );

    // "Partial clickspot transfers are not published" and "Completed clickspot transfers
    // publish atomically in cache order" test the E4 staging helpers; the published
    // result is set directly here.
    clickspot_publish(&[(0, 101, "Door latch"), (1, 102, "Window switch")]);
    nav("status_repeat");
    sim.check(
        "Click spots Repeat reads without moving",
        state().menu.current_clickspot_index == 0 && last_spoken() == "Door latch",
        last_spoken(),
    );
    nav("status_down");
    nav("activate");
    sim.check(
        "Click spots Activate dispatches the selected cache index",
        activations.lock().last() == Some(&1) && state().menu.current_clickspot_index == 1,
        activations.lock().clone(),
    );
    nav("status_down");
    let downward_wrap = last_spoken();
    nav("status_up");
    sim.check(
        "Click spots wrap in both directions",
        downward_wrap == "Door latch"
            && state().menu.current_clickspot_index == 1
            && last_spoken() == "Window switch",
        (&downward_wrap, last_spoken()),
    );

    nav("next_menu");
    nav("next_menu");
    nav("next_menu");
    sim.check(
        "Click spots preserve their cursor across screen changes",
        screen() == 2
            && state().menu.current_clickspot_index == 1
            && last_spoken() == "Click spots: Window switch",
        last_spoken(),
    );

    clickspot_set_pending();
    {
        let st = state();
        let ok = st.clickspot.clickspot_list_loading
            && st.clickspot.clickspot_trigger_list.is_empty()
            && st.menu.current_clickspot_index == 0;
        drop(st);
        sim.check(
            "A vehicle recache immediately clears stale rows and resets the cursor",
            ok,
            "",
        );
    }
    clickspot_publish(&[]);
    nav("status_repeat");
    let activation_count = activations.lock().len();
    nav("activate");
    sim.check(
        "Empty placeholder is announced and non-mutating",
        last_spoken() == "No click spots found"
            && !state().clickspot.clickspot_list_loading
            && activations.lock().len() == activation_count,
        last_spoken(),
    );

    // Validate the production catalog after the navigation state machine checks.
    menus::set_function_items_override(None);
    let real_functions = menus::function_items();
    let t = true;
    let f = false;
    let expected_commands: Vec<(&str, CmdRef)> = vec![
        ("Toggle vehicle scanner", ("v", t, f, f)),
        ("Next vehicle scanner target", ("tab", f, f, f)),
        ("Previous vehicle scanner target", ("tab", f, t, f)),
        ("Lock onto nearest vehicle", ("tab", t, f, f)),
        ("Distance and orientation", ("d", f, f, f)),
        ("Relative bearing", ("d", f, t, f)),
        ("Trailer or ramp alignment", ("v", f, t, f)),
        ("Coupler distance callouts", ("d", t, t, f)),
        ("Toggle alignment instrument", ("i", t, f, f)),
        ("Alignment or cannon readout", ("i", f, f, f)),
        ("Cycle reference band", ("i", f, t, f)),
        ("Pedal tones", ("c", t, f, f)),
        ("Heading guidance", ("h", t, f, f)),
        ("Coordinate guidance", ("g", t, f, f)),
        ("Drift detection", ("d", t, f, f)),
        ("Low-speed detection", ("l", t, t, f)),
        ("Wheel-slip detection", ("k", t, f, f)),
        ("Obstacle detection", ("o", t, f, f)),
        ("Road detection", ("r", t, f, f)),
        ("Road-status readout", ("r", t, t, f)),
        ("Mark waypoint", ("c", f, t, f)),
        ("Speak marked coordinates", ("c", f, f, t)),
        ("Distance and bearing", ("w", f, f, f)),
        ("Toggle route beacon", ("w", t, f, f)),
        ("Nearest point of interest", ("w", f, t, f)),
        ("Redline RPM", ("r", f, t, f)),
        ("Maximum boost pressure", ("t", f, t, f)),
        ("Forced induction details", ("t", f, f, t)),
        ("Air pressure", ("p", f, f, f)),
        ("Attitude", ("a", f, f, f)),
        ("Coordinates", ("c", f, f, f)),
        ("Damage report", ("m", f, f, f)),
        ("Toggle camera information", ("f", f, f, t)),
        ("Camera heading", ("h", f, f, t)),
        ("Camera altitude", ("a", f, f, t)),
        ("Camera pitch", ("p", f, f, t)),
        ("Vehicle bearing", ("v", f, f, t)),
        ("Vehicle distance", ("d", f, f, t)),
        ("Accessible node grabber", ("n", t, f, f)),
        ("Learn bindings mode", ("b", f, t, f)),
        ("Clickspot detection", ("c", t, t, f)),
        ("Switch units", ("u", f, f, f)),
    ];
    let actual_commands: Vec<(String, (String, bool, bool, bool))> = real_functions
        .iter()
        .filter_map(|row| match &row.action {
            FunctionAction::Command(n, c, s, a) => {
                Some((row.label.clone(), (n.to_string(), *c, *s, *a)))
            }
            FunctionAction::Handler(_) => None,
        })
        .collect();
    let expected_owned: Vec<(String, (String, bool, bool, bool))> = expected_commands
        .iter()
        .map(|(l, (n, c, s, a))| (l.to_string(), (n.to_string(), *c, *s, *a)))
        .collect();
    sim.check(
        "Functions catalog maps every item to the intended F9 command",
        actual_commands == expected_owned,
        &actual_commands,
    );
    commands.lock().clear();
    for row in &real_functions {
        if matches!(row.action, FunctionAction::Command(..)) {
            menus::execute_function_item(row);
        }
    }
    let dispatched = commands.lock().clone();
    let want: Vec<(String, bool, bool, bool)> =
        expected_owned.iter().map(|(_, c)| c.clone()).collect();
    sim.check(
        "Every Functions command dispatches to its existing F9 handler",
        dispatched == want,
        &dispatched,
    );
    let terrain = real_functions
        .iter()
        .find(|row| row.label == "Terrain scan")
        .unwrap();
    sim.check(
        "Terrain scan uses the direct driving-only handler",
        matches!(&terrain.action, FunctionAction::Handler(h)
            if Arc::ptr_eq(h, &menus::terrain_scan_handler())),
        "",
    );

    {
        let mut st = state();
        st.scanner.scan_mode_active = false;
        st.camera.free_cam_active = false;
        st.tel.marked_coord_x = None;
        st.tel.protocol_mode = "outgauge".to_string();
        st.tel.last_protocol_flags = 0;
        st.tel.last_air_pressure_max = 0.0;
        st.tel.last_telemetry_ts = 0.0;
    }
    let dependents = [
        "Distance and orientation",
        "Coordinate guidance",
        "Maximum boost pressure",
        "Forced induction details",
        "Air pressure",
        "Camera heading",
        "Terrain scan",
        "Nearest point of interest",
    ];
    let hidden: Vec<String> = {
        let st = state();
        real_functions
            .iter()
            .filter(|row| !(row.is_available)(&st))
            .map(|row| row.label.clone())
            .collect()
    };
    sim.check(
        "Dependent functions hide while unavailable",
        dependents.iter().all(|d| hidden.iter().any(|h| h == d)),
        &hidden,
    );
    {
        let mut st = state();
        st.scanner.scan_mode_active = true;
        st.camera.free_cam_active = true;
        st.tel.marked_coord_x = Some(1.0);
        st.tel.protocol_mode = "extended".to_string();
        st.tel.last_protocol_flags = OG_TURBO;
        // The air system is gated on the tank existing, so an EMPTY tank must still show it.
        st.tel.last_air_pressure = 0.0;
        st.tel.last_telemetry_presence |= TELEMETRY_HAS_AIR;
        st.tel.last_telemetry_ts = time_time();
    }
    let shown: Vec<String> = {
        let st = state();
        real_functions
            .iter()
            .filter(|row| (row.is_available)(&st))
            .map(|row| row.label.clone())
            .collect()
    };
    sim.check(
        "Dependent functions reappear from live state",
        dependents.iter().all(|d| shown.iter().any(|s| s == d)),
        &shown,
    );

    let lua_path = repo_root()
        .join("bng_mod")
        .join("lua")
        .join("ge")
        .join("extensions")
        .join("clickspotAccessible.lua");
    match std::fs::read_to_string(&lua_path) {
        Ok(lua_source) => sim.check(
            "Lua invalidates stale rows across every cache lifecycle",
            lua_source
                .matches("sendListState(\"TRIGGER_LIST_PENDING\")")
                .count()
                >= 4
                && lua_source
                    .matches("sendListState(\"TRIGGER_LIST_OFF\")")
                    .count()
                    >= 3,
            "",
        ),
        Err(e) => println!(
            "SKIP Lua invalidates stale rows ({}: {e})",
            lua_path.display()
        ),
    }

    menus::set_status_metrics_override(None);
    menus::set_function_items_override(None);
    menus::set_f9_invoker_override(None);
    menus::set_clickspot_activator_override(None);
    speech_out::set_speak_override(None);
    speech_out::reset_for_tests();

    if !sim.failures.is_empty() {
        panic!("\n{}", sim.failures.join("\n"));
    }
    println!("\nController menu simulation passed.");
}

// Not ported here, because they test other workstreams' code (see the module docs):
// "Partial clickspot transfers are not published",
// "Completed clickspot transfers publish atomically in cache order" (E4, clickspot staging),
// "Controller clickspot activation uses SNAP then press and release",
// "Keyboard Enter uses the same clickspot activation helper",
// "Successful/Failed clickspot activation is silent/announced ..." (E4, clickspot.rs),
// "Clickspot action announcements default off in both config backends" (B1, config),
// "Configuration UI loads and saves the clickspot announcement checkbox" (U1/U2),
// "Python listener accepts Lua pending and off lifecycle packets" (E4),
// "Python bridge allows exactly the six controller actions" (E9: `ui_bridge_actions.rs`).
