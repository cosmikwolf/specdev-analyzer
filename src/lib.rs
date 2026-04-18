//! specdev-analyzer — domain-agnostic spec analyzer.
//!
//! Public surface is minimal during scaffolding. Modules will land with their
//! respective phases (see `../specdev-next/05-implementation-plan.md`):
//!   * validate pipeline  — Phase 4
//!   * extract pipeline   — Phase 5
//!   * audit + language adapters — Phase 8

pub mod lang;
