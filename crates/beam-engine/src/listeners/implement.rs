//! Owner: E3. Implement proximity / docking / ramp / cannon listener and its sender
//! (`beamtel.py:2700-3154`).
//!
//! Only ever announces the NEAREST object: the extension may report a different one from tick
//! to tick in a crowded yard, and narrating all of them would be unusable.

use std::sync::Arc;

use beam_audio::controller::AudioController;
use beam_base::pyfmt::{py_float_parse, py_strip};

use crate::listeners::dock_phrases::{
    IMPL_ANNOUNCE_PROBE_S, IMPL_DOCK_ENTRY_EXIT_M, IMPL_DOCK_ENTRY_MIN_M, IMPL_PROX_ENTER_M,
    IMPL_PROX_INSIDE_HOLD, IMPL_PROX_LEAVE_HOLD, IMPL_PROX_LEAVE_M, IMPL_PROX_RELATION_HOLD,
    impl_relation_phrase, implement_word, parse_ramp_self, ramp_acquire, ramp_align_phrase,
    ramp_bearing_deg, slam_cue, wire_int,
};
use crate::net::{Decode, IMPLEMENT_LISTEN_PORT, Recv, UdpListener, decode};
use crate::state::{CannonAim, DockReading, state};
use crate::timer::{Event, monotonic, time_time};
use crate::units::fmt_distance;

/// Everything the listener reaches outside the shared state: speech (always
/// `exclude_from_buffer=True`), the audio controller and the command sender.
/// [`LiveImplementSink`] is the real one.
pub trait ImplementSink {
    fn say(&self, text: &str);
    fn send_implement_cmd(&self, command: &str);
    fn clear_dock_target(&self);
    fn clear_cannon_aim(&self);
    fn update_cannon_aim(
        &self,
        elevation: f64,
        bearing: f64,
        target_angle: f64,
        solution_available: bool,
        reachable: bool,
    );
    fn trigger_slam_cue(&self, cue: &str);
    fn update_dock_target(
        &self,
        range_m: f64,
        lateral_m: f64,
        null_value: f64,
        mode: &str,
        acquire: bool,
        bearing_deg: f64,
    );
    fn trigger_dock_approach_cue(&self);
    fn trigger_entry_cue(&self);
}

/// The real collaborators.
pub struct LiveImplementSink {
    pub audio_controller: Arc<AudioController>,
}

impl ImplementSink for LiveImplementSink {
    fn say(&self, text: &str) {
        crate::speech_out::say_excluded(text);
    }
    fn send_implement_cmd(&self, command: &str) {
        send_implement_cmd(command);
    }
    fn clear_dock_target(&self) {
        self.audio_controller.clear_dock_target();
    }
    fn clear_cannon_aim(&self) {
        self.audio_controller.clear_cannon_aim();
    }
    fn update_cannon_aim(&self, e: f64, b: f64, t: f64, s: bool, r: bool) {
        self.audio_controller.update_cannon_aim(e, b, t, s, r);
    }
    fn trigger_slam_cue(&self, cue: &str) {
        self.audio_controller.trigger_slam_cue(cue);
    }
    fn update_dock_target(&self, r: f64, l: f64, n: f64, m: &str, a: bool, b: f64) {
        self.audio_controller.update_dock_target(r, l, n, m, a, b);
    }
    fn trigger_dock_approach_cue(&self) {
        self.audio_controller.trigger_dock_approach_cue();
    }
    fn trigger_entry_cue(&self) {
        self.audio_controller.trigger_entry_cue();
    }
}

/// `implement_listener`'s locals.
#[derive(Debug, Clone)]
pub struct ImplementListener {
    implement_seen: bool,
    next_probe: f64,
    impl_word: String,
    /// Name of the object currently being tracked.
    tracked: Option<String>,
    tracked_relation: Option<String>,
    tracked_inside: bool,
    // Pending transitions, each (value, first_seen_ts), so a flap has to persist.
    pending_relation: Option<(String, f64)>,
    pending_inside: Option<(bool, f64)>,
    pending_leave: Option<f64>,
    /// Which side of the entry gate the last DOCK: line was on; None is "not known yet",
    /// which keeps the earcon off on first acquisition.
    entry_ok: Option<bool>,
    /// Which side of the ramp approach corridor. Starts hunting.
    ramp_acquire: bool,
}

/// `rsplit(",", maxsplit)` in Python's left-to-right order.
fn rsplit(text: &str, maxsplit: usize) -> Vec<&str> {
    let mut v: Vec<&str> = text.rsplitn(maxsplit + 1, ',').collect();
    v.reverse();
    v
}

/// The DOCK: fields after the split, or None where Python's `except ValueError` skips it.
fn parse_dock_fields(fields: &[&str], impl_word: &str) -> Option<DockReading> {
    let long_tail = fields.len() >= 13;
    let f = |i: usize| py_float_parse(fields[i]);
    Some(DockReading {
        name: py_strip(fields[0]).to_string(),
        range: f(1)?,
        lateral: f(2)?,
        vertical: f(3)?,
        idx: wire_int(fields[4])?,
        count: wire_int(fields[5])?,
        kind: py_strip(fields[6]).to_uppercase(),
        lo_z: f(7)?,
        hi_z: f(8)?,
        yaw: f(9)?,
        manual: py_strip(fields[10]) == "1",
        impl_word: impl_word.to_string(),
        entry_theta: if long_tail { Some(f(11)?) } else { None },
        entry_depth: if long_tail { Some(f(12)?) } else { None },
        // An older mod has no other mode to be in, so IMPL is not a guess. The margin stays
        // None rather than 0, which would read as exactly touching both walls.
        mode: if fields.len() == 15 {
            py_strip(fields[13]).to_uppercase()
        } else {
            "IMPL".to_string()
        },
        margin: if fields.len() == 15 {
            Some(f(14)?)
        } else {
            None
        },
        acquire: false,
    })
}

/// `parse_dock` as the sim names it: the DOCK: line's three wire shapes, longest first.
pub fn parse_dock_line(text: &str, impl_word: &str) -> Option<DockReading> {
    let body = text.get(5..)?;
    let mut fields = rsplit(body, 14);
    if fields.len() != 15 {
        fields = rsplit(body, 12);
        // elif, not a second if: a 15-field split is also != 13 and would be re-split to 11.
        if fields.len() != 13 {
            fields = rsplit(body, 10);
        }
    }
    if !matches!(fields.len(), 11 | 13 | 15) {
        return None;
    }
    let mut parsed = parse_dock_fields(&fields, impl_word)?;
    if parsed.mode != "IMPL" && parsed.mode != "RAMP" {
        parsed.mode = "IMPL".to_string();
    }
    Some(parsed)
}

impl ImplementListener {
    /// The listener's prologue, at wall time `now`: ask the mod to re-announce (the "which
    /// implement" latch lives in Lua, where restarting beamtel cannot clear it), and keep
    /// asking until it answers.
    pub fn start(sink: &dyn ImplementSink, now: f64) -> ImplementListener {
        sink.send_implement_cmd("ON");
        ImplementListener {
            implement_seen: false,
            next_probe: now + IMPL_ANNOUNCE_PROBE_S,
            impl_word: "Bucket".to_string(),
            tracked: None,
            tracked_relation: None,
            tracked_inside: false,
            pending_relation: None,
            pending_inside: None,
            pending_leave: None,
            entry_ok: None,
            ramp_acquire: true,
        }
    }

    /// A receive timeout, at wall time `now`: re-probe until the first IMPLEMENT: line.
    pub fn on_timeout(&mut self, sink: &dyn ImplementSink, now: f64) {
        if !self.implement_seen && now >= self.next_probe {
            sink.send_implement_cmd("ON");
            self.next_probe = now + IMPL_ANNOUNCE_PROBE_S;
        }
    }

    fn reset_tracking(&mut self) {
        self.tracked = None;
        self.tracked_relation = None;
        self.tracked_inside = false;
        self.pending_relation = None;
        self.pending_inside = None;
        self.pending_leave = None;
    }

    /// One datagram's text (decoded and stripped). `now` is `time.time()`, `mono` is
    /// `time.monotonic()`.
    pub fn handle_text(&mut self, sink: &dyn ImplementSink, text: &str, now: f64, mono: f64) {
        if text.is_empty() {
            return;
        }

        if text.starts_with("IMPLEMENT:") {
            self.implement_seen = true;
            let part = text.split_once(':').map(|(_, p)| p).unwrap_or("");
            self.impl_word = implement_word(part).to_string();
            let current = if py_strip(part).to_uppercase() == "NONE" {
                String::new()
            } else {
                self.impl_word.clone()
            };
            // A part swap invalidates whatever we were tracking, docking band included.
            self.reset_tracking();
            self.entry_ok = None;
            self.ramp_acquire = true;
            {
                let mut st = state();
                st.dock.implement_word_current = current;
                st.dock.last_dock = None;
                st.dock.last_dock_fail = None;
                st.dock.last_dock_mode = None;
                st.dock.last_dock_name = None;
            }
            sink.clear_dock_target();
            return;
        }

        // Docking lines are a separate instrument and never feed the proximity chain.
        if text == "DOCKCLEAR" {
            // Re-acquiring is not tilting into range; the corridor goes back to hunting.
            self.entry_ok = None;
            self.ramp_acquire = true;
            {
                let mut st = state();
                st.dock.last_dock = None;
                st.dock.last_dock_fail = None;
            }
            sink.clear_dock_target();
            return;
        }

        if let Some(rest) = text.strip_prefix("DOCKFAIL:") {
            // Not spoken unprompted; held for the F9+I readout.
            self.entry_ok = None;
            self.ramp_acquire = true;
            {
                let mut st = state();
                st.dock.last_dock = None;
                st.dock.last_dock_fail = Some(py_strip(rest).to_string());
            }
            sink.clear_dock_target();
            return;
        }

        if let Some(rest) = text.strip_prefix("RAMPALIGN:") {
            let phrase = ramp_align_phrase(rest);
            if !phrase.is_empty() {
                sink.say(&phrase);
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("CANNON:") {
            // "1" is the pre-Old-Cannon spelling for the Large Cannon.
            let payload = py_strip(rest).to_uppercase();
            let kind = if payload == "OLD" {
                "OLD"
            } else if payload == "1" || payload == "LARGE" {
                "LARGE"
            } else {
                "NONE"
            };
            {
                let mut st = state();
                st.dock.cannon_kind = kind.to_string();
                st.dock.cannon_active = kind != "NONE";
                if kind != "OLD" {
                    st.dock.last_cannon_aim = None;
                }
            }
            if kind != "OLD" {
                sink.clear_cannon_aim();
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("RAMPSELF:") {
            // A latch, sent on change only; NONE clears it.
            state().dock.last_ramp_self = parse_ramp_self(rest);
            return;
        }

        if let Some(rest) = text.strip_prefix("CANNONAIM:") {
            let fields: Vec<&str> = rest.split(',').collect();
            if fields.len() != 7 {
                return;
            }
            let f = |i: usize| py_float_parse(fields[i]);
            let (Some(elevation), Some(bearing), Some(target_angle), Some(line_angle)) =
                (f(0), f(1), f(2), f(3))
            else {
                return;
            };
            let (Some(range), Some(speed)) = (f(4), f(5)) else {
                return;
            };
            let aim = CannonAim {
                elevation,
                bearing,
                target_angle,
                line_angle,
                range,
                speed,
                reachable: py_strip(fields[6]) == "1",
                solution_available: speed > 0.0,
                stamp: mono,
            };
            state().dock.last_cannon_aim = Some(aim.clone());
            if aim.range < 0.0 {
                sink.clear_cannon_aim();
            } else {
                sink.update_cannon_aim(
                    aim.elevation,
                    aim.bearing,
                    aim.target_angle,
                    aim.solution_available,
                    aim.reachable,
                );
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("SLAM:") {
            // Sent only on a state change; the hysteresis lives in the mod.
            let slam = py_strip(rest).to_uppercase();
            if let Some(cue) = slam_cue(&slam) {
                sink.trigger_slam_cue(cue);
                let announce = state().cfg.announce_implement_proximity;
                if announce && slam == "COMMITTED" {
                    // Only the committed state is worth a word.
                    sink.say("Over it, clear");
                }
            }
            return;
        }

        if text.starts_with("DOCK:") {
            self.handle_dock(sink, text);
            return;
        }

        if text == "CLEAR" {
            if let Some(tracked) = self.tracked.clone() {
                match self.pending_leave {
                    None => self.pending_leave = Some(now),
                    Some(t) if now - t >= IMPL_PROX_LEAVE_HOLD => {
                        sink.say(&format!("Clear of {tracked}"));
                        self.reset_tracking();
                    }
                    Some(_) => {}
                }
            }
            return;
        }

        let Some(body) = text.strip_prefix("NEAR:") else {
            return;
        };
        let parts = rsplit(body, 4);
        if parts.len() != 5 {
            return;
        }
        let name = py_strip(parts[0]).to_string();
        let Some(dist) = py_float_parse(parts[1]) else {
            return;
        };
        let relation = py_strip(parts[2]).to_uppercase();
        let inside = py_strip(parts[3]) == "1";

        if !state().cfg.announce_implement_proximity {
            return;
        }

        if self.tracked.is_none() || self.tracked.as_deref() != Some(name.as_str()) {
            // Not tracking, or a nearer object took over: announce the new one, with no leave
            // line for the old one, or every pass through a yard becomes a monologue.
            if dist < IMPL_PROX_ENTER_M {
                let (val, unit) = fmt_distance(dist);
                let phrase = impl_relation_phrase(&relation).unwrap_or("");
                sink.say(&format!(
                    "{} approaching {name}, {phrase}, {val} {unit}",
                    self.impl_word
                ));
                self.tracked = Some(name);
                self.tracked_relation = Some(relation);
                self.tracked_inside = inside;
                self.pending_relation = None;
                self.pending_inside = None;
                self.pending_leave = None;
            }
            return;
        }
        let tracked = name;

        // Still on the same object.
        if dist > IMPL_PROX_LEAVE_M {
            match self.pending_leave {
                None => self.pending_leave = Some(now),
                Some(t) if now - t >= IMPL_PROX_LEAVE_HOLD => {
                    sink.say(&format!("Clear of {tracked}"));
                    self.reset_tracking();
                }
                Some(_) => {}
            }
            return;
        }
        self.pending_leave = None;

        if self.tracked_relation.as_deref() != Some(relation.as_str()) {
            match &self.pending_relation {
                Some((r, t)) if *r == relation => {
                    if now - t >= IMPL_PROX_RELATION_HOLD {
                        self.pending_relation = None;
                        sink.say(&format!(
                            "{tracked}, now {}",
                            impl_relation_phrase(&relation).unwrap_or(&relation)
                        ));
                        self.tracked_relation = Some(relation);
                    }
                }
                _ => self.pending_relation = Some((relation, now)),
            }
        } else {
            self.pending_relation = None;
        }

        if inside != self.tracked_inside {
            match self.pending_inside {
                Some((i, t)) if i == inside => {
                    if now - t >= IMPL_PROX_INSIDE_HOLD {
                        self.tracked_inside = inside;
                        self.pending_inside = None;
                        if inside {
                            sink.say(&format!("{} under {tracked}", self.impl_word));
                        } else {
                            sink.say(&format!("{} clear of {tracked}", self.impl_word));
                        }
                    }
                }
                _ => self.pending_inside = Some((inside, now)),
            }
        } else {
            self.pending_inside = None;
        }
    }

    fn handle_dock(&mut self, sink: &dyn ImplementSink, text: &str) {
        let Some(mut parsed) = parse_dock_line(text, &self.impl_word) else {
            return;
        };
        // Which question the ramp instrument answers this tick, latched once and handed to both
        // the tones and the F9+I phrase. Never set in implement mode.
        if parsed.mode == "RAMP" {
            let was_acquiring = self.ramp_acquire;
            self.ramp_acquire = ramp_acquire(parsed.range, parsed.lateral, self.ramp_acquire);
            if was_acquiring && !self.ramp_acquire {
                sink.trigger_dock_approach_cue();
            }
            parsed.acquire = self.ramp_acquire;
        } else {
            // Keep the next ramp acquisition armed.
            self.ramp_acquire = true;
            parsed.acquire = false;
        }
        {
            let mut st = state();
            st.dock.last_dock = Some(parsed.clone());
            st.dock.last_dock_fail = None;
        }
        // The yaw field is the ramp null; implement mode keeps vertical metres.
        let ramp = parsed.mode == "RAMP";
        sink.update_dock_target(
            parsed.range,
            parsed.lateral,
            if ramp { parsed.yaw } else { parsed.vertical },
            &parsed.mode,
            parsed.acquire,
            if parsed.acquire {
                ramp_bearing_deg(parsed.range, parsed.lateral, parsed.yaw)
            } else {
                0.0
            },
        );
        // Announced once on change: the same two tones mean different things in the two modes.
        // In ramp mode the announcement also carries the machine's NAME, and re-fires when it
        // changes; this is the only place the target is named.
        let announce = {
            let mut st = state();
            let ramp_named =
                ramp && st.dock.last_dock_name.as_deref() != Some(parsed.name.as_str());
            let changed = st.dock.last_dock_mode.as_deref() != Some(parsed.mode.as_str());
            let announce = (changed || ramp_named) && st.dock.dock_mode_active;
            if changed {
                st.dock.last_dock_mode = Some(parsed.mode.clone());
            }
            st.dock.last_dock_name = Some(parsed.name.clone());
            announce
        };
        if announce {
            if ramp {
                sink.say(&format!("Ramp alignment, {}", parsed.name));
            } else {
                sink.say("Implement alignment");
            }
        }
        // Entry-gate earcon, on the transition INTO enterable only, with the mod's own two
        // thresholds so a machine breathing on its suspension cannot machine-gun it.
        let depth = if parsed.mode == "IMPL" {
            parsed.entry_depth
        } else {
            None
        };
        match depth {
            Some(depth) if depth >= 0.0 || depth.is_nan() => {
                let thresh = if self.entry_ok == Some(true) {
                    IMPL_DOCK_ENTRY_EXIT_M
                } else {
                    IMPL_DOCK_ENTRY_MIN_M
                };
                let now_ok = depth >= thresh;
                if now_ok && self.entry_ok == Some(false) {
                    sink.trigger_entry_cue();
                }
                self.entry_ok = Some(now_ok);
            }
            _ => self.entry_ok = None,
        }
    }
}

/// `implement_listener(audio_controller, stop_event)`. Runs until `stop_event` is set.
pub fn implement_listener(audio_controller: Arc<AudioController>, stop_event: &Event) {
    let sink = LiveImplementSink { audio_controller };
    match UdpListener::bind(IMPLEMENT_LISTEN_PORT, 0.2, 2048) {
        Ok(mut sock) => {
            log::info!("Implement proximity listener started on port {IMPLEMENT_LISTEN_PORT}");
            let mut l = ImplementListener::start(&sink, time_time());
            while !stop_event.is_set() {
                match sock.recv() {
                    Recv::Timeout => l.on_timeout(&sink, time_time()),
                    Recv::Error(_) => {
                        if stop_event.is_set() {
                            break;
                        }
                        log::error!("Implement proximity listener socket error.");
                    }
                    Recv::Data(data, _) => {
                        let text = decode(data, Decode::Utf8Ignore).unwrap_or_default();
                        let text = py_strip(&text);
                        if text.is_empty() {
                            continue;
                        }
                        l.handle_text(&sink, text, time_time(), monotonic());
                    }
                }
            }
        }
        Err(e) => log::error!("Implement proximity listener failed to bind: {e}"),
    }
    log::info!("Implement proximity listener stopped.");
}

/// `_send_implement_cmd(command)` (port IMPLEMENT_CMD_PORT).
pub fn send_implement_cmd(command: &str) {
    crate::net::send_cmd(
        crate::net::IMPLEMENT_CMD_PORT,
        command,
        "implement proximity command",
    );
}
