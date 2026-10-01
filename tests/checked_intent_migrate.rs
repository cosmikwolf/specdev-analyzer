//! The removal of code generation, and what `migrate` does to a `1.x`
//! tree on the way to `spec_version = "2.0"`.
//!
//! These tests state the target before the code exists. What each test
//! holds fixed:
//!
//! 1. Every generation subcommand is gone: `gen`, `prompt`, `gen-record`,
//!    `gen-record-gate`, `derive`, `manifest`, `cargo-toml`, `extract`,
//!    `report gen-drift`, `report gen-plan`; and `check` exists.
//! 2. `migrate` removes generation state: `gen-lineage.toml`, the inlined
//!    `codegen/` directory, and the module tables that restate the
//!    manifest (`external_deps`). No per-module lineage directory appears.
//! 3. A spec test item's `verifies` moves onto the requirement as
//!    `verified_by`, and the test item leaves the module spec.
//! 4. A requirement that is not planned (set or derived) and has no
//!    `verified_by` after the move gets `test_pending = true` — computed
//!    **after** `--plan-unresolved`, so a requirement whose satisfiers it
//!    marks planned does not get the flag.
//! 5. `migrate` reports modules with no resolvable source and modules whose
//!    source holds only tests; it marks nothing planned unless asked.
//! 6. The rule `codegen_unblocked` is renamed `implementation_unblocked`, and it and
//!    `requirements_have_satisfiers` become errors (the 1.x templates ship both as
//!    warnings).
//! 7. An inlined profile schema that predates `source_file` is refreshed,
//!    and its profile files get a baseline, so `source_file` and `planned`
//!    validate and `drift` no longer reports the profile `not_recorded`.

#[path = "common/checked_intent.rs"]
mod ci;

use ci::{text, tree, tree_v1, Tree};

#[test]
fn generation_subcommands_are_gone() {
    let t = tree("removal", "library-api", &[]);
    for cmd in [
        &["gen", "--preamble"][..],
        &["prompt", "--module", "x"],
        &["gen-record", "--module", "x", "src/x.rs"],
        &["gen-record-gate", "--label", "x"],
        &["derive"],
        &["manifest"],
        &["cargo-toml"],
        &["extract"],
        &["report", "gen-drift"],
        &["report", "gen-plan"],
    ] {
        let out = t.analyzer(cmd);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.code() == Some(2) && err.contains("unrecognized subcommand"),
            "`{}` must be gone:\n{}",
            cmd.join(" "),
            text(&out)
        );
    }
}

#[test]
fn check_is_a_subcommand() {
    let t = tree("check-exists", "library-api", &[]);
    let help = t.analyzer(&["--help"]);
    let listed = String::from_utf8_lossy(&help.stdout)
        .lines()
        .any(|l| l.trim_start().starts_with("check "));
    assert!(listed, "`check` must be listed:\n{}", text(&help));
}

const REQS_V1: &str = r#"[[requirement]]
id = "REQ-M-001"
text = "Parse the input."
nature = "module"

[[requirement]]
id = "REQ-M-002"
text = "Write the output."
nature = "module"

[[requirement]]
id = "REQ-M-003"
text = "Compress later."
nature = "module"
"#;

/// A `1.x` library-api tree with everything `migrate` must handle. The
/// frozen tree already carries `gen-lineage.toml` (one recorded file,
/// `src/built.rs`), the inlined `codegen/` directory and `codegen_unblocked`.
fn v1_project(tag: &str) -> Tree {
    let t = tree_v1(tag, "library-api");
    t.write("specs/requirements.toml", REQS_V1)
        // A test item that verifies a requirement, plus a manifest-restating table.
        .write(
            "specs/modules/tested.toml",
            r#"name = "tested"
visibility = "crate"
description = "Parses."
satisfies = ["REQ-M-001"]

[[items]]
kind = "fn"
name = "test_parse"
description = "Parses the sample."
test = true
test_kind = "unit"
verifies = ["REQ-M-001"]

[[external_deps]]
crate = "nom"
version = "7"
source = "crates-io"
"#,
        )
        .write("src/tested.rs", "pub(crate) fn parse() {}\n#[cfg(test)]\nmod t {\n    #[test]\n    fn test_parse() {}\n}\n")
        .write("specs/modules/built.toml", "name = \"built\"\nvisibility = \"crate\"\ndescription = \"Writes.\"\nsatisfies = [\"REQ-M-002\"]\n")
        .write("src/built.rs", "pub(crate) fn write() {}\n")
        .write("specs/modules/unbuilt.toml", "name = \"unbuilt\"\nvisibility = \"crate\"\ndescription = \"Compresses.\"\nsatisfies = [\"REQ-M-003\"]\n")
        .write("specs/modules/testsonly.toml", "name = \"testsonly\"\nvisibility = \"crate\"\ndescription = \"Stand-in.\"\n")
        .write("src/testsonly.rs", "#[cfg(test)]\nmod t {\n    #[test]\n    fn bench_like() {}\n}\n");
    assert!(t.root.join("specs/specdev/gen-lineage.toml").is_file());
    assert!(t.root.join("specs/specdev/codegen").is_dir());
    assert!(t.read("specs/specdev/domain.toml").contains("codegen_unblocked"));
    // The input must be a valid 1.x tree, or a red test proves nothing.
    let v = t.analyzer(&["validate", "-q"]);
    assert!(
        v.status.success() && text(&v).lines().any(|l| l.starts_with("0 error(s)")),
        "the 1.x fixture must validate clean:\n{}",
        text(&v)
    );
    t
}

fn migrate(t: &Tree, extra: &[&str]) -> String {
    let plugin = t.plugin.to_str().unwrap().to_string();
    let mut args = vec!["migrate", "--plugin-root", plugin.as_str()];
    args.extend_from_slice(extra);
    let out = t.analyzer(&args);
    assert!(out.status.success(), "migrate failed:\n{}", text(&out));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn toml_at(t: &Tree, rel: &str) -> toml::Table {
    t.read(rel).parse().unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn requirement_of(t: &Tree, id: &str) -> toml::Value {
    let doc = toml_at(t, &format!("specs/requirements/{id}.toml"));
    doc["requirement"].as_array().unwrap()[0].clone()
}

#[test]
fn migrate_removes_generation_state() {
    let t = v1_project("mig-state");
    migrate(&t, &[]);
    assert!(t.read("spec.toml").starts_with("spec_version = \"2.0\""));
    assert!(!t.root.join("specs/specdev/gen-lineage.toml").exists(), "gen-lineage.toml must go");
    assert!(!t.root.join("specs/specdev/gen-lineage").exists(), "no per-module lineage directory appears");
    assert!(!t.root.join("specs/specdev/codegen").exists(), "inlined codegen/ must go");
    let tested = toml_at(&t, "specs/modules/tested.toml");
    assert!(!tested.contains_key("external_deps"), "manifest-restating table must go: {tested:?}");
}

#[test]
fn migrate_moves_verifies_and_sets_test_pending() {
    let t = v1_project("mig-verifies");
    migrate(&t, &[]);
    let tested = toml_at(&t, "specs/modules/tested.toml");
    let test_items = tested
        .get("items")
        .and_then(|i| i.as_array())
        .map(|a| a.iter().filter(|i| i.get("test").and_then(|v| v.as_bool()) == Some(true)).count())
        .unwrap_or(0);
    assert_eq!(test_items, 0, "test items leave the module spec: {tested:?}");
    let r1 = requirement_of(&t, "REQ-M-001");
    assert_eq!(r1["verified_by"].as_array().unwrap(), &vec![toml::Value::String("test_parse".into())]);
    assert!(r1.get("test_pending").is_none(), "{r1:?}");
    // Built, no test: truthful flag.
    assert_eq!(requirement_of(&t, "REQ-M-002").get("test_pending").and_then(|v| v.as_bool()), Some(true));
    // Its satisfier is unbuilt but not marked planned (no --plan-unresolved): flagged too.
    assert_eq!(requirement_of(&t, "REQ-M-003").get("test_pending").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn migrate_reports_unresolved_and_tests_only_modules_and_plans_only_when_asked() {
    let t = v1_project("mig-report");
    let out = migrate(&t, &[]);
    assert!(out.contains("unbuilt"), "report the module with no resolvable source:\n{out}");
    assert!(out.contains("testsonly"), "report the module whose source is only tests:\n{out}");
    assert!(!toml_at(&t, "specs/modules/unbuilt.toml").contains_key("planned"), "nothing planned unless asked");
    t.check(&[]).fires("source_resolves", "unbuilt", "error");
}

#[test]
fn plan_unresolved_runs_before_test_pending() {
    let t = v1_project("mig-plan");
    migrate(&t, &["--plan-unresolved"]);
    let unbuilt = toml_at(&t, "specs/modules/unbuilt.toml");
    assert_eq!(unbuilt.get("planned").and_then(|v| v.as_bool()), Some(true), "{unbuilt:?}");
    // REQ-M-003's only satisfier is now planned: derived planned, so no test_pending.
    assert!(requirement_of(&t, "REQ-M-003").get("test_pending").is_none());
    assert_eq!(requirement_of(&t, "REQ-M-002").get("test_pending").and_then(|v| v.as_bool()), Some(true));
    t.check(&[]).silent("source_resolves", "unbuilt").nothing_about("REQ-M-003");
}

#[test]
fn migrate_renames_the_gate_rule() {
    let t = v1_project("mig-rule");
    migrate(&t, &[]);
    let domain = t.read("specs/specdev/domain.toml");
    assert!(domain.contains("implementation_unblocked"), "{domain}");
    assert!(!domain.contains("codegen_unblocked"), "{domain}");
}

/// The 1.x templates ship the coverage check and the gate as "warning".
/// The checked-intent format makes both errors; a rename alone would keep
/// the gate a warning in every migrated tree. `migrate` sets the value and
/// says so.
#[test]
fn migrate_sets_the_error_severities() {
    let t = v1_project("mig-severity");
    let before = t.read("specs/specdev/domain.toml");
    assert!(severity(&before, "requirements_have_satisfiers") == Some("warning".into()), "fixture: {before}");
    assert!(severity(&before, "codegen_unblocked") == Some("warning".into()), "fixture: {before}");
    let out = migrate(&t, &[]);
    let after = t.read("specs/specdev/domain.toml");
    assert_eq!(severity(&after, "requirements_have_satisfiers").as_deref(), Some("error"), "{after}");
    assert_eq!(severity(&after, "implementation_unblocked").as_deref(), Some("error"), "{after}");
    assert!(out.contains("requirements_have_satisfiers") && out.contains("implementation_unblocked"), "report each change:\n{out}");
}

fn severity(domain: &str, rule: &str) -> Option<String> {
    let doc: toml::Table = domain.parse().ok()?;
    doc.get("validation")?.get("rules")?.get(rule)?.as_str().map(str::to_string)
}

/// `schema` without the top-level property `name` (two-space indent) and
/// every line of its body, up to the next property at the same indent.
fn without_property(schema: &str, name: &str) -> String {
    let header = format!("  {name}:");
    let mut out = String::new();
    let mut skipping = false;
    for line in schema.split_inclusive('\n') {
        let at_property = line.starts_with("  ") && !line.starts_with("   ") && !line.trim().is_empty();
        let at_top = !line.starts_with(' ') && !line.trim().is_empty();
        if line.trim_end() == header {
            skipping = true;
            continue;
        }
        if skipping && (at_property || at_top) {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
        }
    }
    out
}

#[test]
fn migrate_refreshes_an_old_inlined_profile_schema() {
    let t = tree_v1("mig-old-schema", "firmware-layered");
    // Make it look like a tree bootstrapped before `source_file` existed:
    // no `source_file` property, no profile-file baseline.
    let schema_rel = "specs/specdev/schemas/module.schema.yaml";
    let schema = t.read(schema_rel);
    t.write(schema_rel, &without_property(&schema, "source_file"));
    assert!(!t.read(schema_rel).contains("source_file"), "the property and its body must go");
    let lineage = t.read("specs/specdev/lineage.toml");
    let cut = lineage.find("[profile_files]").expect("baseline present");
    t.write("specs/specdev/lineage.toml", &lineage[..cut]);
    t.write(
        "specs/modules/Mod.toml",
        "name = \"Mod\"\nlayer = \"application\"\ndescription = \"x\"\nsource_file = \"src/main.rs\"\n",
    )
    .write("src/main.rs", "fn main() {}\n");
    let before = t.analyzer(&["validate", "-q"]);
    assert!(
        String::from_utf8_lossy(&before.stdout).contains("'source_file' was unexpected")
            || String::from_utf8_lossy(&before.stderr).contains("'source_file' was unexpected"),
        "the fixture must reproduce the old-schema error first:\n{}",
        text(&before)
    );
    migrate(&t, &[]);
    t.write(
        "specs/modules/Later.toml",
        "name = \"Later\"\nlayer = \"application\"\ndescription = \"y\"\nplanned = true\n",
    );
    let after = t.analyzer(&["validate", "-q"]);
    let all = text(&after);
    assert!(!all.contains("'source_file' was unexpected") && !all.contains("'planned' was unexpected"), "{all}");
    let drift = t.analyzer(&["drift"]);
    let d = String::from_utf8_lossy(&drift.stdout);
    let profile = d.split("profile:").nth(1).unwrap_or("");
    assert!(!profile.contains("not_recorded"), "profile files must have a baseline after migrate:\n{d}");
}
