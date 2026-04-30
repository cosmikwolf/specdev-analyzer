//! # `specdev_analyzer` — domain-agnostic analyzer for TOML specs.
//!
//! Phase 2 of the blocks + bridges + profiles arc. The analyzer reads a
//! project's `spec.toml` and `specs/specdev/` tree directly; there is **no
//! resolution chain at analyze time** (doc 15 §11 invariant 5). The plugin
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
//! | Audit extract vs. source tree     | [`audit::run_project`]                 | [`audit::AuditReport`]        |
//!
//! ## Phase-2 cut line
//!
//! Phase 2 lands the metaschema amendment: the closed-enum metaschema is
//! retired in favor of a path-qualified resolver and an open-vocabulary
//! [`resolved::EffectiveKindSet`]. Block- and bridge-contributed kinds
//! classify under the canonical base via `extends` chains. The aggregator
//! schema compiles into a [`jsonschema::Validator`] reused across module
//! TOMLs; cycles in the inlined-fragment graph fail at compile time
//! (AGGREGATOR-003).
//!
//! Phase-2 finding codes:
//! - `METASCHEMA-001..005` — resolver findings (missing/unresolved
//!   `extends`, lex-order, canonical/local shadow, invalid grammar).
//! - `AGGREGATOR-005` — failed jsonschema compile.
//! - `SCHEMA` — module TOML structural error.
//! - `DOMAIN-003` — unresolved kind reference in `domain.toml` graph_check.
//! - `REDUCE_UNRESOLVED` — relation endpoint does not name a materialized
//!   `(qualified-kind, name)` pair.
//!
//! Phase-3 error codes (returned as `anyhow::Error` payloads, not findings —
//! these abort a CLI run rather than accumulate into a [`Report`]):
//! - `PLUGIN-001` — plugin root not resolvable.
//! - `BOOTSTRAP-001` — target tree already exists / profile invalid.
//! - `BOOTSTRAP-002` — unknown placeholder in profile template.
//! - `INLINE-001` — block / bridge not found in plugin tree.
//! - `INLINE-002` — already inlined (lineage entry present); use `--merge` (Phase 4).
//! - `INLINE-003` — bridge requires a missing prerequisite block.
//! - `BRIDGE-LEX-001` — bridge name not lex-ordered (`a < b`).
//! - `LINEAGE-001` — lineage.toml is malformed.
//!
//! Phase-4 error codes (also `anyhow::Error` payloads):
//! - `MERGE-001` — `git merge-file` not available on PATH or invocation failed.
//! - `MERGE-002` — merge produced conflicts; resolve markers and retry.
//! - `MERGE-003` — `--merge` requested for a unit with no lineage entry.
//! - `MERGE-FILESET` — file-set changed between upstream and plugin (added or
//!   removed files); v1 doesn't reconcile, manual intervention needed.
//! - `DRIFT-INVARIANT-001` — inlined file present, no upstream entry.
//! - `DRIFT-INVARIANT-002` — upstream entry present, no inlined file.
//! - `DRIFT-INVARIANT-003` — lineage entry points at no files at all.
//!
//! Phase-5 finding codes (in [`Report`] alongside the rest):
//! - `LAYOUT-005` — `specs/architecture.toml` absent (info; project may
//!   have no architecture-level wiring yet).
//! - `LAYOUT-006` — `specs/requirements.toml` absent (info).
//! - `SCHEMA-ARCH` — `architecture.toml` fails its profile-shipped schema.
//! - `SCHEMA-REQ` — `requirements.toml` fails its profile-shipped schema.

pub mod aggregator;
pub mod audit;
pub mod bootstrap;
pub mod canonical;
pub mod config;
pub mod diagnostics;
pub mod domain;
pub mod drift;
pub mod extract;
pub mod graph;
pub mod hash;
pub mod inline;
pub mod kinds;
pub mod lang;
pub mod layout;
pub mod lineage;
pub mod merge;
pub mod path;
pub mod plugin_root;
pub mod reduction;
pub mod upstream;
pub mod reductions;
pub mod report;
pub mod resolve;
pub mod resolved;
pub mod schema;
pub mod validate;

pub use audit::{AuditItem, AuditOptions, AuditReport, AuditSummary, ModuleAudit};
pub use domain::{DomainConfig, Severity};
pub use extract::ExtractionResult;
pub use layout::{LayoutContext, LayoutEntry, LayoutInventory, LayoutKind};
pub use reduction::{Entity, Graph, Relation, SerializedProvenance};
pub use report::{Finding, Provenance, Report};
pub use validate::Inputs;
