//! `profile.yaml` — analyzer dispatch metadata for one profile.
//! Schema: `src/embedded/profile.schema.yaml`.

use anyhow::{Context, Result};
use serde::Deserialize;
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
    pub pubspec_yaml: Option<PubspecYamlSection>,
    #[serde(default)]
    pub bootstrap: Option<BootstrapSection>,
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

/// pubspec.yaml render dispatch — sibling of [`CargoTomlSection`]; the
/// `manifest` subcommand routes on the project language to whichever
/// section applies.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PubspecYamlSection {
    pub shape: PubShapeKind,
    /// Default `environment:` constraints, overridable by
    /// architecture.toml's `[environment]` table.
    #[serde(default)]
    pub environment: Option<PubEnvironment>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PubShapeKind {
    /// An application: emits `publish_to: none`.
    App,
    /// A publishable library package.
    Package,
    /// Descriptive only — the `[workspace]` table in spec.toml is the
    /// actual dispatch signal, mirroring the cargo path.
    Workspace,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PubEnvironment {
    pub sdk: String,
    #[serde(default)]
    pub flutter: Option<String>,
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
pub struct ContextSection {
    #[serde(default)]
    pub module_fields_for_prompt: Vec<String>,
    #[serde(default)]
    pub context_subcommand_extras: Vec<ContextExtra>,
}

/// Open shape — captures whichever fields the manifest supplies. The
/// sub-shape tightens when the dispatch consumer (`context.rs`) pins the
/// discriminator.
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
