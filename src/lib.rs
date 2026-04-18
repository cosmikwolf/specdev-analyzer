//! specdev-analyzer — domain-agnostic spec analyzer.
//!
//! Modules land with their respective phases (see
//! `../specdev-next/05-implementation-plan.md`):
//!   * validate pipeline  — Phase 4 (present)
//!   * extract pipeline   — Phase 5
//!   * audit + language adapters — Phase 8

pub mod config;
pub mod domain;
pub mod extract;
pub mod graph;
pub mod lang;
pub mod path;
pub mod report;
pub mod schema;
pub mod validate;
