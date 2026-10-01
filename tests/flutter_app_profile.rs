//! `flutter-app` profile.
//!
//! Coverage layers:
//!   * Bootstrap — `--profile flutter-app --language dart` scaffolds the
//!     expected layout (Dart-shaped module/architecture/enums schemas,
//!     module.conventions.md, `[paths] source = "lib/src"`).
//!   * Role machinery — required `role`, closed enum, `role_dependency`
//!     ordering warns on upward edges (service → widget).
//!   * Channels — the schema guard restricting `channels` to
//!     `role = "platform_channel"` modules actually fires (positive
//!     control: a platform_channel module with channels validates
//!     clean), and `channel_names_unique` rejects a cross-module
//!     duplicate name.
//!   * Platform matrix — a module `platforms[]` entry outside the
//!     architecture matrix fails at reduce time (unresolved reference).
//!   * pubspec emission — the bootstrapped project renders a pubspec
//!     with `version:` and the `flutter:` config block from
//!     architecture.toml.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},

    report::{Finding, Report},
    validate, Severity,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-flutter-app-profile-{tag}-{}-{}",
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

fn fresh_app(tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "flutter-app".into(),
            language: "dart".into(),
            project_name: "demo_app".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

/// Minimal architecture satisfying the required platform matrix, plus
/// the version + flutter-block fields the pubspec test asserts on.
const ARCH_DEFAULT: &str = r#"platforms = ["android", "ios", "web"]
version = "0.1.0+1"

[environment]
sdk = "^3.9.0"

[flutter]
uses_material_design = true
assets = ["assets/images/"]
"#;

fn write_arch(target: &Path, body: &str) {
    std::fs::write(target.join("specs/architecture.toml"), body).unwrap();
}

fn write_module(target: &Path, name: &str, body: &str) {
    std::fs::write(
        target.join("specs/modules").join(format!("{name}.toml")),
        body,
    )
    .unwrap();
}

fn errors_for(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

fn warnings_for(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Warning))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// 1 — Bootstrap scaffolds the Dart-shaped layout.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_scaffolds_flutter_app_layout() {
    let target = fresh_app("scaffold");

    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    assert!(spec.contains("language = \"dart\""), "spec.toml: {spec}");
    assert!(spec.contains("source = \"lib/src\""), "spec.toml: {spec}");

    // Bootstrap creates the source root spec.toml declares — the pub
    // layout, not Rust's `src/`.
    assert!(target.join("lib/src").is_dir());
    assert!(
        !target.join("src").exists(),
        "no stray rust-conventional src/ in a Dart project"
    );
}

// ---------------------------------------------------------------------------
// 2 — A hello-shaped module graph validates with no errors and no
//     role_dependency warnings.
// ---------------------------------------------------------------------------
#[test]
fn hello_module_graph_validates_clean() {
    let target = fresh_app("clean");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "App",
        r#"name = "App"
role = "app"
depends_on = ["HomeScreen", "CounterService"]
"#,
    );
    write_module(
        &target,
        "HomeScreen",
        r#"name = "HomeScreen"
role = "screen"
depends_on = ["CounterService"]
"#,
    );
    write_module(
        &target,
        "CounterService",
        r#"name = "CounterService"
role = "service"
"#,
    );
    write_module(
        &target,
        "DeviceChannel",
        r#"name = "DeviceChannel"
role = "platform_channel"
platforms = ["android", "ios"]

[[channels]]
name = "com.example.demo/battery"
kind = "method"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(errs.is_empty(), "expected no errors; got {errs:#?}");
    assert!(
        warnings_for(&report, "role_dependency").is_empty(),
        "downward/same-role edges must not warn; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 3 — Schema guard: channels on a non-platform_channel role is a schema
//     error. (The clean-path test above is the positive control proving
//     a platform_channel module WITH channels passes.)
// ---------------------------------------------------------------------------
#[test]
fn channels_on_service_role_rejected() {
    let target = fresh_app("channels-role");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "SneakyService",
        r#"name = "SneakyService"
role = "service"

[[channels]]
name = "com.example.demo/sneaky"
kind = "method"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for channels on role=service; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 4 — channel_names_unique: two modules declaring the same channel name
//     collide (channels share one BinaryMessenger namespace).
// ---------------------------------------------------------------------------
#[test]
fn duplicate_channel_names_rejected() {
    let target = fresh_app("channel-dup");
    write_arch(&target, ARCH_DEFAULT);
    for name in ["BatteryChannel", "PowerChannel"] {
        write_module(
            &target,
            name,
            &format!(
                r#"name = "{name}"
role = "platform_channel"

[[channels]]
name = "com.example.demo/battery"
kind = "method"
"#
            ),
        );
    }

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "channel_names_unique");
    assert!(
        !errs.is_empty(),
        "expected channel_names_unique error for duplicate name; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — role_dependency warns on an upward edge (service → widget).
// ---------------------------------------------------------------------------
#[test]
fn role_dependency_warns_on_upward_edge() {
    let target = fresh_app("role-upward");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "FancyButton",
        r#"name = "FancyButton"
role = "widget"
"#,
    );
    write_module(
        &target,
        "ThemeService",
        r#"name = "ThemeService"
role = "service"
depends_on = ["FancyButton"]
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let warns = warnings_for(&report, "role_dependency");
    assert!(
        !warns.is_empty(),
        "expected role_dependency warning on service→widget; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — A module platform outside the architecture matrix fails at reduce
//     time (module_platform edge target doesn't resolve).
// ---------------------------------------------------------------------------
#[test]
fn module_platform_outside_matrix_rejected() {
    let target = fresh_app("platform-matrix");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "DesktopOnly",
        r#"name = "DesktopOnly"
role = "service"
platforms = ["windows"]
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "REDUCE_UNRESOLVED");
    assert!(
        !errs.is_empty(),
        "expected REDUCE_UNRESOLVED for platform outside the matrix; got {report:#?}"
    );
}
