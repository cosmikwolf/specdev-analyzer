//! `[profile.*]` emission — a closed subset of Cargo's profile grammar,
//! root-only in a workspace.
//!
//!   * Workspace-root render: `[profile.<name>]` after
//!     `[workspace.dependencies]`; built-ins in Cargo's order, custom
//!     profiles after them; each package override right after its parent.
//!   * Standalone render: the same tables after `[dependencies]`.
//!   * Member refusal: a member that declares profiles fails `validate`
//!     (`PROFILES-ROOT-ONLY`, naming the root) and its render emits
//!     nothing for them, with a warning.
//!   * Schema: a key outside the subset, `panic` in a package override, a
//!     custom profile without `inherits`, and an out-of-range knob are all
//!     rejected; the well-formed spec validates clean.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
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
        "specdev-cargo-toml-profiles-{tag}-{}-{}",
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

fn bootstrap_at(target: &Path, profile: &str, name: &str) {
    std::fs::create_dir_all(target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.to_path_buf(),
        },
    )
    .unwrap();
}

fn fresh(profile: &str, name: &str, tag: &str) -> PathBuf {
    let target = tmp_workspace(tag).join("proj");
    bootstrap_at(&target, profile, name);
    target
}

fn discover(target: &Path) -> LayoutContext {
    LayoutContext::discover(target, None).unwrap()
}

fn append_arch(target: &Path, extra: &str) {
    let p = target.join("specs/architecture.toml");
    let arch = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{arch}\n{extra}")).unwrap();
}

fn error_count(target: &Path) -> (usize, String) {
    let report = validate::run_project(target, None).unwrap();
    let n = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .count();
    (n, report.render_plain())
}

const WS_SPEC: &str = r#"spec_version = "1.0"

[project]
name = "demo-ws"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]
resolver = "2"

[[workspace.members]]
name = "alpha"
path = "crates/alpha"
"#;

const WS_ARCH: &str = r#"[workspace_package]
version = "0.1.0"
edition = "2021"

[[workspace_dependencies]]
crate = "defmt"
version = "0.3"
"#;

// Spec order is deliberately scrambled: test before release, overrides in
// reverse lex order, knobs in reverse order. The render must not care.
const PROFILES: &str = r#"[profiles.test]
debug = 2
opt-level = 1

[profiles.test.package.zeta_util]
opt-level = 2

[profiles.test.package.alpha-shared]
opt-level = 2

[profiles.probe]
panic = "abort"
lto = "fat"
inherits = "release"

[profiles.release]
debug = 2
opt-level = "s"
"#;

const RENDERED_PROFILES: &str = r#"
[profile.release]
opt-level = "s"
debug = 2

[profile.test]
opt-level = 1
debug = 2

[profile.test.package.alpha-shared]
opt-level = 2

[profile.test.package.zeta_util]
opt-level = 2

[profile.probe]
inherits = "release"
lto = "fat"
panic = "abort"
"#;

// ---------------------------------------------------------------------------
// 1 — Workspace root: profiles render after [workspace.dependencies] in
//     Cargo's order with fixed key order and lex-sorted overrides; the
//     spec validates clean.
// ---------------------------------------------------------------------------
#[test]
fn workspace_root_renders_profiles_in_cargo_order_with_overrides() {
    let target = fresh("rust-workspace", "demo-ws", "ws-profiles");
    std::fs::write(target.join("spec.toml"), WS_SPEC).unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        format!("{WS_ARCH}\n{PROFILES}"),
    )
    .unwrap();

    let r = render(&discover(&target)).unwrap();
    assert!(
        r.text.ends_with(RENDERED_PROFILES),
        "profiles after [workspace.dependencies], Cargo order, fixed keys:\n{}",
        r.text
    );
    let deps = r
        .text
        .find("[workspace.dependencies]\ndefmt = \"0.3\"\n")
        .unwrap();
    assert!(deps < r.text.find("[profile.release]").unwrap());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    let (n, plain) = error_count(&target);
    assert_eq!(n, 0, "expected 0 errors:\n{plain}");
}

// ---------------------------------------------------------------------------
// 2 — No profiles declared → no [profile.*] anywhere (root and standalone).
// ---------------------------------------------------------------------------
#[test]
fn no_profiles_declared_emits_no_profile_tables() {
    let target = fresh("rust-workspace", "demo-ws", "ws-no-profiles");
    std::fs::write(target.join("spec.toml"), WS_SPEC).unwrap();
    std::fs::write(target.join("specs/architecture.toml"), WS_ARCH).unwrap();
    let t = render(&discover(&target)).unwrap().text;
    assert!(!t.contains("[profile"), "{t}");

    let fw = fresh("firmware-layered", "demo", "fw-no-profiles");
    let t = render(&discover(&fw)).unwrap().text;
    assert!(!t.contains("[profile"), "{t}");
}

// ---------------------------------------------------------------------------
// 3 — Standalone crate: its own profiles render after [dependencies].
// ---------------------------------------------------------------------------
#[test]
fn standalone_firmware_renders_profiles_after_dependencies() {
    let target = fresh("firmware-layered", "demo", "fw-profiles");
    append_arch(
        &target,
        "[profiles.release]\ndebug = 2\nopt-level = \"s\"\n",
    );

    let r = render(&discover(&target)).unwrap();
    assert!(
        r.text
            .ends_with("\n[profile.release]\nopt-level = \"s\"\ndebug = 2\n"),
        "{}",
        r.text
    );
    assert!(r.text.find("[dependencies]").unwrap() < r.text.find("[profile.release]").unwrap());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    let (n, plain) = error_count(&target);
    assert_eq!(n, 0, "a standalone crate may declare profiles:\n{plain}");
}

// ---------------------------------------------------------------------------
// 4 — Member refusal, both paths: validate names the root; render warns
//     and emits nothing for the profiles. Removing them clears both.
// ---------------------------------------------------------------------------
#[test]
fn member_profiles_are_refused_by_validate_and_render() {
    let root = fresh("rust-workspace", "demo-ws", "member-refusal");
    std::fs::write(root.join("spec.toml"), WS_SPEC).unwrap();
    std::fs::write(root.join("specs/architecture.toml"), WS_ARCH).unwrap();
    let member = root.join("crates/alpha");
    bootstrap_at(&member, "library-api", "alpha");
    let member_arch = member.join("specs/architecture.toml");
    let clean = std::fs::read_to_string(&member_arch).unwrap();
    std::fs::write(
        &member_arch,
        format!("{clean}\n[profiles.release]\ndebug = 2\n"),
    )
    .unwrap();

    let report = validate::run_project(&member, None).unwrap();
    let hit = report
        .findings()
        .iter()
        .find(|f| f.rule == "PROFILES-ROOT-ONLY");
    let hit =
        hit.unwrap_or_else(|| panic!("expected PROFILES-ROOT-ONLY:\n{}", report.render_plain()));
    assert!(matches!(hit.severity, Severity::Error));
    assert!(
        hit.message
            .contains(&root.join("spec.toml").display().to_string()),
        "the finding names the workspace root:\n{}",
        hit.message
    );

    let r = render(&discover(&member)).unwrap();
    assert!(
        !r.text.contains("[profile"),
        "member render must not carry profiles:\n{}",
        r.text
    );
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("member of the workspace") && w.contains("spec.toml")),
        "member render warns and names the root:\n{:?}",
        r.warnings
    );

    // Negative control: without the table, neither path complains.
    std::fs::write(&member_arch, clean).unwrap();
    let report = validate::run_project(&member, None).unwrap();
    assert!(report
        .findings()
        .iter()
        .all(|f| f.rule != "PROFILES-ROOT-ONLY"));
    let r = render(&discover(&member)).unwrap();
    assert!(
        r.warnings.iter().all(|w| !w.contains("profiles")),
        "{:?}",
        r.warnings
    );
}

// ---------------------------------------------------------------------------
// 5 — The subset is closed: each of these is a SCHEMA-ARCH error naming
//     the offending key; the well-formed set is the positive control.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_keys_outside_the_subset() {
    let cases: [(&str, &str); 5] = [
        (
            "[profiles.release]\nrustflags = [\"-C\", \"link-arg=-Tlink.x\"]\n",
            "rustflags",
        ),
        (
            "[profiles.release.package.serde]\npanic = \"abort\"\n",
            "panic",
        ),
        ("[profiles.probe]\nopt-level = 1\n", "inherits"),
        ("[profiles.release]\nopt-level = 4\n", "opt-level"),
        ("[profiles.Release]\ndebug = 2\n", "Release"),
    ];
    for (i, (bad, needle)) in cases.iter().enumerate() {
        let target = fresh("firmware-layered", "demo", &format!("fw-profiles-bad-{i}"));
        append_arch(&target, bad);
        let report = validate::run_project(&target, None).unwrap();
        let plain = report.render_plain();
        assert!(
            report
                .findings()
                .iter()
                .any(|f| f.rule == "SCHEMA-ARCH" && matches!(f.severity, Severity::Error)),
            "case {i} ({needle}) must be a SCHEMA-ARCH error:\n{plain}"
        );
        assert!(
            plain.contains(needle),
            "case {i}: report names `{needle}`:\n{plain}"
        );
    }

    let target = fresh("firmware-layered", "demo", "fw-profiles-good");
    append_arch(&target, PROFILES);
    let (n, plain) = error_count(&target);
    assert_eq!(n, 0, "the full subset validates clean:\n{plain}");
}
