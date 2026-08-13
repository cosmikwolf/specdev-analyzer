//! Per-stack composition gates: co-inlined units must resolve to ONE
//! dependency graph, and the graph must contain what the composition was for.
//!
//! # Why this file exists
//!
//! Nothing else here composes two units and *resolves* them. Bridge reference
//! crates come closest, but they only cover pairs a bridge happens to join —
//! the gap is the **un-bridged co-inlined pair**, and the earn-its-keep rule
//! for new bridges deliberately grows that population.
//!
//! The live instance: an editor-topology block and a windowing host block
//! resolved two semver-incompatible copies of the same UI crate for months,
//! building green the whole time. It was closed once on inspection and
//! reopened by the first actual resolve.
//!
//! # The rule
//!
//! **A composition claim is settled by a resolve or a build, never by reading
//! manifests.** Two corollaries, both learned the expensive way:
//!
//! 1. **Assert on the resolved lockfile, not the emitted manifest.** Reading
//!    declared dependency lists is precisely what let the bug above be closed
//!    wrongly — the collision was transitive and no dep list showed it.
//! 2. **A green gate must assert the artifact contains what the build was
//!    for.** An exit code of 0 is not evidence; a build gate elsewhere passed
//!    twice over an artifact that held none of the code it was gating. So each
//!    stack also asserts its join product is present in the lock.
//!
//! # Which crates get a single-version assertion
//!
//! Only crates the composed units **declare** — never a name prefix. The
//! firmware stack resolves two copies of an upstream-internal crate
//! (`embassy-embedded-hal` wants one minor, `embassy-stm32` wants another);
//! that skew lives inside the upstream project's own published tree, is not
//! caused by any unit here, and cannot be fixed here. A prefix glob would fail
//! on it forever, and a gate that fails unfixably gets muted. What matters is
//! whether *our* units agree, so the assertion is scoped to the shared surface
//! they each name.
//!
//! # Shape and cost
//!
//! Linear, one fixture per stack — deliberately not combinatorial over unit
//! pairs. If the set outgrows what CI tolerates, cut whole stacks and say so;
//! do not sample within a stack and report green.
//!
//! Every test here is `#[ignore]`d: resolving touches crates.io, git remotes,
//! and the pub registry, and the rest of the suite is hermetic and must stay
//! so. Run them with `cargo test --test composition_stacks -- --ignored`, or
//! via `.github/scripts/composition-gates.sh`. They run against the **live** plugin
//! tree at the conventional sibling path (not a fixture mirror) because the
//! claim under test is about the units that actually ship; they skip when it
//! is absent.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml, inline,
    layout::LayoutContext,
    pubspec_yaml,
};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Locate the live plugin tree, in the same precedence order the CLI uses for
/// `SPECDEV_PLUGIN_ROOT`, then the two checkout layouts that occur in practice:
/// a sibling directory (a local workspace) and a nested one (a CI job that
/// checks the plugin out inside the analyzer).
///
/// Every candidate is validated by the plugin marker file rather than by
/// `is_dir()`. A bare directory test would accept a wrong path and let these
/// tests skip while reporting success — the precise failure mode this file
/// exists to prevent. The runner script hard-fails when nothing resolves, so a
/// CI run cannot go green having gated nothing.
fn live_plugin_tree() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        std::env::var_os("SPECDEV_PLUGIN_ROOT").map(PathBuf::from),
        manifest_dir.parent().map(|p| p.join("specdev")),
        Some(manifest_dir.join("specdev")),
    ];
    candidates.into_iter().flatten().find(|p| {
        p.join(specdev_analyzer::plugin_root::PLUGIN_MARKER)
            .is_file()
    })
}

fn tmp_project(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-composition-{tag}-{}-{}",
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

/// Bootstrap a project from `profile` and inline the named blocks + bridges.
fn compose(
    tag: &str,
    profile: &str,
    language: &str,
    project_name: &str,
    blocks: &[&str],
    bridges: &[&str],
) -> (PathBuf, PathBuf) {
    let plugin = live_plugin_tree().expect("live plugin tree (checked by caller)");
    let root = tmp_project(tag).join(project_name);
    std::fs::create_dir_all(&root).unwrap();

    bootstrap::run_bootstrap(
        &plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: language.into(),
            project_name: project_name.into(),
            target_dir: root.clone(),
        },
    )
    .unwrap_or_else(|e| panic!("bootstrap {profile}: {e}"));

    let layout = LayoutContext::discover(&root, None).unwrap();
    for b in blocks {
        inline::run_inline_block(&layout, &plugin, b)
            .unwrap_or_else(|e| panic!("inline block {b}: {e}"));
    }
    for b in bridges {
        inline::run_inline_bridge(&layout, &plugin, b)
            .unwrap_or_else(|e| panic!("inline bridge {b}: {e}"));
    }
    (plugin, root)
}

/// Render the build manifest and write it, plus any paired stub the renderer
/// asks for.
///
/// Then materialize a placeholder for every target the manifest declares that
/// the renderer did *not* pair. The renderer pairs `[[bin]]` rows but not
/// `[lib]`, so a freshly emitted library-shaped manifest cannot be resolved
/// until a `src/lib.rs` exists — tracked as a separate emitter finding. The
/// placeholder is inert: it changes nothing about the dependency graph under
/// test, it only makes the manifest parseable by cargo.
fn write_manifest(root: &Path) -> String {
    let layout = LayoutContext::discover(root, None).unwrap();
    let language = layout.spec.project.language.clone();

    let (text, extra) = match language.as_str() {
        "rust" => {
            let r = cargo_toml::render(&layout).expect("cargo_toml::render");
            (r.text, r.extra_files)
        }
        "dart" => {
            let r = pubspec_yaml::render(&layout).expect("pubspec_yaml::render");
            (r.text, Vec::new())
        }
        other => panic!("no manifest emission for language `{other}`"),
    };

    let filename = if language == "dart" {
        "pubspec.yaml"
    } else {
        "Cargo.toml"
    };
    std::fs::write(root.join(filename), &text).unwrap();
    for (rel, body) in &extra {
        let dst = root.join(rel);
        if dst.exists() {
            continue;
        }
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, body).unwrap();
    }

    if language == "rust" {
        ensure_declared_targets(root, &text);
    }
    text
}

/// Create an empty source file for each target the manifest declares, so
/// `cargo generate-lockfile` can parse it. Reads the manifest rather than
/// assuming a layout — a profile is free to point a target anywhere.
fn ensure_declared_targets(root: &Path, manifest: &str) {
    let doc: toml::Value = toml::from_str(manifest).expect("emitted manifest parses as TOML");

    let mut paths: Vec<String> = Vec::new();
    if let Some(lib) = doc.get("lib") {
        paths.push(
            lib.get("path")
                .and_then(|p| p.as_str())
                .unwrap_or("src/lib.rs")
                .to_string(),
        );
    }
    if let Some(bins) = doc.get("bin").and_then(|b| b.as_array()) {
        for bin in bins {
            if let Some(p) = bin.get("path").and_then(|p| p.as_str()) {
                paths.push(p.to_string());
            }
        }
    }

    for rel in paths {
        let dst = root.join(&rel);
        if dst.exists() {
            continue;
        }
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let body = if rel.ends_with("main.rs") {
            "fn main() {}\n"
        } else {
            "// placeholder: makes the emitted manifest resolvable\n"
        };
        std::fs::write(&dst, body).unwrap();
    }
}

// ---------------------------------------------------------------------------
// resolve + lockfile queries
// ---------------------------------------------------------------------------

/// Every `name = version` pair in a `Cargo.lock`, as resolved.
struct CargoLock {
    packages: Vec<(String, String)>,
}

impl CargoLock {
    /// All resolved versions of `name`, deduplicated and sorted. Empty when the
    /// crate is absent from the graph entirely.
    fn versions(&self, name: &str) -> Vec<String> {
        self.packages
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The single resolved version of `name`, or a panic naming what went
    /// wrong: absent (the composition did not produce it) versus split (the
    /// composed units disagree).
    fn sole_version(&self, name: &str, stack: &str) -> String {
        let vs = self.versions(name);
        assert!(
            !vs.is_empty(),
            "{stack}: `{name}` is not in the resolved graph at all — the \
             composition did not produce the crate it exists to produce"
        );
        assert_eq!(
            vs.len(),
            1,
            "{stack}: `{name}` resolved to {} incompatible versions ({}). \
             The co-inlined units disagree; types will not unify across them.",
            vs.len(),
            vs.join(", "),
        );
        vs.into_iter().next().unwrap()
    }
}

fn resolve_cargo(root: &Path, stack: &str) -> CargoLock {
    let out = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(root)
        .output()
        .expect("run cargo generate-lockfile");
    assert!(
        out.status.success(),
        "{stack}: `cargo generate-lockfile` failed — the composition is not \
         satisfiable:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );

    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("read Cargo.lock");
    let doc: toml::Value = toml::from_str(&lock).expect("Cargo.lock parses as TOML");
    let packages = doc
        .get("package")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    Some((
                        p.get("name")?.as_str()?.to_string(),
                        p.get("version")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    CargoLock { packages }
}

/// Resolve a dart project. Returns the package names in `pubspec.lock`.
///
/// Pub's solver admits only one version per package, so there is no
/// split-version case to assert here — the composition claim is that the
/// constraints are jointly satisfiable and both packages survive resolution.
fn resolve_pub(root: &Path, tool: &str, stack: &str) -> BTreeSet<String> {
    let out = Command::new(tool)
        .args(["pub", "get"])
        .current_dir(root)
        .output()
        .expect("run pub get");
    assert!(
        out.status.success(),
        "{stack}: `{tool} pub get` failed — the composition is not \
         satisfiable:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let lock = std::fs::read_to_string(root.join("pubspec.lock")).expect("read pubspec.lock");
    let doc: serde_yaml::Value = serde_yaml::from_str(&lock).expect("pubspec.lock parses as YAML");
    doc.get("packages")
        .and_then(|p| p.as_mapping())
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// reference pins, anchored to what a stack actually resolves
// ---------------------------------------------------------------------------

/// Assert that `unit`'s reference crate compiles against a version of
/// `package` that some real stack actually delivers.
///
/// The offline pin checks compare a unit's declared crates against its
/// reference manifest, so they can only speak about crates the unit declares.
/// A unit that deliberately contributes **no** crates therefore gets no pin
/// checking at all — its reference is anchored to nothing, and it would keep
/// compiling against a version no user receives long after the host moved on.
///
/// Only a resolve can close that: the version arrives transitively, through a
/// host the unit never names. Teaching the offline lint which units are hosts
/// would put unit-specific knowledge in the analyzer, which is exactly what is
/// forbidden — so this assertion lives here, in the fixture, and the lint stays
/// offline and deterministic.
///
/// The reverse direction — a pin in the reference that no stack delivers — is
/// already covered by the reference crates' own `unused_crate_dependencies`
/// lint, which flags any declared dependency no source imports.
fn assert_reference_pins(plugin: &Path, unit: &str, package: &str, resolved: &str, stack: &str) {
    let manifest_path = plugin.join(unit).join("reference/rust/Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let doc: toml::Value = toml::from_str(&text).expect("reference manifest parses as TOML");

    let deps = doc
        .get("dependencies")
        .and_then(|d| d.as_table())
        .unwrap_or_else(|| panic!("{unit}: reference manifest has no [dependencies]"));

    // Collect every requirement on `package`, following `package = "..."`
    // renames — a reference may carry the same crate more than once under
    // different keys when the hosts it stands in for straddle an API break.
    let mut reqs: Vec<String> = Vec::new();
    for (key, val) in deps {
        let (effective, req) = match val {
            toml::Value::String(s) => (key.as_str(), s.clone()),
            toml::Value::Table(t) => {
                let name = t
                    .get("package")
                    .and_then(|p| p.as_str())
                    .unwrap_or(key.as_str());
                match t.get("version").and_then(|v| v.as_str()) {
                    Some(v) => (name, v.to_string()),
                    None => continue, // git/path pin: no version to compare
                }
            }
            _ => continue,
        };
        if effective == package {
            reqs.push(req);
        }
    }

    assert!(
        !reqs.is_empty(),
        "{stack}: `{unit}`'s reference declares no version of `{package}`, but \
         the stack resolves {package} {resolved}. The reference certifies prose \
         about a crate it never compiles against."
    );
    assert!(
        reqs.iter().any(|r| req_admits(r, resolved)),
        "{stack}: `{unit}`'s reference pins {package} {:?}, none of which admit \
         the {resolved} this stack actually resolves. The reference compiles \
         against a version no user receives, so it would stay green through an \
         upstream break.",
        reqs,
    );
}

/// Whether a resolved version satisfies a declared requirement.
///
/// Understands the simple caret requirements these manifests use (`"0.36"`,
/// `"^0.36"`, `"1"`) — for which cargo's compatibility rule is a version-prefix
/// match, and that is what this compares. A range or wildcard requirement would
/// need a real semver parse; none of the reference manifests use one, and a
/// wrong answer here fails loudly rather than passing quietly.
fn req_admits(req: &str, resolved: &str) -> bool {
    let req = req.trim().trim_start_matches(['^', '=', '~']).trim();
    assert!(
        !req.contains([',', '<', '>', '*', ' ']),
        "reference pin `{req}` is not a simple caret requirement; this check \
         compares version prefixes and cannot answer it"
    );
    resolved == req || resolved.starts_with(&format!("{req}."))
}

/// Skip guard shared by every test: these gate the shipped plugin tree, so
/// without it there is nothing to gate.
macro_rules! require_plugin_tree {
    () => {
        if live_plugin_tree().is_none() {
            eprintln!("skip: live plugin tree not present at sibling path");
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// the stacks
// ---------------------------------------------------------------------------

/// desktop: an app profile + a windowing host + the editor-topology block.
///
/// This is the stack that shipped broken. The topology block contributes no UI
/// crate of its own precisely so the host is the sole source of it; if that
/// ever regresses, two copies land here and the emitted `crate::egui` alias
/// silently refers to types from only one of them.
#[test]
#[ignore = "resolves against crates.io; run with --ignored"]
fn desktop_egui_stack_resolves_one_egui() {
    require_plugin_tree!();
    let (plugin, root) = compose(
        "desktop",
        "rust-app",
        "rust",
        "composition-desktop",
        &["eframe", "egui"],
        &[],
    );
    write_manifest(&root);
    let lock = resolve_cargo(&root, "desktop-egui");

    let egui = lock.sole_version("egui", "desktop-egui");
    // The host is what the composition was for; assert it is actually here
    // rather than trusting the exit code.
    let eframe = lock.sole_version("eframe", "desktop-egui");

    // The topology block contributes no crates, so nothing offline can check
    // its reference pins. This is the only place that can: the version it must
    // track arrives transitively, through a host the block never names.
    assert_reference_pins(&plugin, "blocks/egui", "egui", &egui, "desktop-egui");
    assert_reference_pins(&plugin, "blocks/eframe", "eframe", &eframe, "desktop-egui");
}

/// audio plugin: the plugin-framework block + the editor-topology block, joined
/// by their bridge.
///
/// Here the UI crate arrives transitively through the bridge's join-product
/// crate, at a different major line than the desktop stack resolves. Both are
/// correct; what must not happen is both in one graph.
#[test]
#[ignore = "resolves against crates.io and git remotes; run with --ignored"]
fn audio_plugin_stack_resolves_one_egui_through_the_bridge() {
    require_plugin_tree!();
    let (plugin, root) = compose(
        "audio",
        "audio-plugin",
        "rust",
        "composition-audio",
        &["nih-plug", "egui"],
        &["egui+nih-plug"],
    );
    write_manifest(&root);
    let lock = resolve_cargo(&root, "audio-plugin");

    let egui = lock.sole_version("egui", "audio-plugin");
    // The join product is the whole reason the bridge exists.
    lock.sole_version("nih_plug_egui", "audio-plugin");
    lock.sole_version("nih_plug", "audio-plugin");

    // Same block, different host, different major line — and the block's
    // reference stands in for both, so it must carry a pin admitting this one
    // too. Between the two stacks, every host the block claims to serve gets
    // its pin anchored to a version that host actually delivers.
    assert_reference_pins(&plugin, "blocks/egui", "egui", &egui, "audio-plugin");
}

/// firmware: the async-runtime block + the MCU-family block, joined by their
/// bridge.
///
/// The shared surface is the runtime's own crates: the bridge's join-product
/// HAL depends on them too, and types cross that boundary (a timer handle from
/// one is used against peripherals from the other), so a split would be a real
/// break rather than a harmless duplicate.
#[test]
#[ignore = "resolves against crates.io; run with --ignored"]
fn firmware_stack_resolves_one_runtime() {
    require_plugin_tree!();
    let (_plugin, root) = compose(
        "firmware",
        "firmware-layered",
        "rust",
        "composition-firmware",
        &["embassy", "stm32h5-rs"],
        &["embassy+stm32h5-rs"],
    );

    // Pick a concrete part; the profile template ships the example commented
    // out, and the HAL feature it selects is what the bridge's join product
    // resolves against.
    let arch_path = root.join("specs/architecture.toml");
    let arch = std::fs::read_to_string(&arch_path).unwrap();
    let picked = arch.replace("# mcu_part = \"stm32h563\"", "mcu_part = \"stm32h563\"");
    assert_ne!(
        arch, picked,
        "architecture template no longer carries the example mcu_part"
    );
    std::fs::write(&arch_path, picked).unwrap();

    write_manifest(&root);
    let lock = resolve_cargo(&root, "firmware");

    // Declared shared surface only. `embassy-hal-internal` resolves to two
    // versions here and deliberately is NOT asserted: no unit in this tree
    // declares it, the skew is between two upstream-published crates, and it
    // is not actionable from here. See the module docs.
    for crate_name in ["embassy-executor", "embassy-sync", "embassy-time"] {
        lock.sole_version(crate_name, "firmware");
    }
    // The bridge's join product — the HAL the firmware actually runs on.
    lock.sole_version("embassy-stm32", "firmware");
}

/// flutter: the app profile + the FFI-binding block + a platform-services
/// block, with no bridge between them.
///
/// This is the un-bridged co-inlined pair the gate exists for: nothing else in
/// the tree ever resolves these two together.
#[test]
#[ignore = "resolves against the pub registry; run with --ignored"]
fn flutter_stack_resolves_both_packages() {
    require_plugin_tree!();
    if which("flutter").is_none() {
        eprintln!("skip: flutter not on PATH");
        return;
    }
    let (_plugin, root) = compose(
        "flutter",
        "flutter-app",
        "dart",
        "composition_flutter",
        &["flutter-rust-bridge", "universal-ble"],
        &[],
    );
    write_manifest(&root);
    let packages = resolve_pub(&root, "flutter", "flutter");

    for pkg in ["flutter_rust_bridge", "universal_ble"] {
        assert!(
            packages.contains(pkg),
            "flutter: `{pkg}` is not in the resolved graph — the composition \
             did not produce the package it exists to produce. Resolved: {packages:?}"
        );
    }
}

fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .find(|p| p.is_file())
}
