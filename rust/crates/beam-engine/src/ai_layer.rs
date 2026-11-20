//! Owner: E7. The F10 AI layer (`beamtel.py:5344-5390, 9366-9445, 9704-10315`):
//! `_F10_HELP`, `_send_ai_command`, the driving assistant's callbacks, slots selection,
//! presets, AI status readouts, `_on_ai_key_press`, `_start_ai_capture`.

use std::collections::HashMap;
use std::sync::LazyLock;

use beam_base::pyfmt::py_float_str;
use beam_base::pyval::{py_int_parse, py_str_repr};
use indexmap::IndexMap;
use parking_lot::RwLock;

use crate::f9_commands::HelpKey;
use crate::keyboard::{
    COMMAND_TIMEOUT_SEC, KeyEvent, KeyEventType, Layer, kb_close_layer, kb_layer,
};
use crate::net::{AI_CMD_PORT, SLOT_CMD_PORT_OUT, send_cmd};
use crate::speech_out::{protect_speech, say, set_command_context};
use crate::state::{
    AiCommand, Mods, SlotInfo, audio_controller_ref, driving_assistant, slots, state,
};
use crate::timer::{Timer, spawn_daemon};
use crate::units::{MPH_PER_MS, units_mode};

/// `_F10_HELP` (including the generated digit rows), in the Python's insertion order.
pub static F10_HELP: LazyLock<IndexMap<HelpKey, String>> = LazyLock::new(|| {
    const F: bool = false;
    const T: bool = true;
    let mut m: IndexMap<HelpKey, String> = IndexMap::new();
    let base: &[(HelpKey, &str)] = &[
        (
            ("h", F, F, F),
            "Toggle driving assistant for the current vehicle",
        ),
        (("d", F, F, F), "Disable AI"),
        (("t", F, F, F), "Traffic mode"),
        (("r", F, F, F), "Random mode"),
        (("s", F, F, F), "Stop AI"),
        (("c", F, F, F), "Chase mode"),
        (("f", F, F, F), "Follow mode"),
        (("e", F, F, F), "Flee mode"),
        (("a", F, F, F), "Cycle avoid cars"),
        (("a", T, F, F), "Select all vehicles"),
        (("n", T, F, F), "Clear selection"),
        (("l", F, F, F), "Toggle lane driving"),
        (("+", F, F, F), "Increase speed limit"),
        (("=", F, F, F), "Increase speed limit"),
        (("-", F, F, F), "Decrease speed limit"),
        (("0", F, F, F), "Clear speed limit"),
        (("space", F, F, F), "Describe scene (AI)"),
    ];
    for (k, v) in base {
        m.insert(*k, v.to_string());
    }
    // Aggression keys 1-9 (unmodified)
    for (k, v) in [
        ("1", "0.2"),
        ("2", "0.4"),
        ("3", "0.7"),
        ("4", "0.9"),
        ("5", "1.0"),
        ("6", "1.2"),
        ("7", "1.5"),
        ("8", "1.8"),
        ("9", "2.0"),
    ] {
        m.insert((k, F, F, F), format!("Aggression {v}"));
    }
    // Vehicle slot selection (SHIFT+digit) and target (CTRL+digit)
    for k in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"] {
        let label = if k == "0" { "10" } else { k };
        m.insert((k, F, T, F), format!("Toggle slot {label} selection"));
        m.insert((k, T, F, F), format!("Set slot {label} as AI target"));
    }
    // Preset configurations (ALT+digit)
    m.insert(
        ("1", F, F, T),
        "Preset: Motorcade (each follows the one ahead)".into(),
    );
    m.insert(
        ("2", F, F, T),
        "Preset: Gang-up (all chase vehicle 1)".into(),
    );
    m.insert(
        ("3", F, F, T),
        "Preset: Tour of Destruction (chase chain)".into(),
    );
    m.insert(
        ("4", F, F, T),
        "Preset: Tour of Destruction Spectator (you watch)".into(),
    );
    m.insert(
        ("5", F, F, T),
        "Preset: Police mode (all follow CTRL target)".into(),
    );
    m
});

/// `_AGGRESSION_MAP`.
pub const AGGRESSION_MAP: [(&str, f64); 9] = [
    ("1", 0.2),
    ("2", 0.4),
    ("3", 0.7),
    ("4", 0.9),
    ("5", 1.0),
    ("6", 1.2),
    ("7", 1.5),
    ("8", 1.8),
    ("9", 2.0),
];
/// `_AVOID_CYCLE`.
pub const AVOID_CYCLE: [&str; 3] = ["auto", "on", "off"];
/// `_PRESET_DELAY_SEC`.
pub const PRESET_DELAY_SEC: f64 = 3.0;

// ---- Test seams --------------------------------------------------------------------------

/// Receives `(port, command)` instead of the network (tests only).
pub type SendHook = Box<dyn Fn(u16, &str) + Send + Sync>;
/// Receives a `_run_staggered` batch instead of the timed worker (tests only).
pub type StaggerHook = Box<dyn Fn(&[String]) + Send + Sync>;

static SEND_HOOK: RwLock<Option<SendHook>> = RwLock::new(None);
static STAGGER_HOOK: RwLock<Option<StaggerHook>> = RwLock::new(None);

/// Route every E7 command (ports 4449, 4459, 4464) to `hook`. `None` restores sockets.
#[doc(hidden)]
pub fn set_send_hook_for_tests(hook: Option<SendHook>) {
    *SEND_HOOK.write() = hook;
}

/// Capture `_run_staggered` batches instead of sending them 3 s apart.
#[doc(hidden)]
pub fn set_staggered_hook_for_tests(hook: Option<StaggerHook>) {
    *STAGGER_HOOK.write() = hook;
}

pub(crate) fn e7_send(port: u16, cmd: &str, what: &str) {
    if let Some(h) = SEND_HOOK.read().as_ref() {
        h(port, cmd);
        return;
    }
    send_cmd(port, cmd, what);
}

fn say_x(text: &str) {
    say(text, true, true, "python");
}

/// Python's `int(s)` for a key name; raises (panics) as Python does.
fn py_int_key(s: &str) -> i64 {
    py_int_parse(s)
        .and_then(|i| i.to_i64())
        .unwrap_or_else(|| panic!("invalid literal for int() with base 10: {}", py_str_repr(s)))
}

// ---- Senders and the assistant's callbacks -----------------------------------------------

/// `_send_ai_command(cmd)` (port 4449).
pub fn send_ai_command(cmd: &str) {
    e7_send(AI_CMD_PORT, cmd, "AI command");
}

/// `_send_slot_command(cmd)` (port 4459).
pub fn send_slot_command(cmd: &str) {
    e7_send(SLOT_CMD_PORT_OUT, cmd, "slot command");
}

/// `_assistant_cue(kind, choices=())`, the driving assistant's cue callback.
pub fn assistant_cue(kind: &str, choices: &[String]) {
    // The cue comes just before the sentence saying why assistance ended. A compass
    // heading landing with it cut an arrival off 6 ms in, so background speech queues.
    if kind == "off" || kind == "protect" {
        protect_speech();
    }
    if kind == "protect" {
        return;
    }
    if let Some(controller) = audio_controller_ref() {
        let c: Vec<&str> = choices.iter().map(String::as_str).collect();
        controller.trigger_assistant_cue(kind, &c);
    }
}

/// `isinstance(v, (int, float))` (bool included) and `float(v)`.
fn arrival_number(v: Option<&beam_road::pyjson::Value>) -> Option<f64> {
    use beam_road::pyjson::Value as V;
    let v = v?;
    match v {
        V::Bool(_) | V::Int(_) | V::BigInt(_) | V::Float(_) => {
            Some(v.py_float().unwrap_or_else(|e| panic!("{}", e.message())))
        }
        _ => None,
    }
}

/// `_assistant_arrival(arrival)`: an off-road destination stays guidable once the route
/// has been cleared.
pub fn assistant_arrival(arrival: &beam_road::pyjson::Value) {
    let Some(d) = arrival.as_dict() else {
        panic!("'{}' object has no attribute 'get'", arrival.type_name());
    };
    let off_road = d.get("offRoad").is_some_and(|v| v.truthy());
    if !off_road {
        return;
    }
    let (Some(x), Some(y)) = (arrival_number(d.get("x")), arrival_number(d.get("y"))) else {
        return;
    };
    let mut st = state();
    st.tel.marked_coord_x = Some(x);
    st.tel.marked_coord_y = Some(y);
    st.tel.last_coord_bearing_ts = 0.0; // force immediate recalculation
}

// ---- Commands, lights, status ------------------------------------------------------------

/// `_record_ai_command(vid, mode, target_id=None)`.
fn record_ai_command(vid: i64, mode: &str, target_id: Option<i64>) {
    state().ai.ai_vehicle_commands.insert(
        vid,
        AiCommand {
            mode: mode.to_string(),
            target_id,
        },
    );
}

/// `_set_lightbars(state, vids)`.
fn set_lightbars(level: i64, vids: &[i64]) {
    if vids.is_empty() {
        return;
    }
    let ids: Vec<String> = vids.iter().map(i64::to_string).collect();
    send_ai_command(&format!("LIGHTBAR:{level}:{}", ids.join(",")));
}

/// `_clear_police_lights()`.
fn clear_police_lights() {
    let ids: Vec<i64> = {
        let mut st = state();
        if st.ai.police_lights_active.is_empty() {
            return;
        }
        std::mem::take(&mut st.ai.police_lights_active)
            .into_iter()
            .collect()
    };
    set_lightbars(0, &ids);
}

fn status_phrase(mode: &str, tname: Option<&str>) -> String {
    let t = tname.filter(|t| !t.is_empty());
    match mode {
        "disabled" => "AI disabled".to_string(),
        "stop" => "stopped".to_string(),
        "traffic" | "random" => format!("{mode} mode"),
        "chase" => t.map_or("chasing".to_string(), |t| format!("chasing {t}")),
        "follow" => t.map_or("following".to_string(), |t| format!("following {t}")),
        "flee" => t.map_or("fleeing".to_string(), |t| format!("fleeing from {t}")),
        other => other.to_string(),
    }
}

/// `_speak_status_all_response(payload)`: an `AI_STATUS_ALL:` response from the game.
pub fn speak_status_all_response(payload: &str) {
    if payload.is_empty() || payload == "no vehicles" {
        say_x("No vehicles tracked in game");
        return;
    }
    let mut parts = Vec::new();
    for entry in payload.split('|') {
        let fields: Vec<&str> = entry.splitn(4, ',').collect();
        if fields.len() < 3 {
            continue;
        }
        let (slot_num, vname, mode) = (fields[0], fields[1], fields[2]);
        let tname = fields.get(3).copied().unwrap_or("");
        let key = if slot_num == "10" { "0" } else { slot_num };
        let status = status_phrase(mode, Some(tname));
        parts.push(format!("Vehicle {key}, {vname}: {status}"));
    }
    if !parts.is_empty() {
        say_x(&parts.join(". "));
    } else {
        say_x("No status data received");
    }
}

/// `vid == driving_assistant.vehicle` for an int vid.
fn vehicle_is(v: &beam_road::pyjson::Value, vid: i64) -> bool {
    use beam_road::pyjson::Value as V;
    match v {
        V::Int(i) => *i == vid,
        V::Bool(b) => (*b as i64) == vid,
        V::Float(f) => *f == vid as f64,
        _ => false,
    }
}

/// `_speak_ai_status()`: the last commanded AI mode for every tracked vehicle. (No caller
/// in the Python; kept for parity.)
pub fn speak_ai_status() {
    let slot_map = slots().vehicle_slots.clone();
    let commands = state().ai.ai_vehicle_commands.clone();
    let mut vid_to_name: HashMap<i64, String> = HashMap::new();
    for info in slot_map.values() {
        vid_to_name.insert(info.id, info.name.clone());
    }
    let da = driving_assistant();
    if slot_map.is_empty() {
        say_x(if da.active() {
            "Driving assistant on"
        } else {
            "No vehicles tracked"
        });
        return;
    }
    let mut keys: Vec<i64> = slot_map.keys().copied().collect();
    keys.sort();
    let mut parts = Vec::new();
    for slot_num in keys {
        let info = &slot_map[&slot_num];
        let key = slot_key_label(slot_num);
        let status = if da.active() && vehicle_is(&da.vehicle(), info.id) {
            "driving assistant on".to_string()
        } else {
            match commands.get(&info.id) {
                None => "no command given".to_string(),
                Some(cmd) => {
                    let tname = cmd.target_id.and_then(|t| vid_to_name.get(&t).cloned());
                    status_phrase(&cmd.mode, tname.as_deref())
                }
            }
        };
        parts.push(format!("Vehicle {key}, {}: {status}", info.name));
    }
    say_x(&parts.join(". "));
}

/// `_reset_ai_timer()`: restart the F10 inactivity timer without closing the layer.
/// (No caller in the Python; kept for parity.)
pub fn reset_ai_timer() {
    let old = state().ai.ai_timer.take();
    if let Some(t) = old {
        t.cancel();
    }
    let t = Timer::start(COMMAND_TIMEOUT_SEC, || clear_ai_hook(true));
    state().ai.ai_timer = Some(t);
}

/// `_slot_key_label(slot)`: slot 10 is key "0".
pub fn slot_key_label(slot: i64) -> String {
    if slot == 10 {
        "0".to_string()
    } else {
        slot.to_string()
    }
}

/// `_toggle_slot_selection(slot)`. Does not close the F10 layer.
fn toggle_slot_selection(slot: i64) {
    let (name, selected) = {
        let mut s = slots();
        let Some(info) = s.vehicle_slots.get(&slot) else {
            say_x(&format!("Slot {} is empty", slot_key_label(slot)));
            return;
        };
        let name = info.name.clone();
        if Some(slot) == s.target_slot {
            say_x(&format!("Cannot select {name}, it is the target vehicle"));
            return;
        }
        let selected = if s.selected_slots.contains(&slot) {
            s.selected_slots.remove(&slot);
            false
        } else {
            s.selected_slots.insert(slot);
            true
        };
        (name, selected)
    };
    say_x(&format!(
        "{name}, {}",
        if selected { "selected" } else { "unselected" }
    ));
}

/// `_select_all_slots()`. Does not close the F10 layer.
fn select_all_slots() {
    let names: Vec<String> = {
        let mut s = slots();
        let target = s.target_slot;
        let mut selectables: Vec<i64> = s
            .vehicle_slots
            .keys()
            .copied()
            .filter(|k| Some(*k) != target)
            .collect();
        s.selected_slots = selectables.iter().copied().collect();
        selectables.sort();
        selectables
            .iter()
            .map(|k| s.vehicle_slots[k].name.clone())
            .collect()
    };
    if !names.is_empty() {
        say_x(&format!("Selected: {}", names.join(", ")));
    } else {
        say_x("No vehicles to select");
    }
}

/// `_set_target_slot(slot)`, with confirmation if the slot is currently selected.
fn set_target_slot(slot: i64) {
    let (name, is_selected) = {
        let s = slots();
        match s.vehicle_slots.get(&slot) {
            None => {
                drop(s);
                say_x(&format!("Slot {} is empty", slot_key_label(slot)));
                state().ai.pending_target_confirm = None;
                return;
            }
            Some(info) => (info.name.clone(), s.selected_slots.contains(&slot)),
        }
    };
    if is_selected {
        let pending = state().ai.pending_target_confirm;
        if pending == Some(slot) {
            // Confirmed: set target and silently remove from selection.
            {
                let mut s = slots();
                s.target_slot = Some(slot);
                s.selected_slots.remove(&slot);
            }
            state().ai.pending_target_confirm = None;
            say_x(&format!("Target set to {name}, removed from selection"));
        } else {
            state().ai.pending_target_confirm = Some(slot);
            let label = slot_key_label(slot);
            say_x(&format!(
                "{name} is selected. Press CTRL+{label} again to confirm target change, any other key to cancel"
            ));
        }
    } else {
        state().ai.pending_target_confirm = None;
        slots().target_slot = Some(slot);
        say_x(&format!("Target: {name}"));
    }
}

/// `_dispatch_ai_mode(mode)`: to all selected vehicles, or the player vehicle if none.
fn dispatch_ai_mode(mode: &str) {
    if mode == "disabled" {
        send_ai_command("ASSISTANT_OFF");
    }
    clear_police_lights();
    let (selected, target_slot, slot_map) = {
        let s = slots();
        (
            s.selected_slots.clone(),
            s.target_slot,
            s.vehicle_slots.clone(),
        )
    };

    if selected.is_empty() {
        // Legacy single-vehicle path: targets the current player vehicle via scanner.
        send_ai_command(&format!("MODE:{mode}"));
        // Record against slot 1 as best approximation of the player vehicle.
        if let Some(info) = slot_map.get(&1) {
            record_ai_command(info.id, mode, None);
        }
        return;
    }

    let ids: Vec<i64> = selected
        .iter()
        .filter_map(|s| slot_map.get(s).map(|i| i.id))
        .collect();
    let joined = || ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    if matches!(mode, "chase" | "follow" | "flee") {
        let Some(target) = target_slot.and_then(|t| slot_map.get(&t)) else {
            say_x("No target set. Use CTRL+1-0 to set a target vehicle.");
            return;
        };
        let target_id = target.id;
        if ids.is_empty() {
            say_x("No valid vehicles selected");
            return;
        }
        send_ai_command(&format!("MULTI_MODE:{mode}:{target_id}:{}", joined()));
        for vid in &ids {
            record_ai_command(*vid, mode, Some(target_id));
        }
    } else {
        if ids.is_empty() {
            say_x("No valid vehicles selected");
            return;
        }
        send_ai_command(&format!("MULTI_MODE:{mode}:none:{}", joined()));
        for vid in &ids {
            record_ai_command(*vid, mode, None);
        }
    }
}

/// `_run_staggered(commands)`: send them `_PRESET_DELAY_SEC` apart on a daemon thread.
fn run_staggered(commands: Vec<String>) {
    if let Some(h) = STAGGER_HOOK.read().as_ref() {
        h(&commands);
        return;
    }
    run_staggered_with_delay(commands, PRESET_DELAY_SEC);
}

/// `_run_staggered` with the gap as a parameter (tests use a short one).
#[doc(hidden)]
pub fn run_staggered_with_delay(commands: Vec<String>, delay_s: f64) {
    spawn_daemon("Thread", move || {
        for (i, cmd) in commands.iter().enumerate() {
            if i > 0 {
                crate::timer::sleep(delay_s);
            }
            send_ai_command(cmd);
        }
    });
}

type SlotMap = IndexMap<i64, SlotInfo>;

/// `_preset_motorcade`: each vehicle follows the one before it in slot order.
fn preset_motorcade(s: &SlotMap, occupied: &[i64]) {
    if occupied.len() < 2 {
        say_x("Motorcade needs at least 2 vehicles");
        return;
    }
    let mut cmds = Vec::new();
    for i in 1..occupied.len() {
        let follower_id = s[&occupied[i]].id;
        let leader_id = s[&occupied[i - 1]].id;
        cmds.push(format!("MULTI_MODE:follow:{leader_id}:{follower_id}"));
        record_ai_command(follower_id, "follow", Some(leader_id));
    }
    say_x(&format!("Motorcade, {} vehicles", occupied.len()));
    run_staggered(cmds);
}

/// `_preset_gangup`: every vehicle except slot 1 chases slot 1.
fn preset_gangup(s: &SlotMap, occupied: &[i64]) {
    let Some(target) = s.get(&1) else {
        say_x("Gang-up requires a vehicle in slot 1");
        return;
    };
    let target_id = target.id;
    let chasers: Vec<i64> = occupied.iter().copied().filter(|k| *k != 1).collect();
    if chasers.is_empty() {
        say_x("Gang-up needs at least 2 vehicles");
        return;
    }
    let mut cmds = Vec::new();
    for k in &chasers {
        let vid = s[k].id;
        cmds.push(format!("MULTI_MODE:chase:{target_id}:{vid}"));
        record_ai_command(vid, "chase", Some(target_id));
    }
    say_x(&format!(
        "Gang-up, {} vehicles on {}",
        chasers.len(),
        target.name
    ));
    run_staggered(cmds);
}

/// `_preset_tod`: chase chain in slot order.
fn preset_tod(s: &SlotMap, occupied: &[i64]) {
    if occupied.len() < 2 {
        say_x("Tour of Destruction needs at least 2 vehicles");
        return;
    }
    let mut cmds = Vec::new();
    for i in 1..occupied.len() {
        let chaser_id = s[&occupied[i]].id;
        let target_id = s[&occupied[i - 1]].id;
        cmds.push(format!("MULTI_MODE:chase:{target_id}:{chaser_id}"));
        record_ai_command(chaser_id, "chase", Some(target_id));
    }
    say_x(&format!("Tour of Destruction, {} vehicles", occupied.len()));
    run_staggered(cmds);
}

/// `_preset_tod_spectator`: chase chain among slots 2+; slot 1 watches.
fn preset_tod_spectator(s: &SlotMap, occupied: &[i64]) {
    let non_p1: Vec<i64> = occupied.iter().copied().filter(|k| *k != 1).collect();
    if non_p1.len() < 2 {
        say_x("Tour of Destruction Spectator needs at least 3 vehicles total");
        return;
    }
    let mut cmds = Vec::new();
    let lead_id = s[&non_p1[0]].id;
    cmds.push(format!("MULTI_MODE:random:none:{lead_id}"));
    record_ai_command(lead_id, "random", None);
    for i in 1..non_p1.len() {
        let chaser_id = s[&non_p1[i]].id;
        let target_id = s[&non_p1[i - 1]].id;
        cmds.push(format!("MULTI_MODE:chase:{target_id}:{chaser_id}"));
        record_ai_command(chaser_id, "chase", Some(target_id));
    }
    say_x(&format!(
        "Tour of Destruction Spectator, {} AI vehicles, you watch",
        non_p1.len()
    ));
    run_staggered(cmds);
}

/// `_preset_police`: all vehicles except the target follow the target.
fn preset_police(s: &SlotMap, occupied: &[i64], target_slot: Option<i64>) {
    let Some(bad_guy) = target_slot.and_then(|t| s.get(&t)) else {
        say_x("Set the bad guy target first with CTRL+1-0");
        return;
    };
    let bad_guy_id = bad_guy.id;
    let followers: Vec<i64> = occupied
        .iter()
        .copied()
        .filter(|k| Some(*k) != target_slot)
        .collect();
    if followers.is_empty() {
        say_x("Police mode needs at least 2 vehicles");
        return;
    }
    let mut cmds = Vec::new();
    let mut follower_ids = Vec::new();
    for k in &followers {
        let vid = s[k].id;
        follower_ids.push(vid);
        cmds.push(format!("MULTI_MODE:follow:{bad_guy_id}:{vid}"));
        record_ai_command(vid, "follow", Some(bad_guy_id));
    }
    say_x(&format!(
        "Police mode, {} vehicles following {}",
        followers.len(),
        bad_guy.name
    ));
    run_staggered(cmds);
    // Lights + siren on each follower. Vehicles without a sirenSound ignore state 2
    // (BeamNG wraps modulo 2 -> 0); police-equipped vehicles engage both.
    set_lightbars(2, &follower_ids);
    state().ai.police_lights_active = follower_ids.into_iter().collect();
}

/// `_trigger_preset(preset_num)`.
fn trigger_preset(preset_num: i64) {
    let (slot_map, target_slot) = {
        let s = slots();
        (s.vehicle_slots.clone(), s.target_slot)
    };
    let mut occupied: Vec<i64> = slot_map.keys().copied().collect();
    occupied.sort();
    if occupied.is_empty() {
        say_x("No vehicles tracked yet");
        return;
    }
    if preset_num != 5 {
        clear_police_lights();
    }
    match preset_num {
        1 => preset_motorcade(&slot_map, &occupied),
        2 => preset_gangup(&slot_map, &occupied),
        3 => preset_tod(&slot_map, &occupied),
        4 => preset_tod_spectator(&slot_map, &occupied),
        5 => preset_police(&slot_map, &occupied, target_slot),
        _ => {}
    }
}

// ---- The layer ---------------------------------------------------------------------------

/// `_clear_ai_hook(speak_exit)`.
pub fn clear_ai_hook(speak_exit: bool) {
    set_command_context(false);
    {
        let mut st = state();
        st.kb.input_help_mode = false;
        st.ai.pending_target_confirm = None;
    }
    if kb_layer() == Some(Layer::F10) {
        kb_close_layer();
    }
    let timer = {
        let mut st = state();
        st.ai.ai_mods = Mods::default();
        st.ai.ai_timer.take()
    };
    if let Some(t) = timer {
        t.cancel();
    }
    if speak_exit {
        say_x("Exit");
    }
}

/// `_SHIFTED_DIGITS`.
fn shifted_digit(name: &str) -> Option<&'static str> {
    Some(match name {
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        "%" => "5",
        "^" => "6",
        "&" => "7",
        "*" => "8",
        "(" => "9",
        ")" => "0",
        _ => return None,
    })
}

fn speak_speed_limit(v: f64) {
    let _kmh = (v * 3.6).round_ties_even() as i64;
    if units_mode() == "imperial" {
        let mph = (v * MPH_PER_MS).round_ties_even() as i64;
        say_x(&format!("Speed limit {mph} mph"));
    } else {
        say_x(&format!("Speed limit {_kmh} km/h"));
    }
    send_ai_command(&format!("SPEED:{v:.2}"));
}

/// `_on_ai_key_press(event)`: one F10-layer key on the worker.
pub fn on_ai_key_press(event: &KeyEvent) {
    if event.event_type != KeyEventType::Down {
        return;
    }
    let name = event.name.to_lowercase();

    match name.as_str() {
        "ctrl" | "control" | "left ctrl" | "right ctrl" => {
            state().ai.ai_mods.ctrl = true;
            return;
        }
        "shift" | "left shift" | "right shift" => {
            state().ai.ai_mods.shift = true;
            return;
        }
        "alt" | "left alt" | "right alt" => {
            state().ai.ai_mods.alt = true;
            return;
        }
        "f10" => return,
        _ => {}
    }

    // Normalise shifted digit characters before any checks.
    let shifted = shifted_digit(&name);
    let is_shifted_char = shifted.is_some();
    let base_name: &str = shifted.unwrap_or(&name);
    // Python's `in "1234567890"` is a substring test.
    let base_is_digit = "1234567890".contains(base_name);

    // A pending target-change confirmation is cancelled by any key other than the
    // confirming CTRL+digit, before normal processing continues.
    let pending = state().ai.pending_target_confirm;
    if let Some(p) = pending {
        let mods = state().ai.ai_mods;
        if !(mods.ctrl && base_name == slot_key_label(p)) {
            state().ai.pending_target_confirm = None;
            say_x("Target change cancelled");
        }
    }

    let mods = state().ai.ai_mods;

    // SHIFT+1-0: toggle vehicle slot selection (layer stays open).
    if base_is_digit && ((mods.shift && !mods.ctrl) || is_shifted_char) {
        let slot = if base_name == "0" {
            10
        } else {
            py_int_key(base_name)
        };
        toggle_slot_selection(slot);
        return;
    }

    // CTRL+1-0: set AI target slot (layer stays open).
    if base_is_digit && mods.ctrl && !mods.shift && !is_shifted_char {
        let slot = if base_name == "0" {
            10
        } else {
            py_int_key(base_name)
        };
        set_target_slot(slot);
        return;
    }

    // ALT+1-5: fire a preset AI configuration (layer closes after triggering).
    if "12345".contains(name.as_str()) && mods.alt && !mods.ctrl && !mods.shift {
        trigger_preset(py_int_key(&name));
        clear_ai_hook(false);
        return;
    }

    // Toggle input help mode with ? (shift+/)
    if name == "/" || name == "?" {
        let (on, timer) = {
            let mut st = state();
            st.kb.input_help_mode = !st.kb.input_help_mode;
            (st.kb.input_help_mode, st.ai.ai_timer.clone())
        };
        if on {
            say_x("Input help on");
            if let Some(t) = timer {
                t.cancel();
            }
        } else {
            say_x("Input help off");
            clear_ai_hook(false);
        }
        return;
    }

    // In help mode: speak what the key does, don't execute.
    if state().kb.input_help_mode {
        let key: (&str, bool, bool, bool) = match name.as_str() {
            "add" => ("+", false, false, false),
            "subtract" => ("-", false, false, false),
            n => (n, mods.ctrl, mods.shift, mods.alt),
        };
        let desc = F10_HELP
            .iter()
            .find(|(k, _)| k.0 == key.0 && k.1 == key.1 && k.2 == key.2 && k.3 == key.3)
            .map(|(_, v)| v.clone());
        match desc {
            Some(d) if !d.is_empty() => say_x(&d),
            _ => say_x("No command"),
        }
        return;
    }

    let step = 10.0 / 3.6;
    match name.as_str() {
        "h" => driving_assistant().toggle(),
        "d" => {
            say_x("AI disabled");
            dispatch_ai_mode("disabled");
        }
        "t" => {
            say_x("Traffic mode");
            dispatch_ai_mode("traffic");
        }
        "r" => {
            say_x("Random mode");
            dispatch_ai_mode("random");
        }
        "s" => {
            say_x("AI stop");
            dispatch_ai_mode("stop");
        }
        "c" => {
            say_x("Chase mode");
            dispatch_ai_mode("chase");
        }
        "f" => {
            say_x("Follow mode");
            dispatch_ai_mode("follow");
        }
        "e" => {
            say_x("Flee mode");
            dispatch_ai_mode("flee");
        }
        n if AGGRESSION_MAP.iter().any(|(k, _)| *k == n) => {
            let val = AGGRESSION_MAP
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| *v)
                .unwrap_or_default();
            let v = py_float_str(val);
            say_x(&format!("Aggression {v}"));
            send_ai_command(&format!("AGGR:{v}"));
        }
        "+" | "=" | "add" => {
            let v = {
                let mut st = state();
                let v = match st.ai.ai_speed_limit_ms {
                    None => step, // start at 10 km/h
                    Some(v) => v + step,
                };
                st.ai.ai_speed_limit_ms = Some(v);
                v
            };
            speak_speed_limit(v);
        }
        "-" | "subtract" => {
            let v = {
                let mut st = state();
                // `if v is None or v <= step` (NaN takes the subtract branch, as in Python).
                let v = match st.ai.ai_speed_limit_ms {
                    None => step,
                    Some(v) if v <= step => step,
                    Some(v) => v - step,
                };
                st.ai.ai_speed_limit_ms = Some(v);
                v
            };
            speak_speed_limit(v);
        }
        "0" => {
            state().ai.ai_speed_limit_ms = None;
            say_x("Speed limit off");
            send_ai_command("CLEARSPEED");
        }
        "a" if mods.ctrl => {
            select_all_slots();
            return; // layer stays open; no AI command issued
        }
        "n" if mods.ctrl => {
            slots().selected_slots.clear();
            say_x("Selection cleared");
            return;
        }
        "a" => {
            let mode = {
                let mut st = state();
                let idx = AVOID_CYCLE
                    .iter()
                    .position(|m| *m == st.ai.ai_avoid_mode)
                    .unwrap_or(0);
                st.ai.ai_avoid_mode = AVOID_CYCLE[(idx + 1) % AVOID_CYCLE.len()].to_string();
                st.ai.ai_avoid_mode.clone()
            };
            say_x(&format!("Avoid cars {mode}"));
            send_ai_command(&format!("AVOID:{mode}"));
        }
        "l" => {
            let on = {
                let mut st = state();
                st.ai.ai_lane_driving = !st.ai.ai_lane_driving;
                st.ai.ai_lane_driving
            };
            let word = if on { "on" } else { "off" };
            say_x(&format!("Lane driving {word}"));
            send_ai_command(&format!("LANE:{word}"));
        }
        "space" => {
            // Close the layer and start a scene description on its own thread.
            clear_ai_hook(false);
            crate::ai_describer::trigger_ai_describe(audio_controller_ref());
            return;
        }
        _ => {
            clear_ai_hook(false);
            return;
        }
    }

    clear_ai_hook(false);
}

/// `_start_ai_capture()`.
pub fn start_ai_capture() {
    clear_ai_hook(false);
    crate::keyboard::kb_open_layer(Layer::F10);
    // The F10 layer has no timeout; it stays open until a command is issued.
    state().ai.ai_timer = None;
}
