//! `rust-workspace` profile + workspace `cargo-toml` emission (G-005).
//!
//! Three layers of coverage:
//!   * Bootstrap — `--profile rust-workspace` scaffolds the expected
//!     workspace layout (no module.schema.yaml, no module.conventions.md
//!     since workspaces have no module surface).
//!   * Workspace-root render — `[workspace]` + `[workspace.package]` +
//!     `[workspace.dependencies]` emit in idiomatic Cargo order.
//!   * Member-mode render — package fields owned by the workspace's
//!     `[workspace_package]` become `<field>.workspace = true`; deps with
//!     `source = "workspace"` render as `{ workspace = true, ... overlay
//!     ... }` (G-005 acceptance).
//!
//! Regression target: the zfm dogfood project. The `zfm_*` tests
//! reproduce its workspace `Cargo.toml` and per-member `Cargo.toml`
//! shapes.

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
        "specdev-rust-workspace-{tag}-{}-{}",
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

/// Bootstrap a `rust-workspace` project at a fresh temp dir; return the
/// project root. Tests overwrite spec.toml + architecture.toml to
/// provision real members + hoisted deps, then call [`discover`] to get
/// a fresh layout reflecting those overwrites. (`LayoutContext` caches
/// the parsed spec.toml at discover time, so any test that mutates
/// spec.toml after bootstrap MUST re-discover before rendering.)
fn fresh_workspace(tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "demo-ws".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

/// Bootstrap a member library-api crate inside an already-bootstrapped
/// workspace. Returns the member's project root.
fn fresh_member(workspace_root: &Path, dir_name: &str) -> PathBuf {
    let member_dir = workspace_root.join("crates").join(dir_name);
    std::fs::create_dir_all(&member_dir).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: dir_name.into(),
            target_dir: member_dir.clone(),
        },
    )
    .unwrap();
    member_dir
}

fn discover(target: &Path) -> LayoutContext {
    LayoutContext::discover(target, None).unwrap()
}

// ---------------------------------------------------------------------------
// 1 — Bootstrap layout: rust-workspace ships workspace.conventions.md (NOT
//     module.conventions.md). The dynamic enumeration in bootstrap picks it
//     up via the `*.conventions.md` family.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_emits_workspace_conventions_not_module_conventions() {
    let target = fresh_workspace("bootstrap-layout");
    assert!(
        target
            .join("specs/specdev/codegen/rust/workspace.conventions.md")
            .is_file(),
        "rust-workspace bootstraps with workspace.conventions.md"
    );
    assert!(
        !target
            .join("specs/specdev/codegen/rust/module.conventions.md")
            .exists(),
        "rust-workspace must NOT carry module.conventions.md (no module surface)"
    );
    assert!(
        !target
            .join("specs/specdev/schemas/workspace.schema.yaml")
            .exists(),
        "spec.toml's [workspace] table is validated by the analyzer's embedded \
         spec.schema.yaml; the profile ships no copy of that schema"
    );
    assert!(target
        .join("specs/specdev/schemas/architecture.schema.yaml")
        .is_file());
    assert!(
        !target
            .join("specs/specdev/schemas/module.schema.yaml")
            .exists(),
        "rust-workspace must NOT carry module.schema.yaml"
    );
}

// ---------------------------------------------------------------------------
// 2 — Workspace-root render emits [workspace] + members + idiomatic-order
//     [workspace.package] + lex-sorted [workspace.dependencies].
// ---------------------------------------------------------------------------
#[test]
fn workspace_root_render_emits_full_workspace_block() {
    let target = fresh_workspace("ws-root");
    overwrite_spec_with_two_members(&target);
    overwrite_workspace_arch_minimal(&target);

    let layout = discover(&target);
    let r = render(&layout).unwrap();
    let t = r.text;

    assert!(
        t.starts_with("[workspace]\n"),
        "first section is [workspace]\n{t}"
    );
    assert!(t.contains("resolver = \"2\"\n"), "{t}");
    assert!(
        t.contains("members = [\n    \"crates/alpha\",\n    \"crates/beta\",\n]\n"),
        "members in declaration order:\n{t}"
    );
    assert!(t.contains("[workspace.package]\n"));
    let wp_idx = t.find("[workspace.package]").unwrap();
    let wp_block = &t[wp_idx..];
    let v_idx = wp_block.find("version =").unwrap();
    let e_idx = wp_block.find("edition =").unwrap();
    let l_idx = wp_block.find("license =").unwrap();
    let r_idx = wp_block.find("repository =").unwrap();
    let rv_idx = wp_block.find("rust-version =").unwrap();
    assert!(
        v_idx < e_idx && e_idx < l_idx && l_idx < r_idx && r_idx < rv_idx,
        "[workspace.package] field order: version → edition → license → \
         repository → rust-version (matches zfm baseline)\n{t}"
    );
    assert!(t.contains("[workspace.dependencies]\n"));
    assert!(
        t.contains("alpha-shared = { version = \"0.1.0\", default-features = false, path = \"crates/alpha\" }\n"),
        "alpha-shared rendered with path-dep + default-features:\n{t}"
    );
    assert!(t.contains("defmt = \"0.3\"\n"), "{t}");
}

// ---------------------------------------------------------------------------
// 3 — G-005: member-mode render rewrites every workspace-inheritable
//     [package] field to `<field>.workspace = true` when the workspace's
//     [workspace_package] table owns the canonical value.
// ---------------------------------------------------------------------------
#[test]
fn member_mode_emits_workspace_inherited_package_fields() {
    let target = fresh_workspace("member-pkg-inherit");
    overwrite_spec_with_two_members(&target);
    overwrite_workspace_arch_minimal(&target);

    // alpha is a library-api member that opts into workspace package
    // inheritance simply by NOT setting [project].license / .repository
    // / etc. on its own spec.toml — the absence + workspace presence is
    // enough to trigger `*.workspace = true` emit.
    let member_dir = fresh_member(&target, "alpha");
    overwrite_member_spec(
        &member_dir,
        "alpha-shared",
        Some("alpha description goes here"),
    );

    let member_layout = discover(&member_dir);
    let r = render(&member_layout).unwrap();
    let t = r.text;

    let pkg_block: Vec<&str> = t
        .lines()
        .skip_while(|l| !l.starts_with("[package]"))
        .take_while(|l| !l.is_empty())
        .collect();
    assert!(pkg_block.contains(&"name = \"alpha-shared\""), "{t}");
    assert!(pkg_block.contains(&"version.workspace = true"), "{t}");
    assert!(pkg_block.contains(&"edition.workspace = true"), "{t}");
    assert!(pkg_block.contains(&"license.workspace = true"), "{t}");
    assert!(pkg_block.contains(&"repository.workspace = true"), "{t}");
    assert!(pkg_block.contains(&"rust-version.workspace = true"), "{t}");
    assert!(
        pkg_block.contains(&"description = \"alpha description goes here\""),
        "description stays per-crate (never workspace-inherited):\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 4 — Member-mode render leaves non-inherited fields literal — when a
//     workspace's [workspace_package] table is missing a field (e.g.
//     repository), the member's emit falls back to its own [project]
//     value (or omits the line entirely).
// ---------------------------------------------------------------------------
#[test]
fn member_mode_falls_back_to_member_literal_when_workspace_lacks_field() {
    let target = fresh_workspace("member-fallback");
    overwrite_spec_with_two_members(&target);
    // Workspace declares only version + edition; license + repository
    // + rust-version stay member-owned.
    std::fs::write(
        target.join("specs/architecture.toml"),
        "[workspace_package]\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let member_dir = fresh_member(&target, "alpha");
    // Member sets its own license + repository explicitly.
    std::fs::write(
        member_dir.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "alpha-shared"
profile = "library-api"
language = "rust"
license = "Apache-2.0"
repository = "https://example.org/alpha"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();

    let member_layout = discover(&member_dir);
    let r = render(&member_layout).unwrap();
    let t = r.text;

    assert!(t.contains("version.workspace = true"), "{t}");
    assert!(t.contains("edition.workspace = true"), "{t}");
    assert!(t.contains("license = \"Apache-2.0\""), "{t}");
    assert!(
        t.contains("repository = \"https://example.org/alpha\""),
        "{t}"
    );
    assert!(!t.contains("license.workspace"), "{t}");
    assert!(!t.contains("repository.workspace"), "{t}");
    assert!(
        !t.contains("rust-version"),
        "no rust-version anywhere:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Member-mode dep emit: source = "workspace" renders bare
//     `{ workspace = true }` plus per-member `optional = true` overlay
//     and additive `features = [...]` overlay (Cargo permits these at
//     member-dep site).
// ---------------------------------------------------------------------------
#[test]
fn member_mode_workspace_dep_overlays_optional_and_features() {
    let target = fresh_workspace("dep-overlay");
    overwrite_spec_with_two_members(&target);
    overwrite_workspace_arch_minimal(&target);

    let member_dir = fresh_member(&target, "alpha");
    overwrite_member_spec(&member_dir, "alpha-shared", None);

    std::fs::write(
        member_dir.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"

[[external_deps]]
crate = "defmt"
version = "0.3"
source = "workspace"
optional = true

[[external_deps]]
crate = "serde-extras"
version = "1.0"
features = ["derive"]
source = "workspace"
"#,
    )
    .unwrap();

    let member_layout = discover(&member_dir);
    let r = render(&member_layout).unwrap();
    let t = r.text;

    let line_defmt = t
        .lines()
        .find(|l| l.starts_with("defmt ="))
        .unwrap_or_else(|| panic!("defmt line missing:\n{t}"));
    assert_eq!(
        line_defmt, "defmt = { workspace = true, optional = true }",
        "defmt line: workspace + optional overlay\n{t}"
    );
    let line_serde = t
        .lines()
        .find(|l| l.starts_with("serde-extras ="))
        .unwrap_or_else(|| panic!("serde-extras line missing:\n{t}"));
    assert_eq!(
        line_serde, "serde-extras = { workspace = true, features = [\"derive\"] }",
        "serde-extras line: workspace + additive features\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Standalone library-api project (no workspace ancestor) renders
//     with no *.workspace = true — literal package fields throughout.
// ---------------------------------------------------------------------------
#[test]
fn standalone_library_unchanged_without_workspace_ancestor() {
    let workspace = tmp_workspace("standalone");
    let target = workspace.join("standalone-lib");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "loner".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("name = \"loner\"") && r.text.contains("edition = \"2021\"\n"),
        "no workspace ancestor → literal edition; got:\n{}",
        r.text
    );
    assert!(
        !r.text.contains(".workspace = true"),
        "no workspace ancestor → no *.workspace lines:\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 7 — `[features]` synthesis: render emits `default = [...]` line always
//     when [features] is present, and accepts cargo_deps strings (dep:foo
//     and crate/feature) verbatim.
// ---------------------------------------------------------------------------
#[test]
fn library_features_emit_default_line_and_cargo_deps() {
    let workspace = tmp_workspace("features");
    let target = workspace.join("featured");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "featured".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[[features]]
name = "std"
default = true

[[features]]
name = "alloc"

[[features]]
name = "serde"
cargo_deps = ["dep:serde"]
"#,
    )
    .unwrap();

    let r = render(&layout).unwrap();
    let t = r.text;

    assert!(t.contains("[features]\ndefault = [\"std\"]\n"), "{t}");
    assert!(t.contains("alloc = []\n"), "{t}");
    assert!(t.contains("serde = [\"dep:serde\"]\n"), "{t}");
    // The `std` feature is captured by the default = [] synthesis but
    // also gets its own line because parse_library_features keeps it in
    // the features map (a future emitter could elide it; v1 keeps it
    // for easier human review).
    assert!(t.contains("std = []\n"), "{t}");
}

// ---------------------------------------------------------------------------
// 8 — A `rust-workspace` profile spec.toml without a [workspace] table is
//     rejected by the cargo-toml render with a pointed error message (no
//     silent fallback).
// ---------------------------------------------------------------------------
#[test]
fn rust_workspace_profile_without_workspace_table_errors() {
    let workspace = tmp_workspace("missing-table");
    let target = workspace.join("broken");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "broken".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    // Strip the [workspace] table.
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "broken"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."
"#,
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    let err = render(&layout).unwrap_err();
    let msg = format!("{err:#}");
    // The error is shape-driven and profile-name-agnostic: it names the
    // misaligned shape (`workspace`) and the missing layout signal
    // (`[workspace]`), not the specific profile name.
    assert!(
        msg.contains("shape: workspace") && msg.contains("[workspace]"),
        "error names the shape and the missing table:\n{msg}"
    );
}

// ---------------------------------------------------------------------------
// 9 — Walk-up tolerance: workspace ancestor resolution succeeds even when
//     intermediate directories carry no spec.toml. Member at depth 2 with
//     no spec.toml between still finds the workspace at depth 0.
// ---------------------------------------------------------------------------
#[test]
fn workspace_ancestor_walk_up_skips_intermediate_dirs_without_spec_toml() {
    let target = fresh_workspace("walk-up");
    // Override members list so it includes a deep path.
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

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
name = "alpha-shared"
path = "subgroup/alpha"
"#,
    )
    .unwrap();
    overwrite_workspace_arch_minimal(&target);

    // Bootstrap the member two directories deep (subgroup/alpha) — the
    // `subgroup` dir has no spec.toml, so the walk-up must skip past it.
    let member_dir = target.join("subgroup").join("alpha");
    std::fs::create_dir_all(&member_dir).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "alpha-shared".into(),
            target_dir: member_dir.clone(),
        },
    )
    .unwrap();
    overwrite_member_spec(&member_dir, "alpha-shared", None);

    let member_layout = LayoutContext::discover(&member_dir, None).unwrap();
    let r = render(&member_layout).unwrap();
    assert!(
        r.text.contains("version.workspace = true"),
        "walk-up must succeed across `subgroup/` (no spec.toml):\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 10 — zfm regression target. The workspace + per-member render
//      reproduces the dogfood baseline shape (modulo recorded emitter
//      improvements: lex-sorted [workspace.dependencies], lex-sorted
//      [features] lines, alphabetical inline-table key order).
// ---------------------------------------------------------------------------
#[test]
fn zfm_dogfood_regression_workspace_and_two_members() {
    let target = fresh_workspace("zfm-regression");
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "zfm"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]
resolver = "2"

[[workspace.members]]
name = "ym2414b-core"
path = "crates/ym2414b-core"

[[workspace.members]]
name = "ym2414b-sysex"
path = "crates/ym2414b-sysex"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[workspace_package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/Zetaohm/zfm"
rust-version = "1.75"

[[workspace_dependencies]]
crate = "ym2414b-core"
version = "0.1.0"
source = { path = "crates/ym2414b-core" }
default_features = false

[[workspace_dependencies]]
crate = "defmt"
version = "0.3"
"#,
    )
    .unwrap();

    // Reload because spec.toml changed.
    let layout = discover(&target);
    let ws = render(&layout).unwrap().text;
    assert!(ws.contains("[workspace]\nresolver = \"2\"\n"), "{ws}");
    assert!(
        ws.contains("members = [\n    \"crates/ym2414b-core\",\n    \"crates/ym2414b-sysex\",\n]"),
        "{ws}"
    );
    assert!(
        ws.contains("[workspace.package]\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT OR Apache-2.0\"\nrepository = \"https://github.com/Zetaohm/zfm\"\nrust-version = \"1.75\"\n"),
        "[workspace.package] block matches zfm baseline order:\n{ws}"
    );
    assert!(ws.contains("[workspace.dependencies]\n"), "{ws}");
    assert!(ws.contains("defmt = \"0.3\"\n"), "{ws}");
    assert!(
        ws.contains(
            "ym2414b-core = { version = \"0.1.0\", default-features = false, path = \"crates/ym2414b-core\" }\n"
        ),
        "{ws}"
    );

    // ---- ym2414b-core member ----
    let core_dir = fresh_member(&target, "ym2414b-core");
    std::fs::write(
        core_dir.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "ym2414b-core"
profile = "library-api"
language = "rust"
description = "no_std software emulation of the Yamaha YM2414B (OPZ) FM tone-generator LSI"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    std::fs::write(
        core_dir.join("specs/architecture.toml"),
        r#"[[features]]
name = "std"

[[features]]
name = "defmt"
cargo_deps = ["dep:defmt"]
"#,
    )
    .unwrap();
    std::fs::write(
        core_dir.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"

[[external_deps]]
crate = "defmt"
version = "0.3"
source = "workspace"
optional = true
"#,
    )
    .unwrap();
    let core_layout = discover(&core_dir);
    let core_t = render(&core_layout).unwrap().text;
    assert!(core_t.contains("name = \"ym2414b-core\""), "{core_t}");
    assert!(core_t.contains("version.workspace = true"), "{core_t}");
    assert!(core_t.contains("edition.workspace = true"), "{core_t}");
    assert!(core_t.contains("license.workspace = true"), "{core_t}");
    assert!(core_t.contains("repository.workspace = true"), "{core_t}");
    assert!(core_t.contains("rust-version.workspace = true"), "{core_t}");
    assert!(
        core_t.contains(
            "description = \"no_std software emulation of the Yamaha YM2414B (OPZ) FM tone-generator LSI\""
        ),
        "{core_t}"
    );
    assert!(core_t.contains("\n[lib]\n"), "{core_t}");
    assert!(core_t.contains("default = []\n"), "{core_t}");
    assert!(core_t.contains("defmt = [\"dep:defmt\"]\n"), "{core_t}");
    assert!(core_t.contains("std = []\n"), "{core_t}");
    assert!(
        core_t.contains("defmt = { workspace = true, optional = true }\n"),
        "{core_t}"
    );

    // ---- ym2414b-sysex member ----
    let sysex_dir = fresh_member(&target, "ym2414b-sysex");
    std::fs::write(
        sysex_dir.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "ym2414b-sysex"
profile = "library-api"
language = "rust"
description = "no_std parser/serializer for Yamaha TQ5 / TX81Z (OPZ) SysEx bulk dump formats"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    std::fs::write(
        sysex_dir.join("specs/architecture.toml"),
        r#"[[features]]
name = "std"
cargo_deps = ["ym2414b-core/std"]
"#,
    )
    .unwrap();
    std::fs::write(
        sysex_dir.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"

[[external_deps]]
crate = "ym2414b-core"
version = "0.1.0"
source = "workspace"
"#,
    )
    .unwrap();
    let sysex_layout = discover(&sysex_dir);
    let sysex_t = render(&sysex_layout).unwrap().text;
    assert!(sysex_t.contains("name = \"ym2414b-sysex\""), "{sysex_t}");
    assert!(sysex_t.contains("version.workspace = true"), "{sysex_t}");
    assert!(
        sysex_t.contains("rust-version.workspace = true"),
        "{sysex_t}"
    );
    assert!(
        sysex_t.contains("default = []\n"),
        "synthetic default line:\n{sysex_t}"
    );
    assert!(
        sysex_t.contains("std = [\"ym2414b-core/std\"]\n"),
        "namespaced cargo_deps verbatim:\n{sysex_t}"
    );
    assert!(
        sysex_t.contains("ym2414b-core = { workspace = true }\n"),
        "bare workspace dep:\n{sysex_t}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn overwrite_spec_with_two_members(target: &Path) {
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

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
name = "alpha-shared"
path = "crates/alpha"

[[workspace.members]]
name = "beta-utils"
path = "crates/beta"
"#,
    )
    .unwrap();
}

fn overwrite_workspace_arch_minimal(target: &Path) {
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[workspace_package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://example.org/demo-ws"
rust-version = "1.75"

[[workspace_dependencies]]
crate = "alpha-shared"
version = "0.1.0"
source = { path = "crates/alpha" }
default_features = false

[[workspace_dependencies]]
crate = "defmt"
version = "0.3"
"#,
    )
    .unwrap();
}

fn overwrite_member_spec(member_dir: &Path, crate_name: &str, description: Option<&str>) {
    let desc_line = match description {
        Some(d) => format!("description = {:?}\n", d),
        None => String::new(),
    };
    std::fs::write(
        member_dir.join("spec.toml"),
        format!(
            r#"spec_version = "1.0"

[project]
name = "{crate_name}"
profile = "library-api"
language = "rust"
{desc_line}
[paths]
specs = "specs"
source = "src"
"#,
        ),
    )
    .unwrap();
}

fn overwrite_spec_with_two_members_and_root_lists(
    target: &Path,
    default_members: &[&str],
    exclude: &[&str],
) {
    let quoted = |xs: &[&str]| {
        xs.iter()
            .map(|x| format!("{x:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    std::fs::write(
        target.join("spec.toml"),
        format!(
            r#"spec_version = "1.0"

[project]
name = "demo-ws"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]
resolver = "2"
default_members = [{}]
exclude = [{}]

[[workspace.members]]
name = "alpha-shared"
path = "crates/alpha"

[[workspace.members]]
name = "beta-utils"
path = "crates/beta"
"#,
            quoted(default_members),
            quoted(exclude),
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 11 — `[workspace].default_members` names members by NAME; the render
//      resolves each to the member's path and writes `default-members`
//      after `members`, in declaration order (not member-table order).
//      `exclude` renders verbatim after it.
// ---------------------------------------------------------------------------
#[test]
fn workspace_root_render_resolves_default_members_and_emits_exclude() {
    let target = fresh_workspace("ws-default-members");
    overwrite_spec_with_two_members_and_root_lists(
        &target,
        &["beta-utils", "alpha-shared"],
        &["tools/bench"],
    );
    overwrite_workspace_arch_minimal(&target);

    let layout = discover(&target);
    let t = render(&layout).unwrap().text;

    let members = "members = [\n    \"crates/alpha\",\n    \"crates/beta\",\n]\n";
    let defaults = "default-members = [\n    \"crates/beta\",\n    \"crates/alpha\",\n]\n";
    let exclude = "exclude = [\n    \"tools/bench\",\n]\n";
    assert!(t.contains(members), "{t}");
    assert!(
        t.contains(defaults),
        "default members resolved to paths, in the spec's declaration order:\n{t}"
    );
    assert!(t.contains(exclude), "exclude rendered verbatim:\n{t}");
    let m = t.find(members).unwrap();
    let d = t.find(defaults).unwrap();
    let e = t.find(exclude).unwrap();
    let wp = t.find("[workspace.package]").unwrap();
    assert!(
        m < d && d < e && e < wp,
        "order: members → default-members → exclude → [workspace.package]:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 12 — Neither key renders when the spec declares neither: a spec with
//      no `default_members` / `exclude` emits the pre-existing shape
//      byte-for-byte (Cargo's own defaults apply).
// ---------------------------------------------------------------------------
#[test]
fn workspace_root_render_omits_default_members_and_exclude_when_undeclared() {
    let target = fresh_workspace("ws-no-root-lists");
    overwrite_spec_with_two_members(&target);
    overwrite_workspace_arch_minimal(&target);

    let layout = discover(&target);
    let t = render(&layout).unwrap().text;
    assert!(!t.contains("default-members"), "{t}");
    assert!(!t.contains("exclude"), "{t}");

    // Empty lists are the same as absent lists.
    overwrite_spec_with_two_members_and_root_lists(&target, &[], &[]);
    let layout = discover(&target);
    assert_eq!(
        render(&layout).unwrap().text,
        t,
        "empty lists render as absent"
    );
}

// ---------------------------------------------------------------------------
// 13 — An unknown default member fails BOTH paths: `validate` (the
//      profile's `workspace_default_member` relation → REDUCE_UNRESOLVED)
//      and `render` (a hard error, since `manifest` does not run
//      `validate` and Cargo rejects a `default-members` path that is not
//      a member).
// ---------------------------------------------------------------------------
#[test]
fn unknown_default_member_fails_validate_and_render() {
    let target = fresh_workspace("ws-ghost-default-member");
    overwrite_spec_with_two_members_and_root_lists(&target, &["ghost"], &[]);
    overwrite_workspace_arch_minimal(&target);

    let report = validate::run_project(&target, None).unwrap();
    let hits: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        hits.iter().any(|f| f.rule == "REDUCE_UNRESOLVED"
            && f.message.contains("workspace_default_member")
            && f.message.contains("`ghost`")),
        "expected REDUCE_UNRESOLVED naming the relation and `ghost`:\n{}",
        report.render_plain()
    );

    let layout = discover(&target);
    let err = render(&layout).unwrap_err().to_string();
    assert!(
        err.contains("default_members") && err.contains("`ghost`"),
        "render must refuse an unknown default member:\n{err}"
    );
}

// ---------------------------------------------------------------------------
// 14 — The embedded spec.schema.yaml owns the `[workspace]` table: a
//      typo'd key beside `default_members` is still rejected (the
//      table stays closed), and a well-formed spec validates clean.
// ---------------------------------------------------------------------------
#[test]
fn workspace_table_stays_closed_and_valid_root_lists_validate_clean() {
    let target = fresh_workspace("ws-root-lists-valid");
    overwrite_spec_with_two_members_and_root_lists(&target, &["alpha-shared"], &["tools/bench"]);
    overwrite_workspace_arch_minimal(&target);
    let report = validate::run_project(&target, None).unwrap();
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );

    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    std::fs::write(
        target.join("spec.toml"),
        spec.replace("exclude = [", "excluded = ["),
    )
    .unwrap();
    let err = LayoutContext::discover(&target, None)
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    assert!(
        err.contains("excluded"),
        "an unknown key under [workspace] must be rejected by the embedded schema:\n{err}"
    );
}
