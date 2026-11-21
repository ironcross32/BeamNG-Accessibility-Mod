//! The speech layer's logic over a fake Prism.

use super::*;
use parking_lot::Mutex as PMutex;
use serde_json::json;

const NVDA_BITS: u64 = 0xb5; // what NVDA reports on the development machine
const SAPI_BITS: u64 = 0x05ff_ffed; // SAPI and OneCore, same machine

#[derive(Default)]
struct Log {
    contexts_made: usize,
    acquires: Vec<String>,
    spoken: Vec<(String, String, bool)>, // (backend, text, interrupt)
    brailled: Vec<String>,
    stops: usize,
    rates: Vec<(String, f32)>,
    volumes: Vec<(String, f32)>,
    voices: Vec<(String, usize)>,
}

struct FakeBackendSpec {
    name: &'static str,
    id: BackendId,
    available: bool,
    bits: u64,
    voices: Vec<&'static str>,
}

struct World {
    specs: Vec<FakeBackendSpec>,
    log: Log,
    /// How many upcoming speak calls fail, per backend name.
    fail_speak: Vec<(&'static str, usize)>,
    context_fails: bool,
    availability: Option<AvailabilityFn>,
}

type Shared = Arc<PMutex<World>>;

struct FakeLib(Shared);
struct FakeCtx(Shared);
struct FakeBackend {
    w: Shared,
    name: &'static str,
    bits: u64,
    voices: Vec<&'static str>,
}

impl SpeechLib for FakeLib {
    fn install_logging(&self, _: &str) -> Result<(), PrismErr> {
        Ok(())
    }
    fn set_log_level(&self, _: LogLevel) -> Result<(), PrismErr> {
        Ok(())
    }
    fn make_context(&self, cb: AvailabilityFn) -> Result<Arc<dyn SpeechContext>, PrismErr> {
        let mut w = self.0.lock();
        if w.context_fails {
            return Err(PrismErr::Runtime("Prism could not be initialized".into()));
        }
        w.log.contexts_made += 1;
        w.availability = Some(cb);
        Ok(Arc::new(FakeCtx(self.0.clone())))
    }
}

fn not_available() -> PrismErr {
    PrismErr::Prism(prismer::Error::BackendNotAvailable)
}

impl FakeCtx {
    fn make(&self, idx: usize) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
        let mut w = self.0.lock();
        let s = &w.specs[idx];
        if !s.available {
            return Err(not_available());
        }
        let b = FakeBackend {
            w: self.0.clone(),
            name: s.name,
            bits: s.bits,
            voices: s.voices.clone(),
        };
        let name = s.name.to_string();
        w.log.acquires.push(name);
        Ok(Arc::new(b))
    }
}

impl SpeechContext for FakeCtx {
    fn backends_count(&self) -> usize {
        self.0.lock().specs.len()
    }
    fn id_at(&self, index: usize) -> Result<BackendId, PrismErr> {
        self.0
            .lock()
            .specs
            .get(index)
            .map(|s| s.id)
            .ok_or_else(|| PrismErr::Key(format!("No backend matching {index}")))
    }
    fn id_of_name(&self, name: &str) -> Result<BackendId, PrismErr> {
        self.0
            .lock()
            .specs
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.id)
            .ok_or_else(|| {
                PrismErr::Key(format!("No backend matching {}", prism::py_repr_str(name)))
            })
    }
    fn name_of(&self, id: BackendId) -> Result<String, PrismErr> {
        self.0
            .lock()
            .specs
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.to_string())
            .ok_or_else(|| PrismErr::Key("Backend ID not found".into()))
    }
    fn acquire(&self, id: BackendId) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
        let idx = self.0.lock().specs.iter().position(|s| s.id == id);
        match idx {
            Some(i) => self.make(i),
            None => Err(not_available()),
        }
    }
    fn acquire_best(&self) -> Result<Arc<dyn SpeechBackend>, PrismErr> {
        // Registry order is priority order.
        let idx = self.0.lock().specs.iter().position(|s| s.available);
        match idx {
            Some(i) => self.make(i),
            None => Err(not_available()),
        }
    }
}

impl SpeechBackend for FakeBackend {
    fn name(&self) -> Result<String, PrismErr> {
        Ok(self.name.into())
    }
    fn features(&self) -> BackendFeatures {
        BackendFeatures::from_bits(self.bits)
    }
    fn speak(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        prism::check_text(text)?;
        let mut w = self.w.lock();
        if let Some(f) = w
            .fail_speak
            .iter_mut()
            .find(|(n, c)| *n == self.name && *c > 0)
        {
            f.1 -= 1;
            return Err(PrismErr::Prism(prismer::Error::SpeakFailure));
        }
        w.log
            .spoken
            .push((self.name.into(), text.into(), interrupt));
        Ok(())
    }
    fn stop(&self) -> Result<(), PrismErr> {
        self.w.lock().log.stops += 1;
        Ok(())
    }
    fn braille(&self, text: &str) -> Result<(), PrismErr> {
        self.w.lock().log.brailled.push(text.into());
        Ok(())
    }
    fn output(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        self.speak(text, interrupt)
    }
    fn refresh_voices(&self) -> Result<(), PrismErr> {
        Ok(())
    }
    fn voices_count(&self) -> Result<usize, PrismErr> {
        Ok(self.voices.len())
    }
    fn get_voice_name(&self, idx: usize) -> Result<String, PrismErr> {
        Ok(self.voices[idx].into())
    }
    fn set_voice(&self, idx: usize) -> Result<(), PrismErr> {
        self.w.lock().log.voices.push((self.name.into(), idx));
        Ok(())
    }
    fn set_rate(&self, v: f32) -> Result<(), PrismErr> {
        self.w.lock().log.rates.push((self.name.into(), v));
        Ok(())
    }
    fn set_volume(&self, v: f32) -> Result<(), PrismErr> {
        self.w.lock().log.volumes.push((self.name.into(), v));
        Ok(())
    }
}

fn spec(
    name: &'static str,
    id: BackendId,
    available: bool,
    bits: u64,
    voices: Vec<&'static str>,
) -> FakeBackendSpec {
    FakeBackendSpec {
        name,
        id,
        available,
        bits,
        voices,
    }
}

/// NVDA (running or not), JAWS (absent), OneCore, SAPI, in registry order.
fn world(nvda_running: bool) -> Shared {
    Arc::new(PMutex::new(World {
        specs: vec![
            spec(
                "NVDA",
                prism::backend_id::NVDA,
                nvda_running,
                NVDA_BITS,
                vec![],
            ),
            spec("JAWS", prism::backend_id::JAWS, false, NVDA_BITS, vec![]),
            spec(
                "OneCore",
                prism::backend_id::ONE_CORE,
                true,
                SAPI_BITS,
                vec!["Microsoft David", "Microsoft Zira"],
            ),
            spec(
                "SAPI",
                prism::backend_id::SAPI,
                true,
                SAPI_BITS,
                vec!["Microsoft David Desktop", " Microsoft Zira Desktop "],
            ),
        ],
        log: Log::default(),
        fail_speak: vec![],
        context_fails: false,
        availability: None,
    }))
}

struct Harness {
    s: Speech,
    w: Shared,
    now: Arc<PMutex<f64>>,
}

fn harness(w: Shared) -> Harness {
    let now = Arc::new(PMutex::new(1000.0));
    let n2 = now.clone();
    let s = Speech::new(
        Some(Arc::new(FakeLib(w.clone()))),
        Box::new(move || *n2.lock()),
    );
    Harness { s, w, now }
}

fn last_spoken_by(h: &Harness) -> Option<String> {
    h.w.lock().log.spoken.last().map(|x| x.0.clone())
}

// --- priority selection and name matching ---

#[test]
fn auto_picks_highest_priority_available() {
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({"speech_backend": "auto"}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("NVDA"));
    let h = harness(world(false));
    assert!(h.s.init(Some(&json!({}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("OneCore"));
}

#[test]
fn named_backend_is_used_and_unknown_or_absent_falls_back() {
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({"speech_backend": "SAPI"}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("SAPI"));

    // Absent reader: falls back to the best one.
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({"speech_backend": "JAWS"}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("NVDA"));

    // Matching is exact: "Auto" and "sapi" are names Prism does not know.
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({"speech_backend": "sapi"}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("NVDA"));
    let h = harness(world(false));
    assert!(h.s.init(Some(&json!({"speech_backend": "Auto"}))));
    assert_eq!(h.s.backend_name().as_deref(), Some("OneCore"));
}

#[test]
fn no_library_means_no_speech_and_no_panic() {
    let s = Speech::new(None, Box::new(|| 0.0));
    assert!(!s.is_available());
    assert!(!s.init(Some(&json!({}))));
    s.configure(&json!({"speech_backend": "SAPI"}));
    assert!(!s.speak("hello", true));
    assert!(!s.stop());
    assert!(!s.braille("x"));
    assert_eq!(s.backend_name(), None);
    assert_eq!(s.features(), None);
    assert_eq!(s.describe_capabilities(), "speech unavailable");
    assert!(s.list_backends().is_empty());
    assert_eq!(s.list_voices("auto"), (vec![], None));
    assert_eq!(
        s.test_speak("auto", "", Some(50.0), Some(100.0), "x"),
        (false, Some("No speech library is available.".into()))
    );
    s.shutdown();
}

// --- retry cooldown and re-acquire ---

#[test]
fn failed_init_retries_only_after_cooldown() {
    let w = world(true);
    for s in w.lock().specs.iter_mut() {
        s.available = false;
    }
    let h = harness(w);
    assert!(!h.s.init(None));
    assert_eq!(h.w.lock().log.contexts_made, 1);
    // Within the cooldown nothing is attempted.
    for s in h.w.lock().specs.iter_mut() {
        s.available = true;
    }
    *h.now.lock() += RETRY_COOLDOWN_S - 0.001;
    assert!(!h.s.speak("x", true));
    assert!(h.w.lock().log.acquires.is_empty());
    *h.now.lock() += 0.001;
    assert!(h.s.speak("x", true));
    assert_eq!(last_spoken_by(&h).as_deref(), Some("NVDA"));
}

#[test]
fn context_failure_also_takes_the_cooldown() {
    let w = world(true);
    w.lock().context_fails = true;
    let h = harness(w);
    assert!(!h.s.init(None));
    h.w.lock().context_fails = false;
    assert!(!h.s.init(None));
    *h.now.lock() += RETRY_COOLDOWN_S;
    assert!(h.s.init(None));
}

#[test]
fn availability_change_rebuilds_on_next_call() {
    let h = harness(world(false));
    assert!(h.s.speak("one", true));
    assert_eq!(last_spoken_by(&h).as_deref(), Some("OneCore"));
    // NVDA starts: the flag is set from Prism's thread, nothing else happens there.
    h.w.lock().specs[0].available = true;
    let cb = h.w.lock().availability.clone().unwrap();
    cb(prism::backend_id::NVDA, "NVDA", true);
    assert_eq!(h.w.lock().log.acquires.len(), 1);
    assert!(h.s.speak("two", true));
    assert_eq!(last_spoken_by(&h).as_deref(), Some("NVDA"));
    // Consumed: a third call does not acquire again.
    assert!(h.s.speak("three", true));
    assert_eq!(h.w.lock().log.acquires.len(), 2);
}

#[test]
fn failed_call_reacquires_once() {
    let h = harness(world(true));
    assert!(h.s.init(None));
    // The reader exits: speaking on the old handle fails once, the fresh one works.
    h.w.lock().fail_speak = vec![("NVDA", 1)];
    assert!(h.s.speak("hello", false));
    assert_eq!(h.w.lock().log.acquires.len(), 2);
    assert_eq!(
        h.w.lock().log.spoken.last().unwrap(),
        &("NVDA".into(), "hello".into(), false)
    );
    // Failing twice gives up for this utterance.
    h.w.lock().fail_speak = vec![("NVDA", 2)];
    assert!(!h.s.speak("again", true));
}

#[test]
fn reacquire_ignores_cooldown_while_a_backend_is_held() {
    let h = harness(world(true));
    assert!(h.s.init(None));
    h.w.lock().fail_speak = vec![("NVDA", 1)];
    // No clock movement needed: the cooldown only applies with no backend.
    assert!(h.s.speak("x", true));
}

// --- configure ---

#[test]
fn configure_rebuilds_only_when_backend_changes() {
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({"speech_backend": "SAPI", "speech_rate": 50}))));
    assert_eq!(h.w.lock().log.acquires.len(), 1);
    h.s.configure(&json!({"speech_backend": "SAPI", "speech_rate": 70}));
    assert_eq!(h.w.lock().log.acquires.len(), 1);
    assert_eq!(
        h.w.lock().log.rates.last().unwrap(),
        &("SAPI".into(), 0.7f32)
    );
    h.s.configure(&json!({"speech_backend": "auto"}));
    assert_eq!(h.w.lock().log.acquires.len(), 2);
    assert_eq!(h.s.backend_name().as_deref(), Some("NVDA"));
}

#[test]
fn configure_before_any_backend_only_stores() {
    let h = harness(world(true));
    h.s.configure(&json!({"speech_backend": "SAPI"}));
    assert!(h.w.lock().log.acquires.is_empty());
    assert!(h.s.init(None));
    assert_eq!(h.s.backend_name().as_deref(), Some("SAPI"));
}

// --- capability gating and parameter application ---

#[test]
fn screen_reader_gets_no_voice_rate_or_volume() {
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({
        "speech_backend": "NVDA", "speech_voice_name": "Microsoft Zira Desktop",
        "speech_rate": 80, "speech_volume": 40
    }))));
    let w = h.w.lock();
    assert!(w.log.rates.is_empty());
    assert!(w.log.volumes.is_empty());
    assert!(w.log.voices.is_empty());
}

#[test]
fn software_synth_gets_voice_rate_and_volume() {
    let h = harness(world(true));
    assert!(h.s.init(Some(&json!({
        "speech_backend": "SAPI", "speech_voice_name": "  microsoft ZIRA desktop",
        "speech_rate": 50, "speech_volume": 100
    }))));
    let w = h.w.lock();
    assert_eq!(w.log.voices, vec![("SAPI".to_string(), 1)]);
    assert_eq!(w.log.rates, vec![("SAPI".to_string(), 0.5)]);
    assert_eq!(w.log.volumes, vec![("SAPI".to_string(), 1.0)]);
}

#[test]
fn unknown_voice_is_left_alone() {
    let h = harness(world(true));
    assert!(h.s.init(Some(
        &json!({"speech_backend": "SAPI", "speech_voice_name": "Nobody"})
    )));
    assert!(h.w.lock().log.voices.is_empty());
}

#[test]
fn rate_and_volume_coercion_and_clamp() {
    let cases: Vec<(Value, Option<f32>)> = vec![
        (json!(150), Some(1.0)),
        (json!(-5), Some(0.0)),
        (json!(33), Some(0.33)),
        (json!(12.5), Some(0.125)),
        (json!("50"), Some(0.5)),
        (json!(" 20 "), Some(0.2)),
        (json!("nan"), Some(1.0)), // min(1.0, nan) is 1.0 in Python
        (json!("-inf"), Some(0.0)),
        (json!(true), Some(0.01)),
        (json!(false), Some(0.0)),
        (json!("fast"), None),
        (json!([1]), None),
        (Value::Null, None),
    ];
    for (value, want) in cases {
        let h = harness(world(true));
        let cfg = json!({"speech_backend": "SAPI", "speech_rate": value.clone()});
        assert!(h.s.init(Some(&cfg)));
        let got = h.w.lock().log.rates.first().map(|r| r.1);
        assert_eq!(got, want, "speech_rate = {value}");
    }
}

#[test]
fn clamp_matches_python_min_max_order() {
    assert_eq!(clamp_unit(0.5), 0.5);
    assert_eq!(clamp_unit(2.0), 1.0);
    assert_eq!(clamp_unit(-1.0), 0.0);
    assert_eq!(clamp_unit(f64::NAN), 1.0);
    assert!(clamp_unit(-0.0).is_sign_positive());
}

// --- config reading ---

#[test]
fn read_config_coerces_like_str_or() {
    let c = read_config(&json!({}));
    assert_eq!(c.backend.as_deref(), Some("auto"));
    assert_eq!(c.voice_name.as_deref(), Some(""));
    assert_eq!(c.rate, None);
    for (v, want) in [
        (json!(""), "auto"),
        (Value::Null, "auto"),
        (json!(false), "auto"),
        (json!(0), "auto"),
        (json!(5), "5"),
        (json!(2.0), "2.0"),
        (json!(true), "True"),
        (json!("SAPI"), "SAPI"),
        (json!(["a"]), "['a']"),
    ] {
        let c = read_config(&json!({"speech_backend": v.clone()}));
        assert_eq!(c.backend.as_deref(), Some(want), "speech_backend = {v}");
    }
    // Not a dict: {}.
    assert_eq!(read_config(&json!([1, 2])), SpeechConfig::default());
    assert_eq!(
        read_config(&json!({"speech_volume": 0})).volume,
        Some(json!(0))
    );
}

// --- primitives ---

#[test]
fn empty_text_never_initialises() {
    let h = harness(world(true));
    assert!(!h.s.speak("", true));
    assert!(!h.s.output("", true));
    assert!(!h.s.braille(""));
    assert_eq!(h.w.lock().log.contexts_made, 0);
}

#[test]
fn braille_is_gated_on_capability() {
    let h = harness(world(true));
    assert!(h.s.braille("on display"));
    assert_eq!(h.w.lock().log.brailled, vec!["on display".to_string()]);
    let h = harness(world(true));
    assert_eq!(SAPI_BITS & (1 << 4), 0); // SAPI has no braille bit
    h.s.init(Some(&json!({"speech_backend": "SAPI"})));
    assert!(!h.s.braille("nothing"));
    assert!(h.w.lock().log.brailled.is_empty());
}

#[test]
fn stop_and_supports() {
    let h = harness(world(true));
    assert!(h.s.stop());
    assert_eq!(h.w.lock().log.stops, 1);
    assert!(h.s.supports("supports_braille"));
    assert!(!h.s.supports("supports_set_rate"));
    assert!(!h.s.supports("not_a_feature"));
}

#[test]
fn describe_capabilities_phrases() {
    let h = harness(world(true));
    assert_eq!(h.s.describe_capabilities(), "NVDA [speak, braille, stop]");
    let h = harness(world(true));
    h.s.init(Some(&json!({"speech_backend": "SAPI"})));
    assert_eq!(
        h.s.describe_capabilities(),
        "SAPI [speak, stop, rate, volume, voices, pcm]"
    );
    let h = harness(world(true));
    h.w.lock().specs[0].bits = 1;
    assert_eq!(
        h.s.describe_capabilities(),
        "NVDA [no capabilities reported]"
    );
}

// --- enumeration ---

#[test]
fn list_backends_in_registry_order() {
    let h = harness(world(false));
    assert_eq!(h.s.list_backends(), vec!["NVDA", "JAWS", "OneCore", "SAPI"]);
    // Enumeration shares the one context.
    h.s.init(None);
    assert_eq!(h.w.lock().log.contexts_made, 1);
}

#[test]
fn list_voices_with_and_without_count_capability() {
    let h = harness(world(true));
    let (names, feats) = h.s.list_voices("SAPI");
    assert_eq!(
        names,
        vec!["Microsoft David Desktop", " Microsoft Zira Desktop "]
    );
    assert_eq!(feats.unwrap().to_bits(), SAPI_BITS);
    let (names, feats) = h.s.list_voices("auto");
    assert!(names.is_empty());
    assert_eq!(feats.unwrap().to_bits(), NVDA_BITS);
    // An absent reader reports the fallback's features, as speech.py does.
    let (_, feats) = h.s.list_voices("JAWS");
    assert_eq!(feats.unwrap().to_bits(), NVDA_BITS);
    // Probing does not disturb the active backend.
    assert!(h.s.init(None));
    assert!(h.s.speak("x", true));
    assert_eq!(last_spoken_by(&h).as_deref(), Some("NVDA"));
}

#[test]
fn test_speak_applies_pending_settings_and_reports_errors() {
    let h = harness(world(true));
    let r = h.s.test_speak(
        "SAPI",
        "microsoft david desktop",
        Some(30.0),
        Some(250.0),
        "Test line.",
    );
    assert_eq!(r, (true, None));
    {
        let w = h.w.lock();
        assert_eq!(w.log.voices, vec![("SAPI".to_string(), 0)]);
        assert_eq!(w.log.rates, vec![("SAPI".to_string(), 0.3)]);
        assert_eq!(w.log.volumes, vec![("SAPI".to_string(), 1.0)]);
        assert_eq!(
            w.log.spoken.last().unwrap(),
            &("SAPI".into(), "Test line.".into(), true)
        );
    }
    assert!(h.s.test_backend.lock().is_some());
    let r = h.s.test_speak("auto", "", None, None, "");
    assert_eq!(r, (false, Some("Text MUST NOT be empty".into())));
}

#[test]
fn shutdown_drops_backend_and_context() {
    let h = harness(world(true));
    assert!(h.s.init(None));
    h.s.shutdown();
    h.s.shutdown();
    assert!(h.s.init(None));
    assert_eq!(h.w.lock().log.contexts_made, 2);
}
