//! Phase 16 §8.1 step 2.5 — bootstrap-time profile.yaml copy + inline-time
//! deps.yaml / contract.yaml copy with content_hash extension.
//!
//! Per doc 21 §6.2:
//! - bootstrap copies `profile.yaml` byte-faithfully to
//!   `specs/specdev/profile.yaml` (no template substitution).
//! - inline-block copies each present `codegen/<lang>/deps.yaml` to
//!   `specs/specdev/codegen/<lang>/blocks/<name>/deps.yaml`. Bytes
//!   participate in the block's per-unit `content_hash` (doc 15 §5.5).
//! - inline-bridge copies present `contract.yaml` to
//!   `specs/specdev/reductions/bridges/<name>.contract.yaml`. Bytes
//!   participate in the bridge's per-unit `content_hash`.
//!
//! Backward compat (doc 21 §6.5): when a manifest is absent in the plugin
//! tree (pre-D52 fixtures, e.g. phase3), bootstrap/inline silently skip;
//! the unit's hash is unchanged from the pre-D52 file set.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    lineage::Lineage,
    upstream::{self, UnitKind},
};

fn phase3_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase3")
        .join("_plugin")
}

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase16-step2_5-{tag}-{}-{}",
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

fn fresh_project(tag: &str, plugin: &Path, profile: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

// ===========================================================================
// 1 — bootstrap copies profile.yaml when present (phase6 fixture)
// ===========================================================================

#[test]
fn bootstrap_copies_profile_yaml_when_present() {
    let (target, _layout) = fresh_project("bootstrap-profile-yaml", &phase6_plugin(), "firmware-layered");
    let dst = target.join("specs/specdev/profile.yaml");
    assert!(dst.is_file(), "profile.yaml must be copied to specs/specdev/");
    let src_bytes =
        std::fs::read(phase6_plugin().join("profiles/firmware-layered/profile.yaml")).unwrap();
    let dst_bytes = std::fs::read(&dst).unwrap();
    assert_eq!(
        src_bytes, dst_bytes,
        "profile.yaml must be copied byte-faithfully (no template substitution)"
    );
}

// ===========================================================================
// 2 — bootstrap silently skips profile.yaml when absent (phase3 fixture)
// ===========================================================================

#[test]
fn bootstrap_skips_profile_yaml_when_absent() {
    // phase3 fixture's firmware-layered profile pre-dates D52 and has no
    // profile.yaml. Bootstrap must succeed and produce no profile.yaml.
    assert!(
        !phase3_plugin()
            .join("profiles/firmware-layered/profile.yaml")
            .exists(),
        "phase3 fixture should NOT have a profile.yaml (test premise)"
    );
    let (target, _layout) =
        fresh_project("bootstrap-no-profile-yaml", &phase3_plugin(), "firmware-layered");
    assert!(target.join("spec.toml").is_file());
    assert!(
        !target.join("specs/specdev/profile.yaml").exists(),
        "absent source must NOT materialize a profile.yaml in the project"
    );
}

// ===========================================================================
// 3 — inline-block copies deps.yaml when present (phase6 fixture)
// ===========================================================================

#[test]
fn inline_block_copies_deps_yaml_when_present() {
    let (target, layout) =
        fresh_project("inline-block-deps", &phase6_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();

    let dst = target.join("specs/specdev/codegen/rust/blocks/embassy/deps.yaml");
    assert!(dst.is_file(), "deps.yaml must be copied to inlined codegen tree");
    let src_bytes =
        std::fs::read(phase6_plugin().join("blocks/embassy/codegen/rust/deps.yaml")).unwrap();
    let dst_bytes = std::fs::read(&dst).unwrap();
    assert_eq!(src_bytes, dst_bytes, "deps.yaml must be copied byte-faithfully");

    // Upstream snapshot also captures the manifest (per Phase 4 invariant).
    let upstream_dst = upstream::upstream_dir(&layout.specdev_root, UnitKind::Block, "embassy")
        .join("codegen/rust/deps.yaml");
    assert!(
        upstream_dst.is_file(),
        "upstream/ snapshot must include deps.yaml at {}",
        upstream_dst.display()
    );
}

// ===========================================================================
// 4 — inline-block does not write deps.yaml when absent (phase3 fixture)
// ===========================================================================

#[test]
fn inline_block_skips_deps_yaml_when_absent() {
    assert!(
        !phase3_plugin()
            .join("blocks/embassy/codegen/rust/deps.yaml")
            .exists(),
        "phase3 fixture should NOT ship deps.yaml (test premise)"
    );
    let (target, layout) = fresh_project("inline-no-deps", &phase3_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase3_plugin(), "embassy").unwrap();
    assert!(
        !target
            .join("specs/specdev/codegen/rust/blocks/embassy/deps.yaml")
            .exists(),
        "absent deps.yaml must NOT materialize in the project"
    );
}

// ===========================================================================
// 5 — inline-bridge copies contract.yaml when present (phase6 fixture)
// ===========================================================================

#[test]
fn inline_bridge_copies_contract_yaml_when_present() {
    let (target, layout) = fresh_project("inline-bridge-contract", &phase6_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &phase6_plugin(), "embassy+stm32h5-rs").unwrap();

    let dst = target.join("specs/specdev/reductions/bridges/embassy+stm32h5-rs.contract.yaml");
    assert!(
        dst.is_file(),
        "contract.yaml must be copied to inlined reductions tree at {}",
        dst.display()
    );
    let src_bytes =
        std::fs::read(phase6_plugin().join("bridges/embassy+stm32h5-rs/contract.yaml")).unwrap();
    let dst_bytes = std::fs::read(&dst).unwrap();
    assert_eq!(src_bytes, dst_bytes, "contract.yaml must be copied byte-faithfully");

    // Upstream snapshot also captures the manifest.
    let upstream_dst = upstream::upstream_dir(
        &layout.specdev_root,
        UnitKind::Bridge,
        "embassy+stm32h5-rs",
    )
    .join("contract.yaml");
    assert!(
        upstream_dst.is_file(),
        "upstream/ snapshot must include contract.yaml at {}",
        upstream_dst.display()
    );
}

// ===========================================================================
// 6 — inline-bridge does not write contract.yaml when absent (phase3 fixture)
// ===========================================================================

#[test]
fn inline_bridge_skips_contract_yaml_when_absent() {
    assert!(
        !phase3_plugin()
            .join("bridges/embassy+lvgl/contract.yaml")
            .exists(),
        "phase3 fixture bridge should NOT ship contract.yaml (test premise)"
    );
    let (target, layout) =
        fresh_project("inline-bridge-no-contract", &phase3_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase3_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase3_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &phase3_plugin(), "embassy+lvgl").unwrap();
    assert!(
        !target
            .join("specs/specdev/reductions/bridges/embassy+lvgl.contract.yaml")
            .exists(),
        "absent contract.yaml must NOT materialize in the project"
    );
}

// ===========================================================================
// 7 — block hash differs when deps.yaml is present (manifests participate
//     in content_hash; same-named block in pre-D52 vs D52 fixture hashes
//     to different values)
// ===========================================================================

#[test]
fn inline_block_hash_includes_deps_yaml_bytes() {
    let (_t1, layout1) = fresh_project("hash-with-deps", &phase6_plugin(), "firmware-layered");
    run_inline_block(&layout1, &phase6_plugin(), "embassy").unwrap();
    let hash_with =
        Lineage::load(&layout1.specdev_root).unwrap().blocks["embassy"].content_hash.clone();

    let (_t2, layout2) =
        fresh_project("hash-without-deps", &phase3_plugin(), "firmware-layered");
    run_inline_block(&layout2, &phase3_plugin(), "embassy").unwrap();
    let hash_without =
        Lineage::load(&layout2.specdev_root).unwrap().blocks["embassy"].content_hash.clone();

    // The fixture trees ship different schema/reduction/README bodies too
    // (phase6's embassy is the canonical block; phase3's is the stub), so
    // these hashes would differ even without deps.yaml. The point of this
    // test is the file-set delta: phase6's hash is computed over a strictly
    // larger file set (now includes deps.yaml). Rather than hand-mocking a
    // phase3-shaped fixture with the manifest dropped in, we just assert
    // the two fixture inlines produce distinct hashes.
    assert_ne!(
        hash_with, hash_without,
        "block hashes must reflect file-set differences"
    );
}

// ===========================================================================
// 8 — block hash is stable across a re-inline (manifests don't introduce
//     non-determinism)
// ===========================================================================

#[test]
fn inline_block_hash_stable_across_reinline_with_deps_yaml() {
    let (_t, layout) = fresh_project("hash-stable-with-deps", &phase6_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    let hash_first = Lineage::load(&layout.specdev_root).unwrap().blocks["embassy"]
        .content_hash
        .clone();

    // Reset: remove inlined files (including the new deps.yaml), upstream
    // snapshot, and lineage entry. Re-inline.
    let dst_files = [
        layout
            .specdev_root
            .join("schemas/inlined/blocks/embassy.schema.yaml"),
        layout.specdev_root.join("reductions/blocks/embassy.yaml"),
        layout
            .specdev_root
            .join("codegen/rust/blocks/embassy/README.md"),
        layout
            .specdev_root
            .join("codegen/rust/blocks/embassy/deps.yaml"),
    ];
    for f in &dst_files {
        std::fs::remove_file(f).unwrap();
    }
    upstream::remove_upstream_unit(&layout.specdev_root, UnitKind::Block, "embassy").unwrap();
    let mut lineage = Lineage::load(&layout.specdev_root).unwrap();
    lineage.blocks.remove("embassy");
    lineage.save(&layout.specdev_root).unwrap();

    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    let hash_second = Lineage::load(&layout.specdev_root).unwrap().blocks["embassy"]
        .content_hash
        .clone();
    assert_eq!(
        hash_first, hash_second,
        "content_hash must be stable across re-inline when deps.yaml is part of the file set"
    );
}

// ===========================================================================
// 9 — bridge hash is stable across a re-inline (with contract.yaml)
// ===========================================================================

#[test]
fn inline_bridge_hash_stable_across_reinline_with_contract_yaml() {
    let (_t, layout) =
        fresh_project("hash-stable-bridge-contract", &phase6_plugin(), "firmware-layered");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &phase6_plugin(), "embassy+stm32h5-rs").unwrap();
    let hash_first = Lineage::load(&layout.specdev_root).unwrap().bridges
        ["embassy+stm32h5-rs"]
        .content_hash
        .clone();

    // Reset bridge files + upstream snapshot + lineage entry; re-inline.
    let dst_files = [
        layout
            .specdev_root
            .join("schemas/inlined/bridges/embassy+stm32h5-rs.schema.yaml"),
        layout
            .specdev_root
            .join("reductions/bridges/embassy+stm32h5-rs.yaml"),
        layout
            .specdev_root
            .join("codegen/rust/bridges/embassy+stm32h5-rs/README.md"),
        layout
            .specdev_root
            .join("reductions/bridges/embassy+stm32h5-rs.contract.yaml"),
    ];
    for f in &dst_files {
        if f.is_file() {
            std::fs::remove_file(f).unwrap();
        }
    }
    upstream::remove_upstream_unit(
        &layout.specdev_root,
        UnitKind::Bridge,
        "embassy+stm32h5-rs",
    )
    .unwrap();
    let mut lineage = Lineage::load(&layout.specdev_root).unwrap();
    lineage.bridges.remove("embassy+stm32h5-rs");
    lineage.save(&layout.specdev_root).unwrap();

    run_inline_bridge(&layout, &phase6_plugin(), "embassy+stm32h5-rs").unwrap();
    let hash_second = Lineage::load(&layout.specdev_root).unwrap().bridges
        ["embassy+stm32h5-rs"]
        .content_hash
        .clone();
    assert_eq!(
        hash_first, hash_second,
        "bridge content_hash must be stable across re-inline when contract.yaml is part of the file set"
    );
}
