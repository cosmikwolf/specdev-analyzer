//! Extract pipeline.
//!
//! Phase 1 stub. The extract pipeline depends on Phase 2's path-qualified
//! kind resolution to know which module fields to canonicalize and how to
//! synthesize architecture/requirements summaries. Until that lands, both
//! [`run`] and [`run_project`] bail with a clear "available in Phase 2"
//! message.
//!
//! The shape carries forward across phases — `ExtractionResult` and
//! `ExtractedModule` stay in the public surface so consumers can compile
//! against them, but they are placeholders until Phase 2 produces real data.

use std::path::Path;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::layout::LayoutContext;

#[derive(Debug, Default)]
pub struct ExtractionResult {
    pub modules: Vec<ExtractedModule>,
    pub architecture: Value,
    pub requirements: Value,
    pub collections: std::collections::BTreeMap<String, Vec<(String, Value)>>,
}

#[derive(Debug)]
pub struct ExtractedModule {
    pub name: String,
    pub package: Option<String>,
    pub value: Value,
}

/// Library-friendly entry point. Bails in Phase 1; Phase 2 wires module
/// canonicalization once kind resolution lands.
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<ExtractionResult> {
    let _layout = LayoutContext::discover(start, explicit_spec_path)?;
    bail!(
        "specdev-analyzer extract is available in Phase 2 once kind resolution lands. \
         Phase 1 ships layout discovery only — run `specdev-analyzer validate` to \
         enumerate the project layout."
    );
}

/// Phase 1 stub. See [`run_project`] for the bail message.
pub fn run(_layout: &LayoutContext) -> Result<ExtractionResult> {
    bail!(
        "specdev-analyzer extract is available in Phase 2 once kind resolution lands. \
         Phase 1 ships layout discovery only."
    );
}

/// Phase 1 stub. Output writing is implemented but has nothing to write.
pub fn write(_result: &ExtractionResult, _output_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    bail!("specdev-analyzer extract output is available in Phase 2 once kind resolution lands.");
}
