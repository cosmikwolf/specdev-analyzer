//! Triad smoke fixture — the decisions + open_questions triad.
//!
//! The fixture exercises every triad graph check (10 reference-resolution
//! checks + 1 status-consistency check) plus the codegen gate. Subsequent
//! tests (graph-check assertions, validate-time ref resolution, prompt
//! walk, gate refusal) consume `build_triad_smoke()` to scaffold a
//! canonical project and then assert against the analyzer behavior.
//!
//! Acceptance gates:
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

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    prompt, reduction, validate,
};

const FIXTURE_TAG: &str = "specdev-triad-smoke";

/// Path to the byte-pinned plugin tree fixture used across tests.
fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
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
// triad entity + relation kind. The remaining sections add the
// per-graph-check assertions, prompt-walk byte stability, and
// gate-refusal end-to-end.
// ===========================================================================

#[test]
fn triad_smoke_reduces_clean() {
    let (target, _layout) = build_triad_smoke("reduces-clean");
    let (graph, report) = reduction::run_project(&target, None).expect("reduction::run_project");

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
    // open_question_default_via_decision edge.
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
    // fixture, asserting that validate doesn't surface unrelated errors.
    // The dedicated graph-check tests below tighten this to assert the 11
    // graph checks fire as expected.
    let (target, _layout) = build_triad_smoke("validate-baseline");
    let report = validate::run_project(&target, None).expect("validate");
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "validate baseline should be error-free; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn triad_smoke_additivity_when_files_removed() {
    // Acceptance gate 5: removing decisions.toml and open_questions.toml
    // from the fixture must still leave validate clean. The analyzer is
    // additive (the new files + refs are optional); this regression-pins
    // the property.
    let (target, _layout) = build_triad_smoke("additivity");
    std::fs::remove_file(target.join("specs/decisions.toml")).unwrap();
    std::fs::remove_file(target.join("specs/open_questions.toml")).unwrap();

    // Strip references to decisions / OQs from the surviving spec
    // files so missing-id checks don't fire (those checks should fire in
    // the populated case but not here, where the files are simply absent
    // and so should every ref-side field be).
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

// ===========================================================================
// Validate-time ref resolution.
//
// Each of the 11 triad graph checks gets a red-path mutation against
// build_triad_smoke(). Note: the analyzer's reduction step is strict —
// relations whose endpoints don't resolve are dropped at reduction time
// and surface as `REDUCE_UNRESOLVED` errors with TOML provenance. This
// pre-empts the `reference_exists` graph checks. As a result, the seven
// *_resolves rules don't fire on red-path missing-target mutations —
// REDUCE_UNRESOLVED fires first with the offending name in the message.
// The graph_check entries are still authored because (a) they activate
// when relations land in the graph but reference unresolved entities by
// some other path, (b) they're required by the contract surface, and
// (c) they're the natural home for severity overrides per profile via
// [validation.rules]. Tests here therefore assert REDUCE_UNRESOLVED for
// ref-resolution and the dedicated rule names for the consistency /
// cycle / gate checks.
// ===========================================================================

/// Helper: assert that a finding with `rule == expected_rule` fires
/// against the mutated fixture, AND that the message contains `expect_in_msg`.
fn assert_rule_fires(report: &specdev_analyzer::Report, expected_rule: &str, expect_in_msg: &str) {
    let hit = report
        .findings()
        .iter()
        .any(|f| f.rule == expected_rule && f.message.contains(expect_in_msg));
    assert!(
        hit,
        "expected rule `{expected_rule}` to fire with message containing \
         `{expect_in_msg}`; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn triad_red_requirement_decided_by_resolves() {
    // Mutate ZFM-001.decided_by to a non-existent decision (DEC-999, which
    // satisfies the strict ID regex but isn't authored). The analyzer's
    // strict reduction surfaces this as REDUCE_UNRESOLVED naming the
    // missing decision.
    let (target, _) = build_triad_smoke("red-req-decided-by");
    let bad = r#"[[source]]
id = "triad-doc"
kind = "derived"
title = "Triad smoke source"

[[requirement]]
id = "ZFM-001"
text = "Module-ok shall do the thing"
nature = "module"
source = ["triad-doc"]
decided_by = ["DEC-999"]

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
    std::fs::write(target.join("specs/requirements.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "DEC-999");
}

#[test]
fn triad_red_module_guided_by_resolves() {
    let (target, _) = build_triad_smoke("red-module-guided-by");
    let bad = r#"name = "module_ok"
visibility = "crate"
description = "Reachable only through default_committed / closed OQs — codegen-unblocked."
satisfies = ["ZFM-001"]
guided_by = ["DEC-998"]
"#;
    std::fs::write(target.join("specs/modules/module_ok.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "DEC-998");
}

#[test]
fn triad_red_architecture_project_decisions_resolves() {
    let (target, _) = build_triad_smoke("red-arch-proj-dec");
    let bad = r#"project_decisions = ["DEC-997"]

[[features]]
name = "default"
default = true
"#;
    std::fs::write(target.join("specs/architecture.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "DEC-997");
}

#[test]
fn triad_red_decision_supersedes_resolves() {
    let (target, _) = build_triad_smoke("red-supersedes-resolves");
    let bad = r#"[[decision]]
id = "DEC-001"
title = "Smoke decision"
status = "accepted"
text = "We do the thing this way."
consequences = "Downstream knows X."
closes_questions = ["OQ-resolved"]

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
text = "Replaces a non-existent decision."
supersedes = ["DEC-996"]

[[decision]]
id = "DEC-005"
title = "Plain accepted decision"
status = "accepted"
text = "Serves as the closed_by target for OQ-resolved."
"#;
    std::fs::write(target.join("specs/decisions.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "DEC-996");
}

#[test]
fn triad_red_decision_supersedes_acyclic() {
    // Create a cycle: DEC-A supersedes DEC-B and DEC-B supersedes DEC-A.
    // Both must be status=superseded for one to be the supersedes target;
    // we set both as `superseded` (the chain check will also fire, that's
    // expected — we're asserting the acyclic check fires).
    let (target, _) = build_triad_smoke("red-supersedes-cycle");
    let bad = r#"[[decision]]
id = "DEC-A"
title = "A supersedes B"
status = "superseded"
text = "A."
supersedes = ["DEC-B"]

[[decision]]
id = "DEC-B"
title = "B supersedes A"
status = "superseded"
text = "B."
supersedes = ["DEC-A"]

[[decision]]
id = "DEC-005"
title = "Plain accepted decision"
status = "accepted"
text = "Serves as the closed_by target for OQ-resolved."
"#;
    std::fs::write(target.join("specs/decisions.toml"), bad).unwrap();
    // Strip references to removed decisions from arch + reqs.
    std::fs::write(
        target.join("specs/architecture.toml"),
        "[[features]]\nname = \"default\"\ndefault = true\n",
    )
    .unwrap();
    let reqs = r#"[[source]]
id = "triad-doc"
kind = "derived"
title = "Triad smoke source"

[[requirement]]
id = "ZFM-001"
text = "Module-ok shall do the thing"
nature = "module"
source = ["triad-doc"]

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
    let module_ok = r#"name = "module_ok"
visibility = "crate"
description = "Trimmed."
satisfies = ["ZFM-001"]
"#;
    std::fs::write(target.join("specs/modules/module_ok.toml"), module_ok).unwrap();
    // Adjust open_questions to drop OQ-001's default_via (DEC-001 is gone).
    let open_questions = r#"[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();

    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "decision_supersedes_acyclic", "cycle detected");
}

#[test]
fn triad_red_decision_supersedes_chain_consistency_orphan() {
    // A `superseded` decision with no inbound `supersedes` edge.
    let (target, _) = build_triad_smoke("red-orphan-superseded");
    let bad = r#"[[decision]]
id = "DEC-001"
title = "Smoke decision"
status = "accepted"
text = "We do the thing this way."
consequences = "Downstream knows X."
closes_questions = ["OQ-resolved"]

[[decision]]
id = "DEC-002"
title = "Orphan superseded"
status = "superseded"
text = "Marked superseded but no successor claims me."

[[decision]]
id = "DEC-003"
title = "Default for OQ-001"
status = "accepted"
text = "Working default for OQ-001 pending verify."
defaults_for = ["OQ-001"]

[[decision]]
id = "DEC-005"
title = "Plain accepted decision"
status = "accepted"
text = "closed_by target."
"#;
    std::fs::write(target.join("specs/decisions.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "decision_supersedes_chain_consistency", "DEC-002");
}

#[test]
fn triad_red_decision_closes_questions_resolves() {
    let (target, _) = build_triad_smoke("red-closes-questions");
    let bad = r#"[[decision]]
id = "DEC-001"
title = "Smoke decision"
status = "accepted"
text = "Closes a non-existent OQ."
closes_questions = ["OQ-995"]

[[decision]]
id = "DEC-003"
title = "Default for OQ-001"
status = "accepted"
text = "Working default."
defaults_for = ["OQ-001"]

[[decision]]
id = "DEC-005"
title = "Plain accepted"
status = "accepted"
text = "Plain."
"#;
    std::fs::write(target.join("specs/decisions.toml"), bad).unwrap();
    // Drop the OQ-resolved entry since DEC-001 no longer closes it.
    let open_questions = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-001"
verify = "Run scenario X and observe Y."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();
    // Drop ZFM-003 since OQ-resolved is gone.
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
"#;
    std::fs::write(target.join("specs/requirements.toml"), reqs).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "OQ-995");
}

#[test]
fn triad_red_decision_defaults_for_resolves() {
    let (target, _) = build_triad_smoke("red-defaults-for");
    let bad = r#"[[decision]]
id = "DEC-001"
title = "Smoke decision"
status = "accepted"
text = "Plain."
closes_questions = ["OQ-resolved"]

[[decision]]
id = "DEC-003"
title = "Defaults for non-existent OQ"
status = "accepted"
text = "Working default."
defaults_for = ["OQ-994"]

[[decision]]
id = "DEC-005"
title = "Plain accepted"
status = "accepted"
text = "Plain."
"#;
    std::fs::write(target.join("specs/decisions.toml"), bad).unwrap();
    // Re-author OQ-001 to drop default_via (DEC-003 no longer defaults for it).
    let open_questions = r#"[[open_question]]
id = "OQ-001"
title = "Now without a default_via"
status = "open"
text = "Was default_committed but DEC-003 went away."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "OQ-994");
}

#[test]
fn triad_red_open_question_affects_resolves() {
    let (target, _) = build_triad_smoke("red-oq-affects");
    let bad = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-001"
verify = "Run scenario X."
affects = ["ZFM-993"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "ZFM-993");
}

#[test]
fn triad_red_open_question_default_via_resolves() {
    let (target, _) = build_triad_smoke("red-oq-default-via");
    let bad = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-992"
verify = "Run scenario X."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "REDUCE_UNRESOLVED", "DEC-992");
}

#[test]
fn triad_red_open_question_closed_by_resolves() {
    let (target, _) = build_triad_smoke("red-oq-closed-by");
    let bad = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-001"
verify = "Run scenario X."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by a missing decision"
status = "closed"
text = "Resolved."
closed_by = "DEC-VAPOR"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), bad).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "open_question_closed_by_resolves", "DEC-VAPOR");
}

#[test]
fn triad_red_open_question_status_consistency_closed_with_non_accepted_target() {
    // OQ-resolved is `closed` and `closed_by = "DEC-002"`, but DEC-002 is
    // `superseded`, not `accepted`. Status-consistency check should fire.
    let (target, _) = build_triad_smoke("red-status-consistency");
    let open_questions = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-001"
verify = "Run scenario X."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Genuinely unknown."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by a superseded decision"
status = "closed"
text = "DEC-002 is superseded; the OQ shouldn't claim closure via a non-accepted Decision."
closed_by = "DEC-002"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "open_question_status_consistency", "DEC-002");
}

// ===========================================================================
// Codegen prompt-walk.
//
// `prompt::assemble_module_prompt` includes a "Decisions and known
// unknowns" section: module's local guided_by, module's requirements'
// decided_by, plus workspace-tier decisions when the project sits in a
// workspace. Default-committed OQs surface their verify text; closed OQs
// link to the resolving Decision; superseded Decisions are skipped.
// ===========================================================================

#[test]
fn prompt_includes_module_decisions_and_default_committed_oq() {
    let (_, layout) = build_triad_smoke("prompt-module-ok");
    let p = prompt::assemble_module_prompt(&layout, "module_ok").expect("prompt");

    // DEC-001 inlined (from ZFM-001.decided_by AND module_ok.guided_by — deduped).
    assert!(p.contains("Decision DEC-001:"), "DEC-001 absent:\n{p}");
    assert!(
        p.contains("We do the thing this way."),
        "DEC-001 text missing:\n{p}"
    );
    assert!(
        p.contains("Downstream knows X."),
        "DEC-001 consequences missing:\n{p}"
    );

    // OQ-001: default_committed, affects ZFM-001 → in scope for module_ok.
    assert!(
        p.contains("Open question OQ-001 (working assumption)"),
        "OQ-001 default_committed marker missing:\n{p}"
    );
    assert!(
        p.contains("Run scenario X and observe Y."),
        "OQ-001 verify text missing:\n{p}"
    );

    // DEC-002 (superseded) MUST NOT appear.
    assert!(
        !p.contains("Decision DEC-002:"),
        "superseded DEC-002 leaked into prompt:\n{p}"
    );

    // DEC-001 should appear exactly once (deduped across guided_by + decided_by).
    let dec001_count = p.matches("Decision DEC-001:").count();
    assert_eq!(
        dec001_count, 1,
        "DEC-001 not deduped (appeared {dec001_count} times):\n{p}"
    );
}

#[test]
fn prompt_honours_codegen_include() {
    // The field selects which prose fields the prompt inlines. It was
    // documented in every decisions schema and read by nothing: the prompt
    // always carried `text` + `consequences`.
    let (target, layout) = build_triad_smoke("prompt-codegen-include");
    let path = target.join("specs/decisions.toml");
    let decisions = std::fs::read_to_string(&path).unwrap().replace(
        "consequences = \"Downstream knows X.\"\n",
        "consequences = \"Downstream knows X.\"\n\
         alternatives = \"ALT-MARK\"\n\
         codegen_include = [\"alternatives\", \"text\"]\n",
    );
    std::fs::write(&path, decisions).unwrap();

    let p = prompt::assemble_module_prompt(&layout, "module_ok").expect("prompt");
    assert!(
        p.contains("We do the thing this way."),
        "text missing:\n{p}"
    );
    assert!(
        p.contains("Alternatives: ALT-MARK"),
        "alternatives not included:\n{p}"
    );
    assert!(
        !p.contains("Downstream knows X."),
        "consequences included although not listed:\n{p}"
    );
    // Render order is fixed (text before alternatives), not list order.
    assert!(p.find("We do the thing this way.") < p.find("Alternatives: ALT-MARK"));
}

#[test]
fn prompt_blocked_module_surfaces_open_oq() {
    let (_, layout) = build_triad_smoke("prompt-module-blocked");
    // module_blocked satisfies ZFM-002 which is affected by OQ-002 (open).
    // The prompt walk surfaces this with a loud `(UNRESOLVED — gen will refuse)` line.
    let p = prompt::assemble_module_prompt(&layout, "module_blocked").expect("prompt");
    assert!(
        p.contains("Open question OQ-002 (UNRESOLVED"),
        "open OQ-002 not surfaced for module_blocked:\n{p}"
    );
}

#[test]
fn prompt_byte_stable_across_runs() {
    let (_, layout) = build_triad_smoke("prompt-stability");
    let p1 = prompt::assemble_module_prompt(&layout, "module_ok").expect("prompt1");
    let p2 = prompt::assemble_module_prompt(&layout, "module_ok").expect("prompt2");
    assert_eq!(p1, p2, "prompt is not byte-stable across two runs");
}

// ===========================================================================
// Codegen gate.
//
// `prompt::check_codegen_gate` is the API surface; the CLI invokes it
// before assembling the prompt and exits non-zero with the refusal text
// when a reachable OQ has status=open. Tests assert
// (a) green-path module_ok unblocked, (b) module_blocked refused with
// the literal three-exit-options message naming OQ-002, (c) round-trip
// flip OQ-002 to default_committed → unblocked → flip back to open →
// refused again.
// ===========================================================================

#[test]
fn gate_module_ok_unblocked() {
    let (_, layout) = build_triad_smoke("gate-module-ok");
    let r = prompt::check_codegen_gate(&layout, "module_ok").expect("gate");
    assert!(r.is_none(), "module_ok should be unblocked; got: {:?}", r);
}

#[test]
fn gate_module_blocked_refusal_message() {
    let (_, layout) = build_triad_smoke("gate-module-blocked");
    let r = prompt::check_codegen_gate(&layout, "module_blocked").expect("gate");
    let refusal = r.expect("module_blocked should be refused");
    assert_eq!(refusal.module, "module_blocked");
    assert_eq!(refusal.oq_id, "OQ-002");
    let m = &refusal.message;
    assert!(
        m.contains("Refusing to generate code for module `module_blocked`"),
        "missing refusal header:\n{m}"
    );
    assert!(
        m.contains("open question `OQ-002`"),
        "OQ-002 not named:\n{m}"
    );
    assert!(m.contains("`Open blocker`"), "OQ title not surfaced:\n{m}");
    assert!(m.contains("Three exit options:"), "no exit options:\n{m}");
    assert!(
        m.contains("1. Resolve via a decision"),
        "exit 1 missing:\n{m}"
    );
    assert!(m.contains("2. Commit a default"), "exit 2 missing:\n{m}");
    assert!(
        m.contains("3. Mark the question moot"),
        "exit 3 missing:\n{m}"
    );
    assert!(
        m.contains("`specs/open_questions.toml`"),
        "no pointer to open_questions.toml:\n{m}"
    );
}

#[test]
fn gate_round_trip_open_to_default_committed_to_open() {
    let (target, layout) = build_triad_smoke("gate-roundtrip");
    // Initial state: blocked.
    let r1 = prompt::check_codegen_gate(&layout, "module_blocked").expect("g1");
    assert!(r1.is_some(), "initial state should be blocked");

    // Flip OQ-002 to default_committed (referencing an accepted DEC-005).
    let unblocked = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption pending verify."
default_via = "DEC-001"
verify = "Run scenario X and observe Y."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker (now defaulted)"
status = "default_committed"
text = "Was unknown; defaulted to DEC-005 working approach."
default_via = "DEC-005"
verify = "Run scenario Z."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-005"
status = "closed"
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), unblocked).unwrap();
    let r2 = prompt::check_codegen_gate(&layout, "module_blocked").expect("g2");
    assert!(
        r2.is_none(),
        "after default_committed, should be unblocked; got: {:?}",
        r2
    );

    // Flip back to open.
    let blocked_again = r#"[[open_question]]
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
text = "Resolved."
closed_by = "DEC-005"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), blocked_again).unwrap();
    let r3 = prompt::check_codegen_gate(&layout, "module_blocked").expect("g3");
    assert!(r3.is_some(), "after re-flip to open, should be blocked");
}

#[test]
fn triad_red_codegen_unblocked_warning() {
    // The green smoke fixture already has OQ-002 status=open affecting
    // ZFM-002 satisfied by module_blocked — the warning fires by default.
    let (target, _) = build_triad_smoke("red-codegen-unblocked");
    let report = validate::run_project(&target, None).expect("validate");
    assert_rule_fires(&report, "codegen_unblocked", "module_blocked");
    // Ensure module_ok is NOT flagged (its only OQ in scope is OQ-001
    // which is `default_committed`, and OQ-resolved which is `closed`).
    let any_ok_flagged = report
        .findings()
        .iter()
        .any(|f| f.rule == "codegen_unblocked" && f.message.contains("module_ok"));
    assert!(
        !any_ok_flagged,
        "codegen_unblocked should not flag module_ok; got:\n{}",
        report.render_plain()
    );
}

// Regression for the joint-filter — when a project authors dual
// back-references (`closes_questions` ↔ `closed_by`, `defaults_for` ↔
// `default_via`) on the SAME decision pair (so each direction is a
// 2-cycle in the canonical depends_on view), modules_acyclic must NOT
// fire. Previously this scenario was blocked because the cycle check
// chain-walked all canonical-`depends_on` relations including the triad
// ones.
#[test]
fn modules_acyclic_ignores_triad_dual_back_references() {
    let (target, _layout) = build_triad_smoke("acyclic-joint-filter");

    // Author the dual back-reference on DEC-001 ↔ OQ-resolved AND on
    // DEC-003 ↔ OQ-001 (both pair directions present). Previously this
    // would form 2-cycles in the canonical depends_on view and trip
    // modules_acyclic.
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
text = "Co-target for OQ-resolved (kept for parity with the smoke fixture)."
"#;
    std::fs::write(target.join("specs/decisions.toml"), decisions).unwrap();

    let open_questions = r#"[[open_question]]
id = "OQ-001"
title = "Default-committed question"
status = "default_committed"
text = "Working assumption pending verify."
default_via = "DEC-003"
verify = "Run scenario X and observe Y."
affects = ["ZFM-001"]

[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Blocks module_blocked until resolved."
affects = ["ZFM-002"]

[[open_question]]
id = "OQ-resolved"
title = "Closed by DEC-001"
status = "closed"
text = "Symmetric back-reference: DEC-001 closes OQ-resolved AND OQ-resolved.closed_by = DEC-001."
closed_by = "DEC-001"
affects = ["ZFM-003"]
"#;
    std::fs::write(target.join("specs/open_questions.toml"), open_questions).unwrap();

    let report = validate::run_project(&target, None).expect("validate");
    let cycle_findings: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| f.rule == "modules_acyclic")
        .collect();
    assert!(
        cycle_findings.is_empty(),
        "modules_acyclic should not fire on triad dual back-references; got:\n{}",
        report.render_plain()
    );
}
