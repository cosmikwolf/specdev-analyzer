//! `REF-PIN-001` / `REF-PIN-002` — a reference crate must not compile against
//! versions its users never receive.
//!
//! Hermetic counterpart to `plugin_lint::live_plugin_lint_passes` (which only
//! proves the live tree is currently clean). These build synthetic units so the
//! checks are proven to actually fire, and — just as important — proven NOT to
//! fire on the shapes that are legitimately asymmetric: a reference pinning
//! crates the unit does not contribute (every bridge reference does this), and a
//! unit contributing crates the reference has no worked example for.
//!
//! Unit names are deliberately implausible as future real units, per the naming
//! rule that a fictional name in a test becomes an agnosticism-lint violation
//! the day a real unit claims it.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::reference_lint::lint_reference_crates;

fn tmp_tree(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-ref-pin-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// Write a unit with a `codegen/rust/deps.yaml` and a `reference/rust/Cargo.toml`.
fn write_unit(root: &Path, kind: &str, name: &str, deps_yaml: &str, cargo_toml: &str) {
    let unit = root.join(kind).join(name);
    fs::create_dir_all(unit.join("codegen/rust")).unwrap();
    fs::create_dir_all(unit.join("reference/rust")).unwrap();
    fs::write(unit.join("codegen/rust/deps.yaml"), deps_yaml).unwrap();
    fs::write(unit.join("reference/rust/Cargo.toml"), cargo_toml).unwrap();
}

fn findings_for(root: &Path) -> Vec<String> {
    lint_reference_crates(root).expect("lint_reference_crates")
}

const PKG: &str = "[package]\nname = \"r\"\nversion = \"0.0.0\"\n\n";

#[test]
fn version_disagreement_is_flagged() {
    let root = tmp_tree("ver");
    write_unit(
        &root,
        "blocks",
        "some-registry-block",
        r#"crates:
  - name: some_dep
    version: "0.27"
    source: crates-io
"#,
        &format!("{PKG}[dependencies]\nsome_dep = \"0.36\"\n"),
    );
    let f = findings_for(&root);
    assert_eq!(f.len(), 1, "expected exactly one finding, got: {f:#?}");
    assert!(f[0].contains("REF-PIN-001"), "{}", f[0]);
    assert!(f[0].contains("0.27") && f[0].contains("0.36"), "{}", f[0]);
}

#[test]
fn matching_versions_are_clean_in_both_toml_forms() {
    let root = tmp_tree("match");
    let deps = r#"crates:
  - name: some_dep
    version: "1"
    source: crates-io
"#;
    // Shorthand form.
    write_unit(
        &root,
        "blocks",
        "some-shorthand-block",
        deps,
        &format!("{PKG}[dependencies]\nsome_dep = \"1\"\n"),
    );
    // Table form with features.
    write_unit(
        &root,
        "blocks",
        "some-table-block",
        deps,
        &format!("{PKG}[dependencies]\nsome_dep = {{ version = \"1\", features = [\"x\"] }}\n"),
    );
    let f = findings_for(&root);
    assert!(f.is_empty(), "{f:#?}");
}

#[test]
fn unpinned_git_dep_against_a_pinned_reference_is_flagged() {
    let root = tmp_tree("git");
    write_unit(
        &root,
        "blocks",
        "some-git-block",
        r#"crates:
  - name: some_dep
    version: "0.0"
    source:
      git: "https://example.invalid/some.git"
"#,
        &format!(
            "{PKG}[dependencies]\n\
             some_dep = {{ git = \"https://example.invalid/some.git\", rev = \"abc123\" }}\n"
        ),
    );
    let f = findings_for(&root);
    assert_eq!(f.len(), 1, "expected exactly one finding, got: {f:#?}");
    assert!(f[0].contains("REF-PIN-002"), "{}", f[0]);
    assert!(f[0].contains("abc123"), "{}", f[0]);
}

#[test]
fn git_rev_mismatch_is_flagged_as_a_pin_disagreement() {
    let root = tmp_tree("gitrev");
    write_unit(
        &root,
        "blocks",
        "some-gitrev-block",
        r#"crates:
  - name: some_dep
    version: "0.0"
    source:
      git: "https://example.invalid/some.git"
      rev: "aaa111"
"#,
        &format!(
            "{PKG}[dependencies]\n\
             some_dep = {{ git = \"https://example.invalid/some.git\", rev = \"bbb222\" }}\n"
        ),
    );
    let f = findings_for(&root);
    assert_eq!(f.len(), 1, "expected exactly one finding, got: {f:#?}");
    assert!(f[0].contains("REF-PIN-001"), "{}", f[0]);
    assert!(f[0].contains("aaa111") && f[0].contains("bbb222"), "{}", f[0]);
}

#[test]
fn matching_git_rev_is_clean() {
    let root = tmp_tree("gitok");
    write_unit(
        &root,
        "blocks",
        "some-pinned-block",
        r#"crates:
  - name: some_dep
    version: "0.0"
    source:
      git: "https://example.invalid/some.git"
      rev: "abc123"
"#,
        &format!(
            "{PKG}[dependencies]\n\
             some_dep = {{ git = \"https://example.invalid/some.git\", rev = \"abc123\" }}\n"
        ),
    );
    let f = findings_for(&root);
    assert!(f.is_empty(), "{f:#?}");
}

/// A bridge's reference legitimately pins its endpoint blocks' crates, which the
/// bridge itself does not contribute. Those must never flag — the check compares
/// the intersection, not the two sets.
#[test]
fn reference_only_crates_do_not_flag() {
    let root = tmp_tree("extra");
    write_unit(
        &root,
        "bridges",
        "some-a+some-b",
        r#"crates:
  - name: join_product
    version: "0.6"
    source: crates-io
"#,
        &format!(
            "{PKG}[dependencies]\n\
             join_product = \"0.6\"\n\
             endpoint_block_crate = \"0.10\"\n\
             another_endpoint_crate = \"0.8\"\n"
        ),
    );
    let f = findings_for(&root);
    assert!(f.is_empty(), "{f:#?}");
}

/// The inverse asymmetry: a unit may contribute a crate the reference has no
/// worked example for. Silence, not a finding — coverage is REF-COVERAGE-001's
/// job and is scoped to declared cases.
#[test]
fn deps_only_crates_do_not_flag() {
    let root = tmp_tree("depsonly");
    write_unit(
        &root,
        "blocks",
        "some-partial-block",
        r#"crates:
  - name: exercised
    version: "1"
    source: crates-io
  - name: not_exercised
    version: "2"
    source: crates-io
"#,
        &format!("{PKG}[dependencies]\nexercised = \"1\"\n"),
    );
    let f = findings_for(&root);
    assert!(f.is_empty(), "{f:#?}");
}

/// Target-specific dependency tables are read — an embedded reference pins its
/// HAL under `[target.'cfg(..)'.dependencies]`, and a mismatch there is exactly
/// as damaging as one in the plain table.
#[test]
fn target_specific_dependency_tables_are_compared() {
    let root = tmp_tree("target");
    write_unit(
        &root,
        "blocks",
        "some-target-block",
        r#"crates:
  - name: some_hal
    version: "0.6"
    source: crates-io
"#,
        &format!(
            "{PKG}[target.'cfg(target_arch = \"arm\")'.dependencies]\nsome_hal = \"0.7\"\n"
        ),
    );
    let f = findings_for(&root);
    assert_eq!(f.len(), 1, "expected exactly one finding, got: {f:#?}");
    assert!(f[0].contains("REF-PIN-001"), "{}", f[0]);
}

/// A unit with no reference is out of scope entirely — the checks keep a
/// reference honest, they do not require one to exist.
#[test]
fn unit_without_a_reference_is_skipped() {
    let root = tmp_tree("noref");
    let unit = root.join("blocks").join("some-referenceless-block");
    fs::create_dir_all(unit.join("codegen/rust")).unwrap();
    fs::write(
        unit.join("codegen/rust/deps.yaml"),
        "crates:\n  - name: some_dep\n    version: \"1\"\n    source: crates-io\n",
    )
    .unwrap();
    let f = findings_for(&root);
    assert!(f.is_empty(), "{f:#?}");
}
