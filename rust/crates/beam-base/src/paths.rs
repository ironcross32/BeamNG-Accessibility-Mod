//! Config directory, config path, program directory and data-file lookup (configurator.py, beamtel.py, updater.py).
//!
//! The Python has three notions of "where we are":
//!
//! - `CONFIG_DIR`: `%LOCALAPPDATA%\beamtel`, falling back to the home directory when
//!   LOCALAPPDATA is not set ([`config_dir`]).
//! - `program_dir()` / `_get_program_dir()`: the directory the user launched the
//!   executable from, where `bng_screenreader_mod.zip` and updates live. Python uses
//!   `sys.argv[0]` there because a Nuitka onefile build runs from a temp extraction
//!   directory; a Rust exe runs in place, so [`program_dir`] is the exe's directory.
//! - `BASE_DIR`/`HERE`: where the data files (`hrtf_*.npz`) are. Beside the exe in a
//!   release; in a source checkout the exe is under `rust/target/<profile>/` and the
//!   data files are at the repository root, so [`find_data_file`] also walks up to it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The config directory's name under LOCALAPPDATA (or home).
pub const CONFIG_DIR_NAME: &str = "beamtel";
/// The config file's name inside [`config_dir`].
pub const CONFIG_FILENAME: &str = "beamtel_config.json";

/// `_get_config_dir()`: `%LOCALAPPDATA%\beamtel`, or `<home>\beamtel` when LOCALAPPDATA
/// is not set. An empty LOCALAPPDATA yields the relative path `beamtel`, as
/// `os.path.join("", "beamtel")` does.
pub fn config_dir() -> PathBuf {
    config_dir_from(
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("HOMEDRIVE"),
        std::env::var_os("HOMEPATH"),
    )
}

/// [`config_dir`] with the environment passed in, for tests.
pub fn config_dir_from(
    localappdata: Option<OsString>,
    userprofile: Option<OsString>,
    homedrive: Option<OsString>,
    homepath: Option<OsString>,
) -> PathBuf {
    let base = match localappdata {
        Some(v) => PathBuf::from(v),
        None => expanduser_home(userprofile, homedrive, homepath),
    };
    base.join(CONFIG_DIR_NAME)
}

/// `os.path.expanduser("~")` on Windows (ntpath, Python 3.8+): USERPROFILE, else
/// HOMEDRIVE + HOMEPATH, else `~` unchanged.
fn expanduser_home(
    userprofile: Option<OsString>,
    homedrive: Option<OsString>,
    homepath: Option<OsString>,
) -> PathBuf {
    if let Some(p) = userprofile {
        return PathBuf::from(p);
    }
    match homepath {
        None => PathBuf::from("~"),
        Some(hp) => {
            let drive = homedrive.unwrap_or_default();
            // ntpath.join(drive, homepath): a drive like "C:" and a rooted "\Users\x"
            // concatenate.
            let mut s = drive;
            s.push(hp);
            PathBuf::from(s)
        }
    }
}

/// [`config_dir`], created if missing (both Python modules `os.makedirs` it at import).
pub fn ensure_config_dir() -> std::io::Result<PathBuf> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `CONFIG_PATH`: `<config_dir>\beamtel_config.json`.
pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILENAME)
}

/// The directory holding the running executable: where the user launched it from,
/// where `bng_screenreader_mod.zip` sits and where an update is written.
///
/// Falls back to the current directory if the OS cannot say (it always can on
/// Windows).
pub fn program_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The repository root when running from a source checkout: the nearest ancestor of
/// the exe's directory, then of the current directory, that contains
/// `rust/Cargo.toml`. `None` for an installed release.
pub fn repo_root() -> Option<PathBuf> {
    let mut starts = vec![program_dir()];
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.iter().find_map(|start| find_repo_root_from(start))
}

/// The nearest ancestor of `start` (itself included) that contains `rust/Cargo.toml`.
pub fn find_repo_root_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("rust").join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

/// A shipped data file (`hrtf_mit_kemar.npz`, `HRTF_DATA_LICENSES.md`,
/// `bng_screenreader_mod.zip`...): beside the exe first, then at the repository root
/// of a source checkout. `None` if it is in neither.
pub fn find_data_file(name: &str) -> Option<PathBuf> {
    find_data_file_in(name, &program_dir(), repo_root().as_deref())
}

/// [`find_data_file`] with the two places passed in, for tests.
pub fn find_data_file_in(name: &str, program: &Path, repo: Option<&Path>) -> Option<PathBuf> {
    let beside = program.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    let in_repo = repo?.join(name);
    in_repo.is_file().then_some(in_repo)
}

/// The directory the HRTF `.npz` sets are loaded from: the `base_dir` that Python's
/// `select_hrtf_set` is handed. Wherever the default set is found, else the program
/// directory (where a missing file then fails to load, as it does in Python).
pub fn hrtf_base_dir() -> PathBuf {
    let default = crate::hrtf_sets::resolve(crate::hrtf_sets::DEFAULT_HRTF_SET);
    find_data_file(default.filename)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(program_dir)
}

/// `prism.dll`: beside the exe in a release; in a source checkout, the copy inside the
/// pinned `prismatoid` wheel in the project's virtualenv
/// (`.venv\Lib\site-packages\prism\_native\prism.dll`), so both builds load the
/// identical library.
pub fn find_prism_dll() -> Option<PathBuf> {
    find_prism_dll_in(&program_dir(), repo_root().as_deref())
}

/// [`find_prism_dll`] with the two places passed in, for tests.
pub fn find_prism_dll_in(program: &Path, repo: Option<&Path>) -> Option<PathBuf> {
    const NAME: &str = "prism.dll";
    if let Some(p) = find_data_file_in(NAME, program, repo) {
        return Some(p);
    }
    let venv = repo?
        .join(".venv")
        .join("Lib")
        .join("site-packages")
        .join("prism")
        .join("_native")
        .join(NAME);
    venv.is_file().then_some(venv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn config_dir_prefers_localappdata() {
        let d = config_dir_from(
            os(r"C:\Users\x\AppData\Local"),
            os(r"C:\Users\x"),
            None,
            None,
        );
        assert_eq!(d, PathBuf::from(r"C:\Users\x\AppData\Local\beamtel"));
    }

    #[test]
    fn config_dir_falls_back_to_home() {
        let d = config_dir_from(None, os(r"C:\Users\x"), None, None);
        assert_eq!(d, PathBuf::from(r"C:\Users\x\beamtel"));
        let d = config_dir_from(None, None, os("D:"), os(r"\Home\y"));
        assert_eq!(d, PathBuf::from(r"D:\Home\y\beamtel"));
        let d = config_dir_from(None, None, None, None);
        assert_eq!(d, PathBuf::from(r"~\beamtel"));
    }

    #[test]
    fn empty_localappdata_is_relative_like_python() {
        let d = config_dir_from(os(""), os(r"C:\Users\x"), None, None);
        assert_eq!(d, PathBuf::from("beamtel"));
    }

    #[test]
    fn config_path_name() {
        assert!(config_path().ends_with(r"beamtel\beamtel_config.json"));
    }
}
