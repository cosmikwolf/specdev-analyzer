//! # `specdev_analyzer` — domain-agnostic analyzer for TOML specs.
//!
//! Part of the blocks + bridges + profiles arc. The analyzer reads a
//! project's `spec.toml` and `specs/specdev/` tree directly; there is **no
//! resolution chain at analyze time**. The plugin
//! is the source of profiles and blocks at *bootstrap* and *inline* time
//! only — runtime analyze paths consult only project-local content plus
//! the canonical metaschemas that ship embedded in this crate.
//!
//! ## Entry points
//!
//! | Task                              | Call                                   | Returns                       |
//! |-----------------------------------|----------------------------------------|-------------------------------|
//! | Discover the project layout       | [`layout::LayoutContext::discover`]    | [`layout::LayoutContext`]     |
//! | Validate a project end-to-end     | [`validate::run_project`]              | [`Report`]                    |
//! | Resolve the open-vocabulary kinds | [`reductions::resolve`]                | `(EffectiveKindSet, Vec<Finding>)` |
//! | Reduce to a universal graph       | [`reduction::run_project`]             | [`reduction::Graph`] + [`Report`]   |
//! | Extract canonical YAML            | [`extract::run_project`]               | [`extract::ExtractionResult`] |
//!
//! ## Metaschema model
//!
//! The closed-enum metaschema is retired in favor of a path-qualified
//! resolver and an open-vocabulary [`resolved::EffectiveKindSet`]. Block-
//! and bridge-contributed kinds classify under the canonical base via
//! `extends` chains. The aggregator schema compiles into a
//! [`jsonschema::Validator`] reused across module TOMLs; cycles in the
//! inlined-fragment graph fail at compile time (AGGREGATOR-003).
//!
//! Resolver / validation finding codes:
//! - `METASCHEMA-001..005` — resolver findings (missing/unresolved
//!   `extends`, lex-order, canonical/local shadow, invalid grammar).
//! - `AGGREGATOR-005` — failed jsonschema compile.
//! - `SCHEMA` — module TOML structural error.
//! - `DOMAIN-003` — unresolved kind reference in `domain.toml` graph_check.
//! - `REDUCE_UNRESOLVED` — relation endpoint does not name a materialized
//!   `(qualified-kind, name)` pair.
//!
//! Bootstrap / inline error codes (returned as `anyhow::Error` payloads, not
//! findings — these abort a CLI run rather than accumulate into a [`Report`]):
//! - `PLUGIN-001` — plugin root not resolvable.
//! - `BOOTSTRAP-001` — target tree already exists / profile invalid.
//! - `BOOTSTRAP-002` — unknown placeholder in profile template.
//! - `INLINE-001` — block / bridge not found in plugin tree.
//! - `INLINE-002` — already inlined (lineage entry present); use `--merge`.
//! - `INLINE-003` — bridge requires a missing prerequisite block.
//! - `BRIDGE-LEX-001` — bridge name not lex-ordered (`a < b`).
//! - `LINEAGE-001` — lineage.toml is malformed.
//!
//! Merge / drift error codes (also `anyhow::Error` payloads):
//! - `MERGE-001` — `git merge-file` not available on PATH or invocation failed.
//! - `MERGE-002` — merge produced conflicts; resolve markers and retry.
//! - `MERGE-003` — `--merge` requested for a unit with no lineage entry.
//! - `MERGE-FILESET` — file-set changed between upstream and plugin (added or
//!   removed files); v1 doesn't reconcile, manual intervention needed.
//! - `DRIFT-INVARIANT-001` — inlined file present, no upstream entry.
//! - `DRIFT-INVARIANT-002` — upstream entry present, no inlined file.
//! - `DRIFT-INVARIANT-003` — lineage entry points at no files at all.
//!
//! Architecture / requirements finding codes (in [`Report`] alongside the rest):
//! - `LAYOUT-005` — `specs/architecture.toml` absent (info; project may
//!   have no architecture-level wiring yet).
//! - `LAYOUT-006` — `specs/requirements.toml` absent (info).
//! - `SCHEMA-ARCH` — `architecture.toml` fails its profile-shipped schema.
//! - `SCHEMA-REQ` — `requirements.toml` fails its profile-shipped schema.
//!
//! Plugin-lint codes (emitted by `plugin-lint`, not via Report —
//! these run at plugin-author CI time, before any project context):
//! - `BLOCK-LINT-001` — block missing required `schema.yaml`.
//! - `BLOCK-LINT-002` — block missing required `reduction.yaml`.
//! - `BLOCK-LINT-003` — block has no `codegen/<lang>/README.md` under any language.
//! - `BLOCK-LINT-004` — block missing required top-level `README.md`.
//! - `BLOCK-LINT-005` — `schema.yaml` fails standalone JSON Schema compile.
//! - `BLOCK-LINT-006` — `schema.yaml` `$id` doesn't match the convention
//!   `https://specdev.dev/blocks/<name>.v1`.
//! - `BLOCK-LINT-007` — `reduction.yaml` fails the embedded reduction
//!   metaschema (missing `extends`, missing relation `from_kind`/`to_kind`/
//!   `from_spec`/`endpoints`, etc).

pub mod aggregator;
pub mod bootstrap;
pub mod canonical;
pub mod cargo_toml;
pub mod pubspec_yaml;
pub mod cli;
pub mod config;
pub mod context;
pub mod coverage;
pub mod diagnostics;
pub mod domain;
pub mod drift;
pub mod embedded_schemas;
pub mod extract;
pub mod gen_lineage;
pub mod graph;
pub mod hash;
pub mod inline;
pub mod items;
pub mod kinds;
pub mod lang;
pub mod layout;
pub mod lineage;
pub mod manifest;
pub mod merge;
pub mod migrate;
pub mod path;
pub mod plugin_root;
pub mod prompt;
pub mod prose_lint;
pub mod reduction;
pub mod reference_lint;
pub mod upstream;
pub mod reductions;
pub mod report;
pub mod resolve;
pub mod resolved;
pub mod schema;
pub mod validate;

pub use domain::{DomainConfig, Severity};
pub use extract::ExtractionResult;
pub use layout::{LayoutContext, LayoutEntry, LayoutInventory, LayoutKind};
pub use reduction::{Entity, Graph, Relation, SerializedProvenance};
pub use report::{Finding, Provenance, Report};
pub use validate::Inputs;
