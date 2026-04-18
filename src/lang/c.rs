//! C language adapter (Phase 8).

use super::LanguageAdapter;

pub struct CAdapter;

impl LanguageAdapter for CAdapter {
    fn name(&self) -> &'static str {
        "c"
    }
}
