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
