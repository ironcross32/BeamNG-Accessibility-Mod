//! Speech for the game's free-roam drift display (driftAccessible.lua events).
//!
//! The Lua side relays only what the display presents, while it presents it. This
//! turns those events into short phrases and owns the quiet window: while the display
//! shows, the engine holds back automatic speed and compass speech.
//!
//! Events are the dicts `json.loads` produced on the UI bridge, as [`PyValue`]s.

use std::sync::OnceLock;
use std::time::Instant;

use beam_base::pyfmt::{py_f, py_is_space, py_strip};
use parking_lot::Mutex;

use crate::pyops::{PyErr, PyValue, decimal_value, group_thousands};

/// If "hidden" never arrives (bridge reconnect, game reload), stop muting speed and
/// compass this long after the last drift event.
pub const QUIET_TIMEOUT_S: f64 = 10.0;

/// `f"{int(value):,}"`, `None` where `int()` raises `TypeError`/`ValueError`. Any other
/// exception (`OverflowError` for an infinite float) propagates, as in Python.
fn points(value: Option<&PyValue>) -> Result<Option<String>, PyErr> {
    match value.unwrap_or(&PyValue::None).py_int() {
        Ok(n) => Ok(Some(group_thousands(&n))),
        Err(e) if e.is_type_or_value() => Ok(None),
        Err(e) => Err(e),
    }
}

/// The display's own text, with "+80" read as "plus 80" (`\+\s*(\d+)`, where `\d` is
/// any Unicode decimal digit and `\s` any Unicode space, as in Python's `re`).
fn clean(text: Option<&PyValue>) -> String {
    let Some(PyValue::Str(text)) = text else {
        return String::new();
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '+' {
            let mut j = i + 1;
            while j < chars.len() && py_is_space(chars[j]) {
                j += 1;
            }
            let digits_start = j;
            let mut digits = String::new();
            while j < chars.len() {
                match decimal_value(chars[j]) {
                    Some(d) => digits.push(char::from(b'0' + d as u8)),
                    None => break,
                }
                j += 1;
            }
            if j > digits_start {
                let trimmed = digits.trim_start_matches('0');
                let n = if trimmed.is_empty() { "0" } else { trimmed };
                out.push_str("plus ");
                out.push_str(&group_thousands(n));
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    py_strip(&out).to_string()
}

fn ok(phrase: impl Into<String>, interrupt: bool) -> Result<Option<(String, bool)>, PyErr> {
    Ok(Some((phrase.into(), interrupt)))
}

/// `(phrase, interrupt)` for one event, or `None` when there is nothing to say.
///
/// `Err` where Python raised out of `drift_phrase` (an infinite number where an int was
/// expected, an int too large for a float as the combo); the bridge logged it as a
/// callback error and nothing was spoken.
pub fn drift_phrase(event: &PyValue) -> Result<Option<(String, bool)>, PyErr> {
    let Some(ev) = event.as_dict() else {
        return Ok(None);
    };
    let kind = ev.get("event").and_then(PyValue::as_str).unwrap_or("\u{0}none");
    match kind {
        "shown" => {
            let pts = points(ev.get("points"))?;
            let one = PyValue::Int("1".into());
            let combo = match ev.get("combo").unwrap_or(&one).py_float() {
                Ok(c) => c,
                Err(e) if e.is_type_or_value() => 1.0,
                Err(e) => return Err(e),
            };
            if let Some(p) = pts.filter(|p| !p.is_empty() && p != "0") {
                return ok(format!("Drift. {p} points, combo {}", py_f(combo, 1)), false);
            }
            ok("Drift", false)
        }
        "hidden" => ok("Drift over", false),
        "slide" => match ev.get("direction").and_then(PyValue::as_str) {
            Some(d @ ("left" | "right")) => ok(d, false),
            _ => Ok(None),
        },
        "combo" => match points(ev.get("combo"))? {
            Some(c) if !c.is_empty() => ok(format!("Combo {c}"), false),
            _ => Ok(None),
        },
        "tier" | "quick" | "spot_done" | "spot_failed" | "spot_highscore" | "out_of_bounds" => {
            let text = clean(ev.get("text"));
            if text.is_empty() {
                return Ok(None);
            }
            ok(text, matches!(kind, "spot_failed" | "out_of_bounds"))
        }
        "fail" => {
            let mut text = clean(ev.get("text"));
            if text.is_empty() {
                text = "Drift lost".to_string();
            }
            if let Some(lost) = points(ev.get("lost"))?.filter(|l| !l.is_empty() && l != "0") {
                text = format!("{text} {lost} points lost");
            }
            if let Some(total) = points(ev.get("total"))?.filter(|t| !t.is_empty() && t != "0") {
                let sep = if text.ends_with(['!', '.', '?']) { " " } else { ". " };
                text = format!("{text}{sep}Total {total}");
            }
            ok(text, true)
        }
        "scored" => {
            let Some(pts) = points(ev.get("points"))?.filter(|p| !p.is_empty()) else {
                return Ok(None);
            };
            match points(ev.get("total"))?.filter(|t| !t.is_empty()) {
                Some(total) => ok(format!("Plus {pts} points. Total {total}"), false),
                None => ok(format!("Plus {pts} points"), false),
            }
        }
        "wrapup" => match points(ev.get("points"))? {
            Some(p) if !p.is_empty() => ok(format!("Plus {p} points for this drift"), false),
            _ => Ok(None),
        },
        "spot_start" => {
            let mut parts = vec![match ev.get("name") {
                Some(PyValue::Str(name)) if !name.is_empty() => format!("Drift zone {name}"),
                _ => "Drift zone started".to_string(),
            }];
            if let Some(high) = points(ev.get("highScore"))? {
                parts.push(if high != "0" {
                    format!("High score {high}")
                } else {
                    "No high score yet".to_string()
                });
            }
            if let Some(PyValue::List(goals)) = ev.get("goals") {
                let mut open_goals = Vec::new();
                for g in goals {
                    if let PyValue::Dict(g) = g {
                        if !g.get("done").is_some_and(PyValue::truthy) {
                            open_goals.push(points(g.get("points"))?);
                        }
                    }
                }
                let open_goals: Vec<String> = open_goals
                    .into_iter()
                    .flatten()
                    .filter(|g| !g.is_empty())
                    .collect();
                if !open_goals.is_empty() {
                    parts.push(format!("Goal {} points", open_goals.join(", then ")));
                }
            }
            ok(parts.join(". "), false)
        }
        _ => Ok(None),
    }
}

/// The default clock: seconds on a monotonic clock, like Python's `time.monotonic`.
pub fn monotonic() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The speech sink: `(phrase, interrupt, source)`, Python's
/// `say(phrase, interrupt=..., source="drift")`.
pub type SayFn = Box<dyn Fn(&str, bool, &str) + Send + Sync>;
/// A clock in seconds; it must be monotonic.
pub type ClockFn = Box<dyn Fn() -> f64 + Send + Sync>;

struct Inner {
    enabled: bool,
    shown: bool,
    last_event: f64,
}

pub struct DriftAnnouncer {
    say: SayFn,
    clock: ClockFn,
    inner: Mutex<Inner>,
}

impl DriftAnnouncer {
    /// Python's `DriftAnnouncer(say, enabled=True, clock=time.monotonic)`; pass
    /// `Box::new(monotonic)` for the default clock.
    pub fn new(say: SayFn, enabled: bool, clock: ClockFn) -> Self {
        DriftAnnouncer {
            say,
            clock,
            inner: Mutex::new(Inner {
                enabled,
                shown: false,
                last_event: 0.0,
            }),
        }
    }

    /// The public `enabled` attribute.
    pub fn enabled(&self) -> bool {
        self.inner.lock().enabled
    }

    /// `set_enabled(bool(enabled))`: callers pass the truthiness of the config value.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().enabled = enabled;
    }

    /// `set_enabled` with a raw config value, coerced with Python's `bool()`.
    pub fn set_enabled_value(&self, enabled: &PyValue) {
        self.set_enabled(enabled.truthy());
    }

    /// True while the drift display shows: hold automatic speed and compass speech.
    pub fn quiet(&self) -> bool {
        let inner = self.inner.lock();
        inner.enabled && inner.shown && (self.clock)() - inner.last_event < QUIET_TIMEOUT_S
    }

    /// One drift event. `Err` where [`drift_phrase`] raises (after the state update, as
    /// in Python).
    pub fn handle(&self, event: &PyValue) -> Result<(), PyErr> {
        let Some(ev) = event.as_dict() else {
            return Ok(());
        };
        let enabled = {
            let mut inner = self.inner.lock();
            let kind = ev.get("event");
            // "alive" also means shown: a receiver restarted mid-drift missed "shown".
            if kind.is_some_and(|k| k.eq_str("shown") || k.eq_str("alive")) {
                inner.shown = true;
            } else if kind.is_some_and(|k| k.eq_str("hidden")) {
                inner.shown = false;
            }
            inner.last_event = (self.clock)();
            inner.enabled
        };
        if !enabled {
            return Ok(());
        }
        if let Some((phrase, interrupt)) = drift_phrase(event)? {
            (self.say)(&phrase, interrupt, "drift");
        }
        Ok(())
    }
}
