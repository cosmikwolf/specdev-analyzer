//! Merge-policy conformance across the three manifest emitters.
//!
//! The Cargo.toml, pubspec.yaml and package.json emitters are concrete
//! siblings that share merge *discipline*, not code. This table holds
//! them to one policy: for each scenario, a user-authored entry in
//! `architecture.toml [[external_deps]]` meets a block-contributed entry
//! for the same package, and every emitter must (a) keep the
//! first-declared (user) entry and (b) emit exactly the warnings the row
//! expects — same substrings, all three.
//!
//! Policy under test:
//!   * both sides versioned and different → "declared with conflicting
//!     versions"; user version wins.
//!   * identical entries → silent.
//!   * one side unversioned (a local path) vs a registry version → NOT a
//!     version conflict (a path package's version lives in its own
//!     manifest); it IS a source conflict → "declared with conflicting
//!     sources"; the path source survives.
//!   * registry vs git at the same version → source conflict only.
//!
//! One divergence is a documented ecosystem fact, not a drift: the cargo
//! emitter keys its accumulator on `(test_only, crate)` because Cargo
//! unions `[dependencies]` and `[dev-dependencies]` for test builds; pub
//! and npm hold one section per package. That axis is not in this table.
//! The cargo merger also normalizes before comparing sources: an absent
//! source ≡ `"crates-io"`, and `"workspace"` is inheritance (unstated),
//! never a conflict.

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
        "specdev-merge-policy-{tag}-{}-{}",
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

fn run_manifest(project: &Path) -> (Option<i32>, String, String) {
    let out = Command::new(analyzer_bin())
        .args(["manifest", "--project"])
        .arg(project)
        .output()
        .expect("invoke analyzer");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Rust,
    Dart,
    TypeScript,
}

impl Lang {
    fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Dart => "dart",
            Lang::TypeScript => "typescript",
        }
    }
}

/// A dependency shape expressed once, rendered per language into the
/// architecture (TOML) and block (`deps.yaml`) vocabularies.
#[derive(Clone, Copy)]
enum Dep {
    Registry(&'static str),
    Path(&'static str),
    Git {
        url: &'static str,
        rev: &'static str,
        version: &'static str,
    },
}

const PKG: &str = "dupe";

fn arch_entry(lang: Lang, dep: Dep) -> String {
    match (lang, dep) {
        (Lang::Rust, Dep::Registry(v)) => {
            format!("[[external_deps]]\ncrate = \"{PKG}\"\nversion = \"{v}\"\n")
        }
        (Lang::Rust, Dep::Path(p)) => {
            format!("[[external_deps]]\ncrate = \"{PKG}\"\nsource = {{ path = \"{p}\" }}\n")
        }
        (Lang::Rust, Dep::Git { url, rev, version }) => format!(
            "[[external_deps]]\ncrate = \"{PKG}\"\nversion = \"{version}\"\nsource = {{ git = \"{url}\", rev = \"{rev}\" }}\n"
        ),
        (Lang::Dart, Dep::Registry(v)) => {
            format!("[[external_deps]]\npackage = \"{PKG}\"\nversion = \"{v}\"\n")
        }
        (Lang::Dart, Dep::Path(p)) => {
            format!("[[external_deps]]\npackage = \"{PKG}\"\npath = \"{p}\"\n")
        }
        (Lang::Dart, Dep::Git { url, rev, .. }) => format!(
            "[[external_deps]]\npackage = \"{PKG}\"\ngit = {{ url = \"{url}\", ref = \"{rev}\" }}\n"
        ),
        (Lang::TypeScript, Dep::Registry(v)) => {
            format!("[[external_deps]]\npackage = \"{PKG}\"\nversion = \"{v}\"\n")
        }
        (Lang::TypeScript, Dep::Path(p)) => {
            format!("[[external_deps]]\npackage = \"{PKG}\"\npath = \"{p}\"\n")
        }
        (Lang::TypeScript, Dep::Git { url, rev, .. }) => format!(
            "[[external_deps]]\npackage = \"{PKG}\"\ngit = {{ url = \"{url}\", ref = \"{rev}\" }}\n"
        ),
    }
}

fn block_entry(lang: Lang, dep: Dep) -> String {
    match (lang, dep) {
        (Lang::Rust, Dep::Registry(v)) => {
            format!("crates:\n  - name: {PKG}\n    version: \"{v}\"\n")
        }
        (Lang::Rust, Dep::Path(p)) => {
            // The block vocabulary requires a version on every crate; a
            // path source still gets one (checked against the path crate).
            format!("crates:\n  - name: {PKG}\n    version: \"0.0.0\"\n    source:\n      path: \"{p}\"\n")
        }
        (Lang::Rust, Dep::Git { url, rev, version }) => format!(
            "crates:\n  - name: {PKG}\n    version: \"{version}\"\n    source:\n      git: \"{url}\"\n      rev: \"{rev}\"\n"
        ),
        (Lang::Dart, Dep::Registry(v)) => {
            format!("pub:\n  - package: {PKG}\n    version: \"{v}\"\n")
        }
        (Lang::Dart, Dep::Path(p)) => format!("pub:\n  - package: {PKG}\n    path: \"{p}\"\n"),
        (Lang::Dart, Dep::Git { url, rev, .. }) => format!(
            "pub:\n  - package: {PKG}\n    git:\n      url: \"{url}\"\n      ref: \"{rev}\"\n"
        ),
        (Lang::TypeScript, Dep::Registry(v)) => {
            format!("npm:\n  - package: {PKG}\n    version: \"{v}\"\n")
        }
        (Lang::TypeScript, Dep::Path(p)) => {
            format!("npm:\n  - package: {PKG}\n    path: \"{p}\"\n")
        }
        (Lang::TypeScript, Dep::Git { url, rev, .. }) => format!(
            "npm:\n  - package: {PKG}\n    git:\n      url: \"{url}\"\n      ref: \"{rev}\"\n"
        ),
    }
}

/// Bootstrap a library-api project and, for Dart/TypeScript, flip the
/// language and install the profile section that emitter needs. Then
/// write the architecture entry and one block contributing `deps.yaml`.
fn project(lang: Lang, tag: &str, arch: Dep, block: Dep) -> PathBuf {
    let target = tmp_dir(&format!("{}-{tag}", lang.name()));
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

    let arch_path = target.join("specs/architecture.toml");
    match lang {
        Lang::Rust => {
            let existing = std::fs::read_to_string(&arch_path).unwrap_or_default();
            std::fs::write(
                &arch_path,
                format!("{existing}\n{}", arch_entry(lang, arch)),
            )
            .unwrap();
        }
        Lang::Dart => {
            let spec_path = target.join("spec.toml");
            let spec = std::fs::read_to_string(&spec_path).unwrap();
            std::fs::write(
                &spec_path,
                spec.replace("language = \"rust\"", "language = \"dart\""),
            )
            .unwrap();
            std::fs::write(
                target.join("specs/specdev/profile.yaml"),
                "name: t\nlanguage: dart\npubspec_yaml:\n  shape: app\n  environment:\n    sdk: \"^3.6.0\"\n",
            )
            .unwrap();
            std::fs::write(&arch_path, arch_entry(lang, arch)).unwrap();
        }
        Lang::TypeScript => {
            let spec_path = target.join("spec.toml");
            let spec = std::fs::read_to_string(&spec_path).unwrap();
            std::fs::write(
                &spec_path,
                spec.replace("language = \"rust\"", "language = \"typescript\""),
            )
            .unwrap();
            std::fs::write(
                target.join("specs/specdev/profile.yaml"),
                "name: t\nlanguage: typescript\npackage_json:\n  shape: app\n",
            )
            .unwrap();
            std::fs::write(&arch_path, arch_entry(lang, arch)).unwrap();
        }
    }

    std::fs::write(
        target.join("specs/specdev/schemas/inlined/blocks/conf.schema.yaml"),
        "# placeholder block schema (discovery reads only the filename)\n",
    )
    .unwrap();
    let block_codegen = target
        .join("specs/specdev/codegen")
        .join(lang.name())
        .join("blocks/conf");
    std::fs::create_dir_all(&block_codegen).unwrap();
    std::fs::write(block_codegen.join("deps.yaml"), block_entry(lang, block)).unwrap();
    target
}

struct Scenario {
    name: &'static str,
    arch: Dep,
    block: Dep,
    /// Warning substrings every emitter must produce.
    expect: &'static [&'static str],
    /// Warning substrings no emitter may produce.
    forbid: &'static [&'static str],
    /// Output substrings every emitter must contain (the surviving entry).
    survives: &'static [&'static str],
    /// Output substrings no emitter may contain (the losing entry).
    lost: &'static [&'static str],
}

const CONFLICT_VERSIONS: &str = "declared with conflicting versions";
const CONFLICT_SOURCES: &str = "declared with conflicting sources";

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "both_versioned_differ",
        arch: Dep::Registry("^1.2.0"),
        block: Dep::Registry("^9.9.9"),
        expect: &[CONFLICT_VERSIONS, "`dupe`"],
        forbid: &[CONFLICT_SOURCES],
        survives: &["^1.2.0"],
        lost: &["^9.9.9"],
    },
    Scenario {
        name: "identical_entries",
        arch: Dep::Registry("^1.2.0"),
        block: Dep::Registry("^1.2.0"),
        expect: &[],
        forbid: &[CONFLICT_VERSIONS, CONFLICT_SOURCES],
        survives: &["^1.2.0"],
        lost: &[],
    },
    Scenario {
        name: "unversioned_path_vs_registry",
        arch: Dep::Path("../dupe"),
        block: Dep::Registry("^1.2.0"),
        expect: &[CONFLICT_SOURCES, "`dupe`"],
        forbid: &[CONFLICT_VERSIONS],
        survives: &["../dupe"],
        lost: &["^1.2.0"],
    },
    Scenario {
        name: "registry_vs_git_same_version",
        arch: Dep::Registry("^1.2.0"),
        block: Dep::Git {
            url: "https://git.example/dupe",
            rev: "v1.2.0",
            version: "^1.2.0",
        },
        expect: &[CONFLICT_SOURCES, "`dupe`"],
        forbid: &[CONFLICT_VERSIONS],
        survives: &["^1.2.0"],
        lost: &["git.example"],
    },
];

#[test]
fn three_emitters_share_one_merge_policy() {
    let mut failures: Vec<String> = Vec::new();
    for sc in SCENARIOS {
        for lang in [Lang::Rust, Lang::Dart, Lang::TypeScript] {
            let target = project(lang, sc.name, sc.arch, sc.block);
            let (code, stdout, stderr) = run_manifest(&target);
            let ctx = format!("[{} / {}]", sc.name, lang.name());
            if code != Some(0) {
                failures.push(format!("{ctx} manifest exited {code:?}\nstderr: {stderr}"));
                continue;
            }
            for e in sc.expect {
                if !stderr.contains(e) {
                    failures.push(format!(
                        "{ctx} expected warning containing {e:?}\nstderr: {stderr}"
                    ));
                }
            }
            for f in sc.forbid {
                if stderr.contains(f) {
                    failures.push(format!(
                        "{ctx} forbidden warning containing {f:?}\nstderr: {stderr}"
                    ));
                }
            }
            for s in sc.survives {
                if !stdout.contains(s) {
                    failures.push(format!(
                        "{ctx} expected output containing {s:?}\nstdout: {stdout}"
                    ));
                }
            }
            for l in sc.lost {
                if stdout.contains(l) {
                    failures.push(format!(
                        "{ctx} losing entry {l:?} leaked into output\nstdout: {stdout}"
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&target);
        }
    }
    assert!(
        failures.is_empty(),
        "merge-policy conformance failures:\n\n{}",
        failures.join("\n\n")
    );
}
