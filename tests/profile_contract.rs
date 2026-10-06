//! The `profile.yaml` contract handshake (#117).
//!
//! A stale analyzer meeting a newer plugin used to report `MANIFEST-001`
//! against the copy of `profile.yaml` that bootstrap had written into the
//! user's own tree, seconds earlier, with no mention of the analyzer.
//! Both sides reported `0.1.0`, so the skew was undetectable.
//!
//! Coverage:
//!   1. A manifest declaring a contract ahead of this binary errors
//!      `MANIFEST-002`, names both versions, and names the remedy.
//!   2. The error arrives BEFORE schema validation — a manifest that is
//!      both ahead *and* full of unknown keys reports the version, not the
//!      keys. This is the ordering the whole fix rests on.
//!   3. An absent `contract_version` means contract 1, so every tree
//!      bootstrapped before the field existed keeps loading.
//!   4. Pairing guard: the embedded schema's hash is pinned to the
//!      contract version it belongs to, so a shape change that forgets the
//!      bump fails here rather than in a user's project.

use std::path::{Path, PathBuf};

use specdev_analyzer::manifest::profile::{Profile, PROFILE_CONTRACT_VERSION};

const SCHEMA_SOURCE: &str = include_str!("../src/embedded/profile.schema.yaml");

/// Every contract version this binary has ever defined, paired with the
/// SHA-256 of the schema text at that version. Append a row when you bump
/// `PROFILE_CONTRACT_VERSION`; never edit an existing row.
const CONTRACT_HASHES: &[(u64, &str)] = &[
    (
        1,
        "sha256:20dbc5501837573057fb229104875a44e7aa8569323e79552e4c770b7a7c56a5",
    ),
    (
        2,
        "sha256:535a048fdc7767668326b44d7d2efcb99f5ff182dab8ea4ac4276b7d726805de",
    ),
];

fn tmp_file(tag: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "specdev-profile-contract-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("profile.yaml");
    std::fs::write(&p, body).unwrap();
    p
}

fn load_err(path: &Path) -> String {
    format!("{:#}", Profile::load(path).unwrap_err())
}

// ---------------------------------------------------------------------------
// 1. A manifest ahead of the binary reports MANIFEST-002 with both numbers.
// ---------------------------------------------------------------------------
#[test]
fn contract_ahead_of_binary_errors_manifest_002() {
    let ahead = PROFILE_CONTRACT_VERSION + 1;
    let p = tmp_file(
        "ahead",
        &format!("name: demo\nlanguage: rust\ncontract_version: {ahead}\n"),
    );
    let msg = load_err(&p);
    assert!(msg.contains("MANIFEST-002"), "got {msg}");
    assert!(
        msg.contains(&format!("contract_version {ahead}")),
        "message must state what the file declares; got {msg}"
    );
    assert!(
        msg.contains(&format!("understands {PROFILE_CONTRACT_VERSION}")),
        "message must state what this binary supports; got {msg}"
    );
    assert!(
        msg.contains("cargo install"),
        "message must name the remedy; got {msg}"
    );
    assert!(
        msg.contains("newer than the analyzer"),
        "message must say which side is stale; got {msg}"
    );
}

// ---------------------------------------------------------------------------
// 2. The version check runs BEFORE the schema validator.
//
//    This is the whole point. A plugin one contract ahead also carries the
//    keys that contract added; validate first and the user reads a list of
//    `additionalProperties` errors about their own file (#117). The guard
//    here is that the version wins even when both faults are present.
// ---------------------------------------------------------------------------
#[test]
fn version_mismatch_is_reported_before_unknown_keys() {
    let ahead = PROFILE_CONTRACT_VERSION + 1;
    let body = format!(
        "name: demo\nlanguage: rust\ncontract_version: {ahead}\nsome_future_key: 1\nanother_future_key: [a, b]\n"
    );
    let p = tmp_file("ahead-and-unknown", &body);
    let msg = load_err(&p);
    assert!(msg.contains("MANIFEST-002"), "got {msg}");
    assert!(
        !msg.contains("MANIFEST-001"),
        "the version answer must replace the key list, not accompany it; got {msg}"
    );
    assert!(
        !msg.contains("some_future_key"),
        "unknown keys are a symptom of the skew, not the diagnosis; got {msg}"
    );
}

// ---------------------------------------------------------------------------
// 3. Absent contract_version means contract 1 (every pre-field project).
// ---------------------------------------------------------------------------
#[test]
fn absent_contract_version_loads_as_contract_one() {
    let p = tmp_file("absent", "name: demo\nlanguage: rust\n");
    let loaded = Profile::load(&p).expect("a manifest with no contract_version must still load");
    assert_eq!(loaded.name, "demo");
}

#[test]
fn contract_equal_to_binary_loads() {
    let p = tmp_file(
        "equal",
        &format!("name: demo\nlanguage: rust\ncontract_version: {PROFILE_CONTRACT_VERSION}\n"),
    );
    Profile::load(&p).expect("a manifest at this binary's contract must load");
}

// ---------------------------------------------------------------------------
// 4. Pairing guard: schema shape and contract version move together.
//
//    Without this, the handshake depends on an author remembering to bump
//    a constant — and #117 exists because two components at one version
//    number is the same as no version number.
// ---------------------------------------------------------------------------
#[test]
fn contract_pairing_guard() {
    let actual = specdev_analyzer::hash::content_hash(SCHEMA_SOURCE.as_bytes());
    let expected = CONTRACT_HASHES
        .iter()
        .find(|(v, _)| *v == PROFILE_CONTRACT_VERSION)
        .map(|(_, h)| *h)
        .unwrap_or_else(|| {
            panic!(
                "PROFILE_CONTRACT_VERSION is {PROFILE_CONTRACT_VERSION} but CONTRACT_HASHES has \
                 no row for it. Append ({PROFILE_CONTRACT_VERSION}, \"{actual}\")."
            )
        });
    assert_eq!(
        actual,
        expected,
        "\n\nsrc/embedded/profile.schema.yaml changed but PROFILE_CONTRACT_VERSION is still \
         {PROFILE_CONTRACT_VERSION}.\n\nIf the change altered the manifest SHAPE (a new key, a \
         widened enum, a new required field), then an older analyzer can no longer read a \
         profile.yaml authored against it — that is #117. Do three things:\n  \
         1. bump PROFILE_CONTRACT_VERSION in src/manifest/profile.rs\n  \
         2. append ({}, \"{actual}\") to CONTRACT_HASHES here\n  \
         3. set contract_version to the new number in every \
         specdev/profiles/*/profile.yaml (plugin-lint enforces this)\n\nIf the change was only \
         prose — a comment or a description — replace this row's hash with:\n  {actual}\n",
        PROFILE_CONTRACT_VERSION + 1
    );
}

// ---------------------------------------------------------------------------
// 5. Contract 2: the emitter sections are gone.
// ---------------------------------------------------------------------------

const RETIRED: [&str; 4] = [
    "cargo_toml",
    "pubspec_yaml",
    "package_json",
    "derived_files",
];

fn plugin_profiles() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for root in [
        manifest.join("../specdev/profiles"),
        manifest.join("tests/specs/full_plugin/_plugin/profiles"),
    ] {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path().join("profile.yaml");
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn plugin_profiles_are_contract_two_without_emitter_sections() {
    let profiles = plugin_profiles();
    assert!(profiles.len() >= 7, "found {profiles:?}");
    for p in profiles {
        let v: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            v.get("contract_version").and_then(|c| c.as_u64()),
            Some(2),
            "{}",
            p.display()
        );
        for k in RETIRED {
            assert!(
                v.get(k).is_none(),
                "{}: `{k}` is an emitter section",
                p.display()
            );
        }
        for g in v
            .get("gates")
            .and_then(|g| g.as_sequence())
            .into_iter()
            .flatten()
        {
            assert!(
                g.get("scope").is_none(),
                "{}: a gate keeps `scope`",
                p.display()
            );
        }
    }
}

#[test]
fn a_contract_two_manifest_with_an_emitter_section_is_refused() {
    let p = tmp_file(
        "two-retired",
        "name: demo\nlanguage: rust\ncontract_version: 2\ncargo_toml:\n  shape: library\n",
    );
    let msg = load_err(&p);
    assert!(
        msg.contains("MANIFEST-001") && msg.contains("cargo_toml"),
        "got {msg}"
    );
}

/// The read-both window: a `1.x` tree's inlined manifest still loads.
#[test]
fn a_contract_one_manifest_with_emitter_sections_still_loads() {
    let p = tmp_file(
        "one-retired",
        "name: demo\nlanguage: rust\ncontract_version: 1\ncargo_toml:\n  shape: library\n  lib: {}\n\
         derived_files: []\ngates:\n  - label: cargo-check\n    scope: module\n    steps:\n      - [cargo, check]\n",
    );
    let profile = Profile::load(&p).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(profile.gates.len(), 1);
    assert_eq!(
        profile.gates[0].steps,
        vec![vec!["cargo".to_string(), "check".to_string()]]
    );
}
