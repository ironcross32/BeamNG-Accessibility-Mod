//! The .npz/.npy reader used to load the baked HRTF sets (numpy's `np.load`).
//!
//! An `.npz` is a zip of `.npy` members (the bundled sets are deflate-compressed, which
//! the workspace `zip` build supports). Each `.npy` is a magic string, a version, a
//! header that is a Python dict literal (`descr`, `fortran_order`, `shape`), then the raw
//! array. Exactly what the bundled sets use is supported: little-endian `<f8` and `<f4`,
//! C order, any shape (0-d included). Anything else is an error, which `HrtfSet::load`
//! turns into "failed to load", as Python's `except Exception` does.

use std::fmt;
use std::io::Read;
use std::path::Path;

/// A load failure, with a message in the spirit of numpy's.
#[derive(Debug, Clone, PartialEq)]
pub struct NpyError(pub String);

impl fmt::Display for NpyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NpyError {}

fn err<T>(msg: impl Into<String>) -> Result<T, NpyError> {
    Err(NpyError(msg.into()))
}

/// The element data of one array, in its stored dtype.
#[derive(Debug, Clone, PartialEq)]
pub enum NpyData {
    F64(Vec<f64>),
    F32(Vec<f32>),
}

/// One loaded array: C-order data and its shape (`[]` for a 0-d scalar).
#[derive(Debug, Clone, PartialEq)]
pub struct NpyArray {
    pub shape: Vec<usize>,
    pub data: NpyData,
}

impl NpyArray {
    /// `descr` as numpy writes it: `<f8` or `<f4`.
    pub fn dtype(&self) -> &'static str {
        match self.data {
            NpyData::F64(_) => "<f8",
            NpyData::F32(_) => "<f4",
        }
    }

    pub fn len(&self) -> usize {
        match &self.data {
            NpyData::F64(v) => v.len(),
            NpyData::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `.astype(np.float64)`, flattened in C order. Exact for both dtypes.
    pub fn to_f64(&self) -> Vec<f64> {
        match &self.data {
            NpyData::F64(v) => v.clone(),
            NpyData::F32(v) => v.iter().map(|&x| x as f64).collect(),
        }
    }

    /// `float(arr)` for a single-element array, as Python does for `data["samplerate"]`.
    pub fn scalar_f64(&self) -> Result<f64, NpyError> {
        if self.len() != 1 {
            return err("only length-1 arrays can be converted to Python scalars");
        }
        Ok(self.to_f64()[0])
    }

    /// Rows of a 2-D array as float64 (`.astype(np.float64)`), shape `(m, n)`.
    pub fn rows_f64(&self) -> Result<Vec<Vec<f64>>, NpyError> {
        if self.shape.len() != 2 {
            return err(format!("expected a 2-D array, got shape {:?}", self.shape));
        }
        let n = self.shape[1];
        let flat = self.to_f64();
        if n == 0 {
            return Ok(vec![Vec::new(); self.shape[0]]);
        }
        Ok(flat.chunks(n).map(<[f64]>::to_vec).collect())
    }
}

/// Parse one `.npy` file's bytes.
pub fn parse_npy(bytes: &[u8]) -> Result<NpyArray, NpyError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return err("the magic string is not correct; expected b'\\x93NUMPY'");
    }
    let major = bytes[6];
    let (header_len, start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => {
            if bytes.len() < 12 {
                return err("truncated .npy header");
            }
            (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            )
        }
        v => return err(format!("unsupported .npy format version {v}")),
    };
    let end = start + header_len;
    if bytes.len() < end {
        return err("truncated .npy header");
    }
    let header = String::from_utf8_lossy(&bytes[start..end]);
    let descr = dict_value(&header, "descr").ok_or_else(|| NpyError("no 'descr'".into()))?;
    let fortran = dict_value(&header, "fortran_order")
        .ok_or_else(|| NpyError("no 'fortran_order'".into()))?;
    let shape_text = dict_value(&header, "shape").ok_or_else(|| NpyError("no 'shape'".into()))?;
    let descr = descr.trim().trim_matches(|c| c == '\'' || c == '"');
    let shape = parse_shape(shape_text)?;
    let count: usize = shape.iter().product();
    let body = &bytes[end..];
    match fortran.trim() {
        "False" => {}
        "True" => {
            if shape.len() > 1 {
                return err("fortran_order arrays are not supported");
            }
        }
        other => return err(format!("bad fortran_order {other}")),
    }
    let data = match descr {
        "<f8" => {
            if body.len() < count * 8 {
                return err("array data is truncated");
            }
            NpyData::F64(
                body[..count * 8]
                    .chunks_exact(8)
                    .map(|c| f64::from_le_bytes(c.try_into().expect("chunk of 8")))
                    .collect(),
            )
        }
        "<f4" => {
            if body.len() < count * 4 {
                return err("array data is truncated");
            }
            NpyData::F32(
                body[..count * 4]
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes(c.try_into().expect("chunk of 4")))
                    .collect(),
            )
        }
        other => return err(format!("unsupported dtype {other}")),
    };
    Ok(NpyArray { shape, data })
}

/// The raw text of one value in the header dict (`'key': value,`), not unescaped.
fn dict_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let pat_s = format!("'{key}'");
    let pat_d = format!("\"{key}\"");
    let at = header
        .find(&pat_s)
        .map(|i| i + pat_s.len())
        .or_else(|| header.find(&pat_d).map(|i| i + pat_d.len()))?;
    let rest = header[at..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if rest.starts_with('(') {
        let close = rest.find(')')?;
        return Some(&rest[..=close]);
    }
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn parse_shape(text: &str) -> Result<Vec<usize>, NpyError> {
    let inner = text
        .trim()
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(')'))
        .ok_or_else(|| NpyError(format!("bad shape {text}")))?;
    inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim_end_matches('L')
                .parse::<usize>()
                .map_err(|_| NpyError(format!("bad shape {text}")))
        })
        .collect()
}

/// An opened `.npz`: every member read into memory, like `np.load(...)` followed by
/// reading each key.
#[derive(Debug, Clone)]
pub struct Npz {
    members: Vec<(String, NpyArray)>,
}

impl Npz {
    /// Load every `.npy` member of the archive at `path`.
    pub fn open(path: &Path) -> Result<Npz, NpyError> {
        let file = std::fs::File::open(path)
            .map_err(|e| NpyError(format!("[Errno 2] {e}: '{}'", path.display())))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| NpyError(e.to_string()))?;
        let mut members = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| NpyError(e.to_string()))?;
            let name = entry
                .name()
                .map_err(|e| NpyError(e.to_string()))?
                .to_string();
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| NpyError(e.to_string()))?;
            let key = name.strip_suffix(".npy").unwrap_or(&name).to_string();
            members.push((key, parse_npy(&bytes)?));
        }
        Ok(Npz { members })
    }

    /// `data.files`: the member names without `.npy`, in archive order.
    pub fn files(&self) -> Vec<&str> {
        self.members.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// `data[key]`.
    pub fn get(&self, key: &str) -> Result<&NpyArray, NpyError> {
        self.members
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, a)| a)
            .ok_or_else(|| NpyError(format!("{key} is not a file in the archive")))
    }

    pub fn contains(&self, key: &str) -> bool {
        self.members.iter().any(|(k, _)| k == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npy(descr: &str, shape: &str, body: &[u8]) -> Vec<u8> {
        let mut h = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
        while (10 + h.len() + 1) % 64 != 0 {
            h.push(' ');
        }
        h.push('\n');
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend_from_slice(&(h.len() as u16).to_le_bytes());
        out.extend_from_slice(h.as_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn parses_scalar_vector_and_matrix() {
        let a = parse_npy(&npy("<f8", "()", &44100.0f64.to_le_bytes())).unwrap();
        assert_eq!(a.shape, Vec::<usize>::new());
        assert_eq!(a.scalar_f64().unwrap(), 44100.0);

        let body: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let m = parse_npy(&npy("<f4", "(2, 3)", &body)).unwrap();
        assert_eq!(m.shape, vec![2, 3]);
        assert_eq!(m.rows_f64().unwrap()[1], vec![4.0, 5.0, 6.0]);

        let v = parse_npy(&npy("<f8", "(1,)", &2.5f64.to_le_bytes())).unwrap();
        assert_eq!(v.shape, vec![1]);
    }

    #[test]
    fn rejects_what_it_does_not_support() {
        assert!(parse_npy(b"not numpy at all").is_err());
        assert!(parse_npy(&npy("<i8", "(1,)", &[0; 8])).is_err());
        assert!(parse_npy(&npy("<f8", "(4,)", &[0; 8])).is_err());
    }
}
