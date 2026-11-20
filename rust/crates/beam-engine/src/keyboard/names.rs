//! Key names exactly as the `keyboard` package's Windows backend (`_winkeyboard.py`) makes
//! them, because every handler in the engine dispatches on those names: `"space"`, `"up"`,
//! a shifted digit arriving as `"!"`, a shifted letter as `"A"`.
//!
//! The package builds two tables once (`_setup_name_tables`): `to_name`, from
//! `(scan code, virtual key, extended, modifiers)` to the names a key event carries, and
//! `from_name`, from a normalised name to the scan codes a hook on that name must match.
//! Both come from the live keyboard layout through `ToUnicode`, `GetKeyNameTextW` and
//! `MapVirtualKeyW`, plus a fixed table of official virtual-key names. This file repeats the
//! same calls in the same order, including the package's reuse of one `ToUnicode` buffer
//! (a dead key's leftovers show up in the next key's name, which is the "^6" the package's
//! own comment describes).

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;

use super::canonical_names::CANONICAL_NAMES;

/// Modifier bits standing for the package's modifier tuples. The tuples are always built
/// in this order (`shift`, `alt gr`, `num lock`, `caps lock`, `scroll lock`), so a bit set
/// identifies a tuple exactly.
pub(crate) const MOD_SHIFT: u8 = 1;
pub(crate) const MOD_ALT_GR: u8 = 2;
pub(crate) const MOD_NUM_LOCK: u8 = 4;
pub(crate) const MOD_CAPS_LOCK: u8 = 8;
pub(crate) const MOD_SCROLL_LOCK: u8 = 16;

/// `distinct_modifiers`.
const DISTINCT_MODIFIERS: [u8; 8] = [
    0,
    MOD_SHIFT,
    MOD_ALT_GR,
    MOD_NUM_LOCK,
    MOD_SHIFT | MOD_NUM_LOCK,
    MOD_CAPS_LOCK,
    MOD_SHIFT | MOD_CAPS_LOCK,
    MOD_ALT_GR | MOD_NUM_LOCK,
];

/// `sided_modifiers`.
const SIDED_MODIFIERS: [&str; 4] = ["ctrl", "alt", "shift", "windows"];

/// `(scan_code, vk, is_extended, modifiers)`, a `to_name` key.
pub(crate) type Entry = (u32, u32, u32, u8);

/// `official_virtual_keys`: (name, is_keypad). Where the Python dict literal repeats a key
/// (0x15, 0x19) the last value wins, as it does in Python.
pub(crate) fn official_virtual_key(vk: u32) -> Option<(&'static str, bool)> {
    let v = match vk {
        0x03 => ("control-break processing", false),
        0x08 => ("backspace", false),
        0x09 => ("tab", false),
        0x0c => ("clear", false),
        0x0d => ("enter", false),
        0x10 => ("shift", false),
        0x11 => ("ctrl", false),
        0x12 => ("alt", false),
        0x13 => ("pause", false),
        0x14 => ("caps lock", false),
        0x15 => ("ime hangul mode", false),
        0x17 => ("ime junja mode", false),
        0x18 => ("ime final mode", false),
        0x19 => ("ime kanji mode", false),
        0x1b => ("esc", false),
        0x1c => ("ime convert", false),
        0x1d => ("ime nonconvert", false),
        0x1e => ("ime accept", false),
        0x1f => ("ime mode change request", false),
        0x20 => ("spacebar", false),
        0x21 => ("page up", false),
        0x22 => ("page down", false),
        0x23 => ("end", false),
        0x24 => ("home", false),
        0x25 => ("left", false),
        0x26 => ("up", false),
        0x27 => ("right", false),
        0x28 => ("down", false),
        0x29 => ("select", false),
        0x2a => ("print", false),
        0x2b => ("execute", false),
        0x2c => ("print screen", false),
        0x2d => ("insert", false),
        0x2e => ("delete", false),
        0x2f => ("help", false),
        0x30 => ("0", false),
        0x31 => ("1", false),
        0x32 => ("2", false),
        0x33 => ("3", false),
        0x34 => ("4", false),
        0x35 => ("5", false),
        0x36 => ("6", false),
        0x37 => ("7", false),
        0x38 => ("8", false),
        0x39 => ("9", false),
        0x41 => ("a", false),
        0x42 => ("b", false),
        0x43 => ("c", false),
        0x44 => ("d", false),
        0x45 => ("e", false),
        0x46 => ("f", false),
        0x47 => ("g", false),
        0x48 => ("h", false),
        0x49 => ("i", false),
        0x4a => ("j", false),
        0x4b => ("k", false),
        0x4c => ("l", false),
        0x4d => ("m", false),
        0x4e => ("n", false),
        0x4f => ("o", false),
        0x50 => ("p", false),
        0x51 => ("q", false),
        0x52 => ("r", false),
        0x53 => ("s", false),
        0x54 => ("t", false),
        0x55 => ("u", false),
        0x56 => ("v", false),
        0x57 => ("w", false),
        0x58 => ("x", false),
        0x59 => ("y", false),
        0x5a => ("z", false),
        0x5b => ("left windows", false),
        0x5c => ("right windows", false),
        0x5d => ("applications", false),
        0x5f => ("sleep", false),
        0x60 => ("0", true),
        0x61 => ("1", true),
        0x62 => ("2", true),
        0x63 => ("3", true),
        0x64 => ("4", true),
        0x65 => ("5", true),
        0x66 => ("6", true),
        0x67 => ("7", true),
        0x68 => ("8", true),
        0x69 => ("9", true),
        0x6a => ("*", true),
        0x6b => ("+", true),
        0x6c => ("separator", true),
        0x6d => ("-", true),
        0x6e => ("decimal", true),
        0x6f => ("/", true),
        0x70 => ("f1", false),
        0x71 => ("f2", false),
        0x72 => ("f3", false),
        0x73 => ("f4", false),
        0x74 => ("f5", false),
        0x75 => ("f6", false),
        0x76 => ("f7", false),
        0x77 => ("f8", false),
        0x78 => ("f9", false),
        0x79 => ("f10", false),
        0x7a => ("f11", false),
        0x7b => ("f12", false),
        0x7c => ("f13", false),
        0x7d => ("f14", false),
        0x7e => ("f15", false),
        0x7f => ("f16", false),
        0x80 => ("f17", false),
        0x81 => ("f18", false),
        0x82 => ("f19", false),
        0x83 => ("f20", false),
        0x84 => ("f21", false),
        0x85 => ("f22", false),
        0x86 => ("f23", false),
        0x87 => ("f24", false),
        0x90 => ("num lock", false),
        0x91 => ("scroll lock", false),
        0xa0 => ("left shift", false),
        0xa1 => ("right shift", false),
        0xa2 => ("left ctrl", false),
        0xa3 => ("right ctrl", false),
        0xa4 => ("left menu", false),
        0xa5 => ("right menu", false),
        0xa6 => ("browser back", false),
        0xa7 => ("browser forward", false),
        0xa8 => ("browser refresh", false),
        0xa9 => ("browser stop", false),
        0xaa => ("browser search key", false),
        0xab => ("browser favorites", false),
        0xac => ("browser start and home", false),
        0xad => ("volume mute", false),
        0xae => ("volume down", false),
        0xaf => ("volume up", false),
        0xb0 => ("next track", false),
        0xb1 => ("previous track", false),
        0xb2 => ("stop media", false),
        0xb3 => ("play/pause media", false),
        0xb4 => ("start mail", false),
        0xb5 => ("select media", false),
        0xb6 => ("start application 1", false),
        0xb7 => ("start application 2", false),
        0xbb => ("+", false),
        0xbc => (",", false),
        0xbd => ("-", false),
        0xbe => (".", false),
        0xe5 => ("ime process", false),
        0xf6 => ("attn", false),
        0xf7 => ("crsel", false),
        0xf8 => ("exsel", false),
        0xf9 => ("erase eof", false),
        0xfa => ("play", false),
        0xfb => ("zoom", false),
        0xfc => ("reserved ", false),
        0xfd => ("pa1", false),
        0xfe => ("clear", false),
        _ => return None,
    };
    Some(v)
}

/// `keypad_keys`: (scan_code, vk, is_extended).
const KEYPAD_KEYS: [(u32, u32, u32); 36] = [
    (126, 194, 0),
    (126, 194, 0),
    (28, 13, 1),
    (28, 13, 1),
    (53, 111, 1),
    (53, 111, 1),
    (55, 106, 0),
    (55, 106, 0),
    (69, 144, 1),
    (69, 144, 1),
    (71, 103, 0),
    (71, 36, 0),
    (72, 104, 0),
    (72, 38, 0),
    (73, 105, 0),
    (73, 33, 0),
    (74, 109, 0),
    (74, 109, 0),
    (75, 100, 0),
    (75, 37, 0),
    (76, 101, 0),
    (76, 12, 0),
    (77, 102, 0),
    (77, 39, 0),
    (78, 107, 0),
    (78, 107, 0),
    (79, 35, 0),
    (79, 97, 0),
    (80, 40, 0),
    (80, 98, 0),
    (81, 34, 0),
    (81, 99, 0),
    (82, 45, 0),
    (82, 96, 0),
    (83, 110, 0),
    (83, 46, 0),
];

pub(crate) fn is_keypad(scan: u32, vk: u32, ext: u32) -> bool {
    KEYPAD_KEYS.contains(&(scan, vk, ext))
}

static CANONICAL: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    // A dict literal keeps the LAST value for a repeated key; inserting in order does too.
    let mut m = HashMap::with_capacity(CANONICAL_NAMES.len());
    for (k, v) in CANONICAL_NAMES {
        m.insert(*k, *v);
    }
    m
});

/// `_canonical_names.normalize_name(name)`. Python raises `ValueError` on an empty name;
/// that is `None` here.
pub(crate) fn normalize_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let mut name = if name.chars().count() > 1 {
        name.to_lowercase()
    } else {
        name.to_string()
    };
    if name != "_" && name.contains('_') {
        name = name.replace('_', " ");
    }
    Some(match CANONICAL.get(name.as_str()) {
        Some(c) => (*c).to_string(),
        None => name,
    })
}

/// `normalize_name`'s `ValueError` text for an empty name.
pub(crate) fn normalize_error(name: &str) -> String {
    format!(
        "Can only normalize non-empty string names. Unexpected {}",
        crate::speech_out::py_repr_str(name)
    )
}

/// The package's module-level scratch buffers, shared by every caller as in Python.
struct Scratch {
    keyboard_state: [u8; 256],
    unicode_buffer: [u16; 32],
    name_buffer: [u16; 1024],
}

static SCRATCH: Mutex<Scratch> = Mutex::new(Scratch {
    keyboard_state: [0; 256],
    unicode_buffer: [0; 32],
    name_buffer: [0; 1024],
});

/// A `c_wchar` array's `.value`: everything before the first NUL.
#[cfg(windows)]
fn wide_value(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// `get_event_names(scan_code, vk, is_extended, modifiers)`.
pub(crate) fn get_event_names(scan: u32, vk: u32, ext: u32, mods: u8) -> Vec<String> {
    let mut names = Vec::new();
    let keypad = is_keypad(scan, vk, ext);
    let official = official_virtual_key(vk);
    if keypad {
        if let Some((n, _)) = official {
            names.push(n.to_string());
        }
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyNameTextW, MAPVK_VK_TO_CHAR, MapVirtualKeyW, ToUnicode,
        };
        let mut s = SCRATCH.lock();
        let s = &mut *s;
        s.keyboard_state[0x10] = if mods & MOD_SHIFT != 0 { 0x80 } else { 0 };
        s.keyboard_state[0x11] = if mods & MOD_ALT_GR != 0 { 0x80 } else { 0 };
        s.keyboard_state[0x12] = if mods & MOD_ALT_GR != 0 { 0x80 } else { 0 };
        s.keyboard_state[0x14] = u8::from(mods & MOD_CAPS_LOCK != 0);
        s.keyboard_state[0x90] = u8::from(mods & MOD_NUM_LOCK != 0);
        s.keyboard_state[0x91] = u8::from(mods & MOD_SCROLL_LOCK != 0);
        // SAFETY: both buffers are owned arrays of the sizes passed; ToUnicode writes at
        // most `unicode_buffer.len()` UTF-16 units and reads 256 bytes of key state.
        let ret = unsafe { ToUnicode(vk, scan, Some(&s.keyboard_state), &mut s.unicode_buffer, 0) };
        let value = wide_value(&s.unicode_buffer);
        if ret != 0 && !value.is_empty() {
            names.push(value);
            // A second call clears the dead-key state the first one left behind.
            // SAFETY: as above.
            unsafe {
                ToUnicode(vk, scan, Some(&s.keyboard_state), &mut s.unicode_buffer, 0);
            }
        }
        // `scan_code << 16 | is_extended << 24` through ctypes' c_long: truncated to 32 bits.
        let lparam = (scan.wrapping_shl(16) | ext.wrapping_shl(24)) as i32;
        // SAFETY: the buffer is an owned array; the call writes a NUL-terminated name into it.
        let name_ret = unsafe { GetKeyNameTextW(lparam, &mut s.name_buffer) };
        if name_ret != 0 {
            let value = wide_value(&s.name_buffer);
            if !value.is_empty() {
                names.push(value);
            }
        }
        // SAFETY: MapVirtualKeyW takes plain integers and cannot fault.
        let ch = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) } & 0xFF;
        if ch != 0 {
            if let Some(c) = char::from_u32(ch) {
                names.push(c.to_string());
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (scan, mods);
    }
    if !keypad {
        if let Some((n, _)) = official {
            names.push(n.to_string());
        }
    }
    names
}

/// `to_name` and `from_name` after `_setup_name_tables`.
pub(crate) struct NameTables {
    to_name: HashMap<Entry, Vec<String>>,
    /// name -> scan codes (`scan_code or -vk`), in `map_name`'s order, without repeats.
    from_name: HashMap<String, Vec<i32>>,
}

fn code_of(scan: u32, vk: u32) -> i32 {
    if scan != 0 { scan as i32 } else { -(vk as i32) }
}

fn modifiers_preference(mods: u8) -> u32 {
    match mods {
        0 => 0,
        MOD_SHIFT => 1,
        MOD_ALT_GR => 2,
        _ => 10,
    }
}

#[cfg(windows)]
fn scan_vk_pairs() -> Vec<(u32, u32)> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MAPVK_VK_TO_VSC_EX, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW,
    };
    let mut out = Vec::with_capacity(512);
    for sc in 0u32..0x100 {
        // SAFETY: MapVirtualKeyExW takes plain integers and cannot fault.
        out.push((sc, unsafe {
            MapVirtualKeyExW(sc, MAPVK_VSC_TO_VK_EX, None)
        }));
    }
    for vk in 0u32..0x100 {
        // SAFETY: as above.
        out.push((
            unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC_EX, None) },
            vk,
        ));
    }
    out
}

#[cfg(not(windows))]
fn scan_vk_pairs() -> Vec<(u32, u32)> {
    Vec::new()
}

fn build_tables() -> NameTables {
    let mut to_name: HashMap<Entry, Vec<String>> = HashMap::new();
    let mut from_entries: HashMap<String, Vec<(usize, Entry)>> = HashMap::new();
    for (scan, vk) in scan_vk_pairs() {
        for ext in [0u32, 1] {
            for mods in DISTINCT_MODIFIERS {
                let entry = (scan, vk, ext, mods);
                let names = get_event_names(scan, vk, ext, mods);
                if names.is_empty() {
                    continue;
                }
                let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
                let all: Vec<String> = names.into_iter().chain(lower).collect();
                for (i, n) in all.iter().enumerate() {
                    if let Some(norm) = normalize_name(n) {
                        from_entries.entry(norm).or_default().push((i, entry));
                    }
                }
                to_name.insert(entry, all);
            }
        }
    }
    // Alt Gr is added by hand, as the package does.
    for ext in [0u32, 1] {
        for mods in DISTINCT_MODIFIERS {
            let entry = (541, 162, ext, mods);
            to_name.insert(entry, vec!["alt gr".to_string()]);
            from_entries
                .entry("alt gr".to_string())
                .or_default()
                .push((1, entry));
        }
    }
    let mut from_name = HashMap::with_capacity(from_entries.len());
    for (name, mut entries) in from_entries {
        // sorted(set(entries), key=order_key)
        entries.sort();
        entries.dedup();
        entries.sort_by_key(|(i, (scan, vk, ext, mods))| {
            (modifiers_preference(*mods), *i, *ext, *vk, *scan)
        });
        let mut codes: Vec<i32> = Vec::new();
        for (_, (scan, vk, _, _)) in entries {
            let c = code_of(scan, vk);
            if !codes.contains(&c) {
                codes.push(c);
            }
        }
        from_name.insert(name, codes);
    }
    NameTables { to_name, from_name }
}

static TABLES: LazyLock<NameTables> = LazyLock::new(build_tables);
/// Entries first seen at event time (`to_name[entry] = list(get_event_names(*entry))`).
static LATE_NAMES: LazyLock<Mutex<HashMap<Entry, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `_setup_name_tables()`: build the tables now rather than on first use.
pub(crate) fn setup_name_tables() {
    LazyLock::force(&TABLES);
}

/// The name a key event carries (`KeyboardEvent.name`): the first of `to_name[entry]`,
/// normalised, or None when Windows has no name for the key at all.
pub(crate) fn event_name(scan: u32, vk: u32, ext: u32, mods: u8) -> Option<String> {
    let entry = (scan, vk, ext, mods);
    let first = match TABLES.to_name.get(&entry) {
        Some(names) => names.first().cloned(),
        None => {
            let mut late = LATE_NAMES.lock();
            late.entry(entry)
                .or_insert_with(|| get_event_names(scan, vk, ext, mods))
                .first()
                .cloned()
        }
    };
    first.and_then(|n| normalize_name(&n))
}

fn map_name(name: &str) -> Option<&'static [i32]> {
    TABLES
        .from_name
        .get(name)
        .map(Vec::as_slice)
        .filter(|c| !c.is_empty())
}

/// `keyboard.key_to_scan_codes(key, error_if_missing=True)`. The error is Python's
/// `str(ValueError(...))`.
pub(crate) fn key_to_scan_codes(key: &str) -> Result<Vec<i32>, String> {
    key_to_scan_codes_inner(key, true)
}

fn key_to_scan_codes_inner(key: &str, error_if_missing: bool) -> Result<Vec<i32>, String> {
    let Some(normalized) = normalize_name(key) else {
        return Err(normalize_error(key));
    };
    if SIDED_MODIFIERS.contains(&normalized.as_str()) {
        let mut left = key_to_scan_codes_inner(&format!("left {normalized}"), false)?;
        let right = key_to_scan_codes_inner(&format!("right {normalized}"), false)?;
        for c in right {
            if !left.contains(&c) {
                left.push(c);
            }
        }
        return Ok(left);
    }
    match map_name(&normalized) {
        Some(codes) => Ok(codes.to_vec()),
        None if error_if_missing => {
            let repr = crate::speech_out::py_repr_str;
            let inner = format!(
                "Key name {} is not mapped to any known key.",
                repr(&normalized)
            );
            Err(format!(
                "({}, ValueError({}))",
                repr(&format!(
                    "Key {} is not mapped to any known key.",
                    repr(key)
                )),
                repr(&inner)
            ))
        }
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_the_package() {
        assert_eq!(normalize_name("Up").as_deref(), Some("up"));
        assert_eq!(normalize_name("A").as_deref(), Some("A"));
        assert_eq!(normalize_name("left_ctrl").as_deref(), Some("left ctrl"));
        assert_eq!(normalize_name(" ").as_deref(), Some("space"));
        assert_eq!(normalize_name("\t").as_deref(), Some("tab"));
        assert_eq!(normalize_name("escape").as_deref(), Some("esc"));
        assert_eq!(normalize_name(""), None);
    }

    #[test]
    fn official_table_keeps_the_last_duplicate() {
        assert_eq!(official_virtual_key(0x15), Some(("ime hangul mode", false)));
        assert_eq!(official_virtual_key(0x19), Some(("ime kanji mode", false)));
        assert_eq!(official_virtual_key(0x60), Some(("0", true)));
        assert_eq!(official_virtual_key(0x5e), None);
    }
}
