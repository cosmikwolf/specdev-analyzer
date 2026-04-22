//! # `specdev_analyzer` — domain-agnostic analyzer for TOML specs.
//!
//! Use as a library from other Rust tools (e.g. `specdev-visualizer`). The
//! same crate also ships the `specdev-analyzer` binary; both read the same
//! in-memory types.
//!
//! ## Five entry points
//!
//! | Task                            | Call                                                         | Returns                       |
//! |---------------------------------|--------------------------------------------------------------|-------------------------------|
//! | Resolve paths & context         | [`resolve::resolve`]                                         | [`ResolvedContext`]           |
//! | Validate a project              | [`validate::run_project`]                                    | [`Report`]                    |
//! | Extract canonical in-memory form | [`extract::run_project`]                                    | [`ExtractionResult`]          |
//! | Reduce to universal graph       | [`reduction::run_project`]                                   | `(`[`Graph`]`, `[`Report`]`)` |
//! | Audit extract vs. source tree   | [`audit::run_project`]                                       | [`AuditReport`]               |
//!
//! All entry points accept a `project` directory (containing `spec.toml`)
//! and an optional `plugin_root` used for override resolution per `D13`.
//! Consumers do not parse TOML/YAML themselves — the returned types carry
//! the canonical in-memory model.
//!
//! ## Minimal usage
//!
//! ```no_run
//! use std::path::Path;
//! use specdev_analyzer::{extract, reduction, validate};
//!
//! let project = Path::new("./examples/blinky-a2dp");
//! let plugin_root = Some(Path::new("../specdev"));
//!
//! let report = validate::run_project(project, plugin_root).unwrap();
//! assert!(!report.has_errors());
//!
//! let extracted = extract::run_project(project, plugin_root).unwrap();
//! for module in &extracted.modules {
//!     println!("module {}", module.name);
//! }
//!
//! let (graph, _findings) = reduction::run_project(project, plugin_root).unwrap();
//! println!("{} entities, {} relations", graph.entities.len(), graph.relations.len());
//! ```
//!
//! See `tests/library_api.rs` for a living example that pins the shape.

pub mod audit;
pub mod config;
pub mod domain;
pub mod extract;
pub mod graph;
pub mod lang;
pub mod packages;
pub mod path;
pub mod reduction;
pub mod report;
pub mod resolve;
pub mod schema;
pub mod validate;

pub use audit::{AuditItem, AuditOptions, AuditReport, AuditSummary, ModuleAudit};
pub use domain::{DomainConfig, Severity};
pub use extract::{ExtractedModule, ExtractionResult};
pub use reduction::{Entity, Graph, Relation, SerializedProvenance};
pub use report::{Finding, Provenance, Report};
pub use resolve::{Resolved, ResolvedContext, Source};
pub use validate::Inputs;
