//! ValueError, the error a ported Python function raises on bad input.
//!
//! The Python validates protocol packets by raising `ValueError` with a message the
//! listener logs. The Rust returns this instead; the message text is kept byte for
//! byte so the logs read the same.

use std::fmt;

/// Python's `ValueError(msg)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(pub String);

impl ValueError {
    pub fn new(msg: impl Into<String>) -> Self {
        ValueError(msg.into())
    }

    /// The message, as `str(exc)` gives it in Python.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}
