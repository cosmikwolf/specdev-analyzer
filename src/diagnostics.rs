//! File:line:col-shaped error payloads.
//!
//! Phase 1 introduces the `<path>:<line>:<col>: <message>` error format the
//! plan calls for as the CLI baseline. Errors carry a `LocatedError` payload
//! when the originating subsystem (TOML parser, YAML parser, JSON Schema
//! validator) can pinpoint a source position; the top-level `report_error`
//! helper renders that payload as the first line so a user can jump to the
//! offending location with no further plumbing.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, thiserror::Error)]
pub struct LocatedError {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl fmt::Display for LocatedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 && self.col == 0 {
            write!(f, "{}: {}", self.path.display(), self.message)
        } else if self.col == 0 {
            write!(f, "{}:{}: {}", self.path.display(), self.line, self.message)
        } else {
            write!(
                f,
                "{}:{}:{}: {}",
                self.path.display(),
                self.line,
                self.col,
                self.message
            )
        }
    }
}

impl LocatedError {
    pub fn new(path: impl Into<PathBuf>, line: u32, col: u32, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line,
            col,
            message: message.into(),
        }
    }

    /// Build a `LocatedError` from the byte span of a TOML parse error. Walks
    /// the source text to convert the byte offset into a (line, col) pair.
    pub fn from_toml_error(path: &Path, source: &str, err: &toml::de::Error) -> Self {
        let (line, col) = match err.span() {
            Some(span) => byte_offset_to_line_col(source, span.start),
            None => (1, 1),
        };
        Self {
            path: path.to_path_buf(),
            line,
            col,
            message: err.message().to_string(),
        }
    }

    /// Build a `LocatedError` from a `serde_yaml` parse error. The crate
    /// exposes `Location` with line / column directly when present.
    pub fn from_yaml_error(path: &Path, err: &serde_yaml::Error) -> Self {
        let (line, col) = match err.location() {
            Some(loc) => (loc.line() as u32, loc.column() as u32),
            None => (1, 1),
        };
        Self {
            path: path.to_path_buf(),
            line,
            col,
            message: err.to_string(),
        }
    }
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> (u32, u32) {
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    let bytes = source.as_bytes();
    let end = offset.min(bytes.len());
    for &b in &bytes[..end] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Render an `anyhow::Error` chain to stderr. When the chain carries a
/// [`LocatedError`], it appears as the top line so the user gets a
/// machine-parseable `<path>:<line>:<col>: <message>` first.
pub fn report_error(err: &anyhow::Error) {
    if let Some(loc) = err.downcast_ref::<LocatedError>() {
        eprintln!("{loc}");
        // Walk the rest of the chain and emit context, skipping the located
        // payload itself.
        for cause in err.chain().skip(1) {
            if cause.downcast_ref::<LocatedError>().is_some() {
                continue;
            }
            eprintln!("  caused by: {cause}");
        }
    } else {
        eprintln!("{err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offset_to_line_col_handles_multiline() {
        let src = "abc\nde\nfghi";
        assert_eq!(byte_offset_to_line_col(src, 0), (1, 1));
        assert_eq!(byte_offset_to_line_col(src, 4), (2, 1));
        assert_eq!(byte_offset_to_line_col(src, 7), (3, 1));
        assert_eq!(byte_offset_to_line_col(src, 9), (3, 3));
    }

    #[test]
    fn located_error_renders_with_line_col() {
        let e = LocatedError::new("/tmp/foo.toml", 7, 14, "expected string");
        assert_eq!(e.to_string(), "/tmp/foo.toml:7:14: expected string");
    }

    #[test]
    fn located_error_renders_without_col_when_zero() {
        let e = LocatedError::new("/tmp/foo.toml", 7, 0, "expected string");
        assert_eq!(e.to_string(), "/tmp/foo.toml:7: expected string");
    }
}
