//! `manifest` subcommand — package.json emission for TypeScript projects.
//!
//! Coverage:
//!   * TypeScript project: `manifest` emits a package.json covering every
//!     dep shape (registry / git / path→`file:` / link→`link:` /
//!     `workspace:`), `test_only` → devDependencies routing, header
//!     resolution (architecture `[environment]` and `[scripts]` override
//!     the profile defaults key by key), and block-contributed `npm:`
//!     deps under the user > blocks > bridges merge order.
//!   * Determinism: two runs emit byte-identical output; the output parses
//!     as JSON and keeps the fixed top-level key order.
//!   * `--write` writes the language-canonical filename in the project root.
//!   * `cargo-toml` on a TypeScript project errors, pointing at `manifest`.
//!   * A TypeScript profile.yaml without a `package_json` section is a hard
//!     error (manifest-driven, no fallback).
//!   * Workspace root: `--write` emits package.json AND the gen-owned
//!     sibling `pnpm-workspace.yaml`; a second write is byte-identical;
//!     an EDITED, unrecorded sibling is refused (#148) and `--accept-edits`
//!     restores it;
//!     `--output` skips the sibling with a note. A member renders a plain
//!     package.json.
//!   * Live (when `pnpm` is on PATH): `pnpm install` resolves the emitted
//!     manifest, and re-running `manifest` afterwards is byte-identical
//!     to the file on disk (no rewrite trap); a workspace root resolves a
//!     `workspace:*` member.

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
        "specdev-manifest-package-json-{tag}-{}-{}",
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

/// `pnpm --version` succeeds → the live gates run. Tests that need it
/// return early (with a note) otherwise — the CI matrix has no Node.
fn pnpm_available() -> bool {
    // Run from the temp dir: pnpm reads `packageManager` from the nearest
    // package.json and refuses when it names another manager.
    Command::new("pnpm")
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pnpm_install(dir: &Path) -> std::process::Output {
    Command::new("pnpm")
        .args(["install", "--silent"])
        .current_dir(dir)
        .output()
        .expect("invoke pnpm")
}

const TS_PROFILE_YAML: &str = r#"name: test-ts-app
language: typescript
package_json:
  shape: app
  package_manager: "pnpm@10.15.0"
  engines:
    node: ">=22.18.0"
  type: module
  scripts:
    check: "tsc --noEmit"
    test: "vitest run"
"#;

const TS_ARCHITECTURE: &str = r#"version = "0.1.0"

[environment]
node = ">=22.12.0"

[scripts]
test = "vitest run --passWithNoTests"
dev = "next dev"

[[external_deps]]
package = "next"
version = "^16.3.0"

[[external_deps]]
package = "@scope/widgets"
version = "^2.0.0"

[[external_deps]]
package = "from_git"
git = { url = "git+https://git.example/from_git.git", ref = "v1.4.0" }

[[external_deps]]
package = "local_sibling"
path = "../local_sibling"

[[external_deps]]
package = "linked_tool"
link = "../linked_tool"

[[external_deps]]
package = "shared"
workspace = "*"

[[external_deps]]
package = "typescript"
version = "^5.9.0"
test_only = true

[[external_deps]]
package = "vitest"
version = "^3.2.0"
test_only = true
"#;

const BLOCK_DEPS_YAML: &str = r#"npm:
  - package: "@prisma/client"
    version: "^7.10.0"
  - package: next
    version: "^9.9.9"
"#;

/// Bootstrap-flip pattern (same as the Dart manifest tests): bootstrap as
/// rust, flip the language, then install the TypeScript-shaped machinery a
/// TypeScript-profile bootstrap would have produced.
fn flip_to_typescript(target: &Path, profile_yaml: &str) {
    let spec_path = target.join("spec.toml");
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    let spec = spec.replace(
        "language = \"rust\"",
        "language = \"typescript\"\ndescription = \"A spec-driven TypeScript package.\"",
    );
    std::fs::write(&spec_path, spec).unwrap();
    std::fs::write(target.join("specs/specdev/profile.yaml"), profile_yaml).unwrap();
}

fn bootstrap_rust(target: &Path, name: &str) {
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.to_path_buf(),
        },
    )
    .unwrap();
}

fn build_ts_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
    bootstrap_rust(&target, "ble-scan");
    flip_to_typescript(&target, TS_PROFILE_YAML);
    std::fs::write(target.join("specs/architecture.toml"), TS_ARCHITECTURE).unwrap();

    // Module-level external_deps.
    std::fs::write(
        target.join("specs/modules/Scanner.toml"),
        r#"name = "Scanner"
visibility = "pub"

[[external_deps]]
package = "zod"
version = "^4.0.0"
"#,
    )
    .unwrap();

    // One inlined block contributing npm deps: @prisma/client (new) and a
    // conflicting `next` version the user's entry must win over.
    std::fs::write(
        target.join("specs/specdev/schemas/inlined/blocks/ormtest.schema.yaml"),
        "# placeholder block schema (discovery reads only the filename)\n",
    )
    .unwrap();
    let block_codegen = target.join("specs/specdev/codegen/typescript/blocks/ormtest");
    std::fs::create_dir_all(&block_codegen).unwrap();
    std::fs::write(block_codegen.join("deps.yaml"), BLOCK_DEPS_YAML).unwrap();

    target
}

#[test]
fn typescript_manifest_emits_package_json_with_all_dep_shapes() {
    let target = build_ts_project("full");
    let r = run_cli(&target, &["manifest"]);
    assert_eq!(
        r.exit_code,
        Some(0),
        "stderr: {}\nstdout: {}",
        r.stderr,
        r.stdout
    );
    // User `next` ^16.3.0 vs block ^9.9.9 → user wins, conflict warns.
    assert!(
        r.stderr.contains("conflicting versions") && r.stderr.contains("`next`"),
        "stderr: {}",
        r.stderr
    );
    insta::assert_snapshot!(r.stdout);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn typescript_manifest_is_deterministic_json_with_fixed_key_order_and_write_uses_canonical_filename(
) {
    let target = build_ts_project("determinism");
    let a = run_cli(&target, &["manifest"]);
    let b = run_cli(&target, &["manifest"]);
    assert_eq!(a.exit_code, Some(0), "stderr: {}", a.stderr);
    assert_eq!(a.stdout, b.stdout, "manifest emission must be byte-stable");

    // Structural gate: well-formed JSON with the fixed top-level key order
    // (the live `pnpm install` acceptance runs in the gated test below).
    let doc: serde_json::Value =
        serde_json::from_str(&a.stdout).expect("emitted package.json must parse as JSON");
    assert!(doc.is_object(), "package.json is an object");
    let order: Vec<&str> = a
        .stdout
        .lines()
        .filter(|l| l.starts_with("  \"") && l.contains("\": "))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        order,
        vec![
            "name",
            "version",
            "description",
            "private",
            "packageManager",
            "type",
            "engines",
            "scripts",
            "dependencies",
            "devDependencies",
        ],
        "top-level key order is fixed:\n{}",
        a.stdout
    );
    assert!(a.stdout.ends_with("}\n"), "trailing newline");

    let w = run_cli(&target, &["manifest", "--write"]);
    assert_eq!(w.exit_code, Some(0), "stderr: {}", w.stderr);
    let written = std::fs::read_to_string(target.join("package.json")).unwrap();
    assert_eq!(written, a.stdout, "--write must write the stdout bytes");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn cargo_toml_alias_refuses_typescript_project() {
    let target = build_ts_project("alias-guard");
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
fn typescript_profile_without_package_json_section_is_a_hard_error() {
    let target = build_ts_project("no-section");
    std::fs::write(
        target.join("specs/specdev/profile.yaml"),
        "name: test-ts-app\nlanguage: typescript\n",
    )
    .unwrap();
    let r = run_cli(&target, &["manifest"]);
    assert_eq!(r.exit_code, Some(1), "stdout: {}", r.stdout);
    assert!(
        r.stderr.contains("no `package_json` section"),
        "stderr: {}",
        r.stderr
    );
    let _ = std::fs::remove_dir_all(&target);
}

/// A workspace root: spec.toml with `[workspace]` + a workspace-shaped
/// `package_json` section. Returns the root; the `app` member is
/// bootstrapped by the caller when needed.
fn build_ts_workspace(tag: &str) -> PathBuf {
    let root = tmp_dir(tag);
    std::fs::create_dir_all(root.join("specs/specdev")).unwrap();
    std::fs::write(
        root.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "erp-suite"
profile = "test-ts-app"
language = "typescript"
description = "ERP prototype suite"

[paths]
specs = "specs"
source = "src"

[workspace]
members = [
  { name = "app", path = "app" },
  { name = "shared", path = "packages/shared" },
]
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("specs/specdev/profile.yaml"),
        r#"name: test-ts-app
language: typescript
package_json:
  shape: workspace
  package_manager: "pnpm@10.15.0"
  engines:
    node: ">=22.18.0"
  scripts:
    check: "pnpm -r run check"
"#,
    )
    .unwrap();
    // Root-level tooling dep (routes to devDependencies).
    std::fs::write(
        root.join("specs/architecture.toml"),
        r#"[[external_deps]]
package = "prettier"
version = "^3.6.0"
test_only = true
"#,
    )
    .unwrap();
    root
}

#[test]
fn pnpm_workspace_root_writes_package_json_and_workspace_file_and_member_is_plain() {
    let root = build_ts_workspace("workspace");

    // stdout mode: manifest text only, plus a note about the sibling.
    let r = run_cli(&root, &["manifest"]);
    assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
    assert!(
        r.stdout.contains("\"private\": true"),
        "stdout: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("\"prettier\": \"^3.6.0\""),
        "root tooling dep must land; stdout: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("workspaces"),
        "pnpm reads members from pnpm-workspace.yaml, never from package.json; stdout: {}",
        r.stdout
    );
    assert!(
        !root.join("pnpm-workspace.yaml").exists(),
        "stdout mode must not write the sibling"
    );

    // --write: both files, declaration order in the sibling.
    let w = run_cli(&root, &["manifest", "--write"]);
    assert_eq!(w.exit_code, Some(0), "stderr: {}", w.stderr);
    assert_eq!(
        std::fs::read_to_string(root.join("package.json")).unwrap(),
        r.stdout
    );
    let ws = std::fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap();
    assert_eq!(ws, "packages:\n  - \"app\"\n  - \"packages/shared\"\n");
    assert!(
        w.stderr.contains("wrote pnpm-workspace.yaml"),
        "stderr: {}",
        w.stderr
    );

    // A repeat write with the sibling untouched is NOT a conflict: the bytes
    // match the render, so the identical-bytes row fires before the lineage
    // check. This is the regenerate loop, and #148's guard must not break it.
    let w1b = run_cli(&root, &["manifest", "--write"]);
    assert_eq!(
        w1b.exit_code,
        Some(0),
        "an unedited repeat write must stay exit 0: {}",
        w1b.stderr
    );
    assert_eq!(
        std::fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
        ws
    );

    // The sibling is gen-owned, but a hand-edit is no longer DISCARDED
    // silently. Extended with #148: the sibling takes the same matrix as the
    // primary manifest, because bytes that differ from the render and carry no
    // lineage row are indistinguishable from a file the tool has never seen —
    // which in a brownfield monorepo is exactly what they are. This assertion
    // was `Some(0)` + "restores byte-for-byte" before that change.
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - \"nope\"\n",
    )
    .unwrap();
    let w2 = run_cli(&root, &["manifest", "--write"]);
    assert_eq!(
        w2.exit_code,
        Some(1),
        "an edited, unrecorded sibling is refused: {}",
        w2.stderr
    );
    assert!(
        w2.stderr.contains("[MANIFEST-003]") && w2.stderr.contains("pnpm-workspace.yaml"),
        "the refusal names the rule and the sibling: {}",
        w2.stderr
    );
    assert_eq!(
        std::fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
        "packages:\n  - \"nope\"\n",
        "a refusal must leave the user's bytes untouched"
    );

    // `--accept-edits` is the documented override, and it restores the sibling
    // byte-for-byte — the behaviour the bare `--write` used to have.
    let w2b = run_cli(&root, &["manifest", "--write", "--accept-edits"]);
    assert_eq!(w2b.exit_code, Some(0), "stderr: {}", w2b.stderr);
    assert_eq!(
        std::fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
        ws,
        "--accept-edits must restore the sibling byte-for-byte"
    );

    // --output redirects the manifest text alone and says so.
    let out_path = root.join("elsewhere.json");
    std::fs::remove_file(root.join("pnpm-workspace.yaml")).unwrap();
    let o = run_cli(&root, &["manifest", "--output", out_path.to_str().unwrap()]);
    assert_eq!(o.exit_code, Some(0), "stderr: {}", o.stderr);
    assert!(out_path.is_file());
    assert!(
        !root.join("pnpm-workspace.yaml").exists(),
        "--output must not write the sibling"
    );
    assert!(
        o.stderr.contains("not written under --output"),
        "stderr: {}",
        o.stderr
    );

    // Member: a full TypeScript project nested at the member path renders a
    // plain package.json (pnpm needs no member marker).
    let member = root.join("app");
    std::fs::create_dir_all(&member).unwrap();
    bootstrap_rust(&member, "app");
    flip_to_typescript(&member, TS_PROFILE_YAML);
    std::fs::write(
        member.join("specs/architecture.toml"),
        "[[external_deps]]\npackage = \"shared\"\nworkspace = \"*\"\n",
    )
    .unwrap();
    let m = run_cli(&member, &["manifest"]);
    assert_eq!(m.exit_code, Some(0), "stderr: {}", m.stderr);
    assert!(
        m.stdout.contains("\"shared\": \"workspace:*\""),
        "stdout: {}",
        m.stdout
    );
    assert!(!m.stdout.contains("resolution"), "stdout: {}", m.stdout);
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Live gates (pnpm on PATH). Each returns early with a note otherwise.
// ---------------------------------------------------------------------------

#[test]
fn live_pnpm_install_resolves_emitted_manifest_and_manifest_rerun_is_byte_identical() {
    if !pnpm_available() {
        eprintln!("SKIPPED: pnpm not on PATH — live install gate not run");
        return;
    }
    let target = tmp_dir("live");
    bootstrap_rust(&target, "live-app");
    flip_to_typescript(&target, TS_PROFILE_YAML);
    // Registry deps only, tiny, so the install is fast and offline-cache
    // friendly.
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"version = "0.1.0"

[[external_deps]]
package = "is-odd"
version = "^3.0.1"

[[external_deps]]
package = "left-pad"
version = "^1.3.0"
test_only = true
"#,
    )
    .unwrap();
    let w = run_cli(&target, &["manifest", "--write"]);
    assert_eq!(w.exit_code, Some(0), "stderr: {}", w.stderr);
    let before = std::fs::read_to_string(target.join("package.json")).unwrap();

    let out = pnpm_install(&target);
    assert!(
        out.status.success(),
        "pnpm install failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        target.join("pnpm-lock.yaml").is_file(),
        "a successful install writes the lockfile"
    );
    assert!(target.join("node_modules/is-odd").exists());
    assert!(target.join("node_modules/left-pad").exists());

    // No rewrite trap: the install left package.json alone, and a fresh
    // render equals the file on disk.
    let after = std::fs::read_to_string(target.join("package.json")).unwrap();
    assert_eq!(before, after, "pnpm install must not rewrite package.json");
    let again = run_cli(&target, &["manifest"]);
    assert_eq!(
        again.stdout, after,
        "manifest re-run must be byte-identical"
    );
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn live_pnpm_workspace_root_resolves_workspace_member() {
    if !pnpm_available() {
        eprintln!("SKIPPED: pnpm not on PATH — live workspace gate not run");
        return;
    }
    let root = build_ts_workspace("live-workspace");
    let w = run_cli(&root, &["manifest", "--write"]);
    assert_eq!(w.exit_code, Some(0), "stderr: {}", w.stderr);

    // Member `app` via the spec; member `shared` is a hand-written package
    // (a second spec root is not what this gate tests).
    let app = root.join("app");
    std::fs::create_dir_all(&app).unwrap();
    bootstrap_rust(&app, "app");
    flip_to_typescript(&app, TS_PROFILE_YAML);
    std::fs::write(
        app.join("specs/architecture.toml"),
        "[[external_deps]]\npackage = \"shared\"\nworkspace = \"*\"\n",
    )
    .unwrap();
    let m = run_cli(&app, &["manifest", "--write"]);
    assert_eq!(m.exit_code, Some(0), "stderr: {}", m.stderr);
    let shared = root.join("packages/shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        shared.join("package.json"),
        "{\n  \"name\": \"shared\",\n  \"version\": \"0.0.1\",\n  \"private\": true\n}\n",
    )
    .unwrap();

    let out = pnpm_install(&root);
    assert!(
        out.status.success(),
        "pnpm install at the workspace root failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        root.join("pnpm-lock.yaml").is_file(),
        "one lockfile at the root"
    );
    assert!(
        !app.join("pnpm-lock.yaml").exists(),
        "members carry no lockfile"
    );
    let link = app.join("node_modules/shared");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "workspace:* member must be linked into the app; missing {}",
        link.display()
    );
    let _ = std::fs::remove_dir_all(&root);
}
