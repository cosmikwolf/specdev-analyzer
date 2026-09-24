//! `report graph` renders the module graph (#203a).
//!
//! The grouping and the flagged edges come from the project's own
//! `layer_dependency` graph check, so the load-bearing assertion is
//! agreement: the report flags exactly the edges `validate` reports.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn project(profile: &str, tag: &str) -> PathBuf {
    let target = std::env::temp_dir()
        .join(format!("specdev-report-graph-{tag}-{}", std::process::id()))
        .join("proj");
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    std::fs::create_dir_all(target.join("specs/modules")).unwrap();
    target
}

fn module(target: &Path, name: &str, layer: Option<&str>, deps: &[&str]) {
    let mut s = format!("name = \"{name}\"\n");
    if let Some(l) = layer {
        s.push_str(&format!("layer = \"{l}\"\n"));
    }
    let deps: Vec<String> = deps.iter().map(|d| format!("\"{d}\"")).collect();
    s.push_str(&format!("depends_on = [{}]\n", deps.join(", ")));
    std::fs::write(target.join(format!("specs/modules/{name}.toml")), s).unwrap();
}

fn run(target: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .args(args)
        .arg("--project")
        .arg(target)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// firmware-layered orders application → middleware → driver → HAL → PAC.
fn layered_project(tag: &str) -> PathBuf {
    let t = project("firmware-layered", tag);
    module(&t, "App", Some("application"), &["Sensor"]);
    module(&t, "Sensor", Some("driver"), &["Hal"]);
    module(&t, "Hal", Some("HAL"), &[]);
    // The induced violation: a HAL module depending upward on a driver.
    module(&t, "Clock", Some("HAL"), &["Sensor"]);
    t
}

#[test]
fn groups_by_the_declared_ordering_and_flags_what_validate_flags() {
    let t = layered_project("layered");
    let (code, out, err) = run(&t, &["report", "graph"]);
    assert_eq!(code, 0, "{err}");

    // Groups appear in the declared order, empty layers omitted.
    let app = out.find("subgraph g0[\"application\"]").expect(&out);
    let drv = out.find("[\"driver\"]").expect(&out);
    let hal = out.find("[\"HAL\"]").expect(&out);
    assert!(app < drv && drv < hal, "{out}");
    assert!(!out.contains("middleware"), "{out}");
    assert!(out.contains("grouped by `attrs.layer` in the order of graph check `layer_dependency`"));

    // Exactly one flagged edge, and it is Clock -> Sensor.
    assert_eq!(out.matches("==>|upward|").count(), 1, "{out}");
    assert!(err.contains("4 module(s), 3 edge(s), 1 flagged"), "{err}");
    let clock = out.lines().find(|l| l.contains("[\"Clock<br/>")).unwrap();
    let sensor = out.lines().find(|l| l.contains("[\"Sensor<br/>")).unwrap();
    let id = |l: &str| l.trim().split('[').next().unwrap().to_string();
    assert!(
        out.contains(&format!("{} ==>|upward| {}", id(clock), id(sensor))),
        "{out}"
    );
    // The groups are chained so they stack in the declared order.
    assert!(out.contains("  g0 ~~~ g1 ~~~ g2\n"), "{out}");
    // `linkStyle` indexes the written links; it must point at the upward one.
    let links: Vec<&str> = out
        .lines()
        .filter(|l| l.contains(" --> ") || l.contains(" ==>") || l.contains(" ~~~ "))
        .collect();
    let styled: usize = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("linkStyle "))
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect(&out);
    assert!(links[styled].contains("==>|upward|"), "{out}");
    // Degree labels: Sensor is used by App and Clock, uses Hal.
    assert!(sensor.contains("uses 1 · used by 2"), "{sensor}");

    // Agreement with the check itself.
    let (_, vout, verr) = run(&t, &["validate", "--no-color"]);
    let text = format!("{vout}{verr}");
    let flagged: Vec<_> = text
        .lines()
        .filter(|l| l.contains("[layer_dependency]") && l.contains("upward dependency"))
        .collect();
    assert_eq!(flagged.len(), 1, "{text}");
    assert!(
        flagged[0].contains("`Clock`") && flagged[0].contains("`Sensor`"),
        "{text}"
    );
}

#[test]
fn output_is_deterministic_and_dot_matches() {
    let t = layered_project("determinism");
    let (_, a, _) = run(&t, &["report", "graph"]);
    let (_, b, _) = run(&t, &["report", "graph"]);
    assert_eq!(a, b);
    let (code, dot, err) = run(&t, &["report", "graph", "--format", "dot"]);
    assert_eq!(code, 0, "{err}");
    assert!(dot.starts_with("// generated by"), "{dot}");
    assert!(dot.contains("label=\"application\";"), "{dot}");
    assert!(dot.contains("\"Clock\" -> \"Sensor\" [color=red"), "{dot}");
    assert!(dot.contains("\"App\" -> \"Sensor\";"), "{dot}");
}

#[test]
fn a_node_without_a_layer_gets_its_own_group() {
    let t = layered_project("nolayer");
    module(&t, "Loose", None, &["Hal"]);
    let (_, out, _) = run(&t, &["report", "graph"]);
    assert!(out.contains("[\"(no layer)\"]"), "{out}");
    let ungrouped = out.find("(no layer)").unwrap();
    assert!(
        out.find("[\"HAL\"]").unwrap() < ungrouped,
        "(no layer) comes last\n{out}"
    );
}

#[test]
fn a_profile_without_a_layer_check_renders_ungrouped() {
    let t = project("library-api", "flat");
    module(&t, "A", None, &["B"]);
    module(&t, "B", None, &[]);
    let (code, out, err) = run(&t, &["report", "graph"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("no `layer_dependency` graph check declared: not grouped"),
        "{out}"
    );
    assert!(!out.contains("subgraph"), "{out}");
    assert!(out.contains(" --> "), "{out}");
}

#[test]
fn bad_arguments_exit_2() {
    let t = layered_project("badargs");
    let (code, _, err) = run(&t, &["report", "graph", "--format", "png"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("use `mermaid` or `dot`"), "{err}");
    let (code, _, err) = run(&t, &["report", "graph", "--check", "nope"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--check `nope`"), "{err}");
}
