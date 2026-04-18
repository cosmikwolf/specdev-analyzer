//! Rust language adapter (Phase 8).

use super::LanguageAdapter;

pub struct RustAdapter;

impl LanguageAdapter for RustAdapter {
    fn name(&self) -> &'static str {
        "rust"
    }
}
