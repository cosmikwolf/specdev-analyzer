//! Triad smoke fixture (TF1) — Run 2's gating artifact for the
//! Phase 19 decisions + open_questions triad.
//!
//! The fixture exercises every triad graph check (10 reference-resolution
//! checks + 1 status-consistency check) plus the codegen gate. Subsequent
//! tests (graph-check assertions, validate-time ref resolution, prompt
//! walk, gate refusal) consume `build_triad_smoke()` to scaffold a
//! canonical project and then assert against the analyzer behavior.
//!
//! Acceptance gates (per the work order's §TF1):
//!
//! 1. `validate` reports 0 errors, 1 warning (`codegen_unblocked` for
//!    `module_blocked`).
//! 2. `prompt --module module_ok` succeeds and inlines DEC-001 + DEC-003
//!    + OQ-001 verify text in their canonical sections.
//! 3. `gen --module module_ok` succeeds.
//! 4. `gen --module module_blocked` exits non-zero with
//!    `[codegen_unblocked]` naming OQ-002.
//! 5. Removing `decisions.toml` and `open_questions.toml` leaves
//!    `validate` clean (additivity regression).
//!
//! Gate 1 + the per-check positive paths land here in Step 1; the deeper
//! gates (2–5) land alongside the analyzer behavior in Steps 3–5.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    reduction, validate,
};

const FIXTURE_TAG: &str = "specdev-triad-smoke";

/// Path to the byte-pinned plugin tree fixture used across tests.
fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "{FIXTURE_TAG}-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Scaffold a library-api project + author the canonical triad-smoke
/// spec set into it. Returns (project_root, layout).
///
/// The cross-references are deliberately authored one-way per pair —
/// e.g. `DEC-001.closes_questions = ["OQ-resolved"]` is paired with
/// `OQ-resolved.closed_by = "DEC-005"` (not DEC-001) — to avoid
/// forming 2-cycles in the canonical `depends_on` view that the
/// existing `modules_acyclic` graph check operates on. The proposal
/// allows the dual-direction reference to be missing or to disagree
/// (warn-only audit-trail field, not load-bearing). This keeps every
/// graph check exercisable on positive paths without tripping the
/// pre-existing cycle check.
///
/// Spec shape:
///
/// - `architecture.toml` with `project_decisions = ["DEC-001"]`.
/// - `requirements.toml` with three requirements: ZFM-001 references
///   DEC-001 via `decided_by`; ZFM-002 has no decided_by (and is
///   affected by an open OQ — gates `module_blocked`); ZFM-003 has no
///   decided_by (closed OQ resolved against it).
/// - `decisions.toml` with five entries:
///     - DEC-001 (accepted, closes_questions = ["OQ-resolved"])
///     - DEC-002 (superseded — replaced by DEC-004)
///     - DEC-003 (accepted, defaults_for = ["OQ-001"])
///     - DEC-004 (accepted, supersedes = ["DEC-002"])
///     - DEC-005 (accepted, plain — serves as closed_by target)
/// - `open_questions.toml` with three entries:
///     - OQ-001 (default_committed, default_via = "DEC-001", verify
///       string, affects = ["ZFM-001"])
///     - OQ-002 (open, no default_via, affects = ["ZFM-002"]) — this
///       is what blocks codegen for `module_blocked`.
///     - OQ-resolved (closed, closed_by = "DEC-005", affects =
///       ["ZFM-003"])
/// - `modules/module_ok.toml`: satisfies ["ZFM-001"], guided_by
///   ["DEC-001"]. Codegen-unblocked.
/// - `modules/module_blocked.toml`: satisfies ["ZFM-002"]. Reachable
///   through OQ-002 (open) → codegen blocks here.
pub fn build_triad_smoke(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &plugin_root(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "triad_smoke".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    write_specs(&target);

    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn write_specs(target: &Path) {
    let arch = r#"project_decisions = ["DEC-001"]

[[features]]
name = "default"
default = true
"#;
    std::fs::write(target.join("specs/architecture.toml"), arch).unwrap();

    let reqs = r#"[[source]]
id = "triad-doc"
kind = "derived"
title = "Triad smoke source"

[[requirement]]
id = "ZFM-001"
text = "Module-ok shall do the thing"
nature = "module"
source = ["triad-doc"]
decided_by = ["DEC-001"]

[[requirement]]
id = "ZFM-002"
text = "Module-blocked shall do the gated thing"
nature = "module"
source = ["triad-doc"]

[[requirement]]
id = "ZFM-003"
text = "Closed-resolved requirement"
nature = "module"
source = ["triad-doc"]
"#;
    std::fs::write(target.join("specs/requirements.toml"), reqs).unwrap();

    let decisions = r#"[[decision]]
id = "DEC-001"
title = "Smoke decision"
status = "accepted"
text = "We do the thing this way."
consequences = "Downstream knows X."
closes_questions = ["OQ-resolved"]

[[decision]]
id = "DEC-002"
title = "Original approach"
status = "superseded"
text = "Initial choice; replaced by DEC-004 once we learned more."

[[decision]]
id = "DEC-003"
title = "Default for OQ-001"
status = "accepted"
text = "Working default for OQ-001 pending verify."
defaults_for = ["OQ-001"]

[[decision]]
id = "DEC-004"
title = "Successor approach"
status = "accepted"
text = "Replaces DEC-002 with the corrected approach."
supersedes = ["DEC-002"]

[[decision]]
id = "DEC-005"
title = "Plain accepted decision"
status = "accepted"
text = "Serves as the closed_by target for OQ-resolved (one-way pairing)."
"#;
    std::fs::write(target.join("specs/decisions.toml"), decisions).unwrap();

    let open_questions = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption pending verify."
default_via = "DEC-001"
verify = "Run scenario X and observe Y."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown — blocks module_blocked until resolved."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Was a question; resolved by DEC-005 (DEC-001 also lists OQ-resolved in closes_questions for one-way coverage of graph check #6)."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();

    let module_ok = r#"name = "module_ok"
visibility = "crate"
description = "Reachable only through default_committed / closed OQs — codegen-unblocked."
satisfies = ["ZFM-001"]
guided_by = ["DEC-001"]
"#;
    std::fs::write(target.join("specs/modules/module_ok.toml"), module_ok).unwrap();

    let module_blocked = r#"name = "module_blocked"
visibility = "crate"
description = "Satisfies ZFM-002, which is affected by the open OQ-002 — codegen-blocked."
satisfies = ["ZFM-002"]
"#;
    std::fs::write(
        target.join("specs/modules/module_blocked.toml"),
        module_blocked,
    )
    .unwrap();
}

// ===========================================================================
// Baseline — the smoke fixture loads, reduces, and materializes every
// triad entity + relation kind. Subsequent steps (Step 2 / 3 / 4 / 5)
// add the per-graph-check assertions, prompt-walk byte stability, and
// gate-refusal end-to-end.
// ===========================================================================

#[test]
fn triad_smoke_reduces_clean() {
    let (target, _layout) = build_triad_smoke("reduces-clean");
    let (graph, report) =
        reduction::run_project(&target, None).expect("reduction::run_project");

    // Every triad entity kind should materialize at least one entity.
    let entity_kinds: std::collections::HashSet<&str> =
        graph.entities.iter().map(|e| e.kind.as_str()).collect();
    for kind in [
        "Architecture",
        "Decision",
        "OpenQuestion",
        "Module",
        "Requirement",
    ] {
        assert!(
            entity_kinds.contains(kind),
            "expected entity kind `{kind}` in reduction graph; got {entity_kinds:?}"
        );
    }

    // Every triad relation kind should materialize at least one edge.
    let relation_kinds: std::collections::HashSet<&str> =
        graph.relations.iter().map(|r| r.kind.as_str()).collect();
    for kind in [
        "architecture_project_decision",
        "decision_closes_open_question",
        "decision_defaults_open_question",
        "decision_supersedes_decision",
        "module_guided_by_decision",
        "open_question_affects_requirement",
        "open_question_default_via_decision",
        "requirement_decided_by_decision",
    ] {
        assert!(
            relation_kinds.contains(kind),
            "expected relation kind `{kind}` in reduction graph; got {relation_kinds:?}"
        );
    }

    // Reduction should not surface errors on its own (REDUCE_UNRESOLVED
    // would indicate a fixture authoring mistake — every endpoint
    // should resolve).
    let reduction_errors = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .count();
    assert_eq!(
        reduction_errors,
        0,
        "reduction errors on the smoke fixture: {}",
        report.render_plain()
    );

    // The OQ without `default_via` (OQ-002, status = open) emits no
    // open_question_default_via_decision edge — this is the empirical
    // verification of step 0b's reshape.
    let default_via_edges: Vec<_> = graph
        .relations
        .iter()
        .filter(|r| r.kind == "open_question_default_via_decision")
        .collect();
    assert_eq!(
        default_via_edges.len(),
        1,
        "expected exactly one default_via edge (from OQ-001); got: {default_via_edges:?}"
    );
    assert_eq!(default_via_edges[0].from, "OQ-001");
    assert_eq!(default_via_edges[0].to, "DEC-001");

    // Park the project root so a stale tmp dir is obvious in failing
    // test output.
    drop(target);
}

#[test]
fn triad_smoke_validate_baseline() {
    // Establish the baseline that validate runs cleanly on the smoke
    // fixture. With Run 1 reductions in place but Run 2 graph checks
    // not yet authored, this test asserts only that validate doesn't
    // surface unrelated errors. Step 2 + Step 3 will tighten this to
    // assert the 11 graph checks fire as expected.
    let (target, _layout) = build_triad_smoke("validate-baseline");
    let report = validate::run_project(&target, None).expect("validate");
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "validate baseline should be error-free pre-19-C graph checks; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn triad_smoke_additivity_when_files_removed() {
    // Acceptance gate 5 (TF1): removing decisions.toml and
    // open_questions.toml from the fixture must still leave validate
    // clean. Today's analyzer is already additive (the new files +
    // refs are optional); this regression-pins the property going
    // into Run 2.
    let (target, _layout) = build_triad_smoke("additivity");
    std::fs::remove_file(target.join("specs/decisions.toml")).unwrap();
    std::fs::remove_file(target.join("specs/open_questions.toml")).unwrap();

    // Strip references to decisions / OQs from the surviving spec
    // files so missing-id checks don't fire (those checks ARE the
    // point of 19-C — they should fire in the populated case but not
    // here, where the files are simply absent and so should every
    // ref-side field be).
    let arch = r#"[[features]]
name = "default"
default = true
"#;
    std::fs::write(target.join("specs/architecture.toml"), arch).unwrap();

    let reqs = r#"[[source]]
id = "triad-doc"
kind = "derived"
title = "Triad smoke source"

[[requirement]]
id = "ZFM-001"
text = "Module-ok shall do the thing"
nature = "module"
source = ["triad-doc"]
"#;
    std::fs::write(target.join("specs/requirements.toml"), reqs).unwrap();

    let module_ok = r#"name = "module_ok"
visibility = "crate"
description = "Plain module — no decisions/OQs in scope."
satisfies = ["ZFM-001"]
"#;
    std::fs::write(target.join("specs/modules/module_ok.toml"), module_ok).unwrap();
    std::fs::remove_file(target.join("specs/modules/module_blocked.toml")).unwrap();

    let report = validate::run_project(&target, None).expect("validate");
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "removing decisions/open_questions must leave validate clean; got:\n{}",
        report.render_plain()
    );
}
