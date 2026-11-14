//! DPAPI-sealed secrets in the config, from secretstore.py.
//!
//! The config file is plain JSON readable by anything running as the user, and an AI
//! describer key is a billable credential, not a preference. DPAPI's
//! `CryptProtectData` ties the ciphertext to the *user account*, so a copied config, a
//! synced profile or a backup carries nothing usable, while beamtel needs no
//! passphrase, no keyring and no first-run prompt. Scope is the user, not the machine
//! (no `CRYPTPROTECT_LOCAL_MACHINE`): another account on the same PC must not be able
//! to read the key.
//!
//! This is at-rest protection, not secrecy from code running as the user.

use base64::Engine as _;
use windows::Win32::Foundation::{GetLastError, HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

use crate::pyfmt::py_strip;
use crate::pyval::{PyDict, PyValue};

/// Version the marker rather than the payload: the day this has to move off DPAPI the
/// reader must be able to tell the two ciphertexts apart.
pub const PREFIX: &str = "dpapi:v1:";

/// Bound into the ciphertext, so a blob lifted out of this config cannot be decrypted
/// by handing it to some other DPAPI consumer running as the same user. A constant in
/// open source and therefore not itself a secret -- it narrows the blast radius.
const ENTROPY: &[u8] = b"beamtel:config:v1";

pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x01;

/// Setting NAMES whose values are secrets. Matched as a substring because the names
/// are compound (`ai_describer_openai_api_key`), and by name rather than by value so a
/// provider added later is covered the day it is added. The MCP server masks its
/// `get_config` output off this same list.
pub const SECRET_NAME_PARTS: [&str; 6] =
    ["api_key", "apikey", "token", "secret", "password", "passwd"];

/// `is_secret_setting(name)`: does this setting name hold a secret?
pub fn is_secret_setting(name: &str) -> bool {
    let low = name.to_lowercase();
    SECRET_NAME_PARTS.iter().any(|part| low.contains(part))
}

/// Whether DPAPI can be reached at all. Always true on Windows, which is the only
/// platform this builds for.
pub fn available() -> bool {
    true
}

/// A value carrying the DPAPI marker.
pub fn is_protected(value: &str) -> bool {
    value.starts_with(PREFIX)
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// `ctypes.GetLastError()` as Python prints it (a signed C long).
fn last_error() -> i32 {
    // SAFETY: GetLastError only reads the calling thread's last-error value.
    unsafe { GetLastError().0 as i32 }
}

/// Copy an output blob and free it with LocalFree, as DPAPI requires.
fn take_blob(out: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let bytes = if out.pbData.is_null() {
        Vec::new()
    } else {
        // SAFETY: on success DPAPI returns `cbData` readable bytes at `pbData`.
        unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() }
    };
    if !out.pbData.is_null() {
        // SAFETY: `pbData` was allocated by DPAPI with LocalAlloc and is freed once.
        unsafe {
            LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
        }
    }
    bytes
}

/// Encrypt `plaintext` for this Windows user. Returns a `PREFIX`-marked string.
///
/// The text is stripped first. Falls back to returning the plaintext unchanged if
/// DPAPI fails -- refusing to store the key at all would turn a hardening measure into
/// a feature outage, and the caller has just validated that key against a live
/// endpoint.
pub fn protect(plaintext: &str) -> String {
    let text = py_strip(plaintext).to_string();
    if text.is_empty() || is_protected(&text) {
        return text;
    }
    let data_in = blob(text.as_bytes());
    let ent_in = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point at live buffers for the duration of the call, and
    // `out` is a valid out-parameter that DPAPI fills.
    let ok = unsafe {
        CryptProtectData(
            &data_in,
            PCWSTR::null(),
            Some(&ent_in),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok.is_err() {
        log::error!(
            "CryptProtectData failed ({}); storing config secret unencrypted.",
            last_error()
        );
        return text;
    }
    let sealed = take_blob(&out);
    format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(sealed)
    )
}

/// `base64.b64decode(s)` with Python's default (non-validating) rules: characters
/// outside the alphabet are skipped, decoding stops once padding completes a quad,
/// and a dangling partial quad is an error.
pub fn py_b64decode(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() {
        // `.encode("ascii")` raises first.
        return None;
    }
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut quad_pos = 0u32;
    let mut left = 0u32;
    let mut pads = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' {
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    return Some(out);
                }
            }
            continue;
        }
        let Some(v) = val(c) else {
            continue;
        };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = v;
            }
            1 => {
                quad_pos = 2;
                out.push(((left << 2) | (v >> 4)) as u8);
                left = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push(((left << 4) | (v >> 2)) as u8);
                left = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push(((left << 6) | v) as u8);
                left = 0;
            }
        }
    }
    if quad_pos != 0 {
        return None;
    }
    Some(out)
}

/// Decrypt a stored secret.
///
/// Returns the plaintext; `Some("")` for an empty value; and **None** when a protected
/// value could not be decrypted. None is a distinct answer on purpose: a blob from
/// another Windows account is not the same situation as no key configured, and only
/// the caller can say the one thing that helps -- set it again. A value with no marker
/// is returned as-is, which is what carries configs written before this existed.
pub fn unprotect(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    if !is_protected(value) {
        return Some(value.to_string());
    }
    let Some(raw) = py_b64decode(&value[PREFIX.len()..]) else {
        log::error!("Config secret is not valid base64; treating as unreadable.");
        return None;
    };
    let data_in = blob(&raw);
    let ent_in = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &data_in,
            None,
            Some(&ent_in),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok.is_err() {
        log::error!(
            "CryptUnprotectData failed ({}). The stored secret was encrypted by a \
             different Windows user account and must be set again.",
            last_error()
        );
        return None;
    }
    let plain = take_blob(&out);
    match String::from_utf8(plain) {
        Ok(s) => Some(s),
        Err(_) => {
            log::error!("Decrypted config secret is not UTF-8; treating as unreadable.");
            None
        }
    }
}

/// Encrypt any still-plaintext secret in `cfg` in place. Returns true if changed.
///
/// Called from the same place in both load paths as `speech_config::migrate_config`,
/// so an existing install is protected on the next run rather than only when the user
/// next re-enters the key.
pub fn migrate_config(cfg: &mut PyDict) -> bool {
    if !available() {
        return false;
    }
    let mut changed = false;
    let names: Vec<String> = cfg.keys().cloned().collect();
    for name in names {
        if !is_secret_setting(&name) {
            continue;
        }
        let Some(PyValue::Str(value)) = cfg.get(&name) else {
            continue;
        };
        if py_strip(value).is_empty() || is_protected(value) {
            continue;
        }
        let sealed = protect(value);
        if is_protected(&sealed) {
            cfg.insert(name, PyValue::Str(sealed));
            changed = true;
        }
    }
    changed
}
