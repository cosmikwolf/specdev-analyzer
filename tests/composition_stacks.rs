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
//! manifests.** Three corollaries, all learned the expensive way:
//!
//! 1. **Assert on the resolved lockfile, not the emitted manifest.** Reading
//!    declared dependency lists is precisely what let the bug above be closed
//!    wrongly — the collision was transitive and no dep list showed it.
//! 2. **A green gate must assert the artifact contains what the build was
//!    for.** An exit code of 0 is not evidence; a build gate elsewhere passed
//!    twice over an artifact that held none of the code it was gating. So each
//!    stack also asserts its join product is present in the lock.
//! 3. **Ask what the resolver's exit code asserts; it is not the same claim
//!    in every language.** `pnpm` exits 0 on an *unmet peer dependency* and
//!    prints a warning, so a stack that mirrors the cargo shape — resolve,
//!    then assert one version per package — goes green over a composition
//!    whose join product is bound to a host version it does not support.
//!    The single-version assertion cannot see it: there is only one version,
//!    and it is the wrong one. Measured, not assumed; `resolve_pnpm` carries
//!    the numbers and the flag that closes it.
//!
//! # Which packages get a single-version assertion
//!
//! Only packages the composed units **declare** — never a name prefix. Two
//! live instances, and they are not the same shape. The firmware stack
//! resolves two copies of an upstream-internal crate inside **one** upstream
//! tree (`embassy-embedded-hal` wants one minor of `embassy-hal-internal`,
//! `embassy-stm32` wants another). The nextjs stack resolves two copies of
//! `postcss` across **two** upstream trees: `next@16.3.4` pulls `8.5.23` and
//! `vite@8.3.0` pulls `8.5.28`, and the profile declares both hosts — so the
//! co-residency is ours even though the skew is not. Neither is actionable
//! from here: no unit declares the split package, and the fix belongs to an
//! upstream release. A prefix glob would fail on both forever, and a gate
//! that fails unfixably gets muted. What matters is whether *our* units
//! agree, so the assertion is scoped to the shared surface they each name.
//!
//! # Shape and cost
//!
//! Linear, one fixture per stack — deliberately not combinatorial over unit
//! pairs. If the set outgrows what CI tolerates, cut whole stacks and say so;
//! do not sample within a stack and report green.
//!
//! Every test here is `#[ignore]`d: resolving touches crates.io, git remotes,
//! the pub registry and the npm registry, and the default suite must not
//! reach any of them. It is not fully hermetic — `tests/manifest_package_json.rs`
//! runs a live `pnpm install` when pnpm is on PATH — but that one is a single
//! registry and skips without the tool, where these resolve four ecosystems
//! and cannot. Run them with `cargo test --test composition_stacks -- --ignored`, or
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
    package_json, pubspec_yaml,
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
///
/// The canonical filename is chosen **inside** the language arm, not by a
/// second branch below it. It was a two-way `if` that named `Cargo.toml` for
/// everything that was not dart, so adding a third language here would have
/// written a package.json to `Cargo.toml` and failed somewhere else entirely.
/// One arm per language, one place to forget nothing.
fn write_manifest(root: &Path) -> String {
    let layout = LayoutContext::discover(root, None).unwrap();
    let language = layout.spec.project.language.clone();

    let (filename, text, extra) = match language.as_str() {
        "rust" => {
            let r = cargo_toml::render(&layout).expect("cargo_toml::render");
            ("Cargo.toml", r.text, r.extra_files)
        }
        "dart" => {
            let r = pubspec_yaml::render(&layout).expect("pubspec_yaml::render");
            ("pubspec.yaml", r.text, Vec::new())
        }
        "typescript" => {
            let r = package_json::render(&layout).expect("package_json::render");
            // `sibling_files` keys its entries by name, not by path; the
            // other two renderers hand back paths. Normalize here so the
            // write loop below stays one loop.
            let extra = r
                .sibling_files
                .into_iter()
                .map(|(name, body)| (PathBuf::from(name), body))
                .collect();
            ("package.json", r.text, extra)
        }
        other => panic!("no manifest emission for language `{other}`"),
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

/// Every `name = version` pair a resolver wrote into a lockfile. One shape for
/// every ecosystem — "how many versions of this did we end up with" is the
/// same question whether cargo or pnpm answered it.
struct ResolvedLock {
    packages: Vec<(String, String)>,
}

impl ResolvedLock {
    /// All resolved versions of `name`, deduplicated and sorted. Empty when the
    /// package is absent from the graph entirely.
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
             composition did not produce the package it exists to produce"
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

fn resolve_cargo(root: &Path, stack: &str) -> ResolvedLock {
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
    ResolvedLock { packages }
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

/// Resolve a TypeScript project with pnpm. Returns every `name@version` the
/// lockfile's `packages:` section records.
///
/// # Why `--strict-peer-dependencies` is not optional here
///
/// pnpm treats an unmet peer dependency as a *warning*. The two blocks in this
/// stack declare no package in common, so everything that could make them
/// disagree is a peer edge — and without the flag every one of those edges is
/// unasserted.
///
/// Measured on pnpm 10.15.0, same composition, only the declared client range
/// changed:
///
/// | `@prisma/client` | flags                        | exit | versions resolved |
/// |------------------|------------------------------|------|-------------------|
/// | `^7.10.0` (ships)| default                      | 0    | one, `7.10.0`     |
/// | `^7.10.0` (ships)| `--strict-peer-dependencies` | 0    | one, `7.10.0`     |
/// | `2.25.0` (broken)| default                      | 0    | one, `2.25.0`     |
/// | `2.25.0` (broken)| `--strict-peer-dependencies` | 1    | —                 |
///
/// Row three is the whole reason this function does not simply mirror
/// `resolve_cargo`. pnpm bound the adapter to a client its peer range rejects,
/// wrote a lockfile, and exited 0 under `WARN  Issues with peer dependencies
/// found`. **`sole_version` cannot see it** — there is exactly one version in
/// the graph, and it is the unsupported one. Only the flag turns that into
/// `ERR_PNPM_PEER_DEP_ISSUES` and exit 1.
///
/// # Which peer edge this actually guards
///
/// Read the table as a **mechanism proof, not a live risk**, and do not repeat
/// the summary it is easy to write from it. The adapter's peer range is
/// `>=2.26.0 || >=3 || >=4 || >=5 || >=6` — **unbounded above**. Every way an
/// author would realistically move the ORM block satisfies it; `2.25.0` is
/// five majors backwards, an edit nobody makes. So the adapter edge is what
/// makes the two units interact, and it is *not* the edge that can fail.
///
/// The edges that can are the auth block's, and both are bounded:
/// `next-auth@5.0.0-beta.32` peers on `next: ^14.0.0-0 || ^15.0.0 || ^16.0.0`
/// and `react: ^18.2.0 || ^19.0.0`, while the profile floats `next ^16.3.0`
/// and `react ^19.2.0`. The day the profile takes Next 17 or React 20 ahead of
/// the auth block, this gate is what says so. Confirmed reachable: dropping
/// the fixture to `react ^17.0.0` gives `ERR_PNPM_PEER_DEP_ISSUES`, exit 1.
///
/// # What the flag does and does not check
///
/// Not every peer edge. The emitted lockfile records `autoInstallPeers: true`,
/// so pnpm silently installs an **absent** peer and reports nothing; only an
/// edge that is present and unsatisfiable fires. That is still broader than
/// this stack's own units — a pair of upstream packages can fail it — which is
/// the shape the module docs warn about, since a gate that fails unfixably
/// gets muted.
///
/// The shipping stack resolves with zero peer warnings today. That is the
/// current distance to the edge, not a guarantee about the next release. If it
/// ever fires on a pair no unit here owns, exempt that one pair with
/// `peerDependencyRules.allowedVersions`; the fixture is a standalone app and
/// writes no `pnpm-workspace.yaml`, so this means teaching `write_manifest` to
/// emit one — measured to work, but it is a change, not a config edit. Do
/// **not** drop the flag: that is the whole gate.
///
/// `pnpm peers check` reads the same finding out of a finished lockfile and is
/// the more direct instrument. It arrives in **pnpm 11.0.0** — bracketed by
/// execution: absent in 10.20.0 (`Command "peers" not found`), present in
/// 11.0.0. The emitted manifests pin 10.15.0, so revisit when that pin moves.
/// Beware measuring this inside the fixture: pnpm honours the pinned
/// `packageManager` and downshifts, so a newer pnpm reports the older one's
/// answer.
///
/// `--lockfile-only` resolves without linking, and fetches no tarballs for
/// registry deps (`downloaded 0` for this stack; a git source does download).
/// It is the pnpm analogue of `cargo generate-lockfile`, making the same
/// claim — the constraints are jointly satisfiable. About 20 s with a warm
/// store; no cold-store figure measured.
fn resolve_pnpm(root: &Path, stack: &str) -> ResolvedLock {
    let out = Command::new("pnpm")
        .args(["install", "--lockfile-only", "--strict-peer-dependencies"])
        .current_dir(root)
        .output()
        .expect("run pnpm install");
    assert!(
        out.status.success(),
        "{stack}: `pnpm install --lockfile-only --strict-peer-dependencies` \
         failed — the composition is not satisfiable. `ERR_PNPM_PEER_DEP_ISSUES` \
         here means one co-inlined unit's package does not accept the version \
         another unit contributes:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let lock = std::fs::read_to_string(root.join("pnpm-lock.yaml")).expect("read pnpm-lock.yaml");
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&lock).expect("pnpm-lock.yaml parses as YAML");
    let packages = doc
        .get("packages")
        .and_then(|p| p.as_mapping())
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str())
                .map(split_name_version)
                .collect()
        })
        .unwrap_or_default();
    ResolvedLock { packages }
}

/// Split a lockfile-v9 `packages:` key into name and version.
///
/// Reads `packages:`, never `snapshots:`. Both sections describe one
/// resolution, but a `snapshots:` key carries the peer set it was resolved
/// against — `@prisma/client@7.10.0(typescript@7.0.2)` — and a single package
/// at a single version appears under several such keys. Counting those would
/// report a split that is not one. `packages:` keys are suffix-free and
/// deduplicated, so the parenthesis check below is an assertion that the right
/// section was read; it is not a parser for the other one.
///
/// The separator is the **first** `@` after the scope, not the last. Both a
/// scope and a version can contain one, at opposite ends: `@prisma/client@…`
/// puts one in front, and a git specifier puts one in the middle —
/// `foo@git+ssh://git@github.com/u/r.git#abc`. Splitting on the last `@` names
/// that package `foo@git+ssh://git`, and the `sole_version` that follows then
/// reports the real package "not in the resolved graph at all", blaming the
/// composition for a parse bug. `NpmSource::Git` takes a free-form URL, so the
/// shape is reachable from any unit's `deps.yaml`.
///
/// One thing this cannot see: a `link:`, `file:` or `workspace:` dependency
/// never reaches `packages:` at all — pnpm records it under `importers:` only.
/// A stack that asserts such a package gets the "absent" panic and the message
/// will be wrong. No stack does today; teach this function about `importers:`
/// before one does.
fn split_name_version(key: &str) -> (String, String) {
    assert!(
        !key.contains('('),
        "pnpm lock key `{key}` carries a peer suffix — that is a `snapshots:` \
         key, and this reads `packages:`"
    );
    // A scoped name is `@scope/pkg`; its own `@` is at index 0 and the
    // separator cannot precede the `/`. An unscoped name has no `@`, so the
    // first one is the separator.
    let search_from = if key.starts_with('@') {
        key.find('/')
            .unwrap_or_else(|| panic!("pnpm lock key `{key}` starts with `@` but names no scope"))
    } else {
        0
    };
    let at = search_from
        + key[search_from..]
            .find('@')
            .filter(|i| search_from + *i > 0)
            .unwrap_or_else(|| panic!("pnpm lock key `{key}` is not `name@version`"));
    (key[..at].to_string(), key[at + 1..].to_string())
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

/// embedded rtic: the concurrency-framework block + the MCU-family block,
/// co-inlined with NO bridge — the pairing's earn-its-keep gate closed the
/// bridge (its would-be binding was not relation-shaped), so nothing else
/// in the tree ever resolves these two together.
///
/// The shared surface is `cortex-m`, declared by the profile's platform
/// pick: types cross it — the PAC's `InterruptNumber` impl is consumed by
/// rtic's NVIC handling — so a split would be a real break, not a
/// harmless duplicate.
#[test]
#[ignore = "resolves against crates.io; run with --ignored"]
fn rtic_mk20dx_stack_resolves_one_cortex_m() {
    require_plugin_tree!();
    let (_plugin, root) = compose(
        "rtic-mk20dx",
        "firmware-layered",
        "rust",
        "composition-rtic-mk20dx",
        &["rtic", "mk20dx"],
        &[],
    );
    write_manifest(&root);
    let lock = resolve_cargo(&root, "rtic-mk20dx");

    // Every crate the composed units declare, plus the profile's shared
    // platform surface. The chip's PACs arrive transitively through the
    // HAL's part feature and are deliberately NOT asserted by name here —
    // which PAC resolves is the HAL's business.
    for crate_name in ["rtic", "mk20dx-hal", "fugit", "cortex-m"] {
        lock.sole_version(crate_name, "rtic-mk20dx");
    }
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

/// nextjs: the app profile + an ORM block + an auth block, joined by their
/// bridge.
///
/// The one stack whose shared surface no unit declares twice. The ORM block
/// contributes the database client; the auth block contributes the auth
/// framework; between them they name nothing in common, so "do the two units
/// agree on a shared package" — the question the other four stacks ask — is
/// vacuously true here and answers nothing.
///
/// What ties them is the bridge's join product: `@auth/prisma-adapter` names
/// the client as a **peer**, not a dependency. So this stack's primary
/// assertion is `resolve_pnpm`'s `--strict-peer-dependencies`, and the
/// single-version checks below are the secondary claim. Which peer edge is
/// load-bearing is not obvious and is not the adapter's — `resolve_pnpm`
/// names the ones that can actually fail.
///
/// `postcss` resolves to two versions here and is deliberately NOT asserted:
/// no unit in this tree declares it, and the two copies come from two
/// different upstream projects (`next` and `vite`). Not actionable from here;
/// see the module docs.
#[test]
#[ignore = "resolves against the npm registry; run with --ignored"]
fn nextjs_stack_resolves_one_client_under_the_bridge_adapter_peer() {
    require_plugin_tree!();
    if which("pnpm").is_none() {
        eprintln!("skip: pnpm not on PATH");
        return;
    }
    let (_plugin, root) = compose(
        "nextjs",
        "nextjs-app",
        "typescript",
        "composition-nextjs",
        &["prisma", "next-auth"],
        &["next-auth+prisma"],
    );
    write_manifest(&root);
    let lock = resolve_pnpm(&root, "nextjs");

    // Everything the composed units declare, plus the profile's own shared
    // surface — the same rule the rtic stack applies to `cortex-m`. The
    // client is where the units meet: the ORM block contributes it and the
    // join product peers on it, so a split would mean the adapter typing
    // against one client and the application code against another. `react`
    // is the npm analogue and a real break class of its own: two copies in
    // one graph break hooks at runtime, not at type-check.
    for pkg in ["@prisma/client", "next-auth", "next", "react", "react-dom"] {
        lock.sole_version(pkg, "nextjs");
    }
    // The join product, which is the whole reason the bridge exists.
    lock.sole_version("@auth/prisma-adapter", "nextjs");
}

fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .find(|p| p.is_file())
}
