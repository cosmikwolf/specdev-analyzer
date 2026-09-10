//! `profile.yaml` — analyzer dispatch metadata for one profile.
//! Schema: `src/embedded/profile.schema.yaml`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
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
    pub package_json: Option<PackageJsonSection>,
    #[serde(default)]
    pub bootstrap: Option<BootstrapSection>,
    #[serde(default)]
    pub context: Option<ContextSection>,
    #[serde(default)]
    pub surface: Option<SurfaceSection>,
    #[serde(default)]
    pub items: Option<ItemsSection>,
    #[serde(default)]
    pub reverse: Option<ReverseSection>,
    /// Config files written once when absent by `manifest
    /// --write`, then externally owned. Language-neutral; the shared
    /// manifest runner merges them with the rust arm's `[[bin]]` stub.
    #[serde(default)]
    pub paired_files: Vec<PairedFile>,
    /// Rules for files derived per module by `derive --write` (route
    /// shims). Declared order is match order; the first match wins.
    /// Empty for a profile with no derived files.
    #[serde(default)]
    pub derived_files: Vec<DerivedRule>,
    /// The compile-gate registry: every gate the codegen skill runs
    /// after emission, as data (label, scope, argv steps). Empty for a
    /// profile that declares none — which `report gen-plan` reports as
    /// `gates_declared: false` and `gen --tail` refuses.
    #[serde(default)]
    pub gates: Vec<Gate>,
}

/// One `gates` row. `label` is opaque and is the row key in
/// `gen-lineage.toml` together with the module; `steps` is a list of
/// argv lists run in order in the project root (never a shell string).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Gate {
    pub label: String,
    pub scope: GateScope,
    pub steps: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateScope {
    /// One execution per run, one recorded row per module in the run.
    Module,
    /// One execution per run, one recorded row with no module.
    Batch,
}

impl GateScope {
    pub fn as_str(self) -> &'static str {
        match self {
            GateScope::Module => "module",
            GateScope::Batch => "batch",
        }
    }
}

/// One `paired_files` row: a template under the profile's `templates/`
/// (read from the inlined copy at render time) and its
/// project-root-relative target.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PairedFile {
    pub template_file: String,
    pub target: String,
}

/// One `derived_files` rule. `when` is a set of attribute equalities on
/// the module document (ANDed; values compare as rendered scalars);
/// `path` is a template over the module document; `template` is the
/// body file under the profile's `templates/`; `exclusive` names a
/// group whose targets may not share a directory.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DerivedRule {
    pub when: BTreeMap<String, serde_json::Value>,
    pub path: String,
    pub template: String,
    #[serde(default)]
    pub exclusive: Option<String>,
}

/// `items` — extraction policy (#101(b)). The floor is an opaque string
/// the language adapter interprets; `src/` never compares it itself.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct ItemsSection {
    #[serde(default)]
    pub visibility_floor: Option<String>,
}

/// `reverse` — pre-flight policy (#102).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct ReverseSection {
    #[serde(default)]
    pub vocabulary: ReverseVocabulary,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReverseVocabulary {
    /// Inlined block schemas are the vocabulary (the original model).
    #[default]
    Blocks,
    /// The profile's own schema is the vocabulary; no block is needed.
    Profile,
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

/// package.json render dispatch — third sibling of [`CargoTomlSection`]
/// and [`PubspecYamlSection`].
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PackageJsonSection {
    pub shape: PackageJsonShapeKind,
    /// Default `packageManager` (corepack pin); architecture.toml's
    /// `[environment] package_manager` overrides it.
    #[serde(default)]
    pub package_manager: Option<String>,
    /// Default `engines` map; architecture.toml's `[environment] node`
    /// overrides `node`.
    #[serde(default)]
    pub engines: Option<NpmEngines>,
    /// The package.json `type` field; omitted when absent.
    #[serde(default, rename = "type")]
    pub module_type: Option<NpmModuleType>,
    /// Default `scripts`; architecture.toml's `[scripts]` overrides
    /// key-by-key.
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageJsonShapeKind {
    /// An application: emits `"private": true`.
    App,
    /// A publishable package.
    Library,
    /// The pnpm workspace root — the `[workspace]` table in spec.toml is
    /// the actual dispatch signal, mirroring the cargo and pub paths.
    Workspace,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct NpmEngines {
    #[serde(default)]
    pub node: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NpmModuleType {
    Module,
    Commonjs,
}

impl NpmModuleType {
    pub fn as_str(self) -> &'static str {
        match self {
            NpmModuleType::Module => "module",
            NpmModuleType::Commonjs => "commonjs",
        }
    }
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

impl CrateType {
    /// The literal cargo writes in `[lib] crate-type = [...]`.
    pub fn as_cargo_str(self) -> &'static str {
        match self {
            CrateType::Lib => "lib",
            CrateType::Rlib => "rlib",
            CrateType::Cdylib => "cdylib",
            CrateType::Staticlib => "staticlib",
            CrateType::Dylib => "dylib",
            CrateType::ProcMacro => "proc-macro",
        }
    }
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

/// `cargo_toml.features` — descriptive only. The cargo-toml renderer
/// dispatches on `shape` and reads architecture.toml's `[[features]]` by
/// fixed key; nothing reads these two fields. Kept because inlined
/// projects carry them; wiring them in is the v1.1 fixed-key cleanup.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FeaturesSection {
    #[serde(default)]
    pub synthesize_default: bool,
    #[serde(default)]
    pub drain_from: Vec<String>,
}

/// `cargo_toml.architecture_deps` — descriptive only. The renderer reads
/// the slot tables (`platform`, `runtime`, `logger`) and `[[external_deps]]`
/// by fixed key; nothing reads these two fields. Same v1.1 cleanup as
/// [`FeaturesSection`].
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
    /// Load the project's inlined `<specdev_root>/profile.yaml` if it
    /// exists. `Ok(None)` when the manifest is absent (pre-manifest
    /// projects); `Err` only on a malformed manifest. The one loader the
    /// dispatch consumers share — `cargo_toml.rs`, `context.rs`, the
    /// items callers and `reverse` all read the same file.
    pub fn load_inlined(specdev_root: &Path) -> Result<Option<Self>> {
        let path = specdev_root.join("profile.yaml");
        if !path.is_file() {
            return Ok(None);
        }
        Self::load(&path).map(Some)
    }

    /// The declared module-scope gates, in declared order. `verified`
    /// for a module means a pass row at the current key from every one
    /// of these; an empty list means `verified` cannot be computed.
    pub fn module_gates(&self) -> impl Iterator<Item = &Gate> {
        self.gates.iter().filter(|g| g.scope == GateScope::Module)
    }

    /// True when at least one module-scope gate is declared — the
    /// precondition for `verified` to mean anything. A registry with
    /// only batch gates is as good as none here: "every declared
    /// module-scope gate passed" would hold over zero gates forever.
    pub fn gates_declared(&self) -> bool {
        self.module_gates().next().is_some()
    }

    /// The profile-declared visibility floor, if any (`items.visibility_floor`).
    pub fn visibility_floor(&self) -> Option<&str> {
        self.items
            .as_ref()
            .and_then(|i| i.visibility_floor.as_deref())
    }

    /// Load and validate a `profile.yaml`. Path is the file itself
    /// (typically `specs/specdev/profile.yaml` in the inlined tree, or
    /// `profiles/<name>/profile.yaml` in the plugin tree).
    pub fn load(path: &Path) -> Result<Self> {
        let value = super::load_yaml_validated(path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: Profile = serde_json::from_value(value)
            .with_context(|| format!("deserializing profile.yaml at {}", path.display()))?;
        check_gate_labels_unique(&parsed.gates, path)?;
        Ok(parsed)
    }
}

/// Two `gates` rows with one label would write rows under one
/// `(module, label)` key and overwrite each other. JSON Schema cannot
/// express uniqueness across list items, so the loader checks it — the
/// one site no consumer can bypass.
fn check_gate_labels_unique(gates: &[Gate], path: &Path) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for g in gates {
        if !seen.insert(g.label.as_str()) {
            anyhow::bail!(
                "{}: `gates:` declares label `{}` twice; one row per label (rows are keyed by \
                 label and module, so two rows would overwrite each other)",
                path.display(),
                g.label
            );
        }
    }
    Ok(())
}
