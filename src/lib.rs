//! # `specdev_analyzer` — domain-agnostic analyzer for TOML specs.
//!
//! Phase 1 of the blocks + bridges + profiles arc. The analyzer reads a
//! project's `spec.toml` and `specs/specdev/` tree directly; there is **no
//! resolution chain at analyze time** (doc 15 §11 invariant 5). The plugin
//! is the source of profiles and blocks at *bootstrap* and *inline* time
//! only — runtime analyze paths consult only project-local content.
//!
//! ## Phase 1 entry points
//!
//! | Task                              | Call                          | Returns                       |
//! |-----------------------------------|-------------------------------|-------------------------------|
//! | Discover the project layout       | [`layout::LayoutContext::discover`] | [`layout::LayoutContext`]   |
//! | Validate a project (Phase 1 stub) | [`validate::run_project`]     | [`Report`]                    |
//! | Extract canonical YAML            | [`extract::run_project`]      | bails — Phase 2 work          |
//! | Reduce to universal graph         | [`reduction::run_project`]    | bails — Phase 2 work          |
//! | Audit extract vs. source tree     | [`audit::run_project`]        | bails — Phase 2 work          |
//!
//! Phase 1 ships layout discovery, the aggregator `$ref` resolver, the
//! reduction loader, and a stub module-validation pass that emits
//! `LAYOUT-005` info per discovered module. Phase 2 wires the metaschema
//! amendment in and turns the stub into structural validation.

pub mod aggregator;
pub mod audit;
pub mod canonical;
pub mod config;
pub mod diagnostics;
pub mod domain;
pub mod extract;
pub mod graph;
pub mod kinds;
pub mod lang;
pub mod layout;
pub mod path;
pub mod reduction;
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
