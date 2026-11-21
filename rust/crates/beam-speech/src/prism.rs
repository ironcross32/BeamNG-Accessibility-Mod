//! Prism, through the `prismer` crate.
//!
//! Prismer builds Prism from source and links it statically (the workspace enables its
//! `static` feature), so there is no prism.dll to find or ship: speech is always present.
//! Building needs CMake 3.24+ and an MSVC with C++23; see `rust/README.md`.
//!
//! This module adapts Prismer to what `speech.rs` was written against:
//!
//! - Prismer's `Backend<'a>` borrows its `Prism` context and neither is `Send`. The speech
//!   layer keeps both in a process-wide state shared between threads, so [`Context`] sits
//!   in an `Arc` that every [`Backend`] holds, and each serialises calls with a mutex.
//! - Capability checks go by the attribute names of the old Python `BackendFeatures`
//!   (`"supports_braille"`), because `speech::supports` takes a name.
//! - Errors carry the text the configurator's message box shows.
//!
//! Nothing here panics: a failed context or backend is an error the speech layer logs.

use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Ids and capability bits
// ---------------------------------------------------------------------------

/// A backend id: Prism's 64-bit hash of the name. 0 is invalid.
pub type BackendId = u64;

/// The built-in backend ids.
pub mod backend_id {
    use super::BackendId;
    use prismer::BackendId as P;
    pub const INVALID: BackendId = P::INVALID.0;
    pub const SAPI: BackendId = P::SAPI.0;
    pub const AV_SPEECH: BackendId = P::AV_SPEECH.0;
    pub const VOICE_OVER: BackendId = P::VOICE_OVER.0;
    pub const SPEECH_DISPATCHER: BackendId = P::SPEECH_DISPATCHER.0;
    pub const NVDA: BackendId = P::NVDA.0;
    pub const JAWS: BackendId = P::JAWS.0;
    pub const ONE_CORE: BackendId = P::ONE_CORE.0;
    pub const ORCA: BackendId = P::ORCA.0;
    pub const UIA: BackendId = P::UIA.0;
    pub const ZDSR: BackendId = P::ZDSR.0;
    pub const ZOOM_TEXT: BackendId = P::ZOOM_TEXT.0;
}

/// `(attribute name, bit position)` for every capability the speech layer can ask about
/// by name, in declaration order. Bit 1 is unused.
pub const FEATURE_BITS: &[(&str, u32)] = &[
    ("is_supported_at_runtime", 0),
    ("supports_speak", 2),
    ("supports_speak_to_memory", 3),
    ("supports_braille", 4),
    ("supports_output", 5),
    ("supports_is_speaking", 6),
    ("supports_stop", 7),
    ("supports_pause", 8),
    ("supports_resume", 9),
    ("supports_set_volume", 10),
    ("supports_get_volume", 11),
    ("supports_set_rate", 12),
    ("supports_get_rate", 13),
    ("supports_set_pitch", 14),
    ("supports_get_pitch", 15),
    ("supports_refresh_voices", 16),
    ("supports_count_voices", 17),
    ("supports_get_voice_name", 18),
    ("supports_get_voice_language", 19),
    ("supports_get_voice", 20),
    ("supports_set_voice", 21),
    ("supports_get_channels", 22),
    ("supports_get_sample_rate", 23),
    ("supports_get_bit_depth", 24),
    ("performs_silence_trimming_on_speak", 25),
    ("performs_silence_trimming_on_speak_to_memory", 26),
    ("supports_speak_ssml", 27),
    ("supports_speak_to_memory_ssml", 28),
];

/// A backend's capability bits, keeping only the ones [`FEATURE_BITS`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendFeatures(u64);

macro_rules! feature_getters {
    ($($name:ident = $bit:expr;)*) => {
        $(
            #[inline]
            pub fn $name(&self) -> bool {
                self.0 & (1u64 << $bit) != 0
            }
        )*
    };
}

impl BackendFeatures {
    pub fn from_bits(bits: u64) -> Self {
        let mask = FEATURE_BITS.iter().fold(0u64, |m, (_, b)| m | (1u64 << b));
        BackendFeatures(bits & mask)
    }

    pub fn to_bits(&self) -> u64 {
        self.0
    }

    /// The capability by attribute name; an unknown name is false.
    pub fn get(&self, name: &str) -> bool {
        FEATURE_BITS
            .iter()
            .find(|(n, _)| *n == name)
            .is_some_and(|(_, b)| self.0 & (1u64 << b) != 0)
    }

    feature_getters! {
        is_supported_at_runtime = 0;
        supports_speak = 2;
        supports_speak_to_memory = 3;
        supports_braille = 4;
        supports_output = 5;
        supports_is_speaking = 6;
        supports_stop = 7;
        supports_pause = 8;
        supports_resume = 9;
        supports_set_volume = 10;
        supports_get_volume = 11;
        supports_set_rate = 12;
        supports_get_rate = 13;
        supports_set_pitch = 14;
        supports_get_pitch = 15;
        supports_refresh_voices = 16;
        supports_count_voices = 17;
        supports_get_voice_name = 18;
        supports_get_voice_language = 19;
        supports_get_voice = 20;
        supports_set_voice = 21;
        supports_get_channels = 22;
        supports_get_sample_rate = 23;
        supports_get_bit_depth = 24;
        performs_silence_trimming_on_speak = 25;
        performs_silence_trimming_on_speak_to_memory = 26;
        supports_speak_ssml = 27;
        supports_speak_to_memory_ssml = 28;
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A speech error. `Display` is what goes into log lines and, from `test_speak`, the
/// configurator's message box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrismErr {
    /// An error Prism or a backend reported.
    Prism(prismer::Error),
    /// Text rejected before it reached Prism.
    Text(&'static str),
    /// A backend name or registry index that matches nothing.
    Key(String),
    /// The context could not be created.
    Runtime(String),
}

impl std::fmt::Display for PrismErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrismErr::Prism(e) => write!(f, "{e}"),
            PrismErr::Text(m) => f.write_str(m),
            PrismErr::Key(m) => f.write_str(&py_repr_str(m)),
            PrismErr::Runtime(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PrismErr {}

impl From<prismer::Error> for PrismErr {
    fn from(e: prismer::Error) -> Self {
        PrismErr::Prism(e)
    }
}

/// Python's `repr()` of a str, for the characters a backend name or message can carry.
/// The speech layer formats config values this way in its log lines.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Reject empty text and embedded NULs with a readable message, before any call that
/// takes text.
pub fn check_text(text: &str) -> Result<(), PrismErr> {
    if text.is_empty() {
        return Err(PrismErr::Text("Text MUST NOT be empty"));
    }
    if text.contains('\0') {
        return Err(PrismErr::Text("Text MUST NOT contain embedded NULs"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// Prism's log threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    None,
}

impl LogLevel {
    fn to_prismer(self) -> prismer::log::Level {
        use prismer::log::Level as L;
        match self {
            LogLevel::Trace => L::Trace,
            LogLevel::Debug => L::Debug,
            LogLevel::Info => L::Info,
            LogLevel::Warn => L::Warn,
            LogLevel::Error => L::Error,
            LogLevel::None => L::None,
        }
    }
}

/// Prism's version string.
pub fn version_string() -> String {
    prismer::version_string()
}

/// Set Prism's log threshold.
pub fn set_log_level(level: LogLevel) {
    let _previous = prismer::log::set_level(level.to_prismer());
}

static LOG_INSTALLED: OnceLock<()> = OnceLock::new();

/// Route Prism's own log through the `log` facade with target `<logger_name>.<source>`.
/// Once per process; later calls are no-ops.
pub fn install_logging(logger_name: &str) {
    let prefix = logger_name.to_string();
    LOG_INSTALLED.get_or_init(move || {
        prismer::log::set_handler(move |level, source, message| {
            use prismer::log::Level as L;
            let lvl = match level {
                L::Trace => log::Level::Trace,
                L::Debug => log::Level::Debug,
                L::Warn => log::Level::Warn,
                L::Error => log::Level::Error,
                L::Info | L::None => log::Level::Info,
            };
            let src = if source.is_empty() { "prism" } else { source };
            let target = format!("{prefix}.{src}");
            log::log!(target: &target, lvl, "{message}");
        });
    });
}

/// Whether [`install_logging`] has run in this process.
pub fn logging_installed() -> bool {
    LOG_INSTALLED.get().is_some()
}

/// Flush and stop Prism's logger; for process exit.
pub fn log_flush_and_shutdown() {
    prismer::log::flush();
    prismer::log::shutdown();
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// The availability callback: `(backend id, name, available)`. Runs on a Prism thread.
pub type AvailabilityFn = Arc<dyn Fn(BackendId, &str, bool) + Send + Sync>;

/// A Prism context. Dropping the last `Arc` shuts Prism down; backends hold an `Arc` to
/// their context, so it outlives every backend made from it.
pub struct Context {
    prism: prismer::Prism,
    /// Serialises calls on this context from different threads.
    calls: Mutex<()>,
}

// SAFETY: the context is only touched through `&self` methods holding `calls`, and in
// Drop (exclusive). Prism runs its own availability thread against the same context, so
// it is built to be used from more than one thread; serialising our own calls keeps any
// two of them from overlapping.
unsafe impl Send for Context {}
// SAFETY: as above; every shared access is serialised by `calls`.
unsafe impl Sync for Context {}

impl Context {
    /// A new context, with the availability callback when given. Prism's defaults apply to
    /// the polling interval, debounce and backoff; automatic power management is on.
    pub fn new(on_availability: Option<AvailabilityFn>) -> Result<Arc<Context>, PrismErr> {
        let mut builder = prismer::Prism::builder();
        if let Some(cb) = on_availability {
            builder =
                builder
                    .auto_power_manage(true)
                    .on_availability(move |id, name, available| {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            cb(id.0, name, available)
                        }));
                    });
        }
        let prism = builder
            .build()
            .map_err(|e| PrismErr::Runtime(format!("Prism could not be initialized: {e}")))?;
        Ok(Arc::new(Context {
            prism,
            calls: Mutex::new(()),
        }))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn backends_count(&self) -> usize {
        let _g = self.guard();
        self.prism.backend_count()
    }

    /// The id of the backend at `index` in the registry (priority order).
    pub fn id_at(&self, index: usize) -> Result<BackendId, PrismErr> {
        let _g = self.guard();
        self.prism
            .backend_ids()
            .get(index)
            .map(|id| id.0)
            .filter(|&id| id != backend_id::INVALID)
            .ok_or_else(|| PrismErr::Key(format!("No backend matching {index}")))
    }

    /// The id of the backend registered under `name` (exact match).
    pub fn id_of_name(&self, name: &str) -> Result<BackendId, PrismErr> {
        let _g = self.guard();
        self.prism
            .backend_id_by_name(name)
            .map(|id| id.0)
            .map_err(|_| PrismErr::Key(format!("No backend matching {}", py_repr_str(name))))
    }

    pub fn name_of(&self, id: BackendId) -> Result<String, PrismErr> {
        let _g = self.guard();
        self.prism
            .backend_name(prismer::BackendId(id))
            .ok_or_else(|| PrismErr::Key("Backend ID not found".into()))
    }

    pub fn priority_of(&self, id: BackendId) -> i32 {
        let _g = self.guard();
        self.prism.backend_priority(prismer::BackendId(id))
    }

    pub fn exists(&self, id: BackendId) -> bool {
        let _g = self.guard();
        self.prism.backend_exists(prismer::BackendId(id))
    }

    /// Wrap a backend handle, initialising it first when `initialize` is set (an already
    /// initialised shared instance is fine). A failed initialise drops, and so frees, the
    /// handle.
    fn wrap(
        self: &Arc<Self>,
        made: prismer::Result<prismer::Backend<'_>>,
        initialize: bool,
    ) -> Result<Backend, PrismErr> {
        let backend = made?;
        if initialize {
            match backend.initialize() {
                Ok(()) | Err(prismer::Error::AlreadyInitialized) => {}
                Err(e) => return Err(e.into()),
            }
        }
        // SAFETY: a prismer Backend only requires its Prism context to stay alive (the
        // lifetime is a PhantomData borrow). The returned Backend holds an `Arc` to that
        // context and its field order drops the handle before the `Arc`, so the context
        // outlives the handle even though the borrow is erased here.
        let inner = unsafe {
            std::mem::transmute::<prismer::Backend<'_>, prismer::Backend<'static>>(backend)
        };
        Ok(Backend {
            inner,
            calls: Mutex::new(()),
            _ctx: self.clone(),
        })
    }

    /// A new, private instance of the backend with this id.
    pub fn create(self: &Arc<Self>, id: BackendId) -> Result<Backend, PrismErr> {
        let _g = self.guard();
        self.wrap(self.prism.create(prismer::BackendId(id)), true)
    }

    /// A new, private instance of the highest-priority backend that starts.
    pub fn create_best(self: &Arc<Self>) -> Result<Backend, PrismErr> {
        let _g = self.guard();
        self.wrap(self.prism.create_best(), false)
    }

    /// The shared instance of the backend with this id.
    pub fn acquire(self: &Arc<Self>, id: BackendId) -> Result<Backend, PrismErr> {
        let _g = self.guard();
        self.wrap(self.prism.acquire(prismer::BackendId(id)), true)
    }

    /// The shared instance of the highest-priority backend available now.
    pub fn acquire_best(self: &Arc<Self>) -> Result<Backend, PrismErr> {
        let _g = self.guard();
        self.wrap(self.prism.acquire_best(), false)
    }

    pub fn pause_availability_polling(&self) {
        let _g = self.guard();
        self.prism.pause_availability_polling();
    }

    pub fn resume_availability_polling(&self) {
        let _g = self.guard();
        self.prism.resume_availability_polling();
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// A backend handle. Dropping it frees the handle, then releases the context.
pub struct Backend {
    /// Declared before `_ctx`: fields drop in order, so the handle goes first.
    inner: prismer::Backend<'static>,
    /// Serialises calls on this handle.
    calls: Mutex<()>,
    _ctx: Arc<Context>,
}

// SAFETY: the handle is only used through `&self` methods holding `calls`, and in Drop
// (exclusive), so no two threads ever call into the same backend at once.
unsafe impl Send for Backend {}
// SAFETY: as above.
unsafe impl Sync for Backend {}

impl Backend {
    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn name(&self) -> Result<String, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.name())
    }

    pub fn features(&self) -> BackendFeatures {
        let _g = self.guard();
        BackendFeatures::from_bits(self.inner.features().0)
    }

    pub fn speak(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        check_text(text)?;
        let _g = self.guard();
        Ok(self.inner.speak(text, interrupt)?)
    }

    pub fn output(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        check_text(text)?;
        let _g = self.guard();
        Ok(self.inner.output(text, interrupt)?)
    }

    pub fn braille(&self, text: &str) -> Result<(), PrismErr> {
        check_text(text)?;
        let _g = self.guard();
        Ok(self.inner.braille(text)?)
    }

    pub fn stop(&self) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.stop()?)
    }

    pub fn pause(&self) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.pause()?)
    }

    pub fn resume(&self) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.resume()?)
    }

    pub fn refresh_voices(&self) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.refresh_voices()?)
    }

    pub fn is_speaking(&self) -> Result<bool, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.is_speaking()?)
    }

    pub fn volume(&self) -> Result<f32, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.volume()?)
    }

    /// Prism normalises volume, rate and pitch to 0.0-1.0.
    pub fn set_volume(&self, v: f32) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.set_volume(v)?)
    }

    pub fn rate(&self) -> Result<f32, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.rate()?)
    }

    pub fn set_rate(&self, v: f32) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.set_rate(v)?)
    }

    pub fn pitch(&self) -> Result<f32, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.pitch()?)
    }

    pub fn set_pitch(&self, v: f32) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.set_pitch(v)?)
    }

    pub fn voices_count(&self) -> Result<usize, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.voice_count()?)
    }

    pub fn get_voice_name(&self, idx: usize) -> Result<String, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.voice_name(idx)?)
    }

    pub fn get_voice_language(&self, idx: usize) -> Result<String, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.voice_language(idx)?)
    }

    pub fn voice(&self) -> Result<usize, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.voice()?)
    }

    pub fn set_voice(&self, idx: usize) -> Result<(), PrismErr> {
        let _g = self.guard();
        Ok(self.inner.set_voice(idx)?)
    }

    pub fn channels(&self) -> Result<usize, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.channels()?)
    }

    pub fn sample_rate(&self) -> Result<usize, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.sample_rate()?)
    }

    pub fn bit_depth(&self) -> Result<usize, PrismErr> {
        let _g = self.guard();
        Ok(self.inner.bit_depth()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prismer::Features as F;

    #[test]
    fn feature_names_match_prismer_bits() {
        for (name, flag) in [
            ("is_supported_at_runtime", F::IS_SUPPORTED_AT_RUNTIME),
            ("supports_speak", F::SPEAK),
            ("supports_speak_to_memory", F::SPEAK_TO_MEMORY),
            ("supports_braille", F::BRAILLE),
            ("supports_output", F::OUTPUT),
            ("supports_is_speaking", F::IS_SPEAKING),
            ("supports_stop", F::STOP),
            ("supports_pause", F::PAUSE),
            ("supports_resume", F::RESUME),
            ("supports_set_volume", F::SET_VOLUME),
            ("supports_get_volume", F::GET_VOLUME),
            ("supports_set_rate", F::SET_RATE),
            ("supports_get_rate", F::GET_RATE),
            ("supports_set_pitch", F::SET_PITCH),
            ("supports_get_pitch", F::GET_PITCH),
            ("supports_refresh_voices", F::REFRESH_VOICES),
            ("supports_count_voices", F::COUNT_VOICES),
            ("supports_get_voice_name", F::GET_VOICE_NAME),
            ("supports_get_voice_language", F::GET_VOICE_LANGUAGE),
            ("supports_get_voice", F::GET_VOICE),
            ("supports_set_voice", F::SET_VOICE),
            ("supports_get_channels", F::GET_CHANNELS),
            ("supports_get_sample_rate", F::GET_SAMPLE_RATE),
            ("supports_get_bit_depth", F::GET_BIT_DEPTH),
            (
                "performs_silence_trimming_on_speak",
                F::SILENCE_TRIMMING_ON_SPEAK,
            ),
            (
                "performs_silence_trimming_on_speak_to_memory",
                F::SILENCE_TRIMMING_ON_SPEAK_TO_MEMORY,
            ),
            ("supports_speak_ssml", F::SPEAK_SSML),
            ("supports_speak_to_memory_ssml", F::SPEAK_TO_MEMORY_SSML),
        ] {
            let f = BackendFeatures::from_bits(flag.0);
            assert!(f.get(name), "{name} is not bit {:#x}", flag.0);
            assert_eq!(f.to_bits(), flag.0, "{name}");
        }
    }

    #[test]
    fn feature_bits_mask_and_getters() {
        let f = BackendFeatures::from_bits(u64::MAX);
        assert_eq!(f.to_bits(), 0x1FFF_FFFD);
        let nvda = BackendFeatures::from_bits(0xb5);
        assert!(nvda.is_supported_at_runtime());
        assert!(nvda.supports_speak());
        assert!(nvda.supports_braille());
        assert!(nvda.supports_output());
        assert!(nvda.supports_stop());
        assert!(!nvda.supports_set_rate());
        assert!(!f.get("no_such_feature"));
        assert_eq!(BackendFeatures::from_bits(0b10 | (1 << 40)).to_bits(), 0);
    }

    #[test]
    fn backend_ids_are_prisms() {
        assert_eq!(backend_id::SAPI, 0x1D6DF72422CEEE66);
        assert_eq!(backend_id::NVDA, 0x89CC19C5C4AC1A56);
        assert_eq!(backend_id::JAWS, 0xAC3D60E9BD84B53E);
        assert_eq!(backend_id::ONE_CORE, 0x6797D32F0D994CB4);
    }

    #[test]
    fn error_display() {
        assert_eq!(
            PrismErr::Prism(prismer::Error::BackendNotAvailable).to_string(),
            "backend not available"
        );
        let e = PrismErr::Key(format!("No backend matching {}", py_repr_str("Foo")));
        assert_eq!(e.to_string(), "\"No backend matching 'Foo'\"");
        assert_eq!(
            PrismErr::Key("No backend matching 3".into()).to_string(),
            "'No backend matching 3'"
        );
    }

    #[test]
    fn repr_quotes_like_python() {
        assert_eq!(py_repr_str("abc"), "'abc'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_repr_str("a\\b\n"), "'a\\\\b\\n'");
    }

    #[test]
    fn text_checks() {
        assert_eq!(
            check_text("").unwrap_err().to_string(),
            "Text MUST NOT be empty"
        );
        assert_eq!(
            check_text("a\0b").unwrap_err().to_string(),
            "Text MUST NOT contain embedded NULs"
        );
        assert!(check_text("hello").is_ok());
    }
}
