//! Performance regression test — Phase 11 exit criterion.
//!
//! Generates a synthetic project with 100+ module TOML files and asserts
//! end-to-end `validate::run()` + `extract::run()` completes under the
//! one-second budget documented in `specdev-next/05-implementation-plan.md`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use specdev_analyzer::{extract, validate};

const N_MODULES: usize = 120;
const BUDGET_MS: u128 = 1000;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("analyzer crate has a parent dir")
        .to_path_buf()
}

fn domain_file() -> PathBuf {
    workspace_root().join("specdev/domains/firmware/domain.toml")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-perf-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write_spec_toml(project: &Path) {
    let toml = r#"spec_version = "1.0"
[project]
name = "perf"
domain = "firmware"
language = "rust"

[source]
root = "src"
layout = "flat"

[paths.overrides]
"#;
    std::fs::write(project.join("spec.toml"), toml).unwrap();
}

fn write_architecture(project: &Path, n: usize) {
    let mut out = String::new();
    // Chain each module's out port to the next module's in port.
    for i in 0..(n - 1) {
        out.push_str(&format!(
            "[[connections]]\nfrom = \"Mod{i}.out\"\nto   = \"Mod{j}.in\"\nflow_type = \"Signal\"\n\n",
            i = i,
            j = i + 1
        ));
    }
    std::fs::write(project.join("specs/architecture.toml"), out).unwrap();
}

fn write_module(project: &Path, i: usize) {
    // All modules sit on the `driver` layer with no inter-module deps, so
    // layer_dependency and acyclic checks pass cheaply. The module carries a
    // small state machine + a handful of actions/ports to exercise per-module
    // graph checks in a representative way.
    let toml = format!(
        r#"name = "Mod{i}"
layer = "driver"
description = "Synthetic module {i} for perf testing."
depends_on = []
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
error_type = "ModError"

[[ports]]
name = "in"
port_type = "SigIn"
direction = "in"
flow_type = "Signal"

[[ports]]
name = "out"
port_type = "SigOut"
direction = "out"
flow_type = "Signal"

[[actions]]
name = "step"
params = []
returns = "Result<(), ModError>"
blocking = false
isr_safe = false
description = "One step of module {i}."

[[state_machines]]
name = "Fsm{i}"
initial = "idle"
states = ["idle", "running", "done"]

[[state_machines.transitions]]
name = "start"
from = "idle"
to = "running"
event = "start_evt"

[[state_machines.transitions]]
name = "finish"
from = "running"
to = "done"
event = "finish_evt"

[[state_machines.transitions]]
name = "reset"
from = "done"
to = "idle"
event = "reset_evt"

[[errors]]
variant = "GenericError"
description = "Unspecified failure."
"#,
    );
    std::fs::write(project.join(format!("specs/modules/Mod{i}.toml")), toml).unwrap();
}

fn build_project(n: usize) -> PathBuf {
    let project = tmp_dir("project");
    std::fs::create_dir_all(project.join("specs/modules")).unwrap();
    std::fs::create_dir_all(project.join("specs/requirements")).unwrap();
    write_spec_toml(&project);
    write_architecture(&project, n);
    for i in 0..n {
        write_module(&project, i);
    }
    project
}

#[test]
fn analyzer_handles_100_plus_modules_under_one_second() {
    let project = build_project(N_MODULES);

    let inputs = validate::resolve_inputs(&project, Some(&domain_file())).expect("resolve inputs");

    let start = Instant::now();
    let report = validate::run(&inputs).expect("validate run");
    let extracted = extract::run(&inputs).expect("extract run");
    let elapsed = start.elapsed();

    // Sanity: we generated N modules and the pipelines actually processed them.
    assert_eq!(
        extracted.modules.len(),
        N_MODULES,
        "extract should see every module"
    );
    // Report must be a real object (findings may be empty or not; we just want
    // to ensure validate executed without panicking).
    let _ = report.findings().len();

    let ms = elapsed.as_millis();
    eprintln!(
        "perf: validate+extract on {n} modules took {ms} ms ({budget} ms budget)",
        n = N_MODULES,
        ms = ms,
        budget = BUDGET_MS
    );
    assert!(
        ms < BUDGET_MS,
        "analyzer exceeded {BUDGET_MS}ms budget on {N_MODULES} modules: {ms}ms"
    );

    let _ = std::fs::remove_dir_all(&project);
}
