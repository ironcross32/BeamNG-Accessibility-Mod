//! FFI to prism.dll (loaded with libloading), from the prismatoid wheel's prism package.
//!
//! The C API bound here is Prism 0.18.2's, as the wheel's cffi module declares it
//! (`prism/_prism_cffi.pyd`: every function type, constant and struct offset) and as the
//! upstream `include/prism.h` at tag `v0.18.2` declares it. Every symbol is checked against
//! prism.dll's export table.
//!
//! The header documents no ownership rules, so this layer follows exactly what the wheel
//! does: every `const char *` Prism returns (backend name, registry name, voice name, error
//! string) is borrowed and copied at once, never freed; every `PrismBackend *` from
//! `create`/`acquire` is passed to `prism_backend_free` when dropped, including one whose
//! `prism_backend_initialize` failed; a context is closed with `prism_shutdown`.
//!
//! The calling convention is `PRISM_CALL`, which is `__cdecl` on Windows: `extern "C"`.
//!
//! Nothing here panics or aborts on a missing DLL or symbol: [`Prism::global`] is `None`
//! and the speech layer behaves as speech.py does when `import prism` fails.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use libloading::os::windows::{
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, Library,
};

// ---------------------------------------------------------------------------
// Constants (values from the cffi module, which match prism.h 0.18.2)
// ---------------------------------------------------------------------------

/// `PRISM_OK` and the `PrismError` enum.
pub mod error_code {
    pub const PRISM_OK: i32 = 0;
    pub const PRISM_ERROR_NOT_INITIALIZED: i32 = 1;
    pub const PRISM_ERROR_INVALID_PARAM: i32 = 2;
    pub const PRISM_ERROR_NOT_IMPLEMENTED: i32 = 3;
    pub const PRISM_ERROR_NO_VOICES: i32 = 4;
    pub const PRISM_ERROR_VOICE_NOT_FOUND: i32 = 5;
    pub const PRISM_ERROR_SPEAK_FAILURE: i32 = 6;
    pub const PRISM_ERROR_MEMORY_FAILURE: i32 = 7;
    pub const PRISM_ERROR_RANGE_OUT_OF_BOUNDS: i32 = 8;
    pub const PRISM_ERROR_INTERNAL: i32 = 9;
    pub const PRISM_ERROR_NOT_SPEAKING: i32 = 10;
    pub const PRISM_ERROR_NOT_PAUSED: i32 = 11;
    pub const PRISM_ERROR_ALREADY_PAUSED: i32 = 12;
    pub const PRISM_ERROR_INVALID_UTF8: i32 = 13;
    pub const PRISM_ERROR_INVALID_OPERATION: i32 = 14;
    pub const PRISM_ERROR_ALREADY_INITIALIZED: i32 = 15;
    pub const PRISM_ERROR_BACKEND_NOT_AVAILABLE: i32 = 16;
    pub const PRISM_ERROR_UNKNOWN: i32 = 17;
    pub const PRISM_ERROR_INVALID_AUDIO_FORMAT: i32 = 18;
    pub const PRISM_ERROR_INTERNAL_BACKEND_LIMIT_EXCEEDED: i32 = 19;
    pub const PRISM_ERROR_BACKEND_ENTERED_UNDEFINED_STATE: i32 = 20;
    pub const PRISM_ERROR_LIBRARY_LOAD_FAILED: i32 = 21;
    pub const PRISM_ERROR_LIBRARY_INVALID: i32 = 22;
    pub const PRISM_ERROR_INCOMPATIBLE_ABI: i32 = 23;
    pub const PRISM_ERROR_COUNT: i32 = 24;
}

/// `PrismLogLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    None = 5,
}

/// `PRISM_CONFIG_VERSION`; also what `prism_config_init()` writes into `version`.
pub const PRISM_CONFIG_VERSION: u8 = 3;

/// `PrismBackendId`: a 64-bit hash. 0 is invalid.
pub type BackendId = u64;

/// The built-in backend ids (`prism.BackendId`).
pub mod backend_id {
    use super::BackendId;
    pub const INVALID: BackendId = 0;
    pub const SAPI: BackendId = 0x1D6DF72422CEEE66;
    pub const AV_SPEECH: BackendId = 0x28E3429577805C24;
    pub const VOICE_OVER: BackendId = 0xCB4897961A754BCB;
    pub const SPEECH_DISPATCHER: BackendId = 0xE3D6F895D949EBFE;
    pub const NVDA: BackendId = 0x89CC19C5C4AC1A56;
    pub const JAWS: BackendId = 0xAC3D60E9BD84B53E;
    pub const ONE_CORE: BackendId = 0x6797D32F0D994CB4;
    pub const ORCA: BackendId = 0x10AA1FC05A17F96C;
    pub const ANDROID_SCREEN_READER: BackendId = 0xD199C175AEEC494B;
    pub const ANDROID_TTS: BackendId = 0xBC175831BFE4E5CC;
    pub const WEB_SPEECH: BackendId = 0x3572538D44D44A8F;
    pub const UIA: BackendId = 0x6238F019DB678F8E;
    pub const ZDSR: BackendId = 0x3D93C56C9E7F2A2E;
    pub const ZOOM_TEXT: BackendId = 0xAE439D62DC7B1479;
    pub const BOY_PC_READER: BackendId = 0x285ABA1C16F3300F;
    pub const PC_TALKER: BackendId = 0x344B951962E3B835;
    pub const SENSE_READER: BackendId = 0xED4760890B55C2F2;
    pub const SYSTEM_ACCESS: BackendId = 0x8380F2A37B2C3EB6;
    pub const WINDOW_EYES: BackendId = 0x9120D89908785C13;
    pub const SPIEL: BackendId = 0x478B44F14AD3D89C;
}

// ---------------------------------------------------------------------------
// Capability bits
// ---------------------------------------------------------------------------

/// `(attribute name, bit position)` for every field of `prism.BackendFeatures`, in
/// declaration order. Bit 1 is unused (the dataclass skips it, as does the header).
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

/// `prism.BackendFeatures`: the `uint64_t` from `prism_backend_get_features`, keeping only
/// the bits the dataclass names (its `from_bits` drops the rest, so `to_bits` cannot
/// return them).
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

    /// `getattr(feats, name, False)`: an unknown name is false.
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

/// What the Python binding raises. `Display` is `str(exception)`, because speech.py puts
/// `{e}` in log lines and `test_speak` returns `str(e)` to the configurator's message box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrismErr {
    /// A `PrismError` subclass: the code and `prism_error_string(code)`.
    Prism { code: i32, message: Option<String> },
    /// `KeyError(msg)`; `str()` of a KeyError is the repr of its argument.
    Key(String),
    /// `ValueError(msg)`.
    Value(String),
    /// `RuntimeError(msg)`.
    Runtime(String),
    /// `UnicodeDecodeError` from a strict UTF-8 decode of a string Prism returned.
    Unicode(String),
}

impl std::fmt::Display for PrismErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrismErr::Prism { code, message } => match message {
                Some(m) if !m.is_empty() => f.write_str(m),
                _ => write!(f, "Prism Error Code: {code}"),
            },
            PrismErr::Key(m) => f.write_str(&py_repr_str(m)),
            PrismErr::Value(m) | PrismErr::Runtime(m) | PrismErr::Unicode(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PrismErr {}

/// Python's `repr()` of a str, for the characters a backend name or message can carry.
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

/// The wheel's `Text MUST NOT be empty` / `embedded NULLs` checks, done before any call
/// that takes text.
pub fn check_text(text: &str) -> Result<CString, PrismErr> {
    if text.is_empty() {
        return Err(PrismErr::Prism {
            code: error_code::PRISM_ERROR_INVALID_PARAM,
            message: Some("Text MUST NOT be empty".into()),
        });
    }
    CString::new(text).map_err(|_| PrismErr::Prism {
        code: error_code::PRISM_ERROR_INVALID_PARAM,
        message: Some("Text MUST NOT contain embedded NULLs".into()),
    })
}

// ---------------------------------------------------------------------------
// Raw C types
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct RawContext {
    _private: [u8; 0],
}
#[repr(C)]
pub struct RawBackend {
    _private: [u8; 0],
}
#[repr(C)]
pub struct RawRegistry {
    _private: [u8; 0],
}

pub type AvailabilityCallbackFn =
    unsafe extern "C" fn(userdata: *mut c_void, backend: u64, name: *const c_char, available: bool);
pub type LogCallbackFn = unsafe extern "C" fn(
    userdata: *mut c_void,
    level: c_int,
    source: *const c_char,
    message: *const c_char,
);

/// `PrismConfig`: 48 bytes, align 8; offsets 0, 8, 16, 24, 32, 36, 40, 44 (checked with
/// `ffi.offsetof` on the wheel, and by a test below).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismConfig {
    pub version: u8,
    pub registry: *mut RawRegistry,
    pub availability_callback: Option<AvailabilityCallbackFn>,
    pub availability_userdata: *mut c_void,
    pub availability_poll_interval_ms: u32,
    pub availability_debounce_samples: u32,
    pub availability_backoff_max_ms: u32,
    pub availability_auto_power_manage: bool,
}

/// `PrismLogHandler`: 16 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrismLogHandler {
    pub fn_: Option<LogCallbackFn>,
    pub userdata: *mut c_void,
}

type FnVersion = unsafe extern "C" fn() -> u32;
type FnVersionString = unsafe extern "C" fn() -> *const c_char;
type FnConfigInit = unsafe extern "C" fn() -> PrismConfig;
type FnInit = unsafe extern "C" fn(*mut PrismConfig) -> *mut RawContext;
type FnShutdown = unsafe extern "C" fn(*mut RawContext);
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnSetLogHandler = unsafe extern "C" fn(PrismLogHandler) -> PrismLogHandler;
type FnSetLogLevel = unsafe extern "C" fn(c_int) -> c_int;
type FnVoid = unsafe extern "C" fn();
type FnRegistryCount = unsafe extern "C" fn(*mut RawContext) -> usize;
type FnRegistryIdAt = unsafe extern "C" fn(*mut RawContext, usize) -> u64;
type FnRegistryId = unsafe extern "C" fn(*mut RawContext, *const c_char) -> u64;
type FnRegistryName = unsafe extern "C" fn(*mut RawContext, u64) -> *const c_char;
type FnRegistryPriority = unsafe extern "C" fn(*mut RawContext, u64) -> c_int;
type FnRegistryExists = unsafe extern "C" fn(*mut RawContext, u64) -> bool;
type FnRegistryById = unsafe extern "C" fn(*mut RawContext, u64) -> *mut RawBackend;
type FnRegistryBest = unsafe extern "C" fn(*mut RawContext) -> *mut RawBackend;
type FnCtxVoid = unsafe extern "C" fn(*mut RawContext);
type FnAutoPower = unsafe extern "C" fn() -> bool;
type FnBackend = unsafe extern "C" fn(*mut RawBackend) -> c_int;
type FnBackendFree = unsafe extern "C" fn(*mut RawBackend);
type FnBackendName = unsafe extern "C" fn(*mut RawBackend) -> *const c_char;
type FnBackendFeatures = unsafe extern "C" fn(*mut RawBackend) -> u64;
type FnBackendTextBool = unsafe extern "C" fn(*mut RawBackend, *const c_char, bool) -> c_int;
type FnBackendText = unsafe extern "C" fn(*mut RawBackend, *const c_char) -> c_int;
type FnBackendOutBool = unsafe extern "C" fn(*mut RawBackend, *mut bool) -> c_int;
type FnBackendOutF32 = unsafe extern "C" fn(*mut RawBackend, *mut f32) -> c_int;
type FnBackendSetF32 = unsafe extern "C" fn(*mut RawBackend, f32) -> c_int;
type FnBackendOutSize = unsafe extern "C" fn(*mut RawBackend, *mut usize) -> c_int;
type FnBackendSetSize = unsafe extern "C" fn(*mut RawBackend, usize) -> c_int;
type FnBackendIdxStr = unsafe extern "C" fn(*mut RawBackend, usize, *mut *const c_char) -> c_int;

/// Every function this crate calls, resolved once from prism.dll. The `Library` is owned
/// here, so each pointer stays valid for as long as this struct lives; everything that
/// calls one holds an `Arc<Api>`.
pub struct Api {
    pub version: FnVersion,
    pub version_string: FnVersionString,
    pub config_init: FnConfigInit,
    pub init: FnInit,
    pub shutdown: FnShutdown,
    pub error_string: FnErrorString,
    pub set_log_handler: FnSetLogHandler,
    pub set_log_level: FnSetLogLevel,
    pub log_flush: FnVoid,
    pub log_shutdown: FnVoid,
    pub registry_count: FnRegistryCount,
    pub registry_id_at: FnRegistryIdAt,
    pub registry_id: FnRegistryId,
    pub registry_name: FnRegistryName,
    pub registry_priority: FnRegistryPriority,
    pub registry_exists: FnRegistryExists,
    pub registry_create: FnRegistryById,
    pub registry_create_best: FnRegistryBest,
    pub registry_acquire: FnRegistryById,
    pub registry_acquire_best: FnRegistryBest,
    pub availability_poll_pause: FnCtxVoid,
    pub availability_poll_resume: FnCtxVoid,
    pub availability_auto_power_supported: FnAutoPower,
    pub backend_initialize: FnBackend,
    pub backend_free: FnBackendFree,
    pub backend_name: FnBackendName,
    pub backend_get_features: FnBackendFeatures,
    pub backend_speak: FnBackendTextBool,
    pub backend_output: FnBackendTextBool,
    pub backend_braille: FnBackendText,
    pub backend_stop: FnBackend,
    pub backend_pause: FnBackend,
    pub backend_resume: FnBackend,
    pub backend_is_speaking: FnBackendOutBool,
    pub backend_get_volume: FnBackendOutF32,
    pub backend_set_volume: FnBackendSetF32,
    pub backend_get_rate: FnBackendOutF32,
    pub backend_set_rate: FnBackendSetF32,
    pub backend_get_pitch: FnBackendOutF32,
    pub backend_set_pitch: FnBackendSetF32,
    pub backend_refresh_voices: FnBackend,
    pub backend_count_voices: FnBackendOutSize,
    pub backend_get_voice_name: FnBackendIdxStr,
    pub backend_get_voice_language: FnBackendIdxStr,
    pub backend_get_voice: FnBackendOutSize,
    pub backend_set_voice: FnBackendSetSize,
    pub backend_get_channels: FnBackendOutSize,
    pub backend_get_sample_rate: FnBackendOutSize,
    pub backend_get_bit_depth: FnBackendOutSize,
    _lib: Library,
}

impl Api {
    /// Load prism.dll from `path` and resolve every symbol. Any failure is returned as a
    /// message, never a panic.
    pub fn load(path: &Path) -> Result<Api, String> {
        // SAFETY: loading runs prism.dll's DllMain. It is the library the wheel ships and
        // the Python build loads the same file; the extra search flags let its own
        // dependencies resolve from its directory, as the wheel's os.add_dll_directory does.
        let lib = unsafe {
            Library::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map_err(|e| format!("could not load {}: {e}", path.display()))?;

        macro_rules! sym {
            ($name:literal) => {{
                // SAFETY: the type each symbol is read as is the signature the cffi module
                // and prism.h 0.18.2 declare for it; the copied pointer is only called while
                // `_lib` (moved into the returned Api) keeps the DLL mapped.
                let s = unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!("prism.dll has no symbol {}: {e}", $name))?;
                *s
            }};
        }

        Ok(Api {
            version: sym!("prism_version"),
            version_string: sym!("prism_version_string"),
            config_init: sym!("prism_config_init"),
            init: sym!("prism_init"),
            shutdown: sym!("prism_shutdown"),
            error_string: sym!("prism_error_string"),
            set_log_handler: sym!("prism_set_log_handler"),
            set_log_level: sym!("prism_set_log_level"),
            log_flush: sym!("prism_log_flush"),
            log_shutdown: sym!("prism_log_shutdown"),
            registry_count: sym!("prism_registry_count"),
            registry_id_at: sym!("prism_registry_id_at"),
            registry_id: sym!("prism_registry_id"),
            registry_name: sym!("prism_registry_name"),
            registry_priority: sym!("prism_registry_priority"),
            registry_exists: sym!("prism_registry_exists"),
            registry_create: sym!("prism_registry_create"),
            registry_create_best: sym!("prism_registry_create_best"),
            registry_acquire: sym!("prism_registry_acquire"),
            registry_acquire_best: sym!("prism_registry_acquire_best"),
            availability_poll_pause: sym!("prism_availability_poll_pause"),
            availability_poll_resume: sym!("prism_availability_poll_resume"),
            availability_auto_power_supported: sym!("prism_availability_auto_power_supported"),
            backend_initialize: sym!("prism_backend_initialize"),
            backend_free: sym!("prism_backend_free"),
            backend_name: sym!("prism_backend_name"),
            backend_get_features: sym!("prism_backend_get_features"),
            backend_speak: sym!("prism_backend_speak"),
            backend_output: sym!("prism_backend_output"),
            backend_braille: sym!("prism_backend_braille"),
            backend_stop: sym!("prism_backend_stop"),
            backend_pause: sym!("prism_backend_pause"),
            backend_resume: sym!("prism_backend_resume"),
            backend_is_speaking: sym!("prism_backend_is_speaking"),
            backend_get_volume: sym!("prism_backend_get_volume"),
            backend_set_volume: sym!("prism_backend_set_volume"),
            backend_get_rate: sym!("prism_backend_get_rate"),
            backend_set_rate: sym!("prism_backend_set_rate"),
            backend_get_pitch: sym!("prism_backend_get_pitch"),
            backend_set_pitch: sym!("prism_backend_set_pitch"),
            backend_refresh_voices: sym!("prism_backend_refresh_voices"),
            backend_count_voices: sym!("prism_backend_count_voices"),
            backend_get_voice_name: sym!("prism_backend_get_voice_name"),
            backend_get_voice_language: sym!("prism_backend_get_voice_language"),
            backend_get_voice: sym!("prism_backend_get_voice"),
            backend_set_voice: sym!("prism_backend_set_voice"),
            backend_get_channels: sym!("prism_backend_get_channels"),
            backend_get_sample_rate: sym!("prism_backend_get_sample_rate"),
            backend_get_bit_depth: sym!("prism_backend_get_bit_depth"),
            _lib: lib,
        })
    }

    /// `_check_error`: 0 is success; anything else becomes the matching error with
    /// `prism_error_string(code)` as its message.
    pub fn check(&self, code: c_int) -> Result<(), PrismErr> {
        if code == error_code::PRISM_OK {
            return Ok(());
        }
        // SAFETY: prism_error_string takes any enum value and returns a pointer to a
        // NUL-terminated string Prism owns (or NULL), which is copied before returning.
        let msg = unsafe { borrowed_str((self.error_string)(code)) };
        Err(PrismErr::Prism {
            code,
            message: msg.map(|s| s.into_owned()),
        })
    }

    /// `prism_version_string()`.
    pub fn version_str(&self) -> String {
        // SAFETY: no arguments; returns a static NUL-terminated string or NULL.
        unsafe { borrowed_str((self.version_string)()) }
            .map(|s| s.into_owned())
            .unwrap_or_default()
    }
}

/// Copy of a borrowed C string, lossily decoded (for log text and error strings).
///
/// # Safety
/// `p` is NULL or points at a NUL-terminated string valid for the duration of the call.
unsafe fn borrowed_str<'a>(p: *const c_char) -> Option<std::borrow::Cow<'a, str>> {
    if p.is_null() {
        None
    } else {
        // SAFETY: non-null and NUL-terminated per the function's contract.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy())
    }
}

/// Strict UTF-8 copy of a borrowed C string, as `ffi.string(p).decode("utf-8")`.
///
/// # Safety
/// As [`borrowed_str`].
unsafe fn borrowed_str_strict(p: *const c_char) -> Result<String, PrismErr> {
    if p.is_null() {
        return Err(PrismErr::Runtime(
            "cannot use string() on <cdata 'char *' NULL>".into(),
        ));
    }
    // SAFETY: non-null and NUL-terminated per the function's contract.
    let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|e| PrismErr::Unicode(format!("'utf-8' codec can't decode bytes: {e}")))
}

// ---------------------------------------------------------------------------
// The library: loaded once per process, like `import prism`
// ---------------------------------------------------------------------------

static GLOBAL: OnceLock<Option<Prism>> = OnceLock::new();

/// The loaded prism.dll. Cheap to clone.
#[derive(Clone)]
pub struct Prism {
    api: Arc<Api>,
}

impl Prism {
    /// The process's prism.dll, loaded on first use from `beam_base::paths::find_prism_dll`.
    /// `None` (with a warning logged once) when the DLL or a symbol is missing.
    pub fn global() -> Option<&'static Prism> {
        GLOBAL
            .get_or_init(|| {
                let result = match beam_base::paths::find_prism_dll() {
                    None => Err("prism.dll not found beside the program or in the \
                                 project virtualenv"
                        .to_string()),
                    Some(path) => Api::load(&path),
                };
                match result {
                    Ok(api) => Some(Prism { api: Arc::new(api) }),
                    Err(e) => {
                        log::warn!("Prism not available; speech is disabled: {e}");
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Load from an explicit path (tests, tools).
    pub fn load(path: &Path) -> Result<Prism, String> {
        Api::load(path).map(|api| Prism { api: Arc::new(api) })
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    pub fn version_string(&self) -> String {
        self.api.version_str()
    }

    /// `prism.set_log_level(level)`; returns the previous level's raw value.
    pub fn set_log_level(&self, level: LogLevel) -> i32 {
        // SAFETY: takes and returns a plain enum value.
        unsafe { (self.api.set_log_level)(level as c_int) }
    }

    /// `prism.install_logging(logger_name)`: route Prism's own log through the `log`
    /// facade with target `<logger_name>.<source>`. Once per process; later calls are
    /// no-ops, as in the wheel.
    pub fn install_logging(&self, logger_name: &str) {
        let mut installed = LOG_INSTALLED.lock().unwrap_or_else(|p| p.into_inner());
        if *installed {
            return;
        }
        let _ = LOG_TARGET.set(logger_name.to_string());
        let handler = PrismLogHandler {
            fn_: Some(log_forward),
            userdata: std::ptr::null_mut(),
        };
        // SAFETY: `log_forward` is a 'static extern "C" fn that never unwinds and ignores
        // its userdata; the previous handler returned by value is dropped (the wheel only
        // keeps it to restore on uninstall, which this crate does not offer).
        let _prev = unsafe { (self.api.set_log_handler)(handler) };
        *installed = true;
    }

    /// Whether [`Prism::install_logging`] has run in this process.
    pub fn logging_installed() -> bool {
        *LOG_INSTALLED.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The wheel's atexit hook: `prism_log_flush()` then `prism_log_shutdown()`.
    pub fn log_flush_and_shutdown(&self) {
        // SAFETY: no arguments; Prism's own logger teardown.
        unsafe {
            (self.api.log_flush)();
            (self.api.log_shutdown)();
        }
    }

    /// `prism.Context(on_availability=cb)`, with the wheel's defaults for the polling
    /// fields (0, 0, 0, auto power on).
    pub fn context(
        &self,
        on_availability: Option<AvailabilityFn>,
    ) -> Result<Arc<Context>, PrismErr> {
        Context::new(self.api.clone(), on_availability)
    }
}

static LOG_INSTALLED: Mutex<bool> = Mutex::new(false);
static LOG_TARGET: OnceLock<String> = OnceLock::new();

unsafe extern "C" fn log_forward(
    _userdata: *mut c_void,
    level: c_int,
    source: *const c_char,
    message: *const c_char,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: Prism passes NUL-terminated strings (or NULL) valid for this call.
        let src = unsafe { borrowed_str(source) }.unwrap_or("prism".into());
        // SAFETY: as above.
        let msg = unsafe { borrowed_str(message) }.unwrap_or_default();
        let lvl = match level {
            0 => log::Level::Trace,
            1 => log::Level::Debug,
            2 => log::Level::Info,
            3 => log::Level::Warn,
            4 => log::Level::Error,
            _ => log::Level::Info,
        };
        let prefix = LOG_TARGET.get().map(String::as_str).unwrap_or("prism");
        let target = format!("{prefix}.{src}");
        log::log!(target: &target, lvl, "{msg}");
    });
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// The availability callback: `(backend id, name, available)`. Runs on a Prism thread.
pub type AvailabilityFn = Arc<dyn Fn(BackendId, &str, bool) + Send + Sync>;

/// `prism.Context`. Dropping it calls `prism_shutdown`. Backends hold an `Arc` to their
/// context, so the context outlives every backend made from it.
pub struct Context {
    api: Arc<Api>,
    raw: *mut RawContext,
    /// The boxed callback whose address is the C userdata; freed after shutdown.
    availability: Option<*mut AvailabilityFn>,
    /// Serialises calls on this context from different threads.
    calls: Mutex<()>,
}

// SAFETY: the raw context is only touched through `&self` methods that take `calls`, and
// in Drop (exclusive). Prism runs its own availability thread against the same context,
// so it is built to be used from more than one thread; the Python binding calls a context
// from any thread without a lock.
unsafe impl Send for Context {}
// SAFETY: as above; every shared access is serialised by `calls`.
unsafe impl Sync for Context {}

unsafe extern "C" fn availability_trampoline(
    userdata: *mut c_void,
    backend: u64,
    name: *const c_char,
    available: bool,
) {
    let _ = std::panic::catch_unwind(|| {
        if userdata.is_null() {
            return;
        }
        // SAFETY: userdata is the `Box<AvailabilityFn>` leaked in `Context::new`, which is
        // only freed in Drop after `prism_shutdown` has stopped the callbacks.
        let cb = unsafe { &*(userdata as *const AvailabilityFn) };
        // SAFETY: Prism passes a NUL-terminated name (or NULL) valid for this call.
        let name = unsafe { borrowed_str(name) }.unwrap_or_default();
        cb(backend, &name, available);
    });
}

impl Context {
    fn new(
        api: Arc<Api>,
        on_availability: Option<AvailabilityFn>,
    ) -> Result<Arc<Context>, PrismErr> {
        // SAFETY: returns a PrismConfig by value; no pointers in it are owned.
        let mut cfg = unsafe { (api.config_init)() };
        let mut userdata: Option<*mut AvailabilityFn> = None;
        if let Some(cb) = on_availability {
            let raw = Box::into_raw(Box::new(cb));
            userdata = Some(raw);
            cfg.availability_callback = Some(availability_trampoline);
            cfg.availability_userdata = raw as *mut c_void;
            cfg.availability_poll_interval_ms = 0;
            cfg.availability_debounce_samples = 0;
            cfg.availability_backoff_max_ms = 0;
            cfg.availability_auto_power_manage = true;
        }
        // SAFETY: cfg is a valid PrismConfig for the duration of the call; the callback and
        // userdata it names stay valid until Drop runs prism_shutdown.
        let raw = unsafe { (api.init)(&mut cfg) };
        if raw.is_null() {
            if let Some(p) = userdata {
                // SAFETY: Prism did not keep the config; reclaim the box we leaked above.
                drop(unsafe { Box::from_raw(p) });
            }
            return Err(PrismErr::Runtime("Prism could not be initialized".into()));
        }
        Ok(Arc::new(Context {
            api,
            raw,
            availability: userdata,
            calls: Mutex::new(()),
        }))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `backends_count`.
    pub fn backends_count(&self) -> usize {
        let _g = self.guard();
        // SAFETY: raw is a live context (freed only in Drop).
        unsafe { (self.api.registry_count)(self.raw) }
    }

    /// `id_of(index)`.
    pub fn id_at(&self, index: usize) -> Result<BackendId, PrismErr> {
        let _g = self.guard();
        // SAFETY: raw is a live context; any index is accepted (0 means out of range).
        let id = unsafe { (self.api.registry_id_at)(self.raw, index) };
        if id == 0 {
            return Err(PrismErr::Key(format!("No backend matching {index}")));
        }
        Ok(id)
    }

    /// `id_of(name)`.
    pub fn id_of_name(&self, name: &str) -> Result<BackendId, PrismErr> {
        // The wheel encodes the str; an embedded NUL would raise there too.
        let c = CString::new(name).map_err(|_| PrismErr::Value("embedded null byte".into()))?;
        let _g = self.guard();
        // SAFETY: raw is a live context and `c` a NUL-terminated string alive for the call.
        let id = unsafe { (self.api.registry_id)(self.raw, c.as_ptr()) };
        if id == 0 {
            return Err(PrismErr::Key(format!(
                "No backend matching {}",
                py_repr_str(name)
            )));
        }
        Ok(id)
    }

    /// `name_of(id)`.
    pub fn name_of(&self, id: BackendId) -> Result<String, PrismErr> {
        let _g = self.guard();
        // SAFETY: raw is a live context; the returned string is borrowed and copied below
        // while the guard is held.
        let p = unsafe { (self.api.registry_name)(self.raw, id) };
        if p.is_null() {
            return Err(PrismErr::Key("Backend ID not found".into()));
        }
        // SAFETY: non-null NUL-terminated string owned by the registry.
        unsafe { borrowed_str_strict(p) }
    }

    /// `priority_of(id)`.
    pub fn priority_of(&self, id: BackendId) -> i32 {
        let _g = self.guard();
        // SAFETY: raw is a live context.
        unsafe { (self.api.registry_priority)(self.raw, id) }
    }

    /// `exists(id)`.
    pub fn exists(&self, id: BackendId) -> bool {
        let _g = self.guard();
        // SAFETY: raw is a live context.
        unsafe { (self.api.registry_exists)(self.raw, id) }
    }

    fn wrap(self: &Arc<Self>, raw: *mut RawBackend) -> Result<Backend, PrismErr> {
        if raw.is_null() {
            return Err(PrismErr::Value("Invalid or unsupported backend".into()));
        }
        // Built before initialize so a failed initialize still frees the handle, as the
        // wheel's __del__ does for a Backend whose __init__ raised.
        let backend = Backend {
            ctx: self.clone(),
            raw,
            calls: Mutex::new(()),
        };
        // SAFETY: raw is a non-null handle just returned by the registry.
        let res = unsafe { (self.api.backend_initialize)(raw) };
        if res != error_code::PRISM_OK && res != error_code::PRISM_ERROR_ALREADY_INITIALIZED {
            self.api.check(res)?;
        }
        Ok(backend)
    }

    /// `create(id)`: a new backend instance.
    pub fn create(self: &Arc<Self>, id: BackendId) -> Result<Backend, PrismErr> {
        let raw = {
            let _g = self.guard();
            // SAFETY: raw context is live.
            unsafe { (self.api.registry_create)(self.raw, id) }
        };
        self.wrap(raw)
    }

    /// `create_best()`.
    pub fn create_best(self: &Arc<Self>) -> Result<Backend, PrismErr> {
        let raw = {
            let _g = self.guard();
            // SAFETY: raw context is live.
            unsafe { (self.api.registry_create_best)(self.raw) }
        };
        self.wrap(raw)
    }

    /// `acquire(id)`.
    pub fn acquire(self: &Arc<Self>, id: BackendId) -> Result<Backend, PrismErr> {
        let raw = {
            let _g = self.guard();
            // SAFETY: raw context is live.
            unsafe { (self.api.registry_acquire)(self.raw, id) }
        };
        self.wrap(raw)
    }

    /// `acquire_best()`: the highest-priority backend available now.
    pub fn acquire_best(self: &Arc<Self>) -> Result<Backend, PrismErr> {
        let raw = {
            let _g = self.guard();
            // SAFETY: raw context is live.
            unsafe { (self.api.registry_acquire_best)(self.raw) }
        };
        self.wrap(raw)
    }

    pub fn pause_availability_polling(&self) {
        let _g = self.guard();
        // SAFETY: raw context is live.
        unsafe { (self.api.availability_poll_pause)(self.raw) }
    }

    pub fn resume_availability_polling(&self) {
        let _g = self.guard();
        // SAFETY: raw context is live.
        unsafe { (self.api.availability_poll_resume)(self.raw) }
    }

    pub fn auto_power_supported(&self) -> bool {
        // SAFETY: no arguments.
        unsafe { (self.api.availability_auto_power_supported)() }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: exclusive access; raw came from prism_init and is shut down exactly once.
        // No backend outlives this (each holds an Arc to the context).
        unsafe { (self.api.shutdown)(self.raw) };
        if let Some(p) = self.availability.take() {
            // SAFETY: shutdown has stopped the availability thread, so nothing can call the
            // trampoline with this userdata any more; reclaim the box leaked in `new`.
            drop(unsafe { Box::from_raw(p) });
        }
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// `prism.Backend`. Dropping it calls `prism_backend_free`.
pub struct Backend {
    ctx: Arc<Context>,
    raw: *mut RawBackend,
    /// Serialises calls on this handle.
    calls: Mutex<()>,
}

// SAFETY: the handle is only used through `&self` methods holding `calls`, and in Drop
// (exclusive), so no two threads ever call into the same backend at once.
unsafe impl Send for Backend {}
// SAFETY: as above.
unsafe impl Sync for Backend {}

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: exclusive access; raw came from the registry and is freed exactly once,
        // while `ctx` (dropped after this) keeps the context alive.
        unsafe { (self.ctx.api.backend_free)(self.raw) };
    }
}

impl Backend {
    fn api(&self) -> &Api {
        &self.ctx.api
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `name`.
    pub fn name(&self) -> Result<String, PrismErr> {
        let _g = self.guard();
        // SAFETY: raw is a live backend; the string is borrowed and copied under the guard.
        let p = unsafe { (self.api().backend_name)(self.raw) };
        // SAFETY: NULL or a NUL-terminated string owned by the backend.
        unsafe { borrowed_str_strict(p) }
    }

    /// `features`.
    pub fn features(&self) -> BackendFeatures {
        let _g = self.guard();
        // SAFETY: raw is a live backend.
        BackendFeatures::from_bits(unsafe { (self.api().backend_get_features)(self.raw) })
    }

    pub fn speak(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        let c = check_text(text)?;
        let _g = self.guard();
        // SAFETY: live backend; `c` is NUL-terminated UTF-8 alive for the call.
        let r = unsafe { (self.api().backend_speak)(self.raw, c.as_ptr(), interrupt) };
        self.api().check(r)
    }

    pub fn output(&self, text: &str, interrupt: bool) -> Result<(), PrismErr> {
        let c = check_text(text)?;
        let _g = self.guard();
        // SAFETY: live backend; `c` is NUL-terminated UTF-8 alive for the call.
        let r = unsafe { (self.api().backend_output)(self.raw, c.as_ptr(), interrupt) };
        self.api().check(r)
    }

    pub fn braille(&self, text: &str) -> Result<(), PrismErr> {
        let c = check_text(text)?;
        let _g = self.guard();
        // SAFETY: live backend; `c` is NUL-terminated UTF-8 alive for the call.
        let r = unsafe { (self.api().backend_braille)(self.raw, c.as_ptr()) };
        self.api().check(r)
    }

    fn simple(&self, f: FnBackend) -> Result<(), PrismErr> {
        let _g = self.guard();
        // SAFETY: live backend; `f` is one of the no-argument backend calls.
        let r = unsafe { f(self.raw) };
        self.api().check(r)
    }

    pub fn stop(&self) -> Result<(), PrismErr> {
        self.simple(self.api().backend_stop)
    }
    pub fn pause(&self) -> Result<(), PrismErr> {
        self.simple(self.api().backend_pause)
    }
    pub fn resume(&self) -> Result<(), PrismErr> {
        self.simple(self.api().backend_resume)
    }
    pub fn refresh_voices(&self) -> Result<(), PrismErr> {
        self.simple(self.api().backend_refresh_voices)
    }

    pub fn is_speaking(&self) -> Result<bool, PrismErr> {
        let mut out = false;
        let _g = self.guard();
        // SAFETY: live backend; `out` is a valid bool to write.
        let r = unsafe { (self.api().backend_is_speaking)(self.raw, &mut out) };
        self.api().check(r).map(|_| out)
    }

    fn get_f32(&self, f: FnBackendOutF32) -> Result<f32, PrismErr> {
        let mut out = 0f32;
        let _g = self.guard();
        // SAFETY: live backend; `out` is a valid float to write.
        let r = unsafe { f(self.raw, &mut out) };
        self.api().check(r).map(|_| out)
    }

    fn set_f32(&self, f: FnBackendSetF32, v: f32) -> Result<(), PrismErr> {
        let _g = self.guard();
        // SAFETY: live backend; plain value argument.
        let r = unsafe { f(self.raw, v) };
        self.api().check(r)
    }

    fn get_size(&self, f: FnBackendOutSize) -> Result<usize, PrismErr> {
        let mut out = 0usize;
        let _g = self.guard();
        // SAFETY: live backend; `out` is a valid size_t to write.
        let r = unsafe { f(self.raw, &mut out) };
        self.api().check(r).map(|_| out)
    }

    pub fn volume(&self) -> Result<f32, PrismErr> {
        self.get_f32(self.api().backend_get_volume)
    }
    /// Prism normalises volume, rate and pitch to 0.0-1.0.
    pub fn set_volume(&self, v: f32) -> Result<(), PrismErr> {
        self.set_f32(self.api().backend_set_volume, v)
    }
    pub fn rate(&self) -> Result<f32, PrismErr> {
        self.get_f32(self.api().backend_get_rate)
    }
    pub fn set_rate(&self, v: f32) -> Result<(), PrismErr> {
        self.set_f32(self.api().backend_set_rate, v)
    }
    pub fn pitch(&self) -> Result<f32, PrismErr> {
        self.get_f32(self.api().backend_get_pitch)
    }
    pub fn set_pitch(&self, v: f32) -> Result<(), PrismErr> {
        self.set_f32(self.api().backend_set_pitch, v)
    }

    pub fn voices_count(&self) -> Result<usize, PrismErr> {
        self.get_size(self.api().backend_count_voices)
    }

    fn voice_str(&self, f: FnBackendIdxStr, idx: usize) -> Result<String, PrismErr> {
        let mut p: *const c_char = std::ptr::null();
        let _g = self.guard();
        // SAFETY: live backend; `p` is a valid out-pointer. On success Prism stores a
        // string it owns, which is copied before the guard is released.
        let r = unsafe { f(self.raw, idx, &mut p) };
        self.api().check(r)?;
        // SAFETY: NULL or a NUL-terminated string owned by the backend.
        unsafe { borrowed_str_strict(p) }
    }

    pub fn get_voice_name(&self, idx: usize) -> Result<String, PrismErr> {
        self.voice_str(self.api().backend_get_voice_name, idx)
    }
    pub fn get_voice_language(&self, idx: usize) -> Result<String, PrismErr> {
        self.voice_str(self.api().backend_get_voice_language, idx)
    }

    pub fn voice(&self) -> Result<usize, PrismErr> {
        self.get_size(self.api().backend_get_voice)
    }
    pub fn set_voice(&self, idx: usize) -> Result<(), PrismErr> {
        let _g = self.guard();
        // SAFETY: live backend; plain value argument.
        let r = unsafe { (self.api().backend_set_voice)(self.raw, idx) };
        self.api().check(r)
    }

    pub fn channels(&self) -> Result<usize, PrismErr> {
        self.get_size(self.api().backend_get_channels)
    }
    pub fn sample_rate(&self) -> Result<usize, PrismErr> {
        self.get_size(self.api().backend_get_sample_rate)
    }
    pub fn bit_depth(&self) -> Result<usize, PrismErr> {
        self.get_size(self.api().backend_get_bit_depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn config_layout_matches_cffi() {
        assert_eq!(size_of::<PrismConfig>(), 48);
        assert_eq!(align_of::<PrismConfig>(), 8);
        assert_eq!(offset_of!(PrismConfig, version), 0);
        assert_eq!(offset_of!(PrismConfig, registry), 8);
        assert_eq!(offset_of!(PrismConfig, availability_callback), 16);
        assert_eq!(offset_of!(PrismConfig, availability_userdata), 24);
        assert_eq!(offset_of!(PrismConfig, availability_poll_interval_ms), 32);
        assert_eq!(offset_of!(PrismConfig, availability_debounce_samples), 36);
        assert_eq!(offset_of!(PrismConfig, availability_backoff_max_ms), 40);
        assert_eq!(offset_of!(PrismConfig, availability_auto_power_manage), 44);
        assert_eq!(size_of::<PrismLogHandler>(), 16);
    }

    #[test]
    fn feature_bits_match_header_constants() {
        // PRISM_BACKEND_* values from the cffi module.
        let f = BackendFeatures::from_bits(u64::MAX);
        assert_eq!(f.to_bits(), 0x1FFF_FFFD);
        let nvda = BackendFeatures::from_bits(0xb5);
        assert!(nvda.is_supported_at_runtime());
        assert!(nvda.supports_speak());
        assert!(nvda.supports_braille());
        assert!(nvda.supports_output());
        assert!(nvda.supports_stop());
        assert!(!nvda.supports_set_rate());
        assert!(BackendFeatures::from_bits(1 << 12).get("supports_set_rate"));
        assert!(BackendFeatures::from_bits(2048).supports_get_volume());
        assert!(BackendFeatures::from_bits(2097152).supports_set_voice());
        assert!(!f.get("no_such_feature"));
        // Bit 1 and bits above 28 are dropped, as the dataclass drops them.
        assert_eq!(BackendFeatures::from_bits(0b10 | (1 << 40)).to_bits(), 0);
    }

    #[test]
    fn error_display_is_python_str() {
        let e = PrismErr::Prism {
            code: 16,
            message: Some("Backend not available".into()),
        };
        assert_eq!(e.to_string(), "Backend not available");
        let e = PrismErr::Prism {
            code: 99,
            message: None,
        };
        assert_eq!(e.to_string(), "Prism Error Code: 99");
        // str(KeyError(f"No backend matching {'Foo'!r}"))
        let e = PrismErr::Key(format!("No backend matching {}", py_repr_str("Foo")));
        assert_eq!(e.to_string(), "\"No backend matching 'Foo'\"");
        assert_eq!(
            PrismErr::Key("No backend matching 3".into()).to_string(),
            "'No backend matching 3'"
        );
        assert_eq!(
            PrismErr::Value("Invalid or unsupported backend".into()).to_string(),
            "Invalid or unsupported backend"
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
    fn text_checks_match_wheel() {
        assert_eq!(
            check_text("").unwrap_err().to_string(),
            "Text MUST NOT be empty"
        );
        assert_eq!(
            check_text("a\0b").unwrap_err().to_string(),
            "Text MUST NOT contain embedded NULLs"
        );
        assert!(check_text("hello").is_ok());
    }
}
