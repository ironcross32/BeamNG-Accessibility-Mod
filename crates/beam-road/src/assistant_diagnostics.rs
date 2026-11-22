//! Opt-in driving-assistant event and vehicle-state recording for MCP observation,
//! from assistant_diagnostics.py.
//!
//! Sessions are NDJSON under `%LOCALAPPDATA%\beamtel\assistant_diagnostics`, one row
//! per event, written with `json.dumps(row, ensure_ascii=False)` and Windows text-mode
//! line endings (`\r\n`), so either build can review the other's files.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};

use beam_base::pyfmt::py_round;
use parking_lot::Mutex;

use crate::pyfs::{self, glob_sorted, path_str};
use crate::pyjson::{
    self, Dict, DumpOpts, PyErr, Value, dict, dict_of, get_or_empty, py_eq, read_text_lines,
};
use crate::pytime::{self, DateTime};
use crate::road_guidance::R2Packet;

const ROW_LIMIT: usize = 1500;
const ROW_OPTS: DumpOpts = DumpOpts::default_seps().ensure_ascii(false);

struct State {
    stream: Option<Box<dyn Write + Send>>,
    path: Option<PathBuf>,
    started: f64,
    sequence: i64,
    rows: VecDeque<Dict>,
    latest: Value,
    latest_time: f64,
    error: Option<String>,
    road_sample_time: f64,
}

/// Records assistant events and road samples while a session is armed.
pub struct AssistantDiagnosticRecorder {
    pub directory: PathBuf,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    utc_now: Box<dyn Fn() -> DateTime + Send + Sync>,
    state: Mutex<State>,
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl AssistantDiagnosticRecorder {
    /// A recorder on `time.monotonic()` and the system UTC clock.
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self::with_clocks(directory, pytime::monotonic, DateTime::utc_now)
    }

    pub fn with_clocks(
        directory: impl AsRef<Path>,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
        utc_now: impl Fn() -> DateTime + Send + Sync + 'static,
    ) -> Self {
        AssistantDiagnosticRecorder {
            directory: pyfs::py_path(directory.as_ref()),
            clock: Box::new(clock),
            utc_now: Box::new(utc_now),
            state: Mutex::new(State {
                stream: None,
                path: None,
                started: 0.0,
                sequence: 0,
                rows: VecDeque::new(),
                latest: Value::Null,
                latest_time: 0.0,
                error: None,
                road_sample_time: f64::NEG_INFINITY,
            }),
        }
    }

    /// Replace the output stream (the sims hand it an in-memory buffer).
    pub fn set_stream(&self, stream: Option<Box<dyn Write + Send>>) {
        self.state.lock().stream = stream;
    }

    /// The last sequence number written.
    pub fn sequence(&self) -> i64 {
        self.state.lock().sequence
    }

    /// Keep cue timing alongside the drive, including manual driving.
    pub fn record_road(&self, packet: &R2Packet, audio_state: Value) {
        let mut s = self.state.lock();
        let now = (self.clock)();
        if s.stream.is_none() || now - s.road_sample_time < 0.2 {
            return;
        }
        s.road_sample_time = now;
        let data = dict([
            ("state", packet.state.as_str().into()),
            (
                "proximity",
                packet
                    .proximity
                    .as_ref()
                    .map(|p| p.to_value())
                    .unwrap_or(Value::Null),
            ),
            ("speedLimit", packet.speed_limit.into()),
            (
                "signal",
                packet
                    .signal
                    .as_ref()
                    .map(|p| p.to_value())
                    .unwrap_or(Value::Null),
            ),
            ("audio", audio_state),
        ]);
        self.record_locked(&mut s, &dict_of([("kind", "road".into()), ("data", data)]));
    }

    /// Record one event dict (the decoded assistant datagram, or one built here).
    pub fn record(&self, event: &Dict) {
        let mut s = self.state.lock();
        self.record_locked(&mut s, event);
    }

    fn record_locked(&self, s: &mut State, event: &Dict) {
        if s.stream.is_none() {
            return;
        }
        s.sequence += 1;
        let mut row = event.clone();
        row.insert("seq".into(), Value::Int(s.sequence));
        row.insert(
            "t".into(),
            Value::Float(py_round((self.clock)() - s.started, 3)),
        );
        row.shift_remove("token");
        if event.get("kind").is_some_and(|k| k.eq_str("diagnostic")) {
            s.latest = row.get("data").cloned().unwrap_or(Value::Null);
            s.latest_time = (self.clock)();
        }
        let line = pyjson::dumps(&Value::Dict(row.clone()), &ROW_OPTS);
        s.rows.push_back(row);
        while s.rows.len() > ROW_LIMIT {
            s.rows.pop_front();
        }
        let result = match line {
            Ok(text) => {
                let stream = s.stream.as_mut().expect("checked above");
                stream
                    .write_all(format!("{text}\n").as_bytes())
                    .and_then(|_| stream.flush())
            }
            Err(e) => Err(std::io::Error::other(e.to_string())),
        };
        if let Err(exc) = result {
            // Disk trouble must never delay control release or its audio cue.
            s.error = Some(exc.to_string());
            s.stream = None;
        }
    }

    fn status_locked(&self, s: &State) -> Value {
        dict([
            ("recording", s.stream.is_some().into()),
            ("session", s.path.as_deref().map(name_of).into()),
            ("last_seq", s.sequence.into()),
            ("latest", s.latest.clone()),
            (
                "sample_age_s",
                if s.latest_time != 0.0 {
                    Value::Float(py_round((self.clock)() - s.latest_time, 3))
                } else {
                    Value::Null
                },
            ),
            ("error", s.error.clone().into()),
        ])
    }

    pub fn status(&self) -> Value {
        let s = self.state.lock();
        self.status_locked(&s)
    }

    /// Arm a session. `label` falsy means `"driving-assistant"`.
    pub fn start(&self, label: Option<&str>) -> Result<Value, PyErr> {
        let mut s = self.state.lock();
        if s.stream.is_some() {
            return Ok(self.status_locked(&s));
        }
        std::fs::create_dir_all(&self.directory)?;
        let label = label
            .filter(|l| !l.is_empty())
            .unwrap_or("driving-assistant");
        let label = pyjson::take_chars(
            &pyjson::sub_runs(label, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            60,
        );
        let stamp = (self.utc_now)().strftime("%Y%m%dT%H%M%S-%fZ");
        let path = self.directory.join(format!("{stamp}-{label}.ndjson"));
        s.path = Some(path.clone());
        s.stream = Some(Box::new(pyfs::create_new_text(&path)?));
        s.started = (self.clock)();
        s.rows.clear();
        s.latest = Value::Null;
        s.latest_time = 0.0;
        s.error = None;
        s.road_sample_time = f64::NEG_INFINITY;
        self.record_locked(
            &mut s,
            &dict_of([("kind", "start".into()), ("schema", 1.into())]),
        );
        Ok(self.status_locked(&s))
    }

    pub fn stop(&self) -> Value {
        let mut s = self.state.lock();
        if s.stream.is_some() {
            self.record_locked(&mut s, &dict_of([("kind", "stop".into())]));
            s.stream = None;
        }
        self.status_locked(&s)
    }

    /// Rows after `since_seq`, at most `limit` (clamped to 1..=500). Both arguments
    /// go through Python's `int()`, `since_seq` after `or 0`.
    pub fn read(&self, since_seq: &Value, limit: &Value) -> Result<Value, PyErr> {
        let s = self.state.lock();
        let since = if since_seq.truthy() {
            since_seq.py_int()?
        } else {
            0
        };
        let seq_of = |r: &Dict| match r.get("seq") {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let rows: Vec<&Dict> = s.rows.iter().filter(|r| seq_of(r) > since).collect();
        let limit = limit.py_int()?.clamp(i64::MIN, 500).max(1) as usize;
        let selected: Vec<Value> = rows
            .iter()
            .take(limit)
            .map(|r| Value::Dict((*r).clone()))
            .collect();
        let next_seq = match selected.last() {
            Some(Value::Dict(d)) => seq_of(d),
            _ => since,
        };
        let truncated = match s.rows.front() {
            Some(first) if since < seq_of(first) - 1 => Value::Int(seq_of(first)),
            _ => Value::Null,
        };
        Ok(dict([
            ("records", Value::List(selected)),
            ("next_seq", next_seq.into()),
            ("truncated_before", truncated),
        ]))
    }

    /// Summarise a session: the current or newest one, or a bare `.ndjson` name.
    pub fn review(&self, session: Option<&str>) -> Result<Value, PyErr> {
        let s = self.state.lock();
        let path = match session {
            None => match &s.path {
                Some(p) => Some(p.clone()),
                None => glob_sorted(&self.directory, "", ".ndjson").pop(),
            },
            Some(name) => {
                if !pyfs::is_bare_name(name) || !name.ends_with(".ndjson") {
                    return Err(PyErr::ValueError(
                        "session must be a bare .ndjson file name".to_string(),
                    ));
                }
                Some(self.directory.join(name))
            }
        };
        let Some(path) = path else {
            return Ok(dict([
                ("session", Value::Null),
                ("samples", 0.into()),
                ("events", Value::List(Vec::new())),
            ]));
        };
        let (mut samples, mut road_samples, mut automatic) = (0i64, 0i64, 0i64);
        let mut events: Vec<Value> = Vec::new();
        let mut previous_choice: Option<(Value, Value)> = None;
        for line in read_text_lines(&path)? {
            let Ok(row) = pyjson::loads(&line) else {
                continue; // tolerate an interrupted final write
            };
            let kind = row.py_get("kind")?.cloned().unwrap_or(Value::Null);
            if kind.eq_str("diagnostic") {
                samples += 1;
                let vm = get_or_empty(row.get("data"), "vehicle")?;
                let vm = vm.filter(|v| v.truthy());
                let locked = get_or_empty(vm, "locked")?.cloned().unwrap_or(Value::Null);
                let chosen = get_or_empty(vm, "chosen")?.cloned().unwrap_or(Value::Null);
                let automatic_flag = get_or_empty(vm, "automatic")?.is_some_and(Value::truthy);
                let changed = match &previous_choice {
                    None => true,
                    Some((l, c)) => !(py_eq(l, &locked) && py_eq(c, &chosen)),
                };
                if changed && automatic_flag && locked.truthy() {
                    automatic += 1;
                }
                previous_choice = Some((locked, chosen));
            } else if kind.eq_str("road") {
                road_samples += 1;
            } else if !kind.in_str_set(&["alive", "route", "start", "stop"])? {
                events.push(row);
            }
        }
        let event_count = events.len();
        let tail = events.split_off(event_count.saturating_sub(200));
        Ok(dict([
            ("session", name_of(&path).into()),
            ("samples", samples.into()),
            ("road_samples", road_samples.into()),
            ("automatic_choices_observed", automatic.into()),
            ("events", Value::List(tail)),
            ("event_count", event_count.into()),
        ]))
    }

    /// The MCP entry point.
    pub fn control(
        &self,
        action: &str,
        label: Option<&str>,
        session: Option<&str>,
        note: &Value,
        limit: &Value,
        since_seq: &Value,
    ) -> Result<Value, PyErr> {
        match action {
            "start" => self.start(label),
            "stop" => Ok(self.stop()),
            "read" => self.read(since_seq, limit),
            "review" => self.review(session),
            "list" => {
                let n = limit.py_int()?.clamp(i64::MIN, 100).max(1) as usize;
                let mut paths = glob_sorted(&self.directory, "", ".ndjson");
                paths.reverse();
                Ok(dict([(
                    "sessions",
                    Value::List(
                        paths
                            .iter()
                            .take(n)
                            .map(|p| Value::from(name_of(p)))
                            .collect(),
                    ),
                )]))
            }
            "mark" => {
                if self.state.lock().stream.is_none() {
                    return Err(PyErr::ValueError(
                        "no assistant recording is active".to_string(),
                    ));
                }
                let note = if note.truthy() {
                    note.py_str()
                } else {
                    String::new()
                };
                self.record(&dict_of([("kind", "marker".into()), ("note", note.into())]));
                Ok(self.status())
            }
            "status" => Ok(self.status()),
            _ => Err(PyErr::ValueError(
                "action must be start, stop, status, read, mark, review, or list".to_string(),
            )),
        }
    }

    /// The session file being written, if any (`str(self.path)`).
    pub fn path(&self) -> Option<String> {
        self.state.lock().path.as_deref().map(path_str)
    }
}
