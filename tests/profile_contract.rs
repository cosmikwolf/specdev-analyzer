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
const CONTRACT_HASHES: &[(u64, &str)] = &[(
    1,
    "sha256:20dbc5501837573057fb229104875a44e7aa8569323e79552e4c770b7a7c56a5",
)];

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
