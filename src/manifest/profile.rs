//! `profile.yaml` — analyzer dispatch metadata for one profile.
//! Schema: `src/embedded/profile.schema.yaml`.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::path::Path;

const SCHEMA_YAML: &str = include_str!("../embedded/profile.schema.yaml");
const SCHEMA_LABEL: &str = "profile.schema.yaml";

/// The `profile.schema.yaml` contract THIS binary understands.
///
/// Bump this in the same commit as any shape change to
/// `src/embedded/profile.schema.yaml` — a new key, a widened enum, a new
/// required field. `contract_pairing_guard` in `tests/profile_contract.rs`
/// fails if the schema moves and this does not, so the bump is not a thing
/// anyone has to remember.
///
/// Why a number here and not `plugin.json`'s `version`: that value moves
/// when a skill changes, it says nothing about the schema, and
/// `Profile::load` never reads it. This number describes exactly one
/// contract and lives next to the schema it describes.
pub const PROFILE_CONTRACT_VERSION: u64 = 3;

/// A manifest with no `contract_version` predates the field. Every project
/// bootstrapped before this release is in that state, so the absent value
/// has to mean "the contract as it stood then" — which is 1.
const IMPLIED_CONTRACT_VERSION: u64 = 1;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub language: String,
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
    /// Config files `bootstrap` writes once when absent, then externally
    /// owned (`tsconfig.json`, `.gitignore`, …).
    #[serde(default)]
    pub paired_files: Vec<PairedFile>,
    /// The compile-gate registry: every gate the project's gate script
    /// and CI run, as data (label, argv steps). Empty for a profile that
    /// declares none.
    #[serde(default)]
    pub gates: Vec<Gate>,
    /// Per-tool gates and lockfiles, for a language with more than one
    /// tool. The project names its tool in `spec.toml` `[project]
    /// toolchain`; that tool's gates run after the common `gates`.
    #[serde(default)]
    pub toolchains: std::collections::BTreeMap<String, Toolchain>,
}

/// One `toolchains:` entry.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Toolchain {
    /// The lockfiles this tool writes, project-root-relative.
    #[serde(default)]
    pub lockfiles: Vec<String>,
    /// This tool's gates, run after the profile's common `gates`.
    #[serde(default)]
    pub gates: Vec<Gate>,
}

impl Profile {
    /// The gates a project runs: the common `gates`, then the declared
    /// tool's. A profile without `toolchains:` ignores `tool`. With them, an
    /// undeclared or unknown tool is an error that names the choices.
    pub fn gates_for(&self, tool: Option<&str>) -> Result<Vec<Gate>> {
        if self.toolchains.is_empty() {
            return Ok(self.gates.clone());
        }
        let known: Vec<&str> = self.toolchains.keys().map(String::as_str).collect();
        let Some(tool) = tool else {
            anyhow::bail!(
                "profile `{}` gives gates per tool ({}): declare one in spec.toml as \
                 `[project] toolchain = \"<tool>\"`",
                self.name,
                known.join(", ")
            );
        };
        let Some(chain) = self.toolchains.get(tool) else {
            anyhow::bail!(
                "spec.toml declares `toolchain = \"{tool}\"`, but profile `{}` supports {}",
                self.name,
                known.join(", ")
            );
        };
        let mut out = self.gates.clone();
        out.extend(chain.gates.iter().cloned());
        Ok(out)
    }
}

/// The top-level sections a contract-1 manifest may still carry: they fed
/// the manifest emitters, which no longer exist. The loader drops them
/// from a contract-1 manifest (an inlined copy in a `1.x` tree) before the
/// schema runs; a contract-2 manifest that names one is refused.
pub const RETIRED_SECTIONS: [&str; 4] = [
    "cargo_toml",
    "pubspec_yaml",
    "package_json",
    "derived_files",
];

/// One `gates` row. `label` is opaque; `steps` is a list of argv lists run
/// in order in the project root (never a shell string).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Gate {
    pub label: String,
    pub steps: Vec<Vec<String>>,
}

/// One `paired_files` row: a template under the profile's `templates/`
/// (read from the inlined copy at render time) and its
/// project-root-relative target.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PairedFile {
    pub template_file: String,
    pub target: String,
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

    /// The profile-declared visibility floor, if any (`items.visibility_floor`).
    pub fn visibility_floor(&self) -> Option<&str> {
        self.items
            .as_ref()
            .and_then(|i| i.visibility_floor.as_deref())
    }

    /// Validate `profile.yaml` text without a file: the checks [`Self::load`]
    /// runs. `path` only names the file in messages.
    pub fn check_text(text: &str, path: &Path) -> Result<()> {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(text).with_context(|| format!("parsing {}", path.display()))?;
        let mut value: JsonValue =
            serde_json::to_value(yaml).with_context(|| format!("converting {}", path.display()))?;
        check_contract_version(&value, path)?;
        drop_retired_sections(&mut value);
        super::validate_against(&value, path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: Profile = serde_json::from_value(value)
            .with_context(|| format!("deserializing profile.yaml at {}", path.display()))?;
        check_gate_labels(&parsed, path)?;
        Ok(())
    }

    /// Load and validate a `profile.yaml`. Path is the file itself
    /// (typically `specs/specdev/profile.yaml` in the inlined tree, or
    /// `profiles/<name>/profile.yaml` in the plugin tree).
    pub fn load(path: &Path) -> Result<Self> {
        let mut value = super::read_yaml(path)?;
        check_contract_version(&value, path)?;
        drop_retired_sections(&mut value);
        super::validate_against(&value, path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: Profile = serde_json::from_value(value)
            .with_context(|| format!("deserializing profile.yaml at {}", path.display()))?;
        check_gate_labels(&parsed, path)?;
        Ok(parsed)
    }
}

/// Plugin-author lint: every `profiles/*/profile.yaml` in a plugin tree
/// must declare `contract_version` equal to this binary's
/// [`PROFILE_CONTRACT_VERSION`]. Emits pre-prefixed `PROFILE-LINT-001`
/// lines, sorted; an empty vector means clean.
///
/// The schema treats the field as optional and this lint treats it as
/// mandatory, on purpose. The schema also judges *project* trees, which
/// may predate the field and must keep validating. The plugin tree has no
/// such excuse: it is authored in lockstep with the analyzer, and a
/// profile that declares nothing at a contract bump reopens #117 for
/// every binary older than the bump.
///
/// Reads the raw YAML rather than [`Profile::load`] — a load would fail
/// with `MANIFEST-002` on exactly the manifests this lint exists to
/// describe.
pub fn lint_plugin_profiles(plugin_root: &Path) -> Result<Vec<String>> {
    let profiles_dir = plugin_root.join("profiles");
    if !profiles_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut offenders = Vec::new();
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&profiles_dir)
        .with_context(|| format!("reading {}", profiles_dir.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let manifest = dir.join("profile.yaml");
        if !manifest.is_file() {
            // A profile with no manifest is a separate, pre-existing
            // shape (the analyzer falls back to its built-in default);
            // not this lint's business.
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let value = super::read_yaml(&manifest)?;
        match value.get("contract_version").and_then(|v| v.as_u64()) {
            Some(v) if v == PROFILE_CONTRACT_VERSION => {}
            Some(v) => offenders.push(format!(
                "[PROFILE-LINT-001] profiles/{name}/profile.yaml declares contract_version {v}; \
                 this analyzer's contract is {PROFILE_CONTRACT_VERSION}. The plugin tree and the \
                 analyzer are one change unit — they must agree."
            )),
            None => offenders.push(format!(
                "[PROFILE-LINT-001] profiles/{name}/profile.yaml has no contract_version; add \
                 `contract_version: {PROFILE_CONTRACT_VERSION}`. Without it an analyzer older \
                 than this contract reports the user's own file as invalid (#117)."
            )),
        }
    }
    offenders.sort();
    Ok(offenders)
}

/// Compare the manifest's declared contract against the one this binary
/// embeds, BEFORE the schema validator runs.
///
/// Order matters. Run the validator first and a plugin one contract ahead
/// produces `MANIFEST-001` listing every unknown key against the path of
/// the file in the *user's* project — the #117 failure, which names
/// neither the analyzer nor a remedy. Run this first and the same skew
/// produces one line that names both.
///
/// Only a manifest AHEAD of the binary is an error. A manifest behind it
/// (or silent) is the normal state of every tree bootstrapped before the
/// field existed, and the schema is the right judge of those: it accepts
/// old shapes it still understands and rejects the rest by key.
fn check_contract_version(value: &JsonValue, path: &Path) -> Result<()> {
    let declared = value
        .get("contract_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(IMPLIED_CONTRACT_VERSION);
    if declared > PROFILE_CONTRACT_VERSION {
        anyhow::bail!(
            "[MANIFEST-002] {} declares contract_version {declared}; this specdev-analyzer \
             understands {PROFILE_CONTRACT_VERSION}. The plugin tree is newer than the analyzer \
             — nothing is wrong with the file. Re-install the analyzer from its checkout:\n  \
             cargo install --path <specdev-analyzer checkout> --force",
            path.display(),
        );
    }
    Ok(())
}

/// A contract-1 manifest loses the sections and the gate `scope` key that
/// only the manifest emitters read; a contract-2 one is left for the schema
/// to judge.
fn drop_retired_sections(value: &mut JsonValue) {
    let declared = value
        .get("contract_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(IMPLIED_CONTRACT_VERSION);
    if declared >= 2 {
        return;
    }
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for k in RETIRED_SECTIONS {
        map.remove(k);
    }
    if let Some(gates) = map.get_mut("gates").and_then(|g| g.as_array_mut()) {
        for g in gates {
            if let Some(g) = g.as_object_mut() {
                g.remove("scope");
            }
        }
    }
    if let Some(ctx) = map.get_mut("context").and_then(|c| c.as_object_mut()) {
        ctx.remove("module_fields_for_prompt");
    }
}

/// Two `gates` rows with one label would write rows under one
/// `(module, label)` key and overwrite each other. JSON Schema cannot
/// express uniqueness across list items, so the loader checks it — the
/// one site no consumer can bypass.
/// Labels are unique in the common `gates`, and in the common gates plus
/// each tool's: one tool's gates never shadow a common one.
fn check_gate_labels(profile: &Profile, path: &Path) -> Result<()> {
    check_gate_labels_unique(&profile.gates, path)?;
    for chain in profile.toolchains.values() {
        let mut all = profile.gates.clone();
        all.extend(chain.gates.iter().cloned());
        check_gate_labels_unique(&all, path)?;
    }
    Ok(())
}

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
