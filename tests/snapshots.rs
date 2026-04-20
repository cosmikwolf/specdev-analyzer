//! Snapshot suite (D23).
//!
//! Locks down analyzer output at the boundary: the YAML the extract pipeline
//! emits for a known-good project, and the rule-code/severity tuples it emits
//! on known-bad fixtures. We intentionally snapshot *stable* fields only — no
//! free-form jsonschema error strings, no filesystem paths — so snapshots
//! don't churn across library upgrades.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{config::SpecConfig, extract, report::Report, validate};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn workspace_root() -> PathBuf {
    repo_root().parent().unwrap().to_path_buf()
}

fn domain_file() -> PathBuf {
    workspace_root().join("specdev/domains/firmware/domain.toml")
}

fn schema_dir() -> PathBuf {
    workspace_root().join("specdev/schema/firmware/v1")
}

fn blinky_root() -> PathBuf {
    workspace_root().join("specdev/examples/blinky-a2dp")
}

fn tmp_out(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-snapshots-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    p
}

/// Pull the stable (rule, severity) pairs out of a report and sort them for
/// snapshotting. Avoids depending on free-form error messages from jsonschema.
fn rule_codes(report: &Report) -> String {
    let mut rows: Vec<String> = report
        .findings()
        .iter()
        .map(|f| {
            let sev = match f.severity {
                specdev_analyzer::domain::Severity::Error => "error",
                specdev_analyzer::domain::Severity::Warning => "warning",
                specdev_analyzer::domain::Severity::Info => "info",
            };
            let rule = if f.rule.is_empty() {
                "SCHEMA"
            } else {
                f.rule.as_str()
            };
            format!("{sev} {rule}")
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows.join("\n")
}

// -----------------------------------------------------------------------------
// Happy-path extract snapshots
// -----------------------------------------------------------------------------

#[test]
fn extract_blinky_architecture_yaml_snapshot() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let result = extract::run(&inputs).expect("extract");
    let out = tmp_out("arch");
    extract::write(&result, &out).expect("write");
    let yaml = fs::read_to_string(out.join("architecture.yaml")).expect("read arch yaml");
    let _ = fs::remove_dir_all(&out);

    insta::assert_snapshot!(yaml);
}

#[test]
fn extract_blinky_requirements_yaml_snapshot() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let result = extract::run(&inputs).expect("extract");
    let out = tmp_out("reqs");
    extract::write(&result, &out).expect("write");
    let yaml = fs::read_to_string(out.join("requirements.yaml")).expect("read reqs yaml");
    let _ = fs::remove_dir_all(&out);

    insta::assert_snapshot!(yaml);
}

#[test]
fn extract_blinky_module_yaml_snapshot() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let result = extract::run(&inputs).expect("extract");
    let out = tmp_out("mod");
    extract::write(&result, &out).expect("write");
    let yaml = fs::read_to_string(out.join("StatusLed.yaml")).expect("read module yaml");
    let _ = fs::remove_dir_all(&out);

    insta::assert_snapshot!(yaml);
}

// -----------------------------------------------------------------------------
// Schema-error fixtures (rule codes only — error strings are jsonschema's)
// -----------------------------------------------------------------------------

#[test]
fn schema_error_bad_layer_rule_codes() {
    let fixture = repo_root().join("tests/specs/invalid/bad_layer.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    insta::assert_snapshot!(rule_codes(&report));
}

#[test]
fn schema_error_metadata_typo_rule_codes() {
    let fixture = repo_root().join("tests/specs/invalid/metadata_typo.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    insta::assert_snapshot!(rule_codes(&report));
}

#[test]
fn schema_error_fsm_missing_event_rule_codes() {
    let fixture = repo_root().join("tests/specs/invalid/fsm_missing_event.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    insta::assert_snapshot!(rule_codes(&report));
}

// -----------------------------------------------------------------------------
// Graph-error fixture — inline so we can hit LAYER001 + acyclic together
// without adding a whole sibling project to the repo.
// -----------------------------------------------------------------------------

fn write_graph_error_project() -> PathBuf {
    let project = tmp_out("graph");
    fs::create_dir_all(project.join("specs/modules")).unwrap();
    fs::write(
        project.join("spec.toml"),
        r#"spec_version = "1.0"
[project]
name = "graph-errors"
domain = "firmware"
language = "rust"

[source]
root = "src"
layout = "flat"

[paths.overrides]
"#,
    )
    .unwrap();

    // A PAC module that illegally depends on a driver — triggers LAYER001.
    // And depends_on forms a cycle A → B → A — triggers LAYER002.
    let modules = [
        (
            "A",
            "PAC",
            vec!["B"],
            // also: depends_on refers to an existing module, so reference_exists is fine.
        ),
        ("B", "driver", vec!["A"]),
    ];

    for (name, layer, deps) in modules {
        let deps_arr = deps
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            project.join(format!("specs/modules/{name}.toml")),
            format!(
                r#"name = "{name}"
layer = "{layer}"
description = "graph-error fixture module {name}"
depends_on = [{deps_arr}]
satisfies = []

[metadata.MemoryModel]
allocation = "static_alloc"
max_instances = 1
owner = "owned"

[metadata.ConcurrencyModel]
protection = "mutex"
isr_safe = false
reentrant = false
shared_state = []

[metadata.ErrorHandling]
strategy = "result_type"
error_type = "E"
"#
            ),
        )
        .unwrap();
    }
    project
}

#[test]
fn graph_error_layer_and_cycle_rule_codes() {
    let project = write_graph_error_project();
    let inputs = validate::resolve_inputs(&project, Some(&domain_file())).expect("resolve inputs");
    let report = validate::run(&inputs).expect("validate");
    let snapshot = rule_codes(&report);
    let _ = fs::remove_dir_all(&project);

    insta::assert_snapshot!(snapshot);
}

// -----------------------------------------------------------------------------
// Cross-version spec_version resolution — `schema_major()` turns spec_version
// into a directory prefix (`1.x` → `v1`, `2.3` → `v2`).
// -----------------------------------------------------------------------------

fn schema_major_for(version: &str) -> String {
    let project = tmp_out(&format!("ver-{}", version.replace('.', "_")));
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("spec.toml"),
        format!(
            r#"spec_version = "{version}"
[project]
name = "ver"
domain = "firmware"
language = "rust"
"#
        ),
    )
    .unwrap();
    let cfg = SpecConfig::load(&project.join("spec.toml")).unwrap();
    let out = cfg.schema_major();
    let _ = fs::remove_dir_all(&project);
    out
}

#[test]
fn spec_version_maps_to_schema_major_snapshot() {
    let matrix = [
        ("1.0", schema_major_for("1.0")),
        ("1.3", schema_major_for("1.3")),
        ("2.0", schema_major_for("2.0")),
        ("10.25", schema_major_for("10.25")),
    ];
    let rendered = matrix
        .iter()
        .map(|(v, s)| format!("{v} -> {s}"))
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(rendered);
}
