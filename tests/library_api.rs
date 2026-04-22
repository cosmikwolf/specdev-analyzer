//! Shape tests for the library API. Exercises every top-level entry point
//! using only the crate-root re-exports, matching how `specdev-visualizer`
//! (or any external consumer) would call the library.
//!
//! These are shape assertions, not byte equality — correctness is covered
//! by `validate.rs`, `extract.rs`, `reduction.rs`, and the audit tests.
//! The point here is to pin the public API's call graph and return types.

use std::path::{Path, PathBuf};

// Every type imported here must be available at the crate root — that is
// the contract this test pins. Deep module paths are disallowed below.
use specdev_analyzer::{
    audit, extract, reduction, resolve, validate, AuditItem, AuditOptions, AuditReport,
    ExtractedModule, ExtractionResult, Finding, Graph, Provenance, Report, ResolvedContext,
    Severity,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn workspace_root() -> PathBuf {
    repo_root().parent().unwrap().to_path_buf()
}

fn plugin_root() -> PathBuf {
    workspace_root().join("specdev")
}

fn blinky_root() -> PathBuf {
    plugin_root().join("examples/blinky-a2dp")
}

#[test]
fn resolve_produces_context_with_expected_paths() {
    let ctx: ResolvedContext =
        resolve::resolve(&blinky_root(), Some(&plugin_root())).expect("resolve blinky-a2dp");
    assert_eq!(ctx.domain, "firmware");
    assert_eq!(ctx.language, "rust");
    assert!(ctx.spec_toml.ends_with("spec.toml"));
    assert!(ctx.project_root.ends_with("blinky-a2dp"));
    assert!(ctx.plugin_root.is_some());
    // domain_dir should resolve to a real directory containing domain.toml.
    let domain_file = ctx.domain_dir.path.join("domain.toml");
    assert!(domain_file.is_file(), "{}", domain_file.display());
}

#[test]
fn validate_run_project_returns_report() {
    let report: Report =
        validate::run_project(&blinky_root(), Some(&plugin_root())).expect("validate blinky-a2dp");
    assert!(!report.has_errors(), "blinky must validate clean");
    for f in report.findings() {
        // Every finding exposes its rule, severity, and optional provenance.
        let _: &str = f.rule.as_str();
        let _: Severity = f.severity;
        let _: &Option<String> = &f.file;
        let _: &String = &f.pointer;
        let _message: &str = f.message.as_str();
    }
    let _counts: (usize, usize, usize) = report.counts();
    let _rendered: String = report.render_plain();
    let _: &[Finding] = report.findings();
}

#[test]
fn extract_run_project_returns_in_memory_model() {
    let result: ExtractionResult =
        extract::run_project(&blinky_root(), Some(&plugin_root())).expect("extract blinky-a2dp");
    // Blinky defines four modules; pin the count so a shape regression is
    // visible. (The per-module contents are pinned by the existing extract
    // snapshot tests.)
    assert_eq!(result.modules.len(), 4, "blinky has 4 modules");
    for m in &result.modules {
        let _: &ExtractedModule = m;
        assert!(!m.name.is_empty());
        assert!(m.value.is_object(), "module value is a JSON object");
    }
    // Architecture and requirements are always present (even if empty).
    assert!(result.architecture.is_object());
    assert!(result.requirements.is_object() || result.requirements.is_array());
    let _collections = &result.collections;
}

#[test]
fn reduction_run_project_returns_graph_and_report() {
    let (graph, report): (Graph, Report) =
        reduction::run_project(&blinky_root(), Some(&plugin_root())).expect("reduce blinky-a2dp");
    assert!(!report.has_errors(), "blinky reduction must not error");
    assert!(graph.entities.len() >= 4, "at least four entities");
    assert!(!graph.relations.is_empty(), "at least one relation");
    // Spot-check provenance: every entity carries a source file pointer
    // that points back into the project.
    for e in &graph.entities {
        assert!(!e.provenance.source_file.is_empty());
        assert!(!e.kind.is_empty());
        assert!(!e.name.is_empty());
    }
    for r in &graph.relations {
        assert!(!r.from.is_empty());
        assert!(!r.to.is_empty());
    }
}

#[test]
fn audit_run_project_returns_report_shape() {
    // blinky-a2dp ships specs only (no source tree). Audit either returns a
    // report containing no modules or errors with a clear reason; we only
    // care that the call path through the re-exports compiles and runs.
    let ctx = resolve::resolve(&blinky_root(), Some(&plugin_root())).expect("resolve");
    let options = AuditOptions {
        show_uncovered: false,
        expand: false,
        module_filter: None,
        type_map: None,
    };
    match audit::run_project(&ctx, options) {
        Ok(r) => {
            let _: AuditReport = r;
            // Each module audit has a name and an items vec.
            for m in &r.modules {
                assert!(!m.module_name.is_empty());
                for item in &m.items {
                    match item {
                        AuditItem::Match { .. }
                        | AuditItem::Missing { .. }
                        | AuditItem::Mismatch { .. }
                        | AuditItem::Uncovered { .. } => {}
                    }
                }
            }
        }
        Err(_) => {
            // Blinky has no source tree; a clean error is acceptable. The
            // point of this test is that the re-exported call path works.
        }
    }
}

#[test]
fn provenance_and_finding_types_at_crate_root() {
    // Compile-time proof that the key public types are reachable via the
    // crate root, not only via their module paths.
    fn _assert_types(
        _f: &Finding,
        _p: &Provenance,
        _r: &Report,
        _g: &Graph,
        _e: &ExtractionResult,
    ) {
    }
}
