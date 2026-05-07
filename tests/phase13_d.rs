//! Phase 13-D integration tests — HANDOFF.md generator (doc 16 §2).
//!
//! Coverage:
//!   * HANDOFF.md emitted at `specs/HANDOFF.md` on every skeleton run.
//!   * Carries the four documented sections: Coverage matrix, Skeleton
//!     inventory, Plugin gaps, Suggested next steps.
//!   * Coverage matrix flags ORPHAN requirements (no module satisfies).
//!   * Inventory carries per-module stub + re-export counts.
//!   * Suggested next steps splits modules by presence/absence of
//!     `context_files`.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    skeleton,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase13d-{tag}-{}-{}",
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

fn fresh_library_project(tag: &str) -> (PathBuf, LayoutContext) {
    let target = tmp_dir(tag);
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "h_demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn write_two_modules_with_orphan_req(target: &Path) {
    std::fs::write(
        target.join("specs/modules/Core.toml"),
        r#"name = "Core"
visibility = "pub"

[[items]]
kind = "fn"
name = "go"
description = "Run the thing."
satisfies = ["REQ-A"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Sys.toml"),
        r#"name = "Sys"
visibility = "pub"
context_files = ["docs/sys-context.md"]

[[items]]
kind = "fn"
name = "boot"
description = "Boot."
satisfies = ["REQ-B"]
"#,
    )
    .unwrap();
    // Context file for Sys module
    std::fs::write(
        target.join("docs-sys-context.md"),
        "# Sys context\n\nfacts here\n",
    )
    .unwrap();
    std::fs::create_dir_all(target.join("docs")).unwrap();
    std::fs::write(
        target.join("docs/sys-context.md"),
        "# Sys context\n\nfacts here\n",
    )
    .unwrap();

    // Three requirements: A and B satisfied, C orphaned.
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[source]]
id = "s"
title = "spec"

[[requirement]]
id = "REQ-A"
text = "go"
source = "s"

[[requirement]]
id = "REQ-B"
text = "boot"
source = "s"

[[requirement]]
id = "REQ-C"
text = "orphan"
source = "s"
"#,
    )
    .unwrap();
}

#[test]
fn handoff_md_emitted_at_specs_root() {
    let (target, layout) = fresh_library_project("emitted");
    write_two_modules_with_orphan_req(&target);

    let report = skeleton::run_skeleton(&layout).expect("skeleton");
    let h = target.join("specs/HANDOFF.md");
    assert!(h.is_file(), "HANDOFF.md should exist at {}", h.display());
    // Compare canonical forms — discover() canonicalizes /var → /private/var on macOS.
    assert_eq!(
        std::fs::canonicalize(&report.handoff_md_path).unwrap(),
        std::fs::canonicalize(&h).unwrap(),
    );
}

#[test]
fn handoff_md_contains_four_sections() {
    let (target, layout) = fresh_library_project("sections");
    write_two_modules_with_orphan_req(&target);
    skeleton::run_skeleton(&layout).expect("skeleton");
    let body = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    for header in [
        "## Coverage matrix",
        "## Skeleton inventory",
        "## Plugin gaps",
        "## Suggested next steps",
    ] {
        assert!(
            body.contains(header),
            "expected section {header} in HANDOFF.md; body:\n{body}"
        );
    }
}

#[test]
fn handoff_md_flags_orphan_requirement() {
    let (target, layout) = fresh_library_project("orphan");
    write_two_modules_with_orphan_req(&target);
    skeleton::run_skeleton(&layout).expect("skeleton");
    let body = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    let req_c_line = body
        .lines()
        .find(|l| l.contains("`REQ-C`"))
        .expect("REQ-C row must be present");
    assert!(
        req_c_line.contains("ORPHAN"),
        "REQ-C should be marked ORPHAN; got: {req_c_line}"
    );
    // REQ-A row should name Core.
    let req_a_line = body
        .lines()
        .find(|l| l.contains("`REQ-A`"))
        .expect("REQ-A row must be present");
    assert!(req_a_line.contains("`Core`"), "REQ-A row: {req_a_line}");
}

#[test]
fn handoff_md_inventory_carries_stub_and_reexport_counts() {
    let (target, layout) = fresh_library_project("counts");
    std::fs::write(
        target.join("specs/modules/Solo.toml"),
        r#"name = "Solo"
visibility = "pub"

[[items]]
kind = "fn"
name = "f1"
description = "stub one"
satisfies = ["REQ-1"]

[[items]]
kind = "fn"
name = "f2"
description = "stub two"
satisfies = ["REQ-1"]

[[items]]
kind = "re_export"
name = "X"
target = "other::X"
visibility = "pub"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Other.toml"),
        r#"name = "Other"
visibility = "pub"

[[items]]
kind = "struct"
name = "X"
description = "the X type"
satisfies = ["REQ-1"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[source]]
id = "s"
title = "t"

[[requirement]]
id = "REQ-1"
text = "things"
source = "s"
"#,
    )
    .unwrap();
    skeleton::run_skeleton(&layout).expect("skeleton");
    let body = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    // Solo: 2 stubs, 1 re-export.
    let solo_idx = body.find("### `Solo`").expect("Solo header");
    let after_header = solo_idx + "### `Solo`".len();
    let next_section = body[after_header..]
        .find("### ")
        .map(|i| after_header + i)
        .unwrap_or(body.len());
    let solo_window = &body[solo_idx..next_section];
    assert!(
        solo_window.contains("Stubs: **2**"),
        "Solo should have 2 stubs; window:\n{solo_window}"
    );
    assert!(
        solo_window.contains("Re-exports: **1**"),
        "Solo should have 1 re-export; window:\n{solo_window}"
    );
}

#[test]
fn suggested_next_steps_splits_by_context_files() {
    let (target, layout) = fresh_library_project("splits");
    write_two_modules_with_orphan_req(&target);
    skeleton::run_skeleton(&layout).expect("skeleton");
    let body = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    let nxt = body.find("## Suggested next steps").unwrap();
    let after = &body[nxt..];
    // `Sys` has context_files; `Core` does not.
    assert!(
        after.contains("Modules ready for `--strict`"),
        "expected ready-for-strict header; tail:\n{after}"
    );
    assert!(after.contains("`Sys`"), "Sys (with context) should appear ready");
    assert!(
        after.contains("Modules to add `context_files` to"),
        "expected needs-context header; tail:\n{after}"
    );
    assert!(after.contains("`Core`"), "Core should appear in needs-context list");
}

#[test]
fn handoff_md_regenerates_each_run() {
    let (target, layout) = fresh_library_project("regen");
    write_two_modules_with_orphan_req(&target);
    skeleton::run_skeleton(&layout).expect("first run");
    let first = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    // Wipe src so skeleton can rerun (it refuses non-empty src).
    std::fs::remove_dir_all(target.join("src")).unwrap();
    skeleton::run_skeleton(&layout).expect("second run");
    let second = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    assert_eq!(
        first, second,
        "HANDOFF.md content should be deterministic across runs"
    );
}

#[test]
fn handoff_md_without_requirements_toml_omits_matrix() {
    let (target, layout) = fresh_library_project("no-reqs");
    std::fs::write(
        target.join("specs/modules/Lone.toml"),
        r#"name = "Lone"
visibility = "pub"

[[items]]
kind = "struct"
name = "X"
description = "Type X"
satisfies = ["REQ-Z"]
"#,
    )
    .unwrap();
    // Intentionally omit specs/requirements.toml.
    skeleton::run_skeleton(&layout).expect("skeleton");
    let body = std::fs::read_to_string(target.join("specs/HANDOFF.md")).unwrap();
    assert!(
        body.contains("No `requirements.toml` found"),
        "expected omission notice; body:\n{body}"
    );
    // Inventory is still present.
    assert!(body.contains("### `Lone`"));
}
