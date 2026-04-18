//! C++ language adapter (Phase 8).

use super::LanguageAdapter;

pub struct CppAdapter;

impl LanguageAdapter for CppAdapter {
    fn name(&self) -> &'static str {
        "cpp"
    }
}
