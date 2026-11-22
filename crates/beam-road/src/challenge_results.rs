//! Durable telemetry capture and accessible reports for the Proving Grounds climb,
//! from challenge_results.py.
//!
//! Each attempt is an NDJSON trace (compact, non-ASCII kept, `\n` line endings) and,
//! once finished, a `<name>.summary.json` (indent 2, ASCII-escaped, Windows text-mode
//! `\r\n`, no trailing newline) written through a `.tmp` file and an atomic replace.
//! Both are byte-compatible with the Python build's, which reopens them as history.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use beam_base::pyfmt::{py_g, py_round, py_strip};
use beam_reports::pyops::py_hypot;
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::pyfs::{self, glob_sorted, path_str, with_suffix};
use crate::pyjson::{
    self, Dict, DumpOpts, PyErr, Value, dict, dict_of, finite, get_or_empty, or_empty_dict, py_max,
    py_min,
};
use crate::pytime::{self, DateTime};
use crate::road_diagnostics::{classification_value, classify, round_opt};

pub const SCHEMA_VERSION: i64 = 1;
pub const CHALLENGE_ID: &str = "hill_climb";
pub const CLIMB_START_Y: f64 = -780.0;
pub const CLIMB_FINISH_Y: f64 = 2265.0;
pub const ASPHALT_HALF_WIDTH_M: f64 = 7.5;
pub const RUMBLE_OUTER_M: f64 = 10.0;
pub const GRAVEL_OUTER_M: f64 = 12.5;
pub const MAX_SAMPLE_DT_S: f64 = 0.25;
pub const RESET_DISTANCE_M: f64 = 50.0;
pub const MOVING_SPEED_MS: f64 = 0.5;
/// How long a race_complete waits for attempt_aggregated before finishing anyway.
pub const AGGREGATION_TIMEOUT_S: f64 = 3.0;

const ROW_OPTS: DumpOpts = DumpOpts::compact().ensure_ascii(false);
const SUMMARY_OPTS: DumpOpts = DumpOpts::indented(2);

/// `_finite(value, default)`.
fn finite_or(v: Option<&Value>, default: f64) -> f64 {
    finite(v).unwrap_or(default)
}

/// CPython's float `divmod(x, y)`.
fn py_divmod(x: f64, y: f64) -> (f64, f64) {
    let mut m = x % y;
    let mut div = (x - m) / y;
    if m != 0.0 {
        if (y < 0.0) != (m < 0.0) {
            m += y;
            div -= 1.0;
        }
    } else {
        m = 0.0f64.copysign(y);
    }
    let floordiv = if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        0.0f64.copysign(x / y)
    };
    (floordiv, m)
}

/// `_format_time`.
pub fn format_time(seconds: Option<&Value>) -> String {
    let seconds = py_max(0.0, finite_or(seconds, 0.0));
    let (minutes, remainder) = py_divmod(seconds, 60.0);
    if minutes != 0.0 {
        return format!("{}:{:06.3}", minutes.trunc() as i64, remainder);
    }
    format!("{remainder:.3} seconds")
}

/// `_format_elapsed_words`: a split as a sentence-friendly duration.
pub fn format_elapsed_words(seconds: Option<&Value>) -> String {
    let seconds = py_max(0.0, finite_or(seconds, 0.0));
    let (minutes, remainder) = py_divmod(seconds, 60.0);
    if minutes == 0.0 {
        return format!("{remainder:.3} seconds");
    }
    let m = minutes.trunc() as i64;
    let word = if m == 1 { "minute" } else { "minutes" };
    format!("{m} {word}, {remainder:.3} seconds")
}

/// `_format_local_datetime`: a stored ISO UTC timestamp as readable local wall-clock
/// time. `Err(OSError)` where Python's does on Windows: a time before 1970 or after
/// the year 3000, which the CRT's `localtime` refuses and the Python does not catch.
pub fn format_local_datetime(value: Option<&Value>) -> Result<String, PyErr> {
    let text = value.unwrap_or(&Value::Null).py_str();
    let text = py_strip(&text);
    let text = match text.strip_suffix('Z') {
        Some(head) => format!("{head}+00:00"),
        None => text.to_string(),
    };
    let Some(mut parsed) = DateTime::fromisoformat(&text) else {
        return Ok("unknown time".to_string());
    };
    if parsed.offset_s.is_none() {
        parsed.offset_s = Some(0);
    }
    let (local, zone) = match parsed.astimezone_local() {
        Ok(v) => v,
        Err(PyErr::OverflowError(_)) => return Ok("unknown time".to_string()),
        Err(e) => return Err(e),
    };
    let hour = local.strftime("%I");
    let hour = hour.trim_start_matches('0');
    let hour = if hour.is_empty() { "12" } else { hour };
    let suffix = if zone.is_empty() {
        String::new()
    } else {
        format!(" {zone}")
    };
    Ok(format!(
        "{} {}, {} at {hour}:{}{suffix}",
        local.strftime("%B"),
        local.day,
        local.year,
        local.strftime("%M:%S %p")
    ))
}

/// `_format_speed`.
pub fn format_speed(speed_ms: Option<&Value>, units: &str) -> String {
    let speed_ms = finite_or(speed_ms, 0.0);
    if units == "metric" {
        return format!("{:.1} km/h", speed_ms * 3.6);
    }
    format!("{:.1} mph", speed_ms * 2.2369362920544)
}

/// `_format_distance`.
pub fn format_distance(metres: Option<&Value>, units: &str) -> String {
    let metres = finite_or(metres, 0.0);
    if units == "metric" {
        return format!("{metres:.1} metres");
    }
    format!("{:.1} feet", metres * 3.280839895)
}

/// `_surface_for_position`.
pub fn surface_for_position(x: Option<&Value>, y: Option<&Value>) -> &'static str {
    let x = finite_or(x, 0.0).abs();
    let y = finite_or(y, CLIMB_START_Y);
    if y < CLIMB_START_Y || y > CLIMB_FINISH_Y {
        return "outside_course";
    }
    if x <= ASPHALT_HALF_WIDTH_M {
        return "asphalt";
    }
    if x <= RUMBLE_OUTER_M {
        return "rumble";
    }
    if x <= GRAVEL_OUTER_M {
        return "gravel";
    }
    "outside_corridor"
}

/// `_contact_flags`.
pub fn contact_flags(packet: Option<&Value>) -> Result<Value, PyErr> {
    let diagnostic = get_or_empty(packet, "diagnostic")?;
    let names = match get_or_empty(diagnostic, "contactMaterials")? {
        Some(v) if v.truthy() => v.py_str(),
        _ => String::new(),
    }
    .to_uppercase();
    Ok(dict([
        ("rumble", names.contains("RUMBLE_STRIP").into()),
        ("gravel", names.contains("GRAVEL").into()),
        ("materials", names.into()),
    ]))
}

fn sample_t(sample: &Value) -> Result<f64, PyErr> {
    match sample.py_get("t")? {
        Some(v) => v.py_float(),
        None => Err(PyErr::KeyError("'t'".to_string())),
    }
}

/// Time-weighted statistics from raw challenge samples.
pub fn analyze_attempt(
    samples: &[Value],
    checkpoints: &[Value],
    event: &Dict,
) -> Result<Value, PyErr> {
    let mut totals: IndexMap<&'static str, f64> = [
        "sampled_s",
        "moving_s",
        "stopped_s",
        "throttle_s",
        "brake_s",
        "traction_control_s",
        "asphalt_s",
        "off_asphalt_s",
        "rumble_contact_s",
        "gravel_contact_s",
        "nav_on_road_s",
        "nav_off_road_s",
        "wheelspin_s",
        "lockup_s",
        "slide_s",
    ]
    .into_iter()
    .map(|k| (k, 0.0))
    .collect();
    let mut moving_speed_integral = 0.0;
    let mut distance_m = 0.0;
    let (mut max_speed, mut finish_speed, mut max_slip) = (0.0f64, 0.0f64, 0.0f64);
    let (mut start_z, mut end_z): (Option<f64>, Option<f64>) = (None, None);
    let mut previous: Option<&Value> = None;
    let mut gaps: Vec<Value> = Vec::new();
    let (mut dep_left, mut dep_right) = (0i64, 0i64);
    let mut was_asphalt = true;
    let (mut spin_episodes, mut lock_episodes) = (0i64, 0i64);
    let mut previous_slip = Value::from("none");
    let mut slide_episodes = 0i64;
    let mut max_slide_deg = 0.0f64;
    let mut previous_slide = false;

    for sample in samples {
        let telemetry = or_empty_dict(sample.py_get("telemetry")?);
        let position = or_empty_dict(telemetry.py_get("position")?);
        let speed = py_max(0.0, finite_or(telemetry.get("ground_speed_ms"), 0.0));
        max_speed = py_max(max_speed, speed);
        finish_speed = speed;
        if let Some(z) = finite(position.get("z")) {
            if start_z.is_none() {
                start_z = Some(z);
            }
            end_z = Some(z);
        }

        if let Some(prev) = previous {
            let raw_dt = py_max(0.0, sample_t(sample)? - sample_t(prev)?);
            let dt = if raw_dt > MAX_SAMPLE_DT_S {
                gaps.push(py_round(raw_dt, 3).into());
                0.0
            } else {
                raw_dt
            };
            totals["sampled_s"] += dt;
            if speed > MOVING_SPEED_MS {
                totals["moving_s"] += dt;
                moving_speed_integral += speed * dt;
            } else {
                totals["stopped_s"] += dt;
            }
            if finite_or(telemetry.get("throttle"), 0.0) > 0.1 {
                totals["throttle_s"] += dt;
            }
            if finite_or(telemetry.get("brake"), 0.0) > 0.1 {
                totals["brake_s"] += dt;
            }
            if telemetry
                .get("traction_control_active")
                .is_some_and(Value::truthy)
            {
                totals["traction_control_s"] += dt;
            }

            let on_asphalt = match sample.get("surface") {
                Some(s) if s.truthy() => s.eq_str("asphalt"),
                _ => false,
            };
            totals[if on_asphalt {
                "asphalt_s"
            } else {
                "off_asphalt_s"
            }] += dt;
            if was_asphalt && !on_asphalt {
                if finite_or(position.get("x"), 0.0) < 0.0 {
                    dep_left += 1;
                } else {
                    dep_right += 1;
                }
            }
            was_asphalt = on_asphalt;

            let contacts = or_empty_dict(sample.get("contacts"));
            if contacts.py_get("rumble")?.is_some_and(Value::truthy) {
                totals["rumble_contact_s"] += dt;
            }
            if contacts.py_get("gravel")?.is_some_and(Value::truthy) {
                totals["gravel_contact_s"] += dt;
            }
            let state = get_or_empty(sample.get("road"), "state")?;
            if state.is_some_and(|s| s.eq_str("onRoad")) {
                totals["nav_on_road_s"] += dt;
            } else if state.is_some_and(|s| s.eq_str("offRoad")) {
                totals["nav_off_road_s"] += dt;
            }

            let slip = or_empty_dict(sample.get("slip"));
            let kind = if slip.py_get("active")?.is_some_and(Value::truthy) {
                slip.get("kind").cloned().unwrap_or_else(|| "none".into())
            } else {
                Value::from("none")
            };
            if kind.eq_str("wheelspin") || kind.eq_str("lockup") {
                let k = kind.as_str().unwrap_or("");
                totals[if k == "wheelspin" {
                    "wheelspin_s"
                } else {
                    "lockup_s"
                }] += dt;
                max_slip = py_max(max_slip, finite_or(slip.get("magnitude_mps"), 0.0));
                if !pyjson::py_eq(&kind, &previous_slip) {
                    if k == "wheelspin" {
                        spin_episodes += 1;
                    } else {
                        lock_episodes += 1;
                    }
                }
            }
            previous_slip = kind;
            let sliding = slip.get("slide").is_some_and(Value::truthy);
            if sliding {
                totals["slide_s"] += dt;
                max_slide_deg = py_max(max_slide_deg, finite_or(slip.get("slide_deg"), 0.0));
                if !previous_slide {
                    slide_episodes += 1;
                }
            }
            previous_slide = sliding;

            let previous_pos = get_or_empty(prev.get("telemetry"), "position")?;
            let x0 = finite(get_or_empty(previous_pos, "x")?);
            let y0 = finite(get_or_empty(previous_pos, "y")?);
            let x1 = finite(position.get("x"));
            let y1 = finite(position.get("y"));
            if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (x0, y0, x1, y1) {
                let step = py_hypot(x1 - x0, y1 - y0);
                if step < RESET_DISTANCE_M {
                    distance_m += step;
                }
            }
        }
        previous = Some(sample);
    }

    let sampled = totals["sampled_s"];
    let moving = totals["moving_s"];
    let mut official = finite(event.get("official_time_s"));
    let penalty = py_max(0.0, finite_or(event.get("penalty_s"), 0.0));
    let mut raw_time = finite(event.get("raw_time_s"));
    if raw_time.is_none()
        && let Some(o) = official
    {
        raw_time = Some(py_max(0.0, o - penalty));
    }
    let raw_time =
        raw_time.unwrap_or_else(|| py_max(0.0, finite_or(event.get("race_time_s"), sampled)));
    let official = *official.get_or_insert(raw_time + penalty);

    let rounded_totals: Dict = totals
        .iter()
        .map(|(k, v)| (k.to_string(), Value::Float(py_round(*v, 3))))
        .collect();
    let sample_span = if samples.len() > 1 {
        py_max(
            0.0,
            sample_t(&samples[samples.len() - 1])? - sample_t(&samples[0])?,
        )
    } else {
        0.0
    };
    let mut max_lateral: Option<f64> = None;
    for row in samples {
        let pos = get_or_empty(row.get("telemetry"), "position")?;
        let x = finite_or(get_or_empty(pos, "x")?, 0.0).abs();
        max_lateral = Some(max_lateral.map_or(x, |m| py_max(m, x)));
    }
    let max_lateral = max_lateral.unwrap_or(0.0);
    let n = samples.len();
    Ok(dict([
        ("official_time_s", py_round(official, 3).into()),
        ("raw_time_s", py_round(raw_time, 3).into()),
        ("penalty_s", py_round(penalty, 3).into()),
        ("samples", n.into()),
        (
            "sample_rate_hz",
            (if sample_span != 0.0 {
                py_round((n as f64 - 1.0) / sample_span, 2)
            } else {
                0.0
            })
            .into(),
        ),
        (
            "packet_gaps",
            dict([
                ("count", gaps.len().into()),
                ("durations_s", Value::List(gaps)),
            ]),
        ),
        ("distance_m", py_round(distance_m, 2).into()),
        (
            "elevation_gain_m",
            py_round(
                py_max(0.0, end_z.unwrap_or(0.0) - start_z.unwrap_or(0.0)),
                2,
            )
            .into(),
        ),
        (
            "average_speed_mps",
            (if raw_time != 0.0 {
                py_round(distance_m / raw_time, 3)
            } else {
                0.0
            })
            .into(),
        ),
        (
            "moving_average_speed_mps",
            (if moving != 0.0 {
                py_round(moving_speed_integral / moving, 3)
            } else {
                0.0
            })
            .into(),
        ),
        ("max_speed_mps", py_round(max_speed, 3).into()),
        ("finish_speed_mps", py_round(finish_speed, 3).into()),
        ("durations", Value::Dict(rounded_totals)),
        (
            "departures",
            dict([
                ("left", dep_left.into()),
                ("right", dep_right.into()),
                ("total", (dep_left + dep_right).into()),
            ]),
        ),
        ("max_lateral_excursion_m", py_round(max_lateral, 2).into()),
        (
            "slip",
            dict([
                ("wheelspin_episodes", spin_episodes.into()),
                ("lockup_episodes", lock_episodes.into()),
                ("max_magnitude_mps", py_round(max_slip, 3).into()),
                ("slide_episodes", slide_episodes.into()),
                ("max_slide_deg", py_round(max_slide_deg, 1).into()),
            ]),
        ),
        ("checkpoints", Value::List(checkpoints.to_vec())),
        (
            "recoveries",
            (finite_or(event.get("recovery_count"), 0.0).trunc() as i64).into(),
        ),
        (
            "official_confirmed",
            event
                .get("official_confirmed")
                .is_some_and(Value::truthy)
                .into(),
        ),
    ]))
}

// ---------------------------------------------------------------------------------
// Recorder

struct Attempt {
    attempt_id: String,
    name: String,
    raw_path: PathBuf,
    file: Option<BufWriter<File>>,
    writes: i64,
    started_mono: f64,
    started_utc: String,
    samples: Vec<Value>,
    checkpoints: Vec<Value>,
    capturing: bool,
    pending_complete: bool,
    race_complete_event: Dict,
}

struct CState {
    attempt: Option<Attempt>,
    timer: Option<mpsc::Sender<()>>,
}

pub type FinalizeFn = dyn Fn(&Value, bool) + Send + Sync;

struct Inner {
    directory: PathBuf,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    now_utc: Box<dyn Fn() -> DateTime + Send + Sync>,
    finalize_callback: Mutex<Option<Arc<FinalizeFn>>>,
    state: Mutex<CState>,
}

/// Thread-safe lifecycle recorder fed by the Lua bridge and the road listener.
/// Cheap to clone; clones share one recorder.
#[derive(Clone)]
pub struct HillClimbChallengeRecorder {
    inner: Arc<Inner>,
}

fn write_row(attempt: &mut Attempt, row: &Value, flush: bool) -> Result<(), PyErr> {
    let text = pyjson::dumps(row, &ROW_OPTS)?;
    let file = attempt
        .file
        .as_mut()
        .ok_or_else(|| PyErr::OSError("I/O operation on closed file.".to_string()))?;
    file.write_all(format!("{text}\n").as_bytes())?;
    attempt.writes += 1;
    if flush || attempt.writes % 20 == 0 {
        file.flush()?;
    }
    Ok(())
}

fn cancel_timer(s: &mut CState) {
    if let Some(t) = s.timer.take() {
        let _ = t.send(());
    }
}

impl HillClimbChallengeRecorder {
    /// `HillClimbChallengeRecorder(directory)` on `time.monotonic()` and the system
    /// UTC clock. Creates the directory, as the Python constructor does.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self, PyErr> {
        Self::with_clocks(directory, pytime::monotonic, DateTime::utc_now, None)
    }

    pub fn with_clocks(
        directory: impl AsRef<Path>,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
        now_utc: impl Fn() -> DateTime + Send + Sync + 'static,
        finalize_callback: Option<Arc<FinalizeFn>>,
    ) -> Result<Self, PyErr> {
        let directory = pyfs::py_path(directory.as_ref());
        std::fs::create_dir_all(&directory)?;
        Ok(HillClimbChallengeRecorder {
            inner: Arc::new(Inner {
                directory,
                clock: Box::new(clock),
                now_utc: Box::new(now_utc),
                finalize_callback: Mutex::new(finalize_callback),
                state: Mutex::new(CState {
                    attempt: None,
                    timer: None,
                }),
            }),
        })
    }

    pub fn directory(&self) -> &Path {
        &self.inner.directory
    }

    pub fn set_finalize_callback(&self, callback: Option<Arc<FinalizeFn>>) {
        *self.inner.finalize_callback.lock() = callback;
    }

    pub fn is_capturing(&self) -> bool {
        let s = self.inner.state.lock();
        s.attempt.as_ref().is_some_and(|a| a.capturing)
    }

    /// The running attempt's trace file (the sim inspects it).
    pub fn attempt_raw_path(&self) -> Option<PathBuf> {
        let s = self.inner.state.lock();
        s.attempt.as_ref().map(|a| a.raw_path.clone())
    }

    fn attempt_name(&self, attempt_id: Option<&Value>) -> String {
        let text = match attempt_id {
            Some(v) if v.truthy() => v.py_str(),
            _ => "attempt".to_string(),
        };
        let subbed = pyjson::sub_runs(&text, |c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
        });
        let mut safe = pyjson::strip_chars(&subbed, &['-', '.']).to_string();
        if safe.is_empty() {
            safe = "attempt".to_string();
        }
        let stamp = (self.inner.now_utc)().strftime("%Y%m%dT%H%M%S.%fZ");
        format!("{stamp}-{safe}")
    }

    fn start_locked(&self, s: &mut CState, event: &Value) -> Result<(), PyErr> {
        let name = self.attempt_name(event.get("attempt_id"));
        let raw_path = self.inner.directory.join(format!("{name}.ndjson"));
        let handle = BufWriter::new(File::create(&raw_path)?);
        let started_mono = (self.inner.clock)();
        let attempt_id = match event.get("attempt_id") {
            Some(v) if v.truthy() => v.py_str(),
            _ => name.clone(),
        };
        let mut attempt = Attempt {
            attempt_id,
            name,
            raw_path,
            file: Some(handle),
            writes: 0,
            started_mono,
            started_utc: (self.inner.now_utc)().isoformat(),
            samples: Vec::new(),
            checkpoints: Vec::new(),
            capturing: true,
            pending_complete: false,
            race_complete_event: Dict::new(),
        };
        let meta = dict([
            ("kind", "meta".into()),
            ("schema", SCHEMA_VERSION.into()),
            ("challenge_id", CHALLENGE_ID.into()),
            ("attempt_id", attempt.attempt_id.as_str().into()),
            ("started_utc", attempt.started_utc.as_str().into()),
            ("event", event.clone()),
        ]);
        write_row(&mut attempt, &meta, true)?;
        s.attempt = Some(attempt);
        Ok(())
    }

    /// Drop a superseded pre-start trace without adding it to report history.
    fn discard_locked(&self, s: &mut CState) -> Result<(), PyErr> {
        let Some(attempt) = s.attempt.as_mut() else {
            return Ok(());
        };
        cancel_timer_attempt(&mut s.timer);
        attempt.capturing = false;
        if let Some(mut f) = attempt.file.take() {
            let _ = f.flush();
        }
        match std::fs::remove_file(&attempt.raw_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        s.attempt = None;
        Ok(())
    }

    /// Record one road sample while capturing. `Ok(false)` otherwise.
    pub fn record(&self, packet: &Value, telemetry: &Value) -> Result<bool, PyErr> {
        let mut s = self.inner.state.lock();
        let Some(attempt) = s.attempt.as_mut().filter(|a| a.capturing) else {
            return Ok(false);
        };
        let position = get_or_empty(Some(telemetry), "position")?;
        let position = or_empty_dict(position);
        let now = (self.inner.clock)();
        let x = position.get("x");
        let y = position.get("y");
        let telemetry_or = or_empty_dict(Some(telemetry));
        let row = dict([
            ("kind", "sample".into()),
            (
                "t",
                py_round(py_max(0.0, now - attempt.started_mono), 4).into(),
            ),
            ("utc", (self.inner.now_utc)().isoformat().into()),
            ("road", or_empty_dict(Some(packet))),
            ("telemetry", telemetry_or.clone()),
            (
                "course_progress_m",
                py_round(
                    py_max(
                        0.0,
                        py_min(
                            CLIMB_FINISH_Y - CLIMB_START_Y,
                            finite_or(y, CLIMB_START_Y) - CLIMB_START_Y,
                        ),
                    ),
                    3,
                )
                .into(),
            ),
            ("lateral_offset_m", round_opt(finite(x), 3)),
            ("surface", surface_for_position(x, y).into()),
            ("contacts", contact_flags(Some(packet))?),
            ("slip", classification_value(&classify(&telemetry_or)?)),
        ]);
        attempt.samples.push(row.clone());
        write_row(attempt, &row, false)?;
        Ok(true)
    }

    fn prior_best_locked(&self, excluding_name: &str) -> Result<Option<f64>, PyErr> {
        let mut best: Option<f64> = None;
        for path in glob_sorted(&self.inner.directory, "", ".summary.json") {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with(excluding_name) {
                continue;
            }
            let Ok(text) = pyjson::read_text(&path) else {
                continue;
            };
            let Ok(data) = pyjson::loads(&text) else {
                continue;
            };
            if !data
                .py_get("status")?
                .is_some_and(|s| s.eq_str("completed"))
            {
                continue;
            }
            let value = finite(get_or_empty(data.get("statistics"), "official_time_s")?);
            if let Some(v) = value
                && best.is_none_or(|b| v < b)
            {
                best = Some(v);
            }
        }
        Ok(best)
    }

    fn finish_locked(
        &self,
        s: &mut CState,
        status: &str,
        event: &Dict,
        reason: &str,
    ) -> Result<Option<Value>, PyErr> {
        if s.attempt.is_none() {
            return Ok(None);
        }
        cancel_timer(s);
        let attempt = s.attempt.as_mut().expect("checked above");
        attempt.capturing = false;
        let mut merged = attempt.race_complete_event.clone();
        for (k, v) in event {
            merged.insert(k.clone(), v.clone());
        }
        let statistics = analyze_attempt(&attempt.samples, &attempt.checkpoints, &merged)?;
        let name = attempt.name.clone();
        let prior_best = self.prior_best_locked(&name)?;
        let attempt = s.attempt.as_mut().expect("checked above");
        let official = finite(statistics.get("official_time_s")).unwrap_or(0.0);
        let new_best = match merged.get("new_best").filter(|v| !v.is_null()) {
            Some(v) => v.truthy(),
            None => status == "completed" && prior_best.is_none_or(|p| official < p),
        };
        let summary_path = self.inner.directory.join(format!("{name}.summary.json"));
        let summary = dict([
            ("schema", SCHEMA_VERSION.into()),
            ("challenge_id", CHALLENGE_ID.into()),
            ("attempt_id", attempt.attempt_id.as_str().into()),
            ("status", status.into()),
            ("reason", reason.into()),
            ("started_utc", attempt.started_utc.as_str().into()),
            ("ended_utc", (self.inner.now_utc)().isoformat().into()),
            ("raw_path", path_str(&attempt.raw_path).into()),
            ("statistics", statistics.clone()),
            (
                "personal_best",
                dict([
                    ("new_best", new_best.into()),
                    ("previous_best_s", round_opt(prior_best, 3)),
                    (
                        "delta_s",
                        match prior_best {
                            Some(p) => round_opt(Some(official - p), 3),
                            None => Value::Null,
                        },
                    ),
                ]),
            ),
            ("summary_path", path_str(&summary_path).into()),
        ]);
        let end = dict([
            ("kind", "end".into()),
            ("status", status.into()),
            ("reason", reason.into()),
            ("statistics", statistics),
        ]);
        write_row(attempt, &end, true)?;
        if let Some(mut f) = attempt.file.take() {
            f.flush()?;
        }
        let temp_path = with_suffix(&summary_path, ".json.tmp");
        pyfs::write_text(&temp_path, &pyjson::dumps(&summary, &SUMMARY_OPTS)?)?;
        std::fs::rename(&temp_path, &summary_path)?;
        s.attempt = None;
        Ok(Some(summary))
    }

    fn notify(&self, summary: Option<Value>, auto_open: bool) {
        let Some(summary) = summary else {
            return;
        };
        let callback = self.inner.finalize_callback.lock().clone();
        if let Some(cb) = callback {
            // Python swallows any exception from the callback.
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&summary, auto_open)));
        }
    }

    fn timeout_complete(&self) {
        let summary = {
            let mut s = self.inner.state.lock();
            let pending = s.attempt.as_ref().is_some_and(|a| a.pending_complete);
            if pending {
                let mut event = s
                    .attempt
                    .as_ref()
                    .map(|a| a.race_complete_event.clone())
                    .unwrap_or_default();
                event.insert("official_confirmed".into(), false.into());
                match self.finish_locked(&mut s, "completed", &event, "aggregation_timeout") {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("Hill climb aggregation timeout failed: {e}");
                        None
                    }
                }
            } else {
                None
            }
        };
        self.notify(summary, true);
    }

    fn start_timer(&self, s: &mut CState) {
        let (tx, rx) = mpsc::channel::<()>();
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let delay = Duration::from_secs_f64(AGGREGATION_TIMEOUT_S);
        let started = Instant::now();
        std::thread::Builder::new()
            .name("hill-climb-timer".into())
            .spawn(move || {
                match rx.recv_timeout(delay) {
                    Ok(()) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                    // Replaced without cancel(): Python's old Timer still fires.
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Some(rest) = delay.checked_sub(started.elapsed()) {
                            std::thread::sleep(rest);
                        }
                    }
                }
                if let Some(inner) = weak.upgrade() {
                    HillClimbChallengeRecorder { inner }.timeout_complete();
                }
            })
            .ok();
        s.timer = Some(tx);
    }

    /// Apply one Lua lifecycle event. `Ok(None)` for anything that is not a
    /// hill-climb event; otherwise `{"capture": bool}` or `{}`.
    pub fn handle_event(
        &self,
        event: &Value,
        telemetry: Option<&Value>,
    ) -> Result<Option<Value>, PyErr> {
        let Value::Dict(ev) = event else {
            return Ok(None);
        };
        if !ev
            .get("challenge_id")
            .is_some_and(|c| c.eq_str(CHALLENGE_ID))
        {
            return Ok(None);
        }
        let kind = match ev.get("event") {
            Some(v) if v.truthy() => v.py_str(),
            _ => String::new(),
        };
        let mut notifications: Vec<(Option<Value>, bool)> = Vec::new();
        let mut capture: Option<bool> = None;
        {
            let mut s = self.inner.state.lock();
            let has_attempt = s.attempt.is_some();
            if kind == "started" {
                if has_attempt {
                    if ev.get("replace_active").is_some_and(Value::truthy) {
                        self.discard_locked(&mut s)?;
                    } else {
                        notifications.push((
                            self.finish_locked(&mut s, "aborted", &Dict::new(), "restarted")?,
                            false,
                        ));
                    }
                }
                self.start_locked(&mut s, event)?;
                capture = Some(true);
            } else if kind == "checkpoint" && has_attempt {
                let snapshot = or_empty_dict(telemetry);
                let speed = snapshot.py_get("ground_speed_ms")?;
                let attempt = s.attempt.as_mut().expect("checked above");
                let index = finite_or(
                    ev.get("checkpoint_index"),
                    (attempt.checkpoints.len() + 1) as f64,
                );
                let name = match ev.get("checkpoint_name") {
                    Some(v) if v.truthy() => v.py_str(),
                    _ => "Checkpoint".to_string(),
                };
                let split = dict_of([
                    ("index", Value::Int(index.trunc() as i64)),
                    ("name", name.into()),
                    ("time_s", round_opt(finite(ev.get("race_time_s")), 3)),
                    ("speed_mps", round_opt(finite(speed), 3)),
                ]);
                attempt.checkpoints.push(Value::Dict(split.clone()));
                let mut row = dict_of([("kind", "checkpoint".into())]);
                row.extend(split);
                write_row(attempt, &Value::Dict(row), true)?;
            } else if kind == "race_complete" && has_attempt {
                let attempt = s.attempt.as_mut().expect("checked above");
                attempt.capturing = false;
                attempt.pending_complete = true;
                attempt.race_complete_event = ev.clone();
                let row = dict([("kind", "race_complete".into()), ("event", event.clone())]);
                write_row(attempt, &row, true)?;
                capture = Some(false);
                self.start_timer(&mut s);
            } else if kind == "attempt_aggregated" {
                if !has_attempt {
                    self.start_locked(&mut s, event)?;
                    if let Some(a) = s.attempt.as_mut() {
                        a.capturing = false;
                    }
                }
                let mut ev2 = ev.clone();
                ev2.insert("official_confirmed".into(), true.into());
                notifications.push((self.finish_locked(&mut s, "completed", &ev2, "")?, true));
                capture = Some(false);
            } else if (kind == "aborted" || kind == "mission_stopped") && has_attempt {
                notifications.push((self.finish_locked(&mut s, "aborted", ev, &kind)?, false));
                capture = Some(false);
            }
        }
        for (summary, auto_open) in notifications {
            self.notify(summary, auto_open);
        }
        Ok(Some(match capture {
            Some(c) => dict([("capture", c.into())]),
            None => Value::Dict(Dict::new()),
        }))
    }

    /// Finish any running attempt as aborted ("beamtel_shutdown").
    pub fn shutdown(&self) -> Result<(), PyErr> {
        let summary = {
            let mut s = self.inner.state.lock();
            if s.attempt.is_some() {
                self.finish_locked(&mut s, "aborted", &Dict::new(), "beamtel_shutdown")?
            } else {
                None
            }
        };
        self.notify(summary, false);
        Ok(())
    }

    /// Saved summaries, newest first, each with its `summary_path`. Unreadable and
    /// non-dict files are skipped.
    pub fn list_reports(&self, limit: i64) -> Vec<Value> {
        let mut reports = Vec::new();
        let mut paths = glob_sorted(&self.inner.directory, "", ".summary.json");
        paths.reverse();
        for path in paths {
            let Ok(text) = pyjson::read_text(&path) else {
                continue;
            };
            let Ok(Value::Dict(mut report)) = pyjson::loads(&text) else {
                continue;
            };
            report.insert("summary_path".into(), path_str(&path).into());
            reports.push(Value::Dict(report));
            if reports.len() as i64 >= limit {
                break;
            }
        }
        reports
    }

    /// One line of the history list.
    pub fn history_line(summary: &Value, _units: &str) -> Result<String, PyErr> {
        let stats = or_empty_dict(summary.py_get("statistics")?);
        let stamp = format_local_datetime(summary.get("started_utc"))?;
        let status = summary
            .get("status")
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let time_text = if status.eq_str("completed") {
            format_time(stats.py_get("official_time_s")?)
        } else {
            status.py_str()
        };
        let best =
            if get_or_empty(summary.get("personal_best"), "new_best")?.is_some_and(Value::truthy) {
                ", new personal best"
            } else {
                ""
            };
        Ok(format!("{stamp}, {time_text}{best}"))
    }

    /// The report, one readable line per fact.
    pub fn report_lines(summary: &Value, units: &str) -> Result<Vec<String>, PyErr> {
        let stats = or_empty_dict(summary.py_get("statistics")?);
        let durations = or_empty_dict(stats.py_get("durations")?);
        let best = or_empty_dict(summary.get("personal_best"));
        let status = summary
            .get("status")
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let mut lines = vec![
            format!("Status, {}", status.py_str()),
            format!(
                "Started, {}",
                format_local_datetime(summary.get("started_utc"))?
            ),
            format!(
                "Finished, {}",
                format_local_datetime(summary.get("ended_utc"))?
            ),
            format!(
                "Official time, {}",
                format_time(stats.get("official_time_s"))
            ),
            format!("Driving time, {}", format_time(stats.get("raw_time_s"))),
            format!("Penalty time, {}", format_time(stats.get("penalty_s"))),
        ];
        if best.py_get("new_best")?.is_some_and(Value::truthy) {
            lines.push("Personal best, new best time".to_string());
        } else if best.get("delta_s").is_some_and(|v| !v.is_null()) {
            let delta = finite_or(best.get("delta_s"), 0.0);
            lines.push(format!(
                "Personal best comparison, {:.3} seconds {}",
                delta.abs(),
                if delta >= 0.0 { "slower" } else { "faster" }
            ));
        }
        let d = |key: &str| durations.get(key);
        let sub = |outer: &str, key: &str, default: Value| -> Result<Value, PyErr> {
            let empty = Value::Dict(Dict::new());
            let o = stats.get(outer).unwrap_or(&empty);
            Ok(o.py_get(key)?.cloned().unwrap_or(default))
        };
        let max_slide = sub("slip", "max_slide_deg", Value::Float(0.0))?;
        lines.extend([
            format!(
                "Distance travelled, {}",
                format_distance(stats.get("distance_m"), units)
            ),
            format!(
                "Elevation gained, {}",
                format_distance(stats.get("elevation_gain_m"), units)
            ),
            format!(
                "Average speed, {}",
                format_speed(stats.get("average_speed_mps"), units)
            ),
            format!(
                "Moving average speed, {}",
                format_speed(stats.get("moving_average_speed_mps"), units)
            ),
            format!(
                "Maximum speed, {}",
                format_speed(stats.get("max_speed_mps"), units)
            ),
            format!(
                "Finish speed, {}",
                format_speed(stats.get("finish_speed_mps"), units)
            ),
            format!("Stopped time, {}", format_time(d("stopped_s"))),
            format!("Time on asphalt, {}", format_time(d("asphalt_s"))),
            format!("Time off asphalt, {}", format_time(d("off_asphalt_s"))),
            format!(
                "Rumble strip contact, {}",
                format_time(d("rumble_contact_s"))
            ),
            format!("Gravel contact, {}", format_time(d("gravel_contact_s"))),
            format!(
                "Navigation on-road time, {}",
                format_time(d("nav_on_road_s"))
            ),
            format!(
                "Navigation off-road time, {}",
                format_time(d("nav_off_road_s"))
            ),
            format!(
                "Departures, {} total, {} left, {} right",
                sub("departures", "total", 0.into())?.py_str(),
                sub("departures", "left", 0.into())?.py_str(),
                sub("departures", "right", 0.into())?.py_str()
            ),
            format!(
                "Maximum lateral excursion, {}",
                format_distance(stats.get("max_lateral_excursion_m"), units)
            ),
            format!(
                "Wheelspin, {}, {} episodes",
                format_time(d("wheelspin_s")),
                sub("slip", "wheelspin_episodes", 0.into())?.py_str()
            ),
            format!(
                "Wheel lockup, {}, {} episodes",
                format_time(d("lockup_s")),
                sub("slip", "lockup_episodes", 0.into())?.py_str()
            ),
            format!(
                "Maximum wheel slip, {} difference",
                format_speed(Some(&sub("slip", "max_magnitude_mps", Value::Null)?), units)
            ),
            format!(
                "Sideways slide, {}, {} episodes, maximum {} degrees",
                format_time(d("slide_s")),
                sub("slip", "slide_episodes", 0.into())?.py_str(),
                py_round_builtin(&max_slide)?
            ),
            format!(
                "Traction control active, {}",
                format_time(d("traction_control_s"))
            ),
            format!("Throttle applied, {}", format_time(d("throttle_s"))),
            format!("Brake applied, {}", format_time(d("brake_s"))),
            format!(
                "Recoveries, {}",
                stats
                    .get("recoveries")
                    .cloned()
                    .unwrap_or(Value::Int(0))
                    .py_str()
            ),
        ]);
        let checkpoints: Vec<Value> = match stats.get("checkpoints") {
            Some(v) if v.truthy() => match v {
                Value::List(l) => l.clone(),
                Value::Dict(d) => d.keys().map(|k| Value::from(k.as_str())).collect(),
                Value::Str(s) => s.chars().map(|c| Value::from(c.to_string())).collect(),
                other => {
                    return Err(PyErr::TypeError(format!(
                        "'{}' object is not iterable",
                        other.type_name()
                    )));
                }
            },
            _ => Vec::new(),
        };
        for checkpoint in &checkpoints {
            let index = checkpoint.py_get("index")?.cloned().unwrap_or(Value::Null);
            lines.push(format!(
                "Checkpoint {} reached at {} into the run, traveling {}",
                index.py_str(),
                format_elapsed_words(checkpoint.py_get("time_s")?),
                format_speed(checkpoint.py_get("speed_mps")?, units)
            ));
        }
        Ok(lines)
    }
}

fn cancel_timer_attempt(timer: &mut Option<mpsc::Sender<()>>) {
    if let Some(t) = timer.take() {
        let _ = t.send(());
    }
}

/// `round(x)` with no digits, printed: the int Python's builtin returns.
fn py_round_builtin(v: &Value) -> Result<String, PyErr> {
    match v {
        Value::Bool(b) => Ok((*b as i64).to_string()),
        Value::Int(i) => Ok(i.to_string()),
        Value::BigInt(t) => Ok(t.clone()),
        Value::Float(f) => {
            if f.is_nan() {
                Err(PyErr::ValueError(
                    "cannot convert float NaN to integer".to_string(),
                ))
            } else if f.is_infinite() {
                Err(PyErr::OverflowError(
                    "cannot convert float infinity to integer".to_string(),
                ))
            } else {
                let r = f.round_ties_even();
                Ok(format!("{:.0}", if r == 0.0 { 0.0 } else { r }))
            }
        }
        other => Err(PyErr::TypeError(format!(
            "type {} doesn't define __round__ method",
            other.type_name()
        ))),
    }
}

/// The sentence spoken when an attempt completes.
pub fn completion_speech(summary: &Value) -> Result<String, PyErr> {
    let stats = or_empty_dict(summary.py_get("statistics")?);
    let mut phrase = format!(
        "Hill climb complete in {}",
        format_time(stats.py_get("official_time_s")?)
    );
    let penalty = finite_or(stats.get("penalty_s"), 0.0);
    if penalty > 0.0 {
        phrase.push_str(&format!(
            ", including {} seconds of penalties",
            py_g(penalty, 6)
        ));
    }
    if get_or_empty(summary.get("personal_best"), "new_best")?.is_some_and(Value::truthy) {
        phrase.push_str(". New personal best");
    }
    Ok(phrase)
}
