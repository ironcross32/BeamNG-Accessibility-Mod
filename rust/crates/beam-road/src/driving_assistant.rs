//! Confirmed assistant state and session heartbeat, from driving_assistant.py. No
//! steering runs here: the vehicle Lua drives, and this side only tracks which session
//! is live, speaks what it reports, and keeps the session alive.
//!
//! The UDP send, the speech call, the cue player, the clock and the arrival hook are
//! all injected, so the whole protocol runs in tests without a socket.

use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::ReentrantMutex;

use crate::assistant_diagnostics::AssistantDiagnosticRecorder;
use crate::assistant_route::{RouteCallouts, arrival_phrase};
use crate::pyjson::{Dict, PyErr, Value, dict, dict_of, loads};
use crate::pytime;

/// The keyword arguments the Python passes to `speak`. `interrupt: None` means the
/// call left it to the speech layer's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakOpts {
    pub interrupt: Option<bool>,
    pub exclude_from_buffer: bool,
}

pub type SendFn = dyn Fn(&str) + Send + Sync;
pub type SpeakFn = dyn Fn(&str, SpeakOpts) + Send + Sync;
/// `cue(kind, choices=())`.
pub type CueFn = dyn Fn(&str, &[String]) + Send + Sync;
pub type ArrivalFn = dyn Fn(&Value) + Send + Sync;

/// `snapshot()`.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantSnapshot {
    pub active: bool,
    pub pending: bool,
    pub following_route: bool,
    pub vehicle: Value,
    pub sequence: i64,
    pub response_age_s: Option<f64>,
}

impl AssistantSnapshot {
    /// The dict Python returns, in its key order.
    pub fn to_value(&self) -> Value {
        dict([
            ("active", self.active.into()),
            ("pending", self.pending.into()),
            ("following_route", self.following_route.into()),
            ("vehicle", self.vehicle.clone()),
            ("sequence", self.sequence.into()),
            ("response_age_s", self.response_age_s.into()),
        ])
    }
}

struct State {
    callouts: RouteCallouts,
    arrived_at: Option<f64>,
    token: Option<String>,
    active: bool,
    vehicle: Value,
    sequence: i64,
    last_response: f64,
    status_token: Option<String>,
    cue: Arc<CueFn>,
    diagnostics: Option<Arc<AssistantDiagnosticRecorder>>,
}

pub struct DrivingAssistant {
    send: Box<SendFn>,
    speak: Box<SpeakFn>,
    units: Arc<dyn Fn() -> String + Send + Sync>,
    on_arrival: Option<Box<ArrivalFn>>,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    /// Python's RLock: the callbacks run under it and may call back in.
    lock: ReentrantMutex<RefCell<State>>,
}

/// `uuid.uuid4().hex`.
pub fn uuid4_hex() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0u8; 16];
    for (n, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_usize(n);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `token == self.token` for a decoded token and a stored one.
fn token_eq(v: &Value, stored: &Option<String>) -> bool {
    match (v, stored) {
        (Value::Null, None) => true,
        (Value::Str(a), Some(b)) => a == b,
        _ => false,
    }
}

impl DrivingAssistant {
    /// `DrivingAssistant(send, speak, units=..., on_arrival=...)` on `time.monotonic()`.
    /// `units` of `None` is always imperial; the cue does nothing until one is set.
    pub fn new(
        send: impl Fn(&str) + Send + Sync + 'static,
        speak: impl Fn(&str, SpeakOpts) + Send + Sync + 'static,
        units: Option<Arc<dyn Fn() -> String + Send + Sync>>,
        on_arrival: Option<Box<ArrivalFn>>,
    ) -> Self {
        Self::with_clock(send, speak, units, on_arrival, pytime::monotonic)
    }

    pub fn with_clock(
        send: impl Fn(&str) + Send + Sync + 'static,
        speak: impl Fn(&str, SpeakOpts) + Send + Sync + 'static,
        units: Option<Arc<dyn Fn() -> String + Send + Sync>>,
        on_arrival: Option<Box<ArrivalFn>>,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        let units = units.unwrap_or_else(|| Arc::new(|| "imperial".to_string()));
        let for_callouts = units.clone();
        DrivingAssistant {
            send: Box::new(send),
            speak: Box::new(speak),
            units,
            on_arrival,
            clock: Box::new(clock),
            lock: ReentrantMutex::new(RefCell::new(State {
                callouts: RouteCallouts::new(move || for_callouts()),
                arrived_at: None,
                token: None,
                active: false,
                vehicle: Value::Null,
                sequence: -1,
                last_response: 0.0,
                status_token: None,
                cue: Arc::new(|_: &str, _: &[String]| {}),
                diagnostics: None,
            })),
        }
    }

    pub fn set_cue(&self, cue: Arc<CueFn>) {
        self.lock.lock().borrow_mut().cue = cue;
    }

    pub fn set_diagnostics(&self, diagnostics: Option<Arc<AssistantDiagnosticRecorder>>) {
        self.lock.lock().borrow_mut().diagnostics = diagnostics;
    }

    pub fn active(&self) -> bool {
        self.lock.lock().borrow().active
    }

    /// `driving_assistant.active = value`, as the Python fixtures set it directly.
    #[doc(hidden)]
    pub fn set_active_for_tests(&self, active: bool) {
        self.lock.lock().borrow_mut().active = active;
    }

    pub fn token(&self) -> Option<String> {
        self.lock.lock().borrow().token.clone()
    }

    pub fn status_token(&self) -> Option<String> {
        self.lock.lock().borrow().status_token.clone()
    }

    pub fn vehicle(&self) -> Value {
        self.lock.lock().borrow().vehicle.clone()
    }

    pub fn sequence(&self) -> i64 {
        self.lock.lock().borrow().sequence
    }

    pub fn last_response(&self) -> f64 {
        self.lock.lock().borrow().last_response
    }

    pub fn set_last_response(&self, value: f64) {
        self.lock.lock().borrow_mut().last_response = value;
    }

    pub fn snapshot(&self) -> AssistantSnapshot {
        let g = self.lock.lock();
        let s = g.borrow();
        AssistantSnapshot {
            active: s.active,
            pending: s.token.as_deref().is_some_and(|t| !t.is_empty()) && !s.active,
            following_route: s.callouts.following,
            vehicle: s.vehicle.clone(),
            sequence: s.sequence,
            response_age_s: if s.last_response != 0.0 {
                Some((self.clock)() - s.last_response)
            } else {
                None
            },
        }
    }

    fn record(&self, s: &RefCell<State>, value: &Dict) {
        let diagnostics = s.borrow().diagnostics.clone();
        if let Some(d) = diagnostics {
            d.record(value);
        }
    }

    fn cue(&self, s: &RefCell<State>, kind: &str, choices: &[String]) {
        let cue = s.borrow().cue.clone();
        cue(kind, choices);
    }

    pub fn status(&self) {
        let g = self.lock.lock();
        let token = uuid4_hex();
        g.borrow_mut().status_token = Some(token.clone());
        (self.send)(&format!("ASSISTANT_STATUS:{token}"));
    }

    pub fn toggle(&self) {
        let g = self.lock.lock();
        let current = g.borrow().token.clone().filter(|t| !t.is_empty());
        if let Some(token) = current {
            (self.send)(&format!("ASSISTANT_OFF:{token}"));
            return;
        }
        let token = uuid4_hex();
        {
            let mut s = g.borrow_mut();
            s.token = Some(token.clone());
            s.sequence = -1;
            s.last_response = (self.clock)();
            s.callouts.reset();
        }
        (self.send)(&format!("ASSISTANT_TOGGLE:{token}"));
    }

    pub fn seconds_since_arrival(&self) -> Option<f64> {
        let g = self.lock.lock();
        let arrived = g.borrow().arrived_at;
        arrived.map(|t| (self.clock)() - t)
    }

    fn route_report(&self, s: &RefCell<State>, kind: &str, value: &Dict) -> Result<(), PyErr> {
        let Some(data @ Value::Dict(_)) = value.get("data") else {
            return Ok(());
        };
        let result = {
            let mut st = s.borrow_mut();
            if kind == "route_set" {
                st.callouts.route_set(data).map(Some)
            } else {
                st.callouts.update(data)
            }
        };
        let phrase = match result {
            Ok(p) => p,
            Err(PyErr::TypeError(_)) | Err(PyErr::ValueError(_)) => return Ok(()),
            Err(other) => return Err(other),
        };
        if let Some(phrase) = phrase.filter(|p| !p.is_empty()) {
            (self.speak)(
                &phrase,
                SpeakOpts {
                    interrupt: Some(false),
                    exclude_from_buffer: true,
                },
            );
        }
        Ok(())
    }

    /// The arrival sentence, in the player's units, for an off event that carries one.
    fn arrival(&self, s: &RefCell<State>, value: &Dict, message: String) -> Result<String, PyErr> {
        let arrival = match value.get("data") {
            Some(Value::Dict(d)) => d.get("arrival"),
            _ => None,
        };
        let Some(arrival @ Value::Dict(_)) = arrival else {
            return Ok(message);
        };
        s.borrow_mut().arrived_at = Some((self.clock)());
        if let Some(cb) = &self.on_arrival {
            cb(arrival);
        }
        arrival_phrase(arrival, &(self.units)())
    }

    /// Handle one `ASSISTANT:` datagram body.
    ///
    /// Malformed JSON and the shapes Python's `except (ValueError, TypeError,
    /// KeyError, OverflowError)` swallows are ignored. `Err` is what Python would let
    /// escape: an unhashable `kind`, or an `AttributeError`/`TypeError` from a route
    /// or arrival report of the wrong shape.
    pub fn receive(&self, payload: &str) -> Result<(), PyErr> {
        let Ok(Value::Dict(value)) = loads(payload) else {
            return Ok(());
        };
        let (Some(token), Some(kind)) = (value.get("token"), value.get("kind")) else {
            return Ok(());
        };
        let sequence = match value.get("sequence") {
            None => -1,
            Some(v) => match v.py_int() {
                Ok(i) => i,
                Err(_) => return Ok(()),
            },
        };
        let mut message = match value.get("message") {
            None => String::new(),
            Some(Value::Str(m)) => m.clone(),
            Some(_) => return Ok(()),
        };

        let g = self.lock.lock();
        let s: &RefCell<State> = &g;
        {
            let st = s.borrow();
            let status_token_set = st.status_token.as_deref().is_some_and(|t| !t.is_empty());
            if kind.eq_str("status") && token_eq(token, &st.status_token) && status_token_set {
                drop(st);
                s.borrow_mut().status_token = None;
                (self.speak)(
                    &message,
                    SpeakOpts {
                        interrupt: None,
                        exclude_from_buffer: true,
                    },
                );
                return Ok(());
            }
            let token_set = st.token.as_deref().is_some_and(|t| !t.is_empty());
            if !token_eq(token, &st.token) || !token_set || sequence <= st.sequence {
                return Ok(());
            }
        }
        {
            let mut st = s.borrow_mut();
            st.sequence = sequence;
            st.last_response = (self.clock)();
        }
        self.record(s, &value);
        let kind_str = kind.as_str().unwrap_or("");
        if kind.eq_str("on") {
            let mut st = s.borrow_mut();
            st.active = true;
            st.vehicle = value.get("vehicle").cloned().unwrap_or(Value::Null);
        } else if kind.eq_str("off") {
            {
                let mut st = s.borrow_mut();
                st.active = false;
                st.token = None;
                st.vehicle = Value::Null;
                st.callouts.reset();
            }
            message = self.arrival(s, &value, message)?;
            self.cue(s, "off", &[]);
        } else if kind.in_str_set(&["route", "route_set"])? {
            return self.route_report(s, kind_str, &value);
        } else if kind.eq_str("route_clear") {
            s.borrow_mut().callouts.reset();
        } else if kind.eq_str("junction") {
            let parts: Vec<&str> = message.split(',').collect();
            let choices: Vec<String> = ["left", "straight", "right"]
                .iter()
                .filter(|name| parts.contains(name))
                .map(|name| name.to_string())
                .collect();
            self.cue(s, "junction", &choices);
            return Ok(());
        } else if kind.in_str_set(&["diagnostic", "choice"])? {
            return Ok(());
        } else if kind.eq_str("error") {
            // Raw Lua text for the log only; the following off event carries the
            // reason the player hears.
            let detail = match value.get("data") {
                Some(Value::Dict(d)) => d.get("detail").map(Value::py_str).unwrap_or_default(),
                _ => String::new(),
            };
            log::error!(target: "bnvdahook", "Driving assistant fault: {detail}");
            return Ok(());
        } else if !kind.in_str_set(&["say", "slow", "status", "alive"])? {
            return Ok(());
        }
        if kind.eq_str("alive")
            && matches!(value.get("active"), Some(Value::Bool(true)))
            && !s.borrow().active
        {
            {
                let mut st = s.borrow_mut();
                st.active = true;
                st.vehicle = value.get("vehicle").cloned().unwrap_or(Value::Null);
            }
            (self.speak)(
                "Driving assistant on",
                SpeakOpts {
                    interrupt: None,
                    exclude_from_buffer: true,
                },
            );
        }
        if kind.eq_str("say")
            && let Some(Value::Dict(data)) = value.get("data")
            && data.get("protect").is_some_and(Value::truthy)
        {
            // An instruction to stop or change gear: nothing background may talk over it.
            self.cue(s, "protect", &[]);
        }
        if !message.is_empty() && !kind.eq_str("alive") {
            (self.speak)(
                &message,
                SpeakOpts {
                    interrupt: Some(!kind.eq_str("slow")),
                    exclude_from_buffer: true,
                },
            );
        }
        Ok(())
    }

    pub fn heartbeat(&self) {
        let g = self.lock.lock();
        let s: &RefCell<State> = &g;
        let Some(token) = s.borrow().token.clone().filter(|t| !t.is_empty()) else {
            return;
        };
        if (self.clock)() - s.borrow().last_response > 5.0 {
            (self.send)(&format!("ASSISTANT_OFF:{token}"));
            {
                let mut st = s.borrow_mut();
                st.active = false;
                st.token = None;
                st.vehicle = Value::Null;
                st.callouts.reset();
            }
            let message = "Driving assistant connection lost. Steering assistance off";
            self.record(
                s,
                &dict_of([
                    ("kind", "off".into()),
                    ("message", message.into()),
                    ("origin", "receiver_timeout".into()),
                ]),
            );
            self.cue(s, "off", &[]);
            (self.speak)(
                message,
                SpeakOpts {
                    interrupt: None,
                    exclude_from_buffer: true,
                },
            );
        } else {
            (self.send)(&format!("ASSISTANT_HEARTBEAT:{token}"));
        }
    }

    /// The heartbeat loop. `wait(timeout)` is `stop_event.wait(timeout)`: it returns
    /// true once the application is stopping.
    pub fn run(&self, wait: impl Fn(Duration) -> bool) {
        // A new receiver session never inherits an old assistant activation.
        (self.send)("ASSISTANT_OFF");
        while !wait(Duration::from_millis(500)) {
            self.heartbeat();
        }
        (self.send)("ASSISTANT_OFF");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shareable_across_threads() {
        fn is<T: Send + Sync>() {}
        is::<super::DrivingAssistant>();
    }
}
