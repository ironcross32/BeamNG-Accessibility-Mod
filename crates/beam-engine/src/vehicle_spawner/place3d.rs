//! The placement editor: a live 3D editor for a queued vehicle's offset and rotation
//! relative to its anchor, or (opened from the manage page) the target of a teleport.
//!
//! All values are in the anchor's own frame. A tap moves exactly one unit; holding a key
//! repeats it, accelerating in TIME (never in magnitude) from `PLACE_REPEAT_START_SEC`
//! down to `PLACE_REPEAT_MIN_SEC`. Every step pings: horizontal motion is placed by HRTF,
//! height and pitch are pitch-shifted (the HRTF set is horizontal-plane only). After a
//! pause the position is spoken.

use std::sync::atomic::{AtomicBool, Ordering};

use beam_base::pyval::{PyDict, PyValue};

use super::{
    Fx, Held, Item, LaunchMode, PLACE_ACTIVE, PlaceMode, Screen, Vs, ctrl_pressed, monotonic_now,
    py_index, pyjson, shift_pressed, test_hooks, with_vs,
};
use crate::timer::STOP;

/// Ticker quantum: the scheduling granularity for hold-to-repeat, not the repeat rate.
pub const PLACE_TICK_SEC: f64 = 0.005;
/// How long a key must be held before the ticker starts repeating; below it a press is a
/// tap and moves exactly one unit.
pub const PLACE_HOLD_DELAY_SEC: f64 = 0.3;
/// A held key accelerates in time, never in magnitude: escalating the magnitude made the
/// value you land on a function of reaction time, so it could not be aimed.
pub const PLACE_REPEAT_START_SEC: f64 = 0.125;
pub const PLACE_REPEAT_MIN_SEC: f64 = 0.035;
/// Start -> floor, measured from the first repeat.
pub const PLACE_REPEAT_RAMP_SEC: f64 = 1.5;

/// Number-row 1-4 -> feet per translation step. Rotation ignores it.
pub const PLACE_UNIT_STEPS: [(&str, i64); 4] = [("1", 1), ("2", 10), ("3", 100), ("4", 1000)];

/// Degrees per rotation step, chosen by the modifier held at the moment of press.
pub const PLACE_ROT_STEP_DEG: i64 = 45; // bare
pub const PLACE_ROT_STEP_FINE_DEG: i64 = 1; // Ctrl
pub const PLACE_ROT_STEP_COARSE_DEG: i64 = 90; // Shift

/// Value boundaries that earn a ping, and the PLACEMENT_PING_RUNGS index each maps to.
/// Highest crossing wins.
pub const PLACE_CROSS_RUNGS: [(i64, i64); 4] = [(10000, 4), (1000, 3), (100, 2), (10, 1)];

/// A rotation step's own ping rung, since a wrapped angle has no crossings to read.
pub const PLACE_ROT_STEP_RUNGS: [(i64, i64); 3] = [
    (PLACE_ROT_STEP_FINE_DEG, 0),
    (PLACE_ROT_STEP_DEG, 1),
    (PLACE_ROT_STEP_COARSE_DEG, 2),
];

/// Silence after the last movement before the position is spoken automatically.
pub const PLACE_IDLE_READOUT_SEC: f64 = 1.2;

/// Pitch multipliers for cues HRTF cannot place (no elevation in the set).
pub const PLACE_PITCH_UP: f64 = 1.5;
pub const PLACE_PITCH_DOWN: f64 = 0.6;

// Axis indices; also the index of the field each accumulates into (PLACE_AXIS_FIELDS).
pub(crate) const AX_FWD: usize = 0;
pub(crate) const AX_RIGHT: usize = 1;
pub(crate) const AX_UP: usize = 2;
pub(crate) const AX_PITCH: usize = 3;
pub(crate) const AX_ROLL: usize = 4;
pub(crate) const AX_YAW: usize = 5;

/// Key name -> (axis, direction).
pub const PLACE_KEY_AXES: [(&str, usize, i64); 12] = [
    ("up", AX_FWD, 1),
    ("down", AX_FWD, -1),
    ("right", AX_RIGHT, 1),
    ("left", AX_RIGHT, -1),
    ("page up", AX_UP, 1),
    ("page down", AX_UP, -1),
    ("s", AX_PITCH, 1),
    ("w", AX_PITCH, -1),
    ("d", AX_ROLL, 1),
    ("a", AX_ROLL, -1),
    ("q", AX_YAW, 1),
    ("e", AX_YAW, -1),
];

/// `PLACE_TRANSLATE_AXES`.
pub const PLACE_TRANSLATE_AXES: [usize; 3] = [AX_FWD, AX_RIGHT, AX_UP];

/// Axis name -> item field name, by axis index.
pub const PLACE_AXIS_FIELDS: [(&str, &str); 6] = [
    ("fwd", "offFwdFt"),
    ("right", "offRightFt"),
    ("up", "offUpFt"),
    ("pitch", "rotPitchDeg"),
    ("roll", "rotRollDeg"),
    ("yaw", "rotYawDeg"),
];

/// Stored angle signs: +pitch = nose up, +roll = right side down, +yaw = nose swings
/// left. (field index, noun, positive word, negative word)
pub const PLACE_ROT_WORDS: [(usize, &str, &str, &str); 3] = [
    (AX_YAW, "yaw", "left", "right"),
    (AX_PITCH, "nose", "up", "down"),
    (AX_ROLL, "roll", "right", "left"),
];

/// The six placement numbers, in `PLACE_DEFAULTS` order: offFwdFt, offRightFt, offUpFt,
/// rotPitchDeg, rotRollDeg, rotYawDeg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceVals(pub [i64; 6]);

impl PlaceVals {
    pub fn to_vec(self) -> Vec<i64> {
        self.0.to_vec()
    }

    /// The six fields into a dict, in order.
    pub(crate) fn insert_into(self, d: &mut PyDict) {
        for (i, (_, field)) in PLACE_AXIS_FIELDS.iter().enumerate() {
            d.insert((*field).to_string(), PyValue::from(self.0[i]));
        }
    }
}

/// A newly queued item sits 25 ft in front of its anchor, so an item never opened in the
/// editor still spawns clear of it.
pub const PLACE_DEFAULTS: PlaceVals = PlaceVals([25, 0, 0, 0, 0, 0]);
const ZERO: PlaceVals = PlaceVals([0; 6]);

fn axis_of(name: &str) -> Option<(usize, i64)> {
    PLACE_KEY_AXES
        .iter()
        .find(|(k, _, _)| *k == name)
        .map(|(_, a, d)| (*a, *d))
}

fn is_translate(axis: usize) -> bool {
    PLACE_TRANSLATE_AXES.contains(&axis)
}

/// `_place_repeat_interval(held_sec)`: linear from START to MIN, measured from the moment
/// repeating begins, so the user can predict where a hold lands.
pub fn place_repeat_interval(held_sec: f64) -> f64 {
    let frac = (held_sec - PLACE_HOLD_DELAY_SEC) / PLACE_REPEAT_RAMP_SEC;
    let frac = beam_base::pyval::py_max(0.0, beam_base::pyval::py_min(1.0, frac));
    let span = PLACE_REPEAT_MIN_SEC - PLACE_REPEAT_START_SEC;
    PLACE_REPEAT_START_SEC + span * frac
}

/// `_place_rot_step()`: Ctrl wins over Shift, since fine placement cannot be reached any
/// other way.
fn place_rot_step() -> i64 {
    if ctrl_pressed() {
        return PLACE_ROT_STEP_FINE_DEG;
    }
    if shift_pressed() {
        return PLACE_ROT_STEP_COARSE_DEG;
    }
    PLACE_ROT_STEP_DEG
}

/// `_place_cross_rung(old, new)`: a heavier tone each time the value crosses a 10 / 100 /
/// 1000 / 10000 boundary, compared on the absolute value so -99 -> -100 crosses the way
/// 99 -> 100 does. Rung 0 when nothing was crossed, so every step stays audible.
pub fn place_cross_rung(old: i64, new: i64) -> i64 {
    let (a, b) = (old.abs(), new.abs());
    for (size, rung) in PLACE_CROSS_RUNGS {
        if a / size != b / size {
            return rung;
        }
    }
    0
}

fn rot_step_rung(step: i64) -> i64 {
    PLACE_ROT_STEP_RUNGS
        .iter()
        .find(|(s, _)| *s == step)
        .map(|(_, r)| *r)
        .unwrap_or(0)
}

/// `_feet(n, word)`: "12 feet right", "1 foot up".
fn feet(n: i64, word: &str) -> String {
    let a = n.abs();
    format!("{a} {} {word}", if a == 1 { "foot" } else { "feet" })
}

/// `_offset_phrase(vals)`: only the non-zero components.
pub(crate) fn offset_phrase(vals: &PlaceVals) -> String {
    let [fwd, right, up, ..] = vals.0;
    let mut parts = Vec::new();
    if fwd != 0 {
        parts.push(feet(fwd, if fwd > 0 { "forward" } else { "back" }));
    }
    if right != 0 {
        parts.push(feet(right, if right > 0 { "right" } else { "left" }));
    }
    if up != 0 {
        parts.push(feet(up, if up > 0 { "up" } else { "down" }));
    }
    if parts.is_empty() {
        "on the anchor".to_string()
    } else {
        parts.join(", ")
    }
}

/// `_rotation_phrase(vals)`.
pub(crate) fn rotation_phrase(vals: &PlaceVals) -> String {
    let mut parts = Vec::new();
    for (field, noun, pos, neg) in PLACE_ROT_WORDS {
        let v = vals.0[field];
        if v != 0 {
            parts.push(format!(
                "{noun} {} {} degrees",
                if v > 0 { pos } else { neg },
                v.abs()
            ));
        }
    }
    if parts.is_empty() {
        "level".to_string()
    } else {
        parts.join(", ")
    }
}

/// `_placement_phrase(vals)`.
pub(crate) fn placement_phrase(vals: &PlaceVals) -> String {
    format!("{}. {}", offset_phrase(vals), rotation_phrase(vals))
}

// ---- The ticker ----------------------------------------------------------------------------

static TICKER_RUNNING: AtomicBool = AtomicBool::new(false);

/// `_place_ticker_loop()`: drives hold-to-repeat and the idle readout; parked on
/// `PLACE_ACTIVE` whenever the editor is closed.
fn place_ticker_loop() {
    loop {
        PLACE_ACTIVE.wait(None);
        if STOP.is_set() {
            TICKER_RUNNING.store(false, Ordering::SeqCst);
            return;
        }
        crate::timer::sleep(PLACE_TICK_SEC);
        if !PLACE_ACTIVE.is_set() {
            continue;
        }
        let now = monotonic_now();
        with_vs(|vs| vs.place_tick(now));
    }
}

/// `_start_place_ticker()`.
fn start_place_ticker() {
    if test_hooks().no_ticker {
        return;
    }
    if TICKER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::timer::spawn_daemon("vehicle-spawner-place-ticker", place_ticker_loop);
}

impl Vs {
    /// `_place_ping_for(deltas, rungs)`: the one ping that best describes this tick's
    /// movement. Horizontal wins (HRTF can place it), then height, then rotation; the
    /// winning axis brings its own rung.
    fn place_ping_for(&mut self, deltas: &[Option<i64>; 6], rungs: &[Option<i64>; 6]) {
        let d = |a: usize| deltas[a].unwrap_or(0);
        let r = |a: usize| rungs[a].unwrap_or(0);
        let (d_fwd, d_right) = (d(AX_FWD), d(AX_RIGHT));
        if d_fwd != 0 || d_right != 0 {
            let rung = r(AX_FWD).max(r(AX_RIGHT));
            let az = (d_right as f64).atan2(d_fwd as f64).to_degrees();
            self.fx.push(Fx::Ping(rung, az, 1.0));
            return;
        }
        let d_up = d(AX_UP);
        if d_up != 0 {
            let up = if d_up > 0 {
                PLACE_PITCH_UP
            } else {
                PLACE_PITCH_DOWN
            };
            self.fx.push(Fx::Ping(r(AX_UP), 0.0, up));
            return;
        }
        let d_yaw = d(AX_YAW);
        if d_yaw != 0 {
            // Positive yaw swings the nose left.
            let az = if d_yaw > 0 { -90.0 } else { 90.0 };
            self.fx.push(Fx::Ping(r(AX_YAW), az, 1.0));
            return;
        }
        let d_roll = d(AX_ROLL);
        if d_roll != 0 {
            // Positive roll drops the right side.
            let az = if d_roll > 0 { 90.0 } else { -90.0 };
            self.fx.push(Fx::Ping(r(AX_ROLL), az, 1.0));
            return;
        }
        let d_pitch = d(AX_PITCH);
        if d_pitch != 0 {
            let up = if d_pitch > 0 {
                PLACE_PITCH_UP
            } else {
                PLACE_PITCH_DOWN
            };
            self.fx.push(Fx::Ping(r(AX_PITCH), 0.0, up));
        }
    }

    /// `_place_apply_steps(now, pressed)`: one step per held key, one ping for the
    /// result. Two arrows together move along the diagonal.
    fn place_apply_steps(&mut self, now: f64, pressed: &[(String, i64)]) {
        let mut deltas: [Option<i64>; 6] = [None; 6];
        let mut rot_rungs: [Option<i64>; 6] = [None; 6];
        for (name, rot_step) in pressed {
            let Some((axis, direction)) = axis_of(name) else {
                continue;
            };
            let step = if is_translate(axis) {
                self.place_unit_step
            } else {
                // A wrapped angle has no boundaries, so the rung is the step size.
                rot_rungs[axis] = Some(rot_step_rung(*rot_step));
                *rot_step
            };
            deltas[axis] = Some(deltas[axis].unwrap_or(0) + direction * step);
        }
        if deltas.iter().all(Option::is_none) {
            return;
        }
        let mut rungs = rot_rungs;
        for axis in 0..6 {
            let Some(d) = deltas[axis] else { continue };
            let old = self.place_values.0[axis];
            let mut v = old + d;
            if !is_translate(axis) {
                // Keep angles in -180..180 so the readout never winds up.
                v = (v + 180).rem_euclid(360) - 180;
            } else {
                // The rung is a fact about the value this step moved through.
                rungs[axis] = Some(place_cross_rung(old, v));
            }
            self.place_values.0[axis] = v;
        }
        self.place_last_step = now;
        self.place_readout_pending = true;
        self.place_ping_for(&deltas, &rungs);
    }

    /// One pass of the ticker loop's body.
    pub(crate) fn place_tick(&mut self, now: f64) {
        let mut pressed = Vec::new();
        for (k, rec) in self.place_held.iter_mut() {
            if now - rec.t < PLACE_HOLD_DELAY_SEC || now < rec.next {
                continue;
            }
            pressed.push((k.clone(), rec.rot_step));
            // Scheduled from NOW, so a stall never leaves a burst of catch-up steps.
            rec.next = now + place_repeat_interval(now - rec.t);
        }
        let idle_due = self.place_held.is_empty()
            && self.place_readout_pending
            && now - self.place_last_step >= PLACE_IDLE_READOUT_SEC;
        if idle_due {
            self.place_readout_pending = false;
        }
        if !pressed.is_empty() {
            self.place_apply_steps(now, &pressed);
        } else if idle_due {
            let t = self.speak_place3d_text(false);
            self.say(t);
        }
    }

    /// `_place_key_down(name)`: a tap applies one unit now; the ticker does the rest.
    /// Auto-repeat arrives as more key-downs with no key-up, so a key already held is
    /// ignored. The rotation modifier is polled once, here, and frozen for the hold.
    pub(crate) fn place_key_down(&mut self, name: &str) {
        let now = monotonic_now();
        let rot_step = place_rot_step();
        if self.place_held.contains_key(name) {
            return;
        }
        self.place_held.insert(
            name.to_string(),
            Held {
                t: now,
                next: now + PLACE_HOLD_DELAY_SEC + PLACE_REPEAT_START_SEC,
                rot_step,
            },
        );
        self.place_apply_steps(now, &[(name.to_string(), rot_step)]);
    }

    /// `_place_key_up(name)`.
    pub(crate) fn place_key_up(&mut self, name: &str) {
        self.place_held.shift_remove(name);
    }

    /// `_speak_place3d_text(with_mode)`. Only readouts asked for by name carry the mode.
    pub(crate) fn speak_place3d_text(&self, with_mode: bool) -> String {
        let mut text = placement_phrase(&self.place_values);
        if with_mode
            && self.place_mode == PlaceMode::Teleport
            && self.place_launch_mode == LaunchMode::Force
        {
            text.push_str(". Force mode");
        }
        text
    }

    /// `_enter_place3d()`: open the editor on the wizard's target.
    pub(crate) fn enter_place3d(&mut self) {
        let vals = if self.place_mode == PlaceMode::Teleport {
            // Dead on the anchor: a teleport aims an existing vehicle at a spot the user
            // picked, so any other start is an offset they did not ask for.
            self.tp_veh_id.map(|_| ZERO)
        } else {
            self.wizard_target_idx
                .and_then(|i| self.to_spawn.get(i))
                .map(|it| it.vals)
        };
        let Some(vals) = vals else {
            let back = self.wizard_return_screen();
            self.clear_wizard();
            self.say("Placement cancelled.");
            self.enter_screen(back, true, None);
            return;
        };
        self.place_values = vals;
        self.place_snapshot = vals;
        self.place_held.clear();
        self.place_last_step = 0.0;
        self.place_readout_pending = false;
        // Back to one foot every time, so the step a key is about to take is knowable.
        self.place_unit_step = 1;
        start_place_ticker();
        PLACE_ACTIVE.set();
        self.enter_screen(Screen::Place3d, true, Some("Position".to_string()));
    }

    /// `_leave_place3d()`.
    pub(crate) fn leave_place3d(&mut self) {
        PLACE_ACTIVE.clear();
        self.place_held.clear();
        // Disarm the idle readout so a tick slipping through cannot speak a position the
        // user has left.
        self.place_readout_pending = false;
    }

    /// `_wizard_return_screen()`.
    pub(crate) fn wizard_return_screen(&self) -> Screen {
        if self.place_mode == PlaceMode::Teleport {
            Screen::Manage
        } else {
            Screen::ToSpawn
        }
    }

    /// `_clear_wizard()`.
    pub(crate) fn clear_wizard(&mut self) {
        self.wizard_target_idx = None;
        self.wizard_ref_mode = None;
        self.wizard_ref_veh_id = None;
        self.wizard_ref_name = None;
        self.mark_target_idx = None;
        self.place_mode = PlaceMode::Spawn;
        self.tp_veh_id = None;
        self.tp_veh_name = None;
    }

    /// `_commit_teleport(vals)`.
    fn commit_teleport(&mut self, vals: PlaceVals) {
        let veh_id = self.tp_veh_id;
        let id_text = veh_id.map_or("None".to_string(), |v| v.to_string());
        let name = match &self.tp_veh_name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => format!("vehicle {id_text}"),
        };
        let ref_mode = match &self.wizard_ref_mode {
            Some(m) if !m.is_empty() => m.clone(),
            _ => "vehicle".to_string(),
        };
        let anchor = if ref_mode == "camera" {
            "the ground below the camera".to_string()
        } else if self.wizard_ref_veh_id.is_some() && self.wizard_ref_veh_id == veh_id {
            "its own position".to_string()
        } else if let Some(rid) = self.wizard_ref_veh_id {
            match &self.wizard_ref_name {
                Some(n) if !n.is_empty() => n.clone(),
                _ => format!("vehicle {rid}"),
            }
        } else {
            "current vehicle".to_string()
        };
        let mut payload = PyDict::new();
        payload.insert("vehId".into(), opt_int(veh_id));
        payload.insert("refMode".into(), PyValue::from(ref_mode));
        payload.insert("refVehId".into(), opt_int(self.wizard_ref_veh_id));
        payload.insert(
            "mode".into(),
            PyValue::from(self.place_launch_mode.as_str()),
        );
        vals.insert_into(&mut payload);
        self.send(format!(
            "TELEPORT_PLACE:{}",
            pyjson::dumps_compact(&PyValue::Dict(payload))
        ));
        if self.place_launch_mode == LaunchMode::Force {
            // No rotation clause: a launched vehicle lands however physics leaves it.
            self.say(format!(
                "Launching {name} {} from {anchor}.",
                offset_phrase(&vals)
            ));
        } else {
            self.say(format!(
                "Moving {name} {} from {anchor}, {}.",
                offset_phrase(&vals),
                rotation_phrase(&vals)
            ));
        }
    }

    /// `_commit_place3d()`.
    pub(crate) fn commit_place3d(&mut self) {
        self.leave_place3d();
        let vals = self.place_values;
        if self.place_mode == PlaceMode::Teleport {
            self.commit_teleport(vals);
            self.clear_wizard();
            self.enter_screen(Screen::Manage, false, None);
            return;
        }
        let ref_mode = match &self.wizard_ref_mode {
            Some(m) if !m.is_empty() => m.clone(),
            _ => "auto".to_string(),
        };
        let ref_veh_id = self.wizard_ref_veh_id;
        let ref_text = match self
            .wizard_target_idx
            .and_then(|i| self.to_spawn.get_mut(i))
        {
            Some(item) => {
                item.vals = vals;
                item.ref_mode = ref_mode;
                item.ref_veh_id = ref_veh_id;
                ref_phrase(item)
            }
            None => "anchor".to_string(),
        };
        self.say(format!(
            "Placement set: {} from {ref_text}, {}.",
            offset_phrase(&vals),
            rotation_phrase(&vals)
        ));
        self.clear_wizard();
        self.enter_screen(Screen::ToSpawn, true, None);
    }

    /// `_cancel_place3d()`.
    pub(crate) fn cancel_place3d(&mut self) {
        self.leave_place3d();
        self.place_values = self.place_snapshot;
        let back = self.wizard_return_screen();
        self.clear_wizard();
        self.say("Placement cancelled.");
        self.enter_screen(back, true, None);
    }

    /// `_reset_place3d()`.
    pub(crate) fn reset_place3d(&mut self) {
        self.place_values = ZERO;
        self.place_last_step = monotonic_now();
        self.place_readout_pending = false;
        self.say("Reset. On the anchor, level.");
    }

    /// `_on_w(event)`.
    pub(crate) fn on_w(&mut self) {
        if self.screen == Screen::Place3d {
            self.place_key_down("w");
            return;
        }
        if self.screen == Screen::Manage {
            let mv = self.manage_vehicles.clone();
            if mv.is_empty() {
                self.say("No vehicles in the world.");
                return;
            }
            let idx = self.idx_manage.min(mv.len() - 1);
            self.place_mode = PlaceMode::Teleport;
            let (vid, name) = mv[idx].clone();
            self.tp_veh_id = Some(vid);
            self.tp_veh_name = Some(name.clone());
            self.wizard_target_idx = None;
            self.clear_wizard_ref();
            // Not "Anchor vehicle": the ground is one of the choices here.
            self.enter_screen(Screen::Ref, true, Some(format!("Move {name}. Anchor")));
            return;
        }
        if self.screen != Screen::ToSpawn {
            return;
        }
        if self.to_spawn.is_empty() {
            self.say("Queue is empty.");
            return;
        }
        let item = py_index(&self.to_spawn, self.idx_to_spawn);
        if item.replace_veh_id.is_some() {
            self.say("Placement is not needed for replacement vehicles.");
            return;
        }
        self.wizard_target_idx = Some(self.idx_to_spawn);
        self.clear_wizard_ref();
        self.enter_screen(Screen::Ref, true, Some("Anchor vehicle".to_string()));
    }

    /// `_clear_wizard_ref()`. Spawn and teleport show different anchor lists, so a cursor
    /// left over from the other would land on an unrelated option.
    fn clear_wizard_ref(&mut self) {
        self.idx_ref = 0;
        self.wizard_ref_mode = None;
        self.wizard_ref_veh_id = None;
        self.wizard_ref_name = None;
    }

    /// `_set_wizard_ref(ref_mode, ref_veh_id, ref_name)`.
    pub(crate) fn set_wizard_ref(
        &mut self,
        ref_mode: &str,
        ref_veh_id: Option<i64>,
        ref_name: Option<String>,
    ) {
        self.wizard_ref_mode = Some(ref_mode.to_string());
        self.wizard_ref_veh_id = ref_veh_id;
        self.wizard_ref_name = ref_name;
        self.enter_place3d();
    }

    /// `_on_place_digit(name)`: 1-4 pick the translation step.
    pub(crate) fn on_place_digit(&mut self, name: &str) {
        if self.screen != Screen::Place3d {
            return;
        }
        let Some((_, step)) = PLACE_UNIT_STEPS.iter().find(|(k, _)| *k == name) else {
            return;
        };
        self.place_unit_step = *step;
        self.say(if *step == 1 {
            format!("{step} foot")
        } else {
            format!("{step} feet")
        });
    }

    /// `_on_place_letter(name)`: the rotation letters with no other modal binding.
    pub(crate) fn on_place_letter(&mut self, name: &str) {
        if self.screen != Screen::Place3d {
            return;
        }
        self.place_key_down(name);
    }

    /// `_on_x(event)`.
    pub(crate) fn on_x(&mut self) {
        if self.screen == Screen::Place3d {
            if self.place_mode != PlaceMode::Teleport {
                // A queued vehicle doesn't exist yet, so there's nothing to throw.
                self.say("Force mode only applies to teleports.");
                return;
            }
            if self.place_launch_mode == LaunchMode::Force {
                self.place_launch_mode = LaunchMode::Standard;
                self.say("Standard mode. The vehicle will be teleported.");
            } else {
                self.place_launch_mode = LaunchMode::Force;
                self.say("Force mode. The vehicle will be launched.");
            }
            return;
        }
        if self.screen != Screen::ToSpawn {
            return;
        }
        if self.to_spawn.is_empty() {
            return;
        }
        let idx = self.idx_to_spawn;
        let item = py_index(&self.to_spawn, idx);
        if item.replace_veh_id.is_some() {
            let item = &mut self.to_spawn[idx];
            item.replace_slot = None;
            item.replace_veh_id = None;
            item.replace_veh_name = None;
            self.say("Changed to add new vehicle.");
            self.speak_current();
            return;
        }
        if self.slots.is_empty() {
            self.say("No slot data available yet.");
            return;
        }
        self.replace_editing_idx = Some(idx);
        self.idx_replace_slot = 0;
        self.enter_screen(
            Screen::ReplaceSlot,
            true,
            Some("Choose vehicle to replace".to_string()),
        );
    }
}

fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::from)
}

/// `_ref_phrase(item)`.
pub(crate) fn ref_phrase(item: &Item) -> String {
    match item.ref_mode.as_str() {
        "auto" | "prev" => "previous in queue".to_string(),
        "next" => "next in queue".to_string(),
        "vehicle" if item.ref_veh_id.is_some() => {
            format!("marked vehicle {}", item.ref_veh_id.unwrap_or_default())
        }
        _ => "current vehicle".to_string(),
    }
}
