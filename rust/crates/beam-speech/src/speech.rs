//! Speech backend selection, re-acquire, voices, braille, from speech.py.
//!
//! Everything Prism-specific lives here. The rest of the app calls the free functions at
//! the bottom of this file, which never panic: speech failing must never take down a
//! telemetry callout or a keyboard handler.
//!
//! `migrate_config` (the pre-Prism SAPI keys) is `beam_base::speech_config::migrate_config`,
//! called from config loading; it is not part of this module.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use beam_base::pyfmt::{py_float_parse, py_float_str, py_strip};
use parking_lot::Mutex;
use serde_json::Value;

use crate::prism::{self, AvailabilityFn, BackendFeatures, BackendId, LogLevel, PrismErr};

/// "auto" means let Prism pick by backend priority (NVDA outranks JAWS outranks SAPI, and
/// so on). Anything else is matched against a Prism backend name.
pub const AUTO_BACKEND: &str = "auto";

/// A failed init used to latch permanently, which meant that starting beamtel before the
/// screen reader left the session mute until it was restarted. Retry, but not on every
/// utterance: probing every backend is slow enough that doing it per callout would be
/// audible.
pub const RETRY_COOLDOWN_S: f64 = 10.0;

/// The Python logger Prism's own log is bridged under (`prism.install_logging`).
pub const PRISM_LOGGER_NAME: &str = "bnvdahook.prism";

// ---------------------------------------------------------------------------
// The seam: what speech needs from Prism, so tests can fake it
// ---------------------------------------------------------------------------

/// The library (`import prism`).
pub trait SpeechLib: Send + Sync {
    fn install_logging(&self, logger_name: &str) -> Result<(), PrismErr>;
    fn set_log_level(&self, level: LogLevel) -> Result<(), PrismErr>;
    /// `prism.Context(on_availability=...)`.
    fn make_context(
        &self,
        on_availability: AvailabilityFn,
    ) -> Result<Arc<dyn SpeechContext>, PrismErr>;
}

/// `prism.Context`.
pub trait SpeechContext: Send + Sync {
    fn backends_count(&self) -> usize;
    /// `id_of(int)`.
    fn id_at(&self, index: usize) -> Result<BackendId, PrismErr>;
    /// `id_of(str)`.
    fn id_of_name(&self, name: &str) -> Result<BackendId, PrismErr>;
    fn name_of(&self, id: BackendId) -> Result<String, PrismErr>;
    fn acquire(&self, id: BackendId) -> Result<Arc<dyn SpeechBackend>, PrismErr>;
    fn acquire_best(&self) -> Result<Arc<dyn SpeechBackend>, PrismErr>;
}

/// `prism.Backend`, the calls speech.py makes.
pub trait SpeechBackend: Send + Sync {
    fn name(&self) -> Result<String, PrismErr>;
    fn features(&self) -> BackendFeatures;
    fn speak(&self, text: &str, interrupt: bool) -> Result<(), PrismErr>;
    fn stop(&self) -> Result<(), PrismErr>;
    fn braille(&self, text: &str) -> Result<(), PrismErr>;
    fn output(&self, text: &str, interrupt: bool) -> Result<(), PrismErr>;
    fn refresh_voices(&self) -> Result<(), PrismErr>;
    fn voices_count(&self) -> Result<usize, PrismErr>;
    fn get_voice_name(&self, idx: usize) -> Result<String, PrismErr>;
    fn set_voice(&self, idx: usize) -> Result<(), PrismErr>;
    fn set_rate(&self, value: f32) -> Result<(), PrismErr>;
    fn set_volume(&self, value: f32) -> Result<(), PrismErr>;
}

struct RealLib;
struct RealContext(Arc<prism::Context>);

impl SpeechLib for RealLib {
    fn install_logging(&self, logger_name: &str) -> Result<(), PrismErr> {
        prism::install_logging(logger_name);
        Ok(())
    }
    fn set_log_level(&self, level: LogLevel) -> Result<(), PrismErr> {
        prism::set_log_level(level);
        Ok(())
    }
    fn make_context(
        &self,
        on_availability: AvailabilityFn,
    ) -> Result<Arc<dyn SpeechContext>, PrismErr> {
        let ctx = prism::Context::new(Some(on_availability))?;
        Ok(Arc::new(RealContext(ctx)))
    }
}

impl SpeechContext for RealContext {
    fn backends_count(&self) -> usize {
        self.0.backends_count()
    }
    fn id_at(&self, index: usize) -> Result<BackendId, PrismErr> {
        self.0.id_at(index)
    }
    fn id_of_name(&self, name: &str) -> Result<BackendId, PrismErr> {
        self.0.id_of_name(name)
    }
    fn name_of(&self, id: BackendId) -> Result<String, PrismErr> {
        self.0.name_of(id)
    }
    fn acquire(&self, id: BackendId) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
        Ok(Arc::new(self.0.acquire(id)?))
    }
    fn acquire_best(&self) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
        Ok(Arc::new(self.0.acquire_best()?))
    }
}

impl SpeechBackend for prism::Backend {
    fn name(&self) -> Result<String, PrismErr> {
        prism::Backend::name(self)
    }
    fn features(&self) -> BackendFeatures {
        prism::Backend::features(self)
    }
    fn speak(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        prism::Backend::speak(self, text, interrupt)
    }
    fn stop(&self) -> Result<(), PrismErr> {
        prism::Backend::stop(self)
    }
    fn braille(&self, text: &str) -> Result<(), PrismErr> {
        prism::Backend::braille(self, text)
    }
    fn output(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        prism::Backend::output(self, text, interrupt)
    }
    fn refresh_voices(&self) -> Result<(), PrismErr> {
        prism::Backend::refresh_voices(self)
    }
    fn voices_count(&self) -> Result<usize, PrismErr> {
        prism::Backend::voices_count(self)
    }
    fn get_voice_name(&self, idx: usize) -> Result<String, PrismErr> {
        prism::Backend::get_voice_name(self, idx)
    }
    fn set_voice(&self, idx: usize) -> Result<(), PrismErr> {
        prism::Backend::set_voice(self, idx)
    }
    fn set_rate(&self, value: f32) -> Result<(), PrismErr> {
        prism::Backend::set_rate(self, value)
    }
    fn set_volume(&self, value: f32) -> Result<(), PrismErr> {
        prism::Backend::set_volume(self, value)
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// The speech keys pulled out of a full beamtel config (`_read_config`). A field is
/// `None` where the Python dict has no key; `_config` starts out as `{}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpeechConfig {
    pub backend: Option<String>,
    pub voice_name: Option<String>,
    /// Raw config value; `None` for a missing key or JSON null.
    pub rate: Option<Value>,
    pub volume: Option<Value>,
}

/// Python truthiness of a JSON value.
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(value)` for a JSON value as Python's `json` would have loaded it.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                py_float_str(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", prism::py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => prism::py_repr_str(s),
        other => py_str(other),
    }
}

/// `str(cfg.get(key, default) or default)`.
fn str_or(cfg: &serde_json::Map<String, Value>, key: &str, default: &str) -> String {
    match cfg.get(key) {
        Some(v) if py_truthy(v) => py_str(v),
        _ => default.to_string(),
    }
}

/// `_read_config`: pull the speech keys out of a full beamtel config.
pub fn read_config(cfg: &Value) -> SpeechConfig {
    let Some(map) = cfg.as_object() else {
        return SpeechConfig::default();
    };
    let raw = |k: &str| map.get(k).filter(|v| !v.is_null()).cloned();
    SpeechConfig {
        backend: Some(str_or(map, "speech_backend", AUTO_BACKEND)),
        voice_name: Some(str_or(map, "speech_voice_name", "")),
        rate: raw("speech_rate"),
        volume: raw("speech_volume"),
    }
}

/// `float(value)`; the error text is what Python's would be.
fn py_float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| "int too large to convert to float".to_string()),
        Value::String(s) => py_float_parse(s).ok_or_else(|| {
            format!(
                "could not convert string to float: {}",
                prism::py_repr_str(s)
            )
        }),
        Value::Null => {
            Err("float() argument must be a string or a real number, not 'NoneType'".into())
        }
        Value::Array(_) => {
            Err("float() argument must be a string or a real number, not 'list'".into())
        }
        Value::Object(_) => {
            Err("float() argument must be a string or a real number, not 'dict'".into())
        }
    }
}

/// `max(0.0, min(1.0, x))` with Python's argument-order semantics (NaN gives 1.0).
pub fn clamp_unit(x: f64) -> f64 {
    let m = if x < 1.0 { x } else { 1.0 };
    if m > 0.0 { m } else { 0.0 }
}

/// Config stores 0-100 for the UI; Prism normalises to 0.0-1.0.
fn to_unit(v: &Value) -> Result<f32, String> {
    py_float(v).map(|f| clamp_unit(f / 100.0) as f32)
}

// ---------------------------------------------------------------------------
// The module state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Inner {
    ctx: Option<Arc<dyn SpeechContext>>,
    backend: Option<Arc<dyn SpeechBackend>>,
    config: SpeechConfig,
    next_retry_at: f64,
}

/// What `_call` runs on the backend.
#[derive(Clone, Copy)]
enum Action<'a> {
    Speak(&'a str, bool),
    Stop,
    Braille(&'a str),
    Output(&'a str, bool),
}

impl Action<'_> {
    fn name(&self) -> &'static str {
        match self {
            Action::Speak(..) => "speak",
            Action::Stop => "stop",
            Action::Braille(_) => "braille",
            Action::Output(..) => "output",
        }
    }

    fn run(&self, b: &dyn SpeechBackend) -> Result<(), PrismErr> {
        match *self {
            Action::Speak(t, i) => b.speak(t, i),
            Action::Stop => b.stop(),
            Action::Braille(t) => b.braille(t),
            Action::Output(t, i) => b.output(t, i),
        }
    }
}

/// speech.py's module state. The app uses the one process-wide instance behind the free
/// functions below; tests build their own over a fake [`SpeechLib`].
pub struct Speech {
    /// `None`: no speech library. The app always has one (Prism is linked in); tests use
    /// `None` to check that every call degrades quietly.
    lib: Option<Arc<dyn SpeechLib>>,
    /// `_lock`. Construction is serialised: two threads racing would each build a Context.
    inner: Mutex<Inner>,
    /// `_stale`: set from Prism's availability thread when a backend appears or
    /// disappears; consumed on the next call so Prism is never re-entered from its own
    /// callback.
    stale: Arc<AtomicBool>,
    /// `_test_backend`: the Backend must outlive the test utterance.
    test_backend: Mutex<Option<Arc<dyn SpeechBackend>>>,
    /// `time.monotonic()`.
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
}

impl Speech {
    /// A speech layer over `lib` (`None`: no speech library), with `clock` standing in for
    /// `time.monotonic()`.
    pub fn new(
        lib: Option<Arc<dyn SpeechLib>>,
        clock: Box<dyn Fn() -> f64 + Send + Sync>,
    ) -> Speech {
        Speech {
            lib,
            inner: Mutex::new(Inner::default()),
            stale: Arc::new(AtomicBool::new(false)),
            test_backend: Mutex::new(None),
            clock,
        }
    }

    /// Whether there is a speech library at all.
    pub fn is_available(&self) -> bool {
        self.lib.is_some()
    }

    /// `_on_availability`: runs on Prism's thread. Do no work here beyond flagging.
    fn availability_callback(&self) -> AvailabilityFn {
        let stale = self.stale.clone();
        Arc::new(move |_id: BackendId, name: &str, available: bool| {
            log::info!(
                "Speech backend {} {}",
                name,
                if available {
                    "became available"
                } else {
                    "went away"
                }
            );
            stale.store(true, Ordering::SeqCst);
        })
    }

    fn make_context(&self, lib: &Arc<dyn SpeechLib>) -> Result<Arc<dyn SpeechContext>, PrismErr> {
        // ERROR, not WARN: probing for absent screen readers logs benign warnings
        // ("delay-load dummy handle pool exhausted") on every launch, which would look
        // like a fault in bnvdahook.log.
        let bridge = lib
            .install_logging(PRISM_LOGGER_NAME)
            .and_then(|_| lib.set_log_level(LogLevel::Error));
        if let Err(e) = bridge {
            log::debug!("Could not install Prism log bridge: {e}");
        }
        lib.make_context(self.availability_callback())
    }

    /// `_shared_context`: the one Context for this process, created on demand. Everything
    /// goes through it, enumeration included: the configurator panel lives in the same
    /// process as the speaking engine, and a throwaway context would shut Prism down
    /// under it.
    fn shared_context(&self) -> Option<Arc<dyn SpeechContext>> {
        let lib = self.lib.as_ref()?;
        let mut inner = self.inner.lock();
        if inner.ctx.is_none() {
            match self.make_context(lib) {
                Ok(c) => inner.ctx = Some(c),
                Err(e) => {
                    log::warn!("Could not create Prism context: {e}");
                    return None;
                }
            }
        }
        inner.ctx.clone()
    }

    /// `_build_locked`: construct context and backend. Returns whether a backend exists.
    fn build_locked(&self, inner: &mut Inner) -> bool {
        let Some(lib) = self.lib.as_ref() else {
            return false;
        };
        let now = (self.clock)();
        if inner.backend.is_none() && now < inner.next_retry_at {
            return false;
        }

        let attempt = (|| -> Result<(), PrismErr> {
            if inner.ctx.is_none() {
                inner.ctx = Some(self.make_context(lib)?);
            }
            let ctx = inner.ctx.clone().expect("set above");
            let name = inner
                .config
                .backend
                .clone()
                .unwrap_or_else(|| AUTO_BACKEND.into());
            inner.backend = Some(acquire_backend(ctx.as_ref(), &name)?);
            self.stale.store(false, Ordering::SeqCst);
            Ok(())
        })();
        if let Err(e) = attempt {
            log::warn!("Failed to initialize speech: {e}");
            inner.backend = None;
            inner.next_retry_at = now + RETRY_COOLDOWN_S;
            return false;
        }

        apply_params_locked(inner);
        true
    }

    fn init_locked(&self, inner: &mut Inner, cfg: Option<&Value>) -> bool {
        if self.lib.is_none() {
            return false;
        }
        if let Some(cfg) = cfg {
            inner.config = read_config(cfg);
        }
        if inner.backend.is_some() && !self.stale.load(Ordering::SeqCst) {
            return true;
        }
        self.build_locked(inner)
    }

    /// `init(cfg=None)`: make sure the shared backend exists, building it at most once.
    /// Returns whether there is a backend (speech.py returns the backend or `None`, and
    /// every caller only tests it against `None`).
    ///
    /// Called from every thread that speaks, so construction is serialised. Creating the
    /// context and probing backends is slow enough to matter; callers on latency-sensitive
    /// paths warm it first (see `install_hotkeys`).
    pub fn init(&self, cfg: Option<&Value>) -> bool {
        if self.lib.is_none() {
            return false;
        }
        let mut inner = self.inner.lock();
        self.init_locked(&mut inner, cfg)
    }

    /// `configure(cfg)`: apply a new config, rebuilding the backend if the choice changed.
    pub fn configure(&self, cfg: &Value) {
        if self.lib.is_none() {
            return;
        }
        let mut inner = self.inner.lock();
        let previous = inner.config.backend.clone();
        inner.config = read_config(cfg);
        if inner.backend.is_some() && inner.config.backend != previous {
            self.build_locked(&mut inner);
        } else {
            apply_params_locked(&inner);
        }
    }

    /// `_call`: run a backend method, re-acquiring once if the backend has gone away.
    ///
    /// A screen reader can exit mid-session (BeamNG sessions run for hours), which leaves
    /// the handle pointing at a dead client. One retry against a freshly acquired backend
    /// covers both that and the reverse case where the reader started after we did.
    fn call(&self, action: Action<'_>) -> bool {
        let mut inner = self.inner.lock();
        if !self.init_locked(&mut inner, None) {
            return false;
        }
        let Some(backend) = inner.backend.clone() else {
            return false;
        };
        match action.run(backend.as_ref()) {
            Ok(()) => true,
            Err(e) => {
                log::debug!(
                    "Speech {} failed on {}: {e}",
                    action.name(),
                    backend_name_locked(&inner)
                );
                if !self.build_locked(&mut inner) {
                    return false;
                }
                let Some(fresh) = inner.backend.clone() else {
                    return false;
                };
                match action.run(fresh.as_ref()) {
                    Ok(()) => true,
                    Err(e2) => {
                        log::warn!("Speech {} failed after re-acquire: {e2}", action.name());
                        false
                    }
                }
            }
        }
    }

    /// `speak(text, interrupt=True)`. Prism defaults interrupt to False; BEAM's is True.
    pub fn speak(&self, text: &str, interrupt: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        self.call(Action::Speak(text, interrupt))
    }

    /// `stop()`: interrupt whatever is being spoken.
    pub fn stop(&self) -> bool {
        self.call(Action::Stop)
    }

    /// `braille(text)`: send to a connected braille display, if the backend supports it.
    pub fn braille(&self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if !self.supports("supports_braille") {
            return false;
        }
        self.call(Action::Braille(text))
    }

    /// `output(text, interrupt=True)`: speak and braille in one call.
    pub fn output(&self, text: &str, interrupt: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        self.call(Action::Output(text, interrupt))
    }

    /// `backend_name()`: `None` when there is no backend.
    pub fn backend_name(&self) -> Option<String> {
        let mut inner = self.inner.lock();
        if !self.init_locked(&mut inner, None) {
            return None;
        }
        Some(backend_name_locked(&inner))
    }

    /// `features()`: the active backend's capability bits, or `None`.
    pub fn features(&self) -> Option<BackendFeatures> {
        let mut inner = self.inner.lock();
        if !self.init_locked(&mut inner, None) {
            return None;
        }
        inner.backend.as_ref().map(|b| b.features())
    }

    /// `supports(feature_name)`, by the `BackendFeatures` attribute name.
    pub fn supports(&self, feature_name: &str) -> bool {
        self.features().is_some_and(|f| f.get(feature_name))
    }

    /// `describe_capabilities()`: one-line summary of the active backend, for the
    /// startup log.
    pub fn describe_capabilities(&self) -> String {
        let Some(feats) = self.features() else {
            return "speech unavailable".into();
        };
        let supported: Vec<&str> = [
            ("speak", feats.supports_speak()),
            ("braille", feats.supports_braille()),
            ("stop", feats.supports_stop()),
            ("rate", feats.supports_set_rate()),
            ("volume", feats.supports_set_volume()),
            ("voices", feats.supports_set_voice()),
            ("pcm", feats.supports_speak_to_memory()),
        ]
        .into_iter()
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| s)
        .collect();
        // f"{backend_name()}": None prints as "None".
        let name = self.backend_name().unwrap_or_else(|| "None".into());
        let list = if supported.is_empty() {
            "no capabilities reported".to_string()
        } else {
            supported.join(", ")
        };
        format!("{name} [{list}]")
    }

    // --- Enumeration helpers, used by the configurator ---

    /// `list_backends()`: names of every backend Prism knows about on this machine, in
    /// registry (priority) order.
    pub fn list_backends(&self) -> Vec<String> {
        let Some(ctx) = self.shared_context() else {
            return Vec::new();
        };
        let result: Result<Vec<String>, PrismErr> = (0..ctx.backends_count())
            .map(|i| ctx.id_at(i).and_then(|id| ctx.name_of(id)))
            .collect();
        match result {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Could not enumerate speech backends: {e}");
                Vec::new()
            }
        }
    }

    /// `list_voices(name)`: `(names, features)` for a backend, without disturbing the
    /// active one. Uses a fresh Backend so the configurator can probe SAPI while beamtel
    /// keeps speaking through NVDA. An unavailable named backend falls back to the best
    /// one, as `_acquire_backend` does.
    pub fn list_voices(
        &self,
        backend_name_or_auto: &str,
    ) -> (Vec<String>, Option<BackendFeatures>) {
        let Some(ctx) = self.shared_context() else {
            return (Vec::new(), None);
        };
        let attempt = || -> Result<(Vec<String>, BackendFeatures), PrismErr> {
            let backend = acquire_backend(ctx.as_ref(), backend_name_or_auto)?;
            let feats = backend.features();
            let mut names = Vec::new();
            if feats.supports_count_voices() {
                let _ = backend.refresh_voices();
                for i in 0..backend.voices_count()? {
                    names.push(backend.get_voice_name(i)?);
                }
            }
            Ok((names, feats))
        };
        match attempt() {
            Ok((names, feats)) => (names, Some(feats)),
            Err(e) => {
                log::warn!("Could not enumerate voices for {backend_name_or_auto}: {e}");
                (Vec::new(), None)
            }
        }
    }

    /// `test_speak(...)`: speak a test line on an arbitrary backend. Returns
    /// `(ok, error_message)`.
    pub fn test_speak(
        &self,
        backend_name_or_auto: &str,
        voice_name: &str,
        rate: Option<f64>,
        volume: Option<f64>,
        text: &str,
    ) -> (bool, Option<String>) {
        let Some(ctx) = self.shared_context() else {
            return (false, Some("No speech library is available.".into()));
        };
        let attempt = || -> Result<(), PrismErr> {
            let backend = acquire_backend(ctx.as_ref(), backend_name_or_auto)?;
            let feats = backend.features();
            if !voice_name.is_empty() && feats.supports_set_voice() {
                if let Some(idx) = find_voice_index(backend.as_ref(), voice_name) {
                    backend.set_voice(idx)?;
                }
            }
            if let Some(r) = rate {
                if feats.supports_set_rate() {
                    backend.set_rate(clamp_unit(r / 100.0) as f32)?;
                }
            }
            if let Some(v) = volume {
                if feats.supports_set_volume() {
                    backend.set_volume(clamp_unit(v / 100.0) as f32)?;
                }
            }
            backend.speak(text, true)?;
            // The Backend must outlive the utterance; speak() is asynchronous and freeing
            // the handle here would cut the test off mid-word.
            *self.test_backend.lock() = Some(backend);
            Ok(())
        };
        match attempt() {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        }
    }

    /// `shutdown()`: release the backend and context. Safe to call more than once.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock();
        inner.backend = None;
        inner.ctx = None;
    }
}

/// `_acquire_backend`: the configured backend, falling back to Prism's best choice.
fn acquire_backend(
    ctx: &dyn SpeechContext,
    name: &str,
) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
    if !name.is_empty() && name != AUTO_BACKEND {
        match ctx.id_of_name(name).and_then(|id| ctx.acquire(id)) {
            Ok(b) => return Ok(b),
            Err(e) => log::warn!(
                "Configured speech backend '{name}' unavailable ({e}); \
                 falling back to the best available one."
            ),
        }
    }
    ctx.acquire_best()
}

/// `_find_voice_index`: case-insensitive, whitespace-trimmed name match.
fn find_voice_index(backend: &dyn SpeechBackend, name: &str) -> Option<usize> {
    let want = py_strip(name).to_lowercase();
    if want.is_empty() {
        return None;
    }
    let _ = backend.refresh_voices();
    let count = backend.voices_count().ok()?;
    for i in 0..count {
        let n = backend.get_voice_name(i).ok()?;
        if py_strip(&n).to_lowercase() == want {
            return Some(i);
        }
    }
    None
}

/// `_apply_params_locked`: voice, rate and volume, skipping whatever this backend cannot
/// do. Screen reader backends own their own speech parameters (NVDA reports no set_rate),
/// so this is a no-op for most users and only bites for the software synths.
fn apply_params_locked(inner: &Inner) {
    let Some(backend) = inner.backend.as_ref() else {
        return;
    };
    let feats = backend.features();

    let voice_name = inner.config.voice_name.as_deref().unwrap_or("");
    if !voice_name.is_empty() && feats.supports_set_voice() {
        match find_voice_index(backend.as_ref(), voice_name) {
            None => log::info!(
                "Configured voice '{voice_name}' not found on {}",
                backend.name().unwrap_or_else(|_| "unknown".into())
            ),
            Some(idx) => {
                if let Err(e) = backend.set_voice(idx) {
                    log::debug!("Could not set voice: {e}");
                }
            }
        }
    }

    for (key, supported, value) in [
        ("rate", feats.supports_set_rate(), &inner.config.rate),
        ("volume", feats.supports_set_volume(), &inner.config.volume),
    ] {
        let Some(value) = value else { continue };
        if !supported {
            continue;
        }
        let result = to_unit(value).and_then(|v| {
            let r = if key == "rate" {
                backend.set_rate(v)
            } else {
                backend.set_volume(v)
            };
            r.map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            log::debug!("Could not set {key}: {e}");
        }
    }
}

/// `_backend_name_locked`.
fn backend_name_locked(inner: &Inner) -> String {
    match inner.backend.as_ref() {
        None => "none".into(),
        Some(b) => b.name().unwrap_or_else(|_| "unknown".into()),
    }
}

// ---------------------------------------------------------------------------
// The process-wide instance and the module-level API
// ---------------------------------------------------------------------------

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

static SPEECH: LazyLock<Speech> = LazyLock::new(|| {
    let lib: Arc<dyn SpeechLib> = Arc::new(RealLib);
    LazyLock::force(&START);
    Speech::new(Some(lib), Box::new(|| START.elapsed().as_secs_f64()))
});

/// The process-wide speech layer.
pub fn global() -> &'static Speech {
    &SPEECH
}

/// Whether the speech library is present. Always true: Prism is linked into the exe.
/// Whether a backend can actually speak is [`init`]'s answer.
pub fn is_available() -> bool {
    SPEECH.is_available()
}

/// `speech.init(cfg)`. True when a backend exists.
pub fn init(cfg: Option<&Value>) -> bool {
    SPEECH.init(cfg)
}

/// `speech.configure(cfg)`.
pub fn configure(cfg: &Value) {
    SPEECH.configure(cfg)
}

/// `speech.speak(text, interrupt)`.
pub fn speak(text: &str, interrupt: bool) -> bool {
    SPEECH.speak(text, interrupt)
}

/// `speech.stop()`.
pub fn stop() -> bool {
    SPEECH.stop()
}

/// `speech.braille(text)`.
pub fn braille(text: &str) -> bool {
    SPEECH.braille(text)
}

/// `speech.output(text, interrupt)`.
pub fn output(text: &str, interrupt: bool) -> bool {
    SPEECH.output(text, interrupt)
}

/// `speech.backend_name()`.
pub fn backend_name() -> Option<String> {
    SPEECH.backend_name()
}

/// `speech.features()`.
pub fn features() -> Option<BackendFeatures> {
    SPEECH.features()
}

/// `speech.supports(feature_name)`.
pub fn supports(feature_name: &str) -> bool {
    SPEECH.supports(feature_name)
}

/// `speech.describe_capabilities()`.
pub fn describe_capabilities() -> String {
    SPEECH.describe_capabilities()
}

/// `speech.list_backends()`.
pub fn list_backends() -> Vec<String> {
    SPEECH.list_backends()
}

/// `speech.list_voices(name)`.
pub fn list_voices(backend_name_or_auto: &str) -> (Vec<String>, Option<BackendFeatures>) {
    SPEECH.list_voices(backend_name_or_auto)
}

/// `speech.test_speak(...)`.
pub fn test_speak(
    backend_name_or_auto: &str,
    voice_name: &str,
    rate: Option<f64>,
    volume: Option<f64>,
    text: &str,
) -> (bool, Option<String>) {
    SPEECH.test_speak(backend_name_or_auto, voice_name, rate, volume, text)
}

/// `speech.shutdown()`.
pub fn shutdown() {
    SPEECH.shutdown()
}

/// Flush and stop Prism's logger. Rust has no atexit, so the application calls this once
/// on exit. A no-op when the log bridge was never installed.
pub fn atexit() {
    if prism::logging_installed() {
        prism::log_flush_and_shutdown();
    }
}

#[cfg(test)]
mod tests;
