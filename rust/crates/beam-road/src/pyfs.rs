//! The pathlib and text-file behaviour the recorders depend on, as Python has it on
//! Windows.
//!
//! - `str(Path(x))` uses backslashes, drops `.` components, collapses repeated
//!   separators and trailing ones ([`py_path`]).
//! - `Path.glob` is case-insensitive and `sorted()` of paths in one directory compares
//!   the lowercased names ([`glob_sorted`]).
//! - A file opened in text mode without `newline=` writes `\n` as `\r\n`
//!   ([`CrlfWriter`]).

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `Path(text)` as Windows pathlib normalises it.
pub fn py_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().replace('/', "\\");
    let (prefix, rest) = if let Some(r) = s.strip_prefix("\\\\") {
        ("\\\\", r)
    } else {
        ("", s.as_str())
    };
    let rooted = rest.starts_with('\\');
    let parts: Vec<&str> = rest
        .split('\\')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let mut out = String::from(prefix);
    if rooted {
        out.push('\\');
    }
    out.push_str(&parts.join("\\"));
    if out.is_empty() {
        out.push('.');
    }
    PathBuf::from(out)
}

/// `str(path)`.
pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `Path(text).name == text`: a bare file name, with no directory part and no drive.
pub fn is_bare_name(text: &str) -> bool {
    if text.contains('/') || text.contains('\\') {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        return false;
    }
    // Path(".").name is "", Path("").name is "".
    text != "."
}

/// `sorted(directory.glob(prefix + "*" + suffix))`: the entries whose name matches,
/// case-insensitively, ordered by lowercased name. A missing directory matches
/// nothing.
pub fn glob_sorted(directory: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let (lp, ls) = (prefix.to_lowercase(), suffix.to_lowercase());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            (lower.chars().count() >= lp.chars().count() + ls.chars().count()
                && lower.starts_with(&lp)
                && lower.ends_with(&ls))
            .then(|| (lower, directory.join(&name)))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// `path.with_suffix(new)`: replace the last suffix of the file name.
pub fn with_suffix(path: &Path, new: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name.as_str(),
    };
    path.with_file_name(format!("{stem}{new}"))
}

/// A file written in Python's Windows text mode: every `\n` becomes `\r\n`.
pub struct CrlfWriter<W: Write>(pub W);

impl<W: Write> Write for CrlfWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut out = Vec::with_capacity(buf.len() + 8);
        for &b in buf {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        self.0.write_all(&out)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// `open(path, "x", encoding="utf-8")`: create, failing if it exists, text mode.
pub fn create_new_text(path: &Path) -> io::Result<CrlfWriter<File>> {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    Ok(CrlfWriter(f))
}

/// `Path.write_text(text, encoding="utf-8")`: text mode, so `\n` is written `\r\n`.
pub fn write_text(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text.replace('\n', "\r\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(py_path(Path::new("C:/a//b/./c/")), PathBuf::from(r"C:\a\b\c"));
        assert_eq!(
            with_suffix(Path::new(r"d\x.ndjson"), ".summary.json"),
            PathBuf::from(r"d\x.summary.json")
        );
        assert!(is_bare_name("x.ndjson"));
        assert!(!is_bare_name("../x.ndjson"));
        assert!(!is_bare_name("C:x.ndjson"));
    }
}
