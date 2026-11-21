//! Owner: E10. The mod installer of `config_ui.py`: `install_mod_interactive`, the
//! freeroam-layout cleanup and the stamped mod-version comparison. Prompts go through
//! `ui_sink::UiSink::message_box`.
//!
//! The Install Mod button and the updater's phase two both run this, so the post-update
//! install and the button are the same act.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use beam_base::pyval::{PyDict, PyValue, json_dumps_indent2, json_loads};

use crate::ui_sink::{Answer, Buttons, MessageBox, MessageKind, UiSink};
use crate::updater::{Version, parse_version, pyos};

pub const MOD_ZIP: &str = "bng_screenreader_mod.zip";
/// Written into the mod zip by the build's `package_mod()`; a file mtime cannot say which
/// of two zips is newer.
pub const MOD_VERSION_MEMBER: &str = "bnvda_mod_version.json";
pub const BNVDA_APP_NAME: &str = "bnvdaHook";

/// Where the installer looks: the program directory (the zip beside the exe) and
/// `os.getenv("LOCALAPPDATA", os.path.expanduser("~"))` (the game's folders).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInstallEnv {
    pub program_dir: PathBuf,
    pub local_appdata: PathBuf,
}

impl ModInstallEnv {
    pub fn real() -> Self {
        let local_appdata = match std::env::var_os("LOCALAPPDATA") {
            Some(v) => PathBuf::from(v),
            None => expanduser_home(),
        };
        ModInstallEnv {
            program_dir: beam_base::paths::program_dir(),
            local_appdata,
        }
    }
}

/// `os.path.expanduser("~")` on Windows: USERPROFILE, else HOMEDRIVE + HOMEPATH, else `~`.
fn expanduser_home() -> PathBuf {
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p);
    }
    match std::env::var_os("HOMEPATH") {
        None => PathBuf::from("~"),
        Some(hp) => {
            let mut s = std::env::var_os("HOMEDRIVE").unwrap_or_default();
            s.push(hp);
            PathBuf::from(s)
        }
    }
}

/// `_freeroam_layout_path(local_appdata)`.
pub fn freeroam_layout_path(local_appdata: &Path) -> PathBuf {
    local_appdata
        .join("BeamNG")
        .join("BeamNG.drive")
        .join("current")
        .join("settings")
        .join("ui_apps")
        .join("layouts")
        .join("default")
        .join("freeroam.uilayout.json")
}

/// `_load_bnvda_layout(layout_path)`: load a freeroam layout and count the obsolete BNVDA
/// app entries. `Err` is `str(e)` of the OSError, UnicodeError, JSONDecodeError or
/// ValueError the caller catches.
pub fn load_bnvda_layout(layout_path: &Path) -> Result<(PyDict, usize), String> {
    let mut bytes = Vec::new();
    fs::File::open(layout_path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| pyos::errno(&e, Some(layout_path)))?;
    // encoding="utf-8-sig"
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = std::str::from_utf8(body).map_err(|e| pyos::utf8_decode_error(body, &e))?;
    let layout = json_loads(text).map_err(|e| e.0)?;

    let PyValue::Dict(layout) = layout else {
        return Err("The layout's top-level JSON value is not an object.".to_string());
    };
    let Some(PyValue::List(apps)) = layout.get("apps") else {
        return Err("The layout does not contain an 'apps' list.".to_string());
    };
    let entry_count = apps.iter().filter(|a| is_bnvda_app(a)).count();
    Ok((layout, entry_count))
}

fn is_bnvda_app(app: &PyValue) -> bool {
    matches!(app, PyValue::Dict(d) if d.get("appName") == Some(&PyValue::from(BNVDA_APP_NAME)))
}

/// `_remove_bnvda_layout_entries(layout)`: remove every obsolete BNVDA app entry from a
/// validated layout.
pub fn remove_bnvda_layout_entries(layout: &mut PyDict) {
    if let Some(PyValue::List(apps)) = layout.get_mut("apps") {
        apps.retain(|a| !is_bnvda_app(a));
    }
}

/// `tempfile`'s `_RandomNameSequence`: eight characters of `[a-z0-9_]`.
fn random_name(state: &mut u64) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    (0..8)
        .map(|_| {
            // xorshift64*
            *state ^= *state >> 12;
            *state ^= *state << 25;
            *state ^= *state >> 27;
            let r = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            CHARS[(r >> 33) as usize % CHARS.len()] as char
        })
        .collect()
}

/// `tempfile.NamedTemporaryFile(prefix=..., suffix=".tmp", dir=..., delete=False)`.
fn named_temporary_file(dir: &Path, prefix: &str) -> Result<(fs::File, PathBuf), String> {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ ((std::process::id() as u64) << 32)
        | 1;
    for _ in 0..10000 {
        let path = dir.join(format!("{prefix}{}.tmp", random_name(&mut seed)));
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // Windows reports an existing directory as access denied; CPython retries.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && dir.is_dir() => continue,
            Err(e) => return Err(pyos::errno(&e, Some(&path))),
        }
    }
    Err("[Errno 17] No usable temporary file name found".to_string())
}

/// `shutil.copy2(src, dst)`: data, then the timestamps and the read-only bit.
pub(crate) fn copy2(src: &Path, dst: &Path) -> Result<PathBuf, String> {
    let dst = if dst.is_dir() {
        dst.join(src.file_name().unwrap_or_default())
    } else {
        dst.to_path_buf()
    };
    let mut fsrc = fs::File::open(src).map_err(|e| pyos::errno(&e, Some(src)))?;
    {
        let mut fdst = fs::File::create(&dst).map_err(|e| pyos::errno(&e, Some(&dst)))?;
        io::copy(&mut fsrc, &mut fdst).map_err(|e| pyos::errno(&e, None))?;
    }
    let meta = fs::metadata(src).map_err(|e| pyos::win(&e, Some(src), None))?;
    let times = fs::FileTimes::new()
        .set_accessed(meta.accessed().unwrap_or(std::time::SystemTime::now()))
        .set_modified(meta.modified().unwrap_or(std::time::SystemTime::now()));
    fs::OpenOptions::new()
        .write(true)
        .open(&dst)
        .and_then(|f| f.set_times(times))
        .map_err(|e| pyos::win(&e, Some(&dst), None))?;
    if meta.permissions().readonly() {
        fs::set_permissions(&dst, meta.permissions()).map_err(|e| pyos::win(&e, Some(&dst), None))?;
    }
    Ok(dst)
}

/// `_write_layout_with_backup(layout_path, layout)`: back up and atomically replace a
/// validated BeamNG UI layout. Returns the backup path.
pub fn write_layout_with_backup(layout_path: &Path, layout: &PyDict) -> Result<PathBuf, String> {
    let timestamp = beam_road::pytime::DateTime::local_now().strftime("%Y%m%d-%H%M%S-%f");
    write_layout_with_backup_at(layout_path, layout, &timestamp)
}

/// [`write_layout_with_backup`] with the backup's timestamp given.
pub fn write_layout_with_backup_at(
    layout_path: &Path,
    layout: &PyDict,
    timestamp: &str,
) -> Result<PathBuf, String> {
    let layout_dir = layout_path.parent().unwrap_or(Path::new(""));
    let layout_name = layout_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut backup = layout_path.as_os_str().to_os_string();
    backup.push(format!(".pre-bnvda-removal-{timestamp}.bak"));
    let backup_path = PathBuf::from(backup);

    let mut temp_path: Option<PathBuf> = None;
    let result = (|| -> Result<PathBuf, String> {
        let (mut f, path) = named_temporary_file(layout_dir, &format!("{layout_name}."))?;
        temp_path = Some(path.clone());
        let mut text = json_dumps_indent2(&PyValue::Dict(layout.clone()));
        text.push('\n');
        f.write_all(text.as_bytes())
            .and_then(|()| f.flush())
            .and_then(|()| f.sync_all())
            .map_err(|e| pyos::errno(&e, None))?;
        drop(f);

        copy2(layout_path, &backup_path)?;
        fs::rename(&path, layout_path).map_err(|e| pyos::win(&e, Some(&path), Some(layout_path)))?;
        temp_path = None;
        Ok(backup_path.clone())
    })();
    if let Some(t) = temp_path
        && t.exists()
    {
        let _ = fs::remove_file(&t);
    }
    result
}

/// `_mod_zip_version(path)`: `(parsed, raw)` for a mod zip's stamped version, or `None`
/// when unstamped. Unstamped is a real answer: a zip built before stamping, older than
/// any stamped one. A zip that cannot be opened is also `None`; the caller then falls
/// back to a byte comparison, and if that fails too it asks.
pub fn mod_zip_version(path: &Path) -> Option<(Version, String)> {
    let f = fs::File::open(path).ok()?;
    let mut zf = zip::ZipArchive::new(f).ok()?;
    let mut bytes = Vec::new();
    zf.by_name(MOD_VERSION_MEMBER)
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let raw = json_loads(text).ok()?;
    let PyValue::Dict(d) = raw else {
        return None;
    };
    let Some(PyValue::Str(version)) = d.get("version") else {
        return None;
    };
    let parsed = parse_version(version)?;
    Some((parsed, version.clone()))
}

/// `_same_bytes(a, b)`: true when two files are byte-identical. Unreadable counts as
/// "not sure", i.e. false.
pub fn same_bytes(a: &Path, b: &Path) -> bool {
    let inner = || -> io::Result<bool> {
        if fs::metadata(a)?.len() != fs::metadata(b)?.len() {
            return Ok(false);
        }
        let mut fa = fs::File::open(a)?;
        let mut fb = fs::File::open(b)?;
        let mut ba = vec![0u8; 65536];
        let mut bb = vec![0u8; 65536];
        loop {
            let na = read_full(&mut fa, &mut ba)?;
            let nb = read_full(&mut fb, &mut bb)?;
            if ba[..na] != bb[..nb] {
                return Ok(false);
            }
            if na == 0 {
                return Ok(true);
            }
        }
    };
    inner().unwrap_or(false)
}

fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

fn message(
    sink: &dyn UiSink,
    kind: MessageKind,
    title: &str,
    text: String,
    buttons: Buttons,
    default: Answer,
) -> Answer {
    sink.message_box(&MessageBox {
        kind,
        title: title.to_string(),
        text,
        buttons,
        default,
    })
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `install_mod_interactive(parent)` on the real program directory and LOCALAPPDATA:
/// true only when it reaches the "Installation Complete" box. Every early return has
/// already told the user what went wrong, so the value is for the log only.
pub fn install_mod_interactive(sink: &dyn UiSink) -> bool {
    install_mod_interactive_in(sink, &ModInstallEnv::real())
}

/// [`install_mod_interactive`] against explicit directories.
pub fn install_mod_interactive_in(sink: &dyn UiSink, env: &ModInstallEnv) -> bool {
    // Locate the zip alongside this program.
    let src_zip = env.program_dir.join(MOD_ZIP);
    if !src_zip.is_file() {
        let dir = src_zip.parent().map(path_str).unwrap_or_default();
        message(
            sink,
            MessageKind::Error,
            "Mod File Not Found",
            format!(
                "'{MOD_ZIP}' was not found in the program directory:\n{dir}\n\n\
                 Installation cannot proceed."
            ),
            Buttons::Ok,
            Answer::Ok,
        );
        return false;
    }

    // Confirm the game's mods directory exists.
    let local_appdata = &env.local_appdata;
    let mods_dir = local_appdata
        .join("BeamNG")
        .join("BeamNG.drive")
        .join("current")
        .join("mods");
    if !mods_dir.is_dir() {
        message(
            sink,
            MessageKind::Error,
            "Mods Directory Not Found",
            format!(
                "The BeamNG.drive mods directory was not found:\n{}\n\n\
                 Please run BeamNG.drive at least once before installing the mod.",
                path_str(&mods_dir)
            ),
            Buttons::Ok,
            Answer::Ok,
        );
        return false;
    }

    // Decide whether to copy, by the STAMPED version, never by file mtime: the two zips
    // live on different machines and are moved by tools that each treat mtime
    // differently, so an mtime ordering is a comparison of clocks.
    let dst_zip = mods_dir.join(MOD_ZIP);
    let mut do_copy = false;
    let mut mod_result = "kept the existing installed file";
    if !dst_zip.is_file() {
        do_copy = true;
    } else if same_bytes(&src_zip, &dst_zip) {
        mod_result = "the installed file is already identical";
    } else {
        let src_ver = mod_zip_version(&src_zip);
        let dst_ver = mod_zip_version(&dst_zip);
        match (&src_ver, &dst_ver) {
            // Newer, the same version rebuilt, or replacing a zip from before stamping.
            (Some(_), None) => do_copy = true,
            (Some(s), Some(d)) if s.0 >= d.0 => do_copy = true,
            (Some(s), Some(d)) => {
                let ans = message(
                    sink,
                    MessageKind::Warning,
                    "Older Version Detected",
                    format!(
                        "The mod in the program directory is version {}, but version {} \
                         is already installed.\n\
                         Do you want to install the older one anyway?",
                        s.1, d.1
                    ),
                    Buttons::YesNo,
                    Answer::No,
                );
                if ans == Answer::Yes {
                    do_copy = true;
                }
            }
            (None, _) => {
                // Nothing to order them by; say exactly that rather than inventing a
                // direction.
                let ans = message(
                    sink,
                    MessageKind::Question,
                    "Cannot Determine Version",
                    "The mod in the program directory differs from the one already \
                     installed, but neither carries a version stamp, so which is \
                     newer cannot be determined.\n\n\
                     Install the one from the program directory?"
                        .to_string(),
                    Buttons::YesNo,
                    Answer::Yes,
                );
                if ans == Answer::Yes {
                    do_copy = true;
                }
            }
        }
    }

    if do_copy {
        if let Err(e) = copy2(&src_zip, &dst_zip) {
            message(
                sink,
                MessageKind::Error,
                "Installation Failed",
                format!("Failed to copy mod file:\n{e}"),
                Buttons::Ok,
                Answer::Ok,
            );
            return false;
        }

        // Verify the result.
        if !dst_zip.is_file() {
            message(
                sink,
                MessageKind::Warning,
                "Verification Failed",
                "The copy appeared to succeed but the destination file cannot be found.\n\
                 Installation may have failed."
                    .to_string(),
                Buttons::Ok,
                Answer::Ok,
            );
            return false;
        }
        mod_result = "copied the mod file";
    }

    // Offer to remove the obsolete HUD app from the freeroam layout.
    let layout_path = freeroam_layout_path(local_appdata);
    let mut layout_result = "layout file was not found; no cleanup was needed".to_string();
    let mut backup_path: Option<PathBuf> = None;
    if layout_path.is_file() {
        match load_bnvda_layout(&layout_path) {
            Err(e) => {
                layout_result = "could not inspect the layout; it was not changed".to_string();
                message(
                    sink,
                    MessageKind::Warning,
                    "Invalid Freeroam Layout",
                    format!(
                        "The mod file was {mod_result}, but the BeamNG.drive freeroam UI \
                         layout could not be read:\n{}\n\n{e}\n\n\
                         The layout was not changed.",
                        path_str(&layout_path)
                    ),
                    Buttons::Ok,
                    Answer::Ok,
                );
            }
            Ok((mut layout, entry_count)) => {
                if entry_count == 0 {
                    layout_result = "obsolete BNVDA Hook entry was not present".to_string();
                } else {
                    let noun = if entry_count == 1 { "entry" } else { "entries" };
                    let answer = message(
                        sink,
                        MessageKind::Question,
                        "Remove Obsolete HUD App",
                        format!(
                            "The freeroam UI layout contains {entry_count} obsolete BNVDA \
                             Hook {noun}. The mod now starts automatically when the game \
                             starts, so these HUD entries are no longer needed.\n\n\
                             Do you want to back up the layout and remove them?"
                        ),
                        Buttons::YesNo,
                        Answer::No,
                    );
                    if answer == Answer::Yes {
                        remove_bnvda_layout_entries(&mut layout);
                        match write_layout_with_backup(&layout_path, &layout) {
                            Err(e) => {
                                layout_result =
                                    "cleanup failed; the original layout was preserved".to_string();
                                message(
                                    sink,
                                    MessageKind::Warning,
                                    "Layout Cleanup Failed",
                                    format!(
                                        "The mod file was {mod_result}, but the obsolete BNVDA \
                                         Hook layout entry could not be removed:\n\
                                         {}\n\n{e}\n\n\
                                         The original layout was preserved.",
                                        path_str(&layout_path)
                                    ),
                                    Buttons::Ok,
                                    Answer::Ok,
                                );
                            }
                            Ok(b) => {
                                backup_path = Some(b);
                                layout_result = format!("removed {entry_count} obsolete {noun}");
                            }
                        }
                    } else {
                        layout_result =
                            format!("kept {entry_count} obsolete {noun} at your request");
                    }
                }
            }
        }
    }

    let backup_detail = match &backup_path {
        Some(b) => format!("\nLayout backup: {}", path_str(b)),
        None => String::new(),
    };

    message(
        sink,
        MessageKind::Info,
        "Installation Complete",
        format!(
            "The mod installation is complete.\n\n\
             Mod file: {mod_result}.\n\
             Freeroam layout: {layout_result}.\n\n\
             {}{backup_detail}",
            path_str(&dst_zip)
        ),
        Buttons::Ok,
        Answer::Ok,
    );
    true
}
