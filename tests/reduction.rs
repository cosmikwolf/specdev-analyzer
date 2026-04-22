//! Reduction executor snapshots (phase 14). Locks down the full graph YAML
//! emitted for blinky-a2dp and counter-egui, plus the provenance shape of an
//! `unresolved endpoint` finding synthesized from a minimal broken project.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{domain::Severity, reduction, report::Report};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn workspace_root() -> PathBuf {
    repo_root().parent().unwrap().to_path_buf()
}

fn meta_dir() -> PathBuf {
    workspace_root().join("specdev/schema/meta/v1")
}

fn firmware_reduction() -> PathBuf {
    workspace_root().join("specdev/domains/firmware/reduction.yaml")
}

fn egui_reduction() -> PathBuf {
    workspace_root().join("specdev/domains/egui/reduction.yaml")
}

fn blinky_root() -> PathBuf {
    workspace_root().join("specdev/examples/blinky-a2dp")
}

fn counter_root() -> PathBuf {
    workspace_root().join("specdev/examples/counter-egui")
}

fn render_findings(r: &Report) -> String {
    let mut rows: Vec<String> = r
        .findings()
        .iter()
        .map(|f| {
            let sev = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            let file = f.file.as_deref().unwrap_or("");
            let line = f.line.map(|n| format!(":{n}")).unwrap_or_default();
            format!(
                "{sev} [{}] {}{}{}: {}",
                f.rule, file, line, f.pointer, f.message
            )
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

// -----------------------------------------------------------------------------
// Full-graph snapshots for the two reference projects.
// -----------------------------------------------------------------------------

#[test]
fn reduction_firmware_blinky_graph() {
    let spec = reduction::load_reduction_spec(&firmware_reduction(), &meta_dir())
        .expect("load firmware reduction");
    let (graph, report) = reduction::execute(&spec, &blinky_root()).expect("execute");
    assert!(
        !report.has_errors(),
        "blinky reduction reported errors:\n{}",
        render_findings(&report)
    );
    let yaml = reduction::to_yaml(&graph).expect("to_yaml");
    insta::assert_snapshot!(yaml);
}

#[test]
fn reduction_egui_counter_graph() {
    let spec = reduction::load_reduction_spec(&egui_reduction(), &meta_dir())
        .expect("load egui reduction");
    let (graph, report) = reduction::execute(&spec, &counter_root()).expect("execute");
    assert!(
        !report.has_errors(),
        "counter reduction reported errors:\n{}",
        render_findings(&report)
    );
    let yaml = reduction::to_yaml(&graph).expect("to_yaml");
    insta::assert_snapshot!(yaml);
}

// -----------------------------------------------------------------------------
// Error shape: a missing-port architecture connection surfaces as a
// REDUCE_UNRESOLVED finding with provenance on the architecture file.
// -----------------------------------------------------------------------------

#[test]
fn reduction_firmware_unresolved_endpoint() {
    let tmp = std::env::temp_dir().join(format!(
        "specdev-reduction-broken-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("specs/modules")).unwrap();

    // One module with a single port. The architecture connects it to a port
    // that doesn't exist on either side, so both endpoints of the `connects`
    // relation are unresolved.
    fs::write(
        tmp.join("specs/modules/LoneModule.toml"),
        r#"
name = "LoneModule"
layer = "driver"
description = "Single-port module used to force an unresolved connection endpoint."
depends_on = []
satisfies = []

[metadata.MemoryModel]
allocation = "static_alloc"
max_instances = 1
owner = "owned"

[metadata.ConcurrencyModel]
protection = "none"
isr_safe = false
reentrant = false
shared_state = []

[metadata.ErrorHandling]
strategy = "result_type"
error_type = "LoneError"

[metadata.Ownership]
owner = "owned"

[[ports]]
name = "in_port"
port_type = "DataPort"
direction = "in"
flow_type = "Bits"
"#,
    )
    .unwrap();

    fs::write(
        tmp.join("specs/architecture.toml"),
        r#"
[[connections]]
from = "LoneModule.out_port"
to   = "Nowhere.in_port"
flow_type = "Bits"
"#,
    )
    .unwrap();

    let spec = reduction::load_reduction_spec(&firmware_reduction(), &meta_dir())
        .expect("load firmware reduction");
    let (_graph, report) = reduction::execute(&spec, &tmp).expect("execute");

    let rendered = render_findings(&report);
    let _ = fs::remove_dir_all(&tmp);
    insta::assert_snapshot!(rendered);
}
