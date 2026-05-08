//! `profile.yaml` — analyzer dispatch metadata for one profile.
//! Schema: `src/embedded/profile.schema.yaml`. Authoritative spec: doc 21 §3.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::path::Path;

const SCHEMA_YAML: &str = include_str!("../embedded/profile.schema.yaml");
const SCHEMA_LABEL: &str = "profile.schema.yaml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub language: String,
    #[serde(default)]
    pub cargo_toml: Option<CargoTomlSection>,
    #[serde(default)]
    pub bootstrap: Option<BootstrapSection>,
    #[serde(default)]
    pub skeleton: Option<SkeletonSection>,
    #[serde(default)]
    pub context: Option<ContextSection>,
    #[serde(default)]
    pub surface: Option<SurfaceSection>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CargoTomlSection {
    pub shape: ShapeKind,
    #[serde(default)]
    pub default_edition: Option<String>,
    #[serde(default)]
    pub lib: Option<LibSection>,
    #[serde(default)]
    pub bin: Option<BinSection>,
    #[serde(default)]
    pub features: Option<FeaturesSection>,
    #[serde(default)]
    pub architecture_deps: Option<ArchitectureDepsSection>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Library,
    Binary,
    Workspace,
    Plugin,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LibSection {
    #[serde(default)]
    pub crate_type: Vec<CrateType>,
    #[serde(default)]
    pub cdylib_only: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CrateType {
    Lib,
    Rlib,
    Cdylib,
    Staticlib,
    Dylib,
    ProcMacro,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BinSection {
    pub name_template: String,
    pub path_template: String,
    #[serde(default)]
    pub gate: Option<BinGate>,
    #[serde(default)]
    pub paired_stub: Option<PairedStub>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BinGate {
    /// Bin only emits when architecture.toml's `<table>.<field>` array
    /// contains `<value>`.
    ArchitectureArrayContains {
        table: String,
        field: String,
        value: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PairedStub {
    /// Path under `profiles/<name>/`. Bytes are the stub body; supports
    /// `{project_name}` substitution at cargo-toml render time.
    pub template_file: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FeaturesSection {
    #[serde(default)]
    pub synthesize_default: bool,
    #[serde(default)]
    pub drain_from: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ArchitectureDepsSection {
    #[serde(default)]
    pub typed_slots: Vec<TypedSlot>,
    #[serde(default)]
    pub open_arrays: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TypedSlot {
    pub table: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BootstrapSection {
    #[serde(default)]
    pub steps: Vec<BootstrapStep>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStep {
    EmitWorkspaceRoot,
    EmitMemberStubs,
    WalkMembers,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SkeletonSection {
    pub supported: bool,
    #[serde(default)]
    pub entry_file: String,
    #[serde(default)]
    pub entry_template: Option<EntryTemplate>,
    #[serde(default)]
    pub extra_files: Vec<JsonValue>,
    #[serde(default)]
    pub module_layout: Option<ModuleLayout>,
    #[serde(default)]
    pub steps: Vec<SkeletonStep>,
    #[serde(default)]
    pub audio_path_stub: Option<AudioPathStub>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioPathStub {
    SilenceAndLog,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryTemplate {
    Empty,
    LibReExports,
    MainUnimplemented,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleLayout {
    Flat,
    Nested,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkeletonStep {
    EmitCargoToml,
    WalkModules,
    EmitHandoffMd,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ContextSection {
    #[serde(default)]
    pub module_fields_for_prompt: Vec<String>,
    #[serde(default)]
    pub context_subcommand_extras: Vec<ContextExtra>,
}

/// Open shape in step 1 — captures whichever fields the manifest
/// supplies. Sub-shape tightens in step 5 (`context.rs` refactor) when
/// the dispatch consumer pins the discriminator.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ContextExtra {
    pub field: String,
    #[serde(default)]
    pub from_attr: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SurfaceSection {
    #[serde(default = "default_true")]
    pub has_modules: bool,
    #[serde(default = "default_true")]
    pub has_requirements: bool,
    #[serde(default = "default_true")]
    pub has_architecture: bool,
}

fn default_true() -> bool {
    true
}

impl Profile {
    /// Load and validate a `profile.yaml`. Path is the file itself
    /// (typically `specs/specdev/profile.yaml` in the inlined tree, or
    /// `profiles/<name>/profile.yaml` in the plugin tree).
    pub fn load(path: &Path) -> Result<Self> {
        let value = super::load_yaml_validated(path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: Profile = serde_json::from_value(value)
            .with_context(|| format!("deserializing profile.yaml at {}", path.display()))?;
        Ok(parsed)
    }
}
