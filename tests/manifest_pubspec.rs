//! `manifest` subcommand — per-language dispatch + pubspec.yaml emission.
//!
//! Coverage:
//!   * Dart project: `manifest` emits a pubspec.yaml covering every dep
//!     shape (hosted default / hosted custom URL / git / path / sdk),
//!     `test_only` → dev_dependencies routing, dependency_overrides,
//!     environment resolution (architecture overrides profile default),
//!     and block-contributed `pub:` deps under the user > blocks >
//!     bridges merge order (user version wins, conflict warns).
//!   * Determinism: two runs emit byte-identical output.
//!   * `--write` writes the language-canonical filename in the project root.
//!   * `cargo-toml` on a Dart project errors, pointing at `manifest`.
//!   * Rust project: `manifest` output is byte-identical to `cargo-toml`
//!     (the alias contract).
//!   * Pub workspace: root emits `workspace:` member list; a member
//!     emits `resolution: workspace`.
//!   * A Dart profile.yaml without a `pubspec_yaml` section is a hard
//!     error (manifest-driven, no fallback).

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-manifest-pubspec-{tag}-{}-{}",
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

fn analyzer_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("specdev-analyzer")
}

struct CliResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_cli(project: &Path, args: &[&str]) -> CliResult {
    let out = Command::new(analyzer_bin())
        .args(args)
        .arg("--project")
        .arg(project)
        .output()
        .expect("invoke analyzer");
    CliResult {
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

const DART_PROFILE_YAML: &str = r#"name: test-dart-app
language: dart
pubspec_yaml:
  shape: app
  environment:
    sdk: "^3.6.0"
    flutter: ">=3.27.0"
"#;

const DART_ARCHITECTURE: &str = r#"[environment]
sdk = "^3.7.0"

[[external_deps]]
package = "http"
version = "^1.2.0"

[[external_deps]]
package = "flutter"
sdk = "flutter"

[[external_deps]]
package = "corp_widgets"
hosted = "https://pub.corp.example"
version = "^2.0.0"

[[external_deps]]
package = "fm_dsp"
git = { url = "https://git.example/fm_dsp", ref = "v0.3.0", path = "packages/fm_dsp" }

[[external_deps]]
package = "local_proto"
path = "../local_proto"

[[external_deps]]
package = "flutter_test"
sdk = "flutter"
test_only = true

[[external_deps]]
package = "mocktail"
version = "^1.0.0"
test_only = true

[[dependency_overrides]]
package = "http"
version = "^1.2.1"
"#;

const BLOCK_DEPS_YAML: &str = r#"pub:
  - package: universal_ble
    version: "^2.1.0"
  - package: http
    version: "^9.9.9"
"#;

/// Bootstrap-flip pattern (same as the dart items tests): bootstrap as
/// rust, flip the language, then install the Dart-shaped machinery a
/// Dart-profile bootstrap would have produced.
fn build_dart_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "ble-scan".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let spec_path = target.join("spec.toml");
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    let spec = spec.replace("language = \"rust\"", "language = \"dart\"");
    std::fs::write(&spec_path, spec).unwrap();

    std::fs::write(target.join("specs/specdev/profile.yaml"), DART_PROFILE_YAML).unwrap();
    std::fs::write(target.join("specs/architecture.toml"), DART_ARCHITECTURE).unwrap();

    // Module-level external_deps.
    std::fs::write(
        target.join("specs/modules/Scanner.toml"),
        r#"name = "Scanner"
visibility = "pub"

[[external_deps]]
package = "collection"
version = "^1.19.0"
"#,
    )
    .unwrap();

    // One inlined block contributing pub deps: universal_ble (new) and a
    // conflicting `http` version the user's entry must win over.
    std::fs::write(
        target.join("specs/specdev/schemas/inlined/blocks/bletest.schema.yaml"),
        "# placeholder block schema (discovery reads only the filename)\n",
    )
    .unwrap();
    let block_codegen = target.join("specs/specdev/codegen/dart/blocks/bletest");
    std::fs::create_dir_all(&block_codegen).unwrap();
    std::fs::write(block_codegen.join("deps.yaml"), BLOCK_DEPS_YAML).unwrap();

    target
}

/// #106: an unversioned side is NOT a version conflict.
///
/// `pub_reference.schema.yaml` requires only `package`; version is
/// "Optional alongside git/path/sdk sources", and `PubRef.version` is
/// `Option<String>`. So an architecture-level path dep and a block's
/// versioned entry for the same package is legal spec input.
///
/// The cargo emitter settled this in #99: it only warns when BOTH sides
/// carry a version and they differ, because an unversioned reference gets
/// its version from the path crate's own manifest. `pubspec_yaml.rs`
/// never got that fix and tested `existing.version != r.version`, so
/// `None` vs `Some` read as a conflict and warned on correct input.
#[test]
fn dart_manifest_unversioned_path_dep_is_not_a_version_conflict() {
    let target = build_dart_project("unversioned-merge");

    // The architecture already declares `local_proto` as a bare path dep
    // with no version. Have the block declare the same package WITH one.
    let block_codegen = target.join("specs/specdev/codegen/dart/blocks/bletest");
    std::fs::write(
        block_codegen.join("deps.yaml"),
        "pub:\n  - package: local_proto\n    version: \"^1.2.0\"\n",
    )
    .unwrap();

    let r = run_cli(&target, &["manifest"]);
    assert_eq!(
        r.exit_code,
        Some(0),
        "stderr: {}\nstdout: {}",
        r.stderr,
        r.stdout
    );
    assert!(
        !r.stderr.contains("`local_proto` declared with conflicting versions"),
        "unversioned path dep + versioned block entry must not warn as a \
         version conflict (#106).\nstderr: {}",
        r.stderr
    );
    // The path source still wins, exactly as the cargo side keeps the
    // first-declared source.
    assert!(
        r.stdout.contains("local_proto:") && r.stdout.contains("../local_proto"),
        "first-declared path source must survive the merge.\nstdout: {}",
        r.stdout
    );
    // What IS reported instead: the two entries really do name different
    // sources (a path vs the default hosted registry), and that is the
    // accurate warning for this input. Pinned here so the fix is not read
    // as "silence the merger". NOTE: the cargo emitter stays silent on a
    // source difference, so the two mergers still diverge here — tracked
    // separately; do not "align" them by deleting this warning.
    assert!(
        r.stderr.contains("`local_proto` declared with conflicting sources"),
        "a genuine source difference must still be reported.\nstderr: {}",
        r.stderr
    );
}

#[test]
fn dart_manifest_emits_pubspec_with_all_dep_shapes() {
    let target = build_dart_project("full");
    let r = run_cli(&target, &["manifest"]);
    assert_eq!(
        r.exit_code,
        Some(0),
        "stderr: {}\nstdout: {}",
        r.stderr,
        r.stdout
    );
    // User `http` ^1.2.0 vs block ^9.9.9 → user wins, conflict warns.
    assert!(
        r.stderr.contains("conflicting versions") && r.stderr.contains("`http`"),
        "stderr: {}",
        r.stderr
    );
    insta::assert_snapshot!(r.stdout);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn dart_manifest_is_deterministic_and_write_uses_canonical_filename() {
    let target = build_dart_project("determinism");
    let a = run_cli(&target, &["manifest"]);
    let b = run_cli(&target, &["manifest"]);
    assert_eq!(a.exit_code, Some(0), "stderr: {}", a.stderr);
    assert_eq!(a.stdout, b.stdout, "manifest emission must be byte-stable");

    // Structural gate: the emission must be well-formed YAML with the
    // pubspec top-level shape. (The live `dart pub get` acceptance run
    // needs a Dart SDK and stays out of CI, mirroring the firmware
    // live-build gates.)
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&a.stdout).expect("emitted pubspec must parse as YAML");
    let map = doc.as_mapping().expect("pubspec is a mapping");
    for key in ["name", "environment", "dependencies"] {
        assert!(
            map.contains_key(serde_yaml::Value::String(key.into())),
            "pubspec missing `{key}`:\n{}",
            a.stdout
        );
    }

    let w = run_cli(&target, &["manifest", "--write"]);
    assert_eq!(w.exit_code, Some(0), "stderr: {}", w.stderr);
    let written = std::fs::read_to_string(target.join("pubspec.yaml")).unwrap();
    assert_eq!(written, a.stdout, "--write must write the stdout bytes");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn cargo_toml_alias_refuses_dart_project() {
    let target = build_dart_project("alias-guard");
    let r = run_cli(&target, &["cargo-toml"]);
    assert_eq!(r.exit_code, Some(2), "stdout: {}", r.stdout);
    assert!(
        r.stderr.contains("use `specdev-analyzer manifest`"),
        "stderr: {}",
        r.stderr
    );
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn dart_profile_without_pubspec_section_is_a_hard_error() {
    let target = build_dart_project("no-section");
    std::fs::write(
        target.join("specs/specdev/profile.yaml"),
        "name: test-dart-app\nlanguage: dart\n",
    )
    .unwrap();
    let r = run_cli(&target, &["manifest"]);
    assert_eq!(r.exit_code, Some(1), "stdout: {}", r.stdout);
    assert!(
        r.stderr.contains("no `pubspec_yaml` section"),
        "stderr: {}",
        r.stderr
    );
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn rust_manifest_matches_cargo_toml_alias_byte_for_byte() {
    let target = tmp_dir("rust-alias");
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let via_manifest = run_cli(&target, &["manifest"]);
    let via_alias = run_cli(&target, &["cargo-toml"]);
    assert_eq!(via_manifest.exit_code, Some(0), "stderr: {}", via_manifest.stderr);
    assert_eq!(via_alias.exit_code, Some(0), "stderr: {}", via_alias.stderr);
    assert_eq!(
        via_manifest.stdout, via_alias.stdout,
        "manifest and its cargo-toml alias must render identically on rust"
    );
    assert!(via_manifest.stdout.contains("[package]"));
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn pub_workspace_root_and_member_shapes() {
    let root = tmp_dir("workspace");
    // Hand-written workspace root: spec.toml with [workspace] + the
    // minimal specdev machinery (profile.yaml with a workspace-shaped
    // pubspec_yaml section).
    std::fs::create_dir_all(root.join("specs/specdev")).unwrap();
    std::fs::write(
        root.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "fm-suite"
profile = "test-dart-app"
language = "dart"
description = "FM synth app suite"

[paths]
specs = "specs"
source = "lib"

[workspace]
members = [
  { name = "app", path = "app" },
]
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("specs/specdev/profile.yaml"),
        r#"name: test-dart-app
language: dart
pubspec_yaml:
  shape: workspace
  environment:
    sdk: "^3.6.0"
"#,
    )
    .unwrap();

    let r = run_cli(&root, &["manifest"]);
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    // Pub's documented aggregator-root shape: placeholder name, never
    // published.
    assert!(r.stdout.contains("name: _\n"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("publish_to: none"), "stdout: {}", r.stdout);
    assert!(
        r.stdout.contains("workspace:\n  - \"app\"\n"),
        "stdout: {}",
        r.stdout
    );

    // Member: a full Dart project nested at the member path.
    let member = root.join("app");
    std::fs::create_dir_all(&member).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "app".into(),
            target_dir: member.clone(),
        },
    )
    .unwrap();
    let spec_path = member.join("spec.toml");
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    std::fs::write(
        &spec_path,
        spec.replace("language = \"rust\"", "language = \"dart\""),
    )
    .unwrap();
    std::fs::write(member.join("specs/specdev/profile.yaml"), DART_PROFILE_YAML).unwrap();

    let m = run_cli(&member, &["manifest"]);
    assert_eq!(m.exit_code, Some(0), "stderr: {}", m.stderr);
    assert!(
        m.stdout.contains("resolution: workspace\n"),
        "member pubspec must opt into workspace resolution; stdout: {}",
        m.stdout
    );
    // The member has zero deps — an empty `dependencies:` section would
    // be a YAML null, so it must be omitted entirely.
    assert!(
        !m.stdout.contains("dependencies:"),
        "empty dependency sections must be omitted; stdout: {}",
        m.stdout
    );
    let _ = std::fs::remove_dir_all(&root);
}
