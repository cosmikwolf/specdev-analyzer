//! The state × check matrix: what `check` reports for each state a module,
//! a unit entry or a requirement can be in.
//!
//! These tests state the target before the code exists. One assertion per
//! matrix cell. The principle every cell follows: `planned` and
//! `test_pending` suppress only findings about something *missing*; code
//! that exists is always checked.
//!
//! What each test holds fixed:
//!
//! 1. Modules × source rule: a module that is not planned and has no
//!    source is an error; a planned module with source gets a stale-flag
//!    warning; the other two states are silent.
//! 2. Modules × open-question gate: the gate keys on code existing, so it
//!    fires on both states whose source resolves, planned or not.
//! 3. Modules × internal edges: an undeclared import is an error in both
//!    states whose source resolves; a declared import is silent.
//! 4. Modules and unit entries × pack spec ↔ code: a spec entry missing
//!    from the code is an error unless planned (entry, table or module);
//!    an entry found in the code always has its attributes checked, and a
//!    planned one also warns.
//! 5. Requirements × has a place / has a test / test exists, across the
//!    seven requirement states, incl. derived planned and ignored tests.
//!
//! Every file mixes cells that must fire with cells that must stay silent,
//! so a `check` that reports nothing cannot pass it.

use crate::ci;

use ci::{fw_module, requirement, tree, Tree};

/// Four modules, one per module state. `BuiltOk` and `PlannedBuilt` have
/// source at the convention path; `Missing` and `PlannedEmpty` do not.
fn module_states(tag: &str) -> Tree {
    let t = tree(tag, "firmware-layered", &[]);
    t.write("specs/modules/BuiltOk.toml", &fw_module("BuiltOk", ""))
        .write("specs/modules/Missing.toml", &fw_module("Missing", ""))
        .write(
            "specs/modules/PlannedEmpty.toml",
            &fw_module("PlannedEmpty", "planned = true\n"),
        )
        .write(
            "specs/modules/PlannedBuilt.toml",
            &fw_module("PlannedBuilt", "planned = true\n"),
        )
        .write("src/built_ok.rs", "pub fn built() {}\n")
        .write("src/planned_built.rs", "pub fn built() {}\n");
    t
}

#[test]
fn modules_by_source_rule() {
    let t = module_states("src-rule");
    let c = t.check(&[]);
    c.silent("source_resolves", "BuiltOk")
        .fires("source_resolves", "Missing", "error")
        .silent("source_resolves", "PlannedEmpty")
        .silent("source_resolves", "PlannedBuilt")
        .fires("stale_planned", "PlannedBuilt", "warning")
        .silent("stale_planned", "PlannedEmpty")
        .silent("stale_planned", "BuiltOk")
        .exit_is(1);
}

#[test]
fn modules_by_open_question_gate() {
    let t = module_states("oq-gate");
    t.write("specs/requirements/REQ-GATE-001.toml", &requirement("REQ-GATE-001", ""))
        .write(
            "specs/open_questions/OQ-001.toml",
            "[[open_question]]\nid = \"OQ-001\"\ntitle = \"Open\"\nstatus = \"open\"\ntext = \"Unknown.\"\naffects = [\"REQ-GATE-001\"]\n",
        );
    for m in ["BuiltOk", "Missing", "PlannedEmpty", "PlannedBuilt"] {
        let planned = if m.starts_with("Planned") {
            "planned = true\n"
        } else {
            ""
        };
        t.write(
            &format!("specs/modules/{m}.toml"),
            &fw_module(m, &format!("{planned}satisfies = [\"REQ-GATE-001\"]\n")),
        );
    }
    let c = t.check(&[]);
    c.fires("implementation_unblocked", "BuiltOk", "error")
        .silent("implementation_unblocked", "Missing")
        .silent("implementation_unblocked", "PlannedEmpty")
        .fires("implementation_unblocked", "PlannedBuilt", "error");
}

#[test]
fn modules_by_internal_edges() {
    let t = module_states("edges");
    t.write(
        "specs/modules/Target.toml",
        "name = \"Target\"\nlayer = \"driver\"\ndescription = \"Imported.\"\n",
    )
    .write("src/target.rs", "pub fn poke() {}\n")
    .write(
        "specs/modules/Declared.toml",
        &fw_module("Declared", "depends_on = [\"Target\"]\n"),
    )
    .write(
        "src/declared.rs",
        "use crate::target::poke;\npub fn run() { poke() }\n",
    )
    // Undeclared imports from both states whose source resolves.
    .write(
        "src/built_ok.rs",
        "use crate::target::poke;\npub fn built() { poke() }\n",
    )
    .write(
        "src/planned_built.rs",
        "use crate::target::poke;\npub fn built() { poke() }\n",
    );
    let c = t.check(&[]);
    c.fires("internal_edges_declared", "BuiltOk", "error")
        .silent("internal_edges_declared", "Missing")
        .silent("internal_edges_declared", "PlannedEmpty")
        .fires("internal_edges_declared", "PlannedBuilt", "error")
        .silent("internal_edges_declared", "Declared");
}

/// The rtic + mk20dx pack: tasks in one `#[rtic::app]` (RTIC allows one
/// per crate), matched to spec entries by task name.
const RTIC_APP: &str = r#"#![no_std]
#![no_main]
#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    #[shared] struct Shared {}
    #[local] struct Local {}
    #[init] fn init(_cx: init::Context) -> (Shared, Local) { (Shared {}, Local {}) }
    #[task(priority = 1)] async fn found_ok(_cx: found_ok::Context) {}
    #[task(priority = 2)] async fn found_bad(_cx: found_bad::Context) {}
    #[task(priority = 2)] async fn planned_found(_cx: planned_found::Context) {}
    #[task(priority = 1)] async fn later_found(_cx: later_found::Context) {}
    #[task(priority = 2)] async fn pb_task(_cx: pb_task::Context) {}
}
"#;

fn task(name: &str, priority: u8, extra: &str) -> String {
    format!(
        "[[rtic.tasks]]\nname = \"{name}\"\nkind = \"software\"\npriority = {priority}\n{extra}\n"
    )
}

#[test]
fn modules_and_unit_entries_by_pack_spec_to_code() {
    let t = tree("pack", "firmware-layered", &["rtic", "mk20dx"]);
    let main = "source_file = \"src/main.rs\"\n";
    t.write("src/main.rs", RTIC_APP)
        // Not planned, source resolves: entries checked.
        .write(
            "specs/modules/App.toml",
            &format!(
                "{}{}{}{}{}{}",
                fw_module("App", main),
                task("found_ok", 1, ""),
                task("found_bad", 1, ""),
                task("missing_task", 1, ""),
                task("planned_found", 1, "planned = true"),
                task("planned_missing", 1, "planned = true")
            ),
        )
        // Not planned, no source: only the source rule applies.
        .write(
            "specs/modules/NoSrc.toml",
            &(fw_module("NoSrc", "") + &task("nosrc_task", 1, "")),
        )
        // Planned module, no source: its entries' absence is suppressed.
        .write(
            "specs/modules/Future.toml",
            &(fw_module("Future", "planned = true\n") + &task("future_task", 1, "")),
        )
        // Planned module whose source resolves: stale warning, entries still checked.
        .write(
            "specs/modules/PlannedBuilt.toml",
            &(fw_module("PlannedBuilt", &format!("planned = true\n{main}"))
                + &task("pb_task", 1, "")),
        )
        // Planned unit table: passes down to its entries.
        .write(
            "specs/modules/Later.toml",
            &format!(
                "{}[rtic]\nplanned = true\n\n{}{}",
                fw_module("Later", main),
                task("later_found", 1, ""),
                task("later_missing", 1, "")
            ),
        );
    let c = t.check(&[]);
    // Unit entries, not planned.
    c.silent("pack_attribute_matches", "App.found_ok")
        .fires("pack_attribute_matches", "App.found_bad", "error")
        .fires("pack_entity_in_code", "App.missing_task", "error");
    // Unit entries, planned (entry flag): found → warns and is checked; missing → silent.
    c.fires("stale_planned", "App.planned_found", "warning")
        .fires("pack_attribute_matches", "App.planned_found", "error")
        .silent("pack_entity_in_code", "App.planned_missing");
    // Unit entries, planned (table flag).
    c.fires("stale_planned", "Later.later_found", "warning")
        .silent("pack_entity_in_code", "Later.later_missing");
    // Modules column.
    c.fires("source_resolves", "NoSrc", "error")
        .silent("pack_entity_in_code", "NoSrc.nosrc_task")
        .silent("pack_entity_in_code", "Future.future_task")
        .fires("stale_planned", "PlannedBuilt", "warning")
        .fires("pack_attribute_matches", "PlannedBuilt.pb_task", "error");
}

/// Requirements in each state. `BuiltOk` is built and holds one real and
/// one ignored test; `PlannedEmpty` is planned with no source.
fn requirement_states(tag: &str) -> Tree {
    let t = module_states(tag);
    t.write(
        "src/built_ok.rs",
        "pub fn built() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn real_test() {}\n    #[test]\n    #[ignore]\n    fn ignored_test() {}\n}\n",
    );
    let reqs: &[(&str, &str)] = &[
        // Row 1: a satisfier that is built; not pending.
        ("REQ-A-001", "verified_by = [\"missing_a1\"]\n"),
        ("REQ-A-002", ""),
        ("REQ-A-003", "verified_by = [\"real_test\"]\n"),
        // Row 2: no satisfier; not planned.
        ("REQ-B-001", "verified_by = [\"missing_b1\"]\n"),
        ("REQ-B-002", ""),
        // Row 3: derived planned (every satisfier planned).
        ("REQ-C-001", "verified_by = [\"missing_c1\"]\n"),
        ("REQ-C-002", ""),
        // Row 4: planned = true (set).
        (
            "REQ-D-001",
            "planned = true\nverified_by = [\"missing_d1\"]\n",
        ),
        ("REQ-D-002", "planned = true\n"),
        // Row 5: test_pending = true.
        ("REQ-E-001", "test_pending = true\n"),
        (
            "REQ-E-002",
            "test_pending = true\nverified_by = [\"real_test\"]\n",
        ),
        (
            "REQ-E-003",
            "test_pending = true\nverified_by = [\"missing_e3\"]\n",
        ),
        ("REQ-E-004", "test_pending = true\n"),
        // Row 6: both flags.
        ("REQ-F-001", "planned = true\ntest_pending = true\n"),
        // Row 7: verified only by an ignored test.
        ("REQ-G-001", "verified_by = [\"ignored_test\"]\n"),
    ];
    for (id, extra) in reqs {
        t.write(
            &format!("specs/requirements/{id}.toml"),
            &requirement(id, extra),
        );
    }
    let built = [
        "REQ-A-001",
        "REQ-A-002",
        "REQ-A-003",
        "REQ-D-002",
        "REQ-E-001",
        "REQ-E-002",
        "REQ-E-003",
        "REQ-G-001",
    ];
    let sat = |ids: &[&str]| {
        format!(
            "satisfies = [{}]\n",
            ids.iter()
                .map(|i| format!("\"{i}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    t.write(
        "specs/modules/BuiltOk.toml",
        &fw_module("BuiltOk", &sat(&built)),
    )
    .write(
        "specs/modules/PlannedEmpty.toml",
        &fw_module(
            "PlannedEmpty",
            &format!("planned = true\n{}", sat(&["REQ-C-001", "REQ-C-002"])),
        ),
    );
    t
}

#[test]
fn requirements_row_1_built_satisfier() {
    let c = requirement_states("req-row1").check(&[]);
    c.silent("requirements_have_satisfiers", "REQ-A-001")
        .silent("requirements_have_tests", "REQ-A-001")
        .fires("verified_by_resolves", "REQ-A-001", "error")
        .fires("requirements_have_tests", "REQ-A-002", "error")
        .nothing_about("REQ-A-003");
}

#[test]
fn requirements_row_2_no_satisfier() {
    let c = requirement_states("req-row2").check(&[]);
    c.fires("requirements_have_satisfiers", "REQ-B-001", "error")
        .silent("requirements_have_tests", "REQ-B-001")
        .fires("verified_by_resolves", "REQ-B-001", "error")
        .fires("requirements_have_tests", "REQ-B-002", "error");
}

#[test]
fn requirements_row_3_derived_planned() {
    let c = requirement_states("req-row3").check(&[]);
    // Pass on "has a place" (it has a satisfier); the rest suppressed; no stale warning.
    c.nothing_about("REQ-C-001").nothing_about("REQ-C-002");
    // Sibling that must fire in the same run.
    c.fires("requirements_have_tests", "REQ-A-002", "error");
}

#[test]
fn requirements_row_4_planned_set() {
    let c = requirement_states("req-row4").check(&[]);
    c.nothing_about("REQ-D-001")
        .fires("stale_planned", "REQ-D-002", "warning")
        .silent("requirements_have_tests", "REQ-D-002");
}

#[test]
fn requirements_row_5_test_pending() {
    let c = requirement_states("req-row5").check(&[]);
    c.nothing_about("REQ-E-001")
        .fires("stale_test_pending", "REQ-E-002", "warning")
        .silent("verified_by_resolves", "REQ-E-003")
        .silent("requirements_have_tests", "REQ-E-003")
        // "Has a place" still runs under test_pending.
        .fires("requirements_have_satisfiers", "REQ-E-004", "error");
}

#[test]
fn requirements_row_6_both_flags_act_as_planned() {
    let c = requirement_states("req-row6").check(&[]);
    c.nothing_about("REQ-F-001")
        .fires("requirements_have_satisfiers", "REQ-B-002", "error");
}

#[test]
fn requirements_row_7_only_ignored_tests() {
    let c = requirement_states("req-row7").check(&[]);
    c.fires("verified_only_by_ignored_tests", "REQ-G-001", "info")
        .silent("verified_by_resolves", "REQ-G-001")
        .silent("requirements_have_satisfiers", "REQ-G-001")
        .silent("requirements_have_tests", "REQ-G-001");
}
