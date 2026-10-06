//! Unit rule packs: the `checks.toml` a block or bridge ships.
//!
//! `inline` copies a unit's pack to `specs/specdev/checks/{blocks,bridges}/<name>.toml`
//! and its query files to `specs/specdev/queries/{blocks,bridges}/<name>/<lang>/`.
//! The inlined copy is the one that runs: a project changes a pack by editing
//! it — inlining is the only override path — and `drift` reports the edit.
//!
//! A pack holds:
//!
//! - `[rules]` — default severities;
//! - `[[graph_checks]]` — in the `domain.toml` graph-check shape, run by
//!   `validate`;
//! - `[[code_checks]]` — a query over the source, compared with the spec's
//!   entities by key, run by `check`.
//!
//! A rule's severity: the project's `domain.toml` `[validation.rules]`, else
//! the pack's `[rules]`, else `error`. Packs are never merged into
//! `domain.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::domain::{GraphCheck, Severity};
use crate::report::Finding;
use crate::upstream::UnitKind;

const SCHEMA_YAML: &str = include_str!("embedded/checks.schema.yaml");

/// The finding code for an inlined pack that does not parse or fails its schema.
pub const PACK_LOAD_RULE: &str = "PACK-001";

/// A pack's contents, as authored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackFile {
    #[serde(default)]
    pub rules: BTreeMap<String, Severity>,
    #[serde(default)]
    pub graph_checks: Vec<GraphCheck>,
    #[serde(default)]
    pub code_checks: Vec<CodeCheck>,
}

/// One spec ↔ code check.
#[derive(Debug, Clone, Deserialize)]
pub struct CodeCheck {
    pub language: String,
    /// Unit-relative: `languages/<lang>/<file>.scm`.
    pub query: String,
    /// The spec entity kind compared with the query's matches.
    pub entity_kind: String,
    /// The capture holding the code key.
    pub key_capture: String,
    #[serde(default)]
    pub arg_key_capture: Option<String>,
    /// A fixed argument name, used in place of `arg_key_capture`.
    #[serde(default)]
    pub arg_key: Option<String>,
    #[serde(default)]
    pub arg_value_capture: Option<String>,
    /// Source argument name → spec attribute name.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// One inlined pack.
#[derive(Debug, Clone)]
pub struct Pack {
    pub kind: UnitKind,
    pub name: String,
    pub path: PathBuf,
    pub file: PackFile,
}

impl Pack {
    /// Where a code check's query is inlined.
    pub fn query_path(&self, specdev_root: &Path, check: &CodeCheck) -> Option<PathBuf> {
        crate::inline::unit_rel_dst(self.kind, &self.name, Path::new(&check.query))
            .map(|rel| specdev_root.join(rel))
    }
}

fn validator() -> &'static jsonschema::Validator {
    static CELL: OnceLock<jsonschema::Validator> = OnceLock::new();
    CELL.get_or_init(|| {
        let value: serde_json::Value =
            serde_yaml::from_str(SCHEMA_YAML).expect("embedded checks.schema.yaml parses");
        jsonschema::options()
            .build(&value)
            .expect("embedded checks.schema.yaml compiles")
    })
}

/// Parse and validate a pack's text. The error lists every schema failure.
pub fn parse(text: &str) -> Result<PackFile> {
    let table: toml::Table = text
        .parse()
        .map_err(|e| anyhow!("does not parse as TOML: {e}"))?;
    let json = serde_json::to_value(&table).context("converting to JSON")?;
    let errors: Vec<String> = validator()
        .iter_errors(&json)
        .map(|e| {
            format!(
                "{} at `{}`",
                crate::diagnostics::format_schema_error(&e),
                e.instance_path()
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(anyhow!("fails checks.schema.yaml: {}", errors.join("; ")));
    }
    serde_json::from_value(json).context("reading the pack")
}

/// Every inlined pack under `specdev_root`, sorted by kind and name, and one
/// error finding for each pack that does not load.
pub fn load_inlined(specdev_root: &Path) -> (Vec<Pack>, Vec<Finding>) {
    let mut packs = Vec::new();
    let mut findings = Vec::new();
    for kind in [UnitKind::Block, UnitKind::Bridge] {
        let dir = specdev_root.join("checks").join(kind.dir_segment());
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let loaded = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("cannot be read: {e}"))
                .and_then(|text| parse(&text));
            match loaded {
                Ok(file) => packs.push(Pack {
                    kind,
                    name,
                    path,
                    file,
                }),
                Err(e) => findings.push(
                    Finding::new(
                        PACK_LOAD_RULE,
                        Severity::Error,
                        format!("rule pack {} {e:#}; none of its checks ran", path.display()),
                    )
                    .with_file(path.display().to_string()),
                ),
            }
        }
    }
    (packs, findings)
}

/// A rule's severity: the project's, else the pack's, else `error`.
pub fn severity(
    project: &BTreeMap<String, Severity>,
    pack: &BTreeMap<String, Severity>,
    rule: &str,
) -> Severity {
    project
        .get(rule)
        .or_else(|| pack.get(rule))
        .copied()
        .unwrap_or(Severity::Error)
}
