//! `items apply` Stage-3 mutation.
//!
//! Coverage:
//!   * Approval gate: refuses unapproved / pending-action proposals.
//!   * `add-to-spec` appends a new [[items]] entry.
//!   * `remove-from-spec` removes the matching entry; preserves
//!     surrounding TOML (comments + non-items keys).
//!   * `update-spec` replaces the matching entry.
//!   * `skip` is a no-op.
//!   * Audit log lands at `<upstream>/items-sync/<module>.applied.yaml`.
//!   * Proposal file is removed after successful apply.
//!   * Module spec read-back validates clean (validate stays green).

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
        "specdev-items-apply-{tag}-{}-{}",
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

fn bootstrap_lib(target: &Path) {
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.to_path_buf(),
        },
    )
    .unwrap();
}

struct CliResult {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_apply(project: &Path, module: &str) -> CliResult {
    let out = Command::new(analyzer_bin())
        .arg("items")
        .arg("apply")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg(module)
        .output()
        .expect("invoke items apply");
    CliResult {
        success: out.status.success(),
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn run_diff(project: &Path, module: &str) -> CliResult {
    let out = Command::new(analyzer_bin())
        .arg("items")
        .arg("diff")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg(module)
        .output()
        .expect("invoke items diff");
    CliResult {
        success: out.status.success(),
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn run_propose(project: &Path, module: &str) -> CliResult {
    let out = Command::new(analyzer_bin())
        .arg("items")
        .arg("propose")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg(module)
        .output()
        .expect("invoke items propose");
    CliResult {
        success: out.status.success(),
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn proposal_path(target: &Path, module: &str) -> PathBuf {
    target
        .join("specs/specdev/upstream/items-sync")
        .join(format!("{module}.proposal.yaml"))
}

fn audit_path(target: &Path, module: &str) -> PathBuf {
    target
        .join("specs/specdev/upstream/items-sync")
        .join(format!("{module}.applied.yaml"))
}

/// Mutate an existing proposal file, setting the top-level approval
/// fields and walking entries to apply (action, rationale,
/// proposed_spec) per `entry_recs` keyed by entry id.
fn approve_and_set_actions(
    path: &Path,
    entry_recs: &[(&str, &str, Option<&str>, Option<serde_yaml::Value>)],
) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let map = doc.as_mapping_mut().unwrap();
    map.insert("approved".into(), serde_yaml::Value::Bool(true));
    map.insert(
        "approver".into(),
        serde_yaml::Value::String("test@example.com".into()),
    );
    map.insert(
        "approved_at".into(),
        serde_yaml::Value::String("2026-05-03T00:00:00Z".into()),
    );
    let entries = map
        .get_mut(serde_yaml::Value::String("entries".into()))
        .unwrap()
        .as_sequence_mut()
        .unwrap();
    for entry in entries.iter_mut() {
        let em = entry.as_mapping_mut().unwrap();
        let id = em
            .get(serde_yaml::Value::String("id".into()))
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        for (target_id, action, rationale, proposed) in entry_recs {
            if id == *target_id {
                let rec = em
                    .get_mut(serde_yaml::Value::String("recommendation".into()))
                    .unwrap()
                    .as_mapping_mut()
                    .unwrap();
                rec.insert("action".into(), serde_yaml::Value::String((*action).into()));
                if let Some(r) = rationale {
                    rec.insert("rationale".into(), serde_yaml::Value::String((*r).into()));
                }
                if let Some(p) = proposed {
                    rec.insert("proposed_spec".into(), p.clone());
                }
            }
        }
    }
    std::fs::write(path, serde_yaml::to_string(&doc).unwrap()).unwrap();
}

/// Set `recommendation.rename_to` on the entry with `entry_id`. The
/// `rename` action carries the new name here; `approve_and_set_actions`
/// (which only knows action/rationale/proposed_spec) doesn't model it.
fn set_rename_to(path: &Path, entry_id: &str, new_name: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let entries = doc
        .as_mapping_mut()
        .unwrap()
        .get_mut(serde_yaml::Value::String("entries".into()))
        .unwrap()
        .as_sequence_mut()
        .unwrap();
    for entry in entries.iter_mut() {
        let em = entry.as_mapping_mut().unwrap();
        let id = em
            .get(serde_yaml::Value::String("id".into()))
            .and_then(|v| v.as_str())
            .unwrap();
        if id == entry_id {
            let rec = em
                .get_mut(serde_yaml::Value::String("recommendation".into()))
                .unwrap()
                .as_mapping_mut()
                .unwrap();
            rec.insert(
                "rename_to".into(),
                serde_yaml::Value::String(new_name.into()),
            );
        }
    }
    std::fs::write(path, serde_yaml::to_string(&doc).unwrap()).unwrap();
}

fn yaml_map(pairs: &[(&str, serde_yaml::Value)]) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    for (k, v) in pairs {
        m.insert(serde_yaml::Value::String((*k).into()), v.clone());
    }
    serde_yaml::Value::Mapping(m)
}

#[test]
fn apply_refuses_unapproved_proposal() {
    let target = tmp_dir("unapproved");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "x."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    let prop = run_propose(&target, "Foo");
    assert_eq!(prop.exit_code, Some(1));

    let r = run_apply(&target, "Foo");
    assert_eq!(
        r.exit_code,
        Some(1),
        "expected refusal; stderr={}",
        r.stderr
    );
    assert!(r.stderr.contains("ITEM-302"), "stderr={}", r.stderr);
}

#[test]
fn apply_refuses_pending_action() {
    let target = tmp_dir("pending");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "x."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    run_propose(&target, "Foo");

    // Approve top-level but leave actions on `pending`.
    let path = proposal_path(&target, "Foo");
    approve_and_set_actions(&path, &[]);

    let r = run_apply(&target, "Foo");
    assert_eq!(r.exit_code, Some(1));
    assert!(r.stderr.contains("ITEM-305"), "stderr={}", r.stderr);
}

#[test]
fn apply_remove_drops_matching_items_entry_and_writes_audit() {
    let target = tmp_dir("remove");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    std::fs::write(
        &module_path,
        r#"# Hand-authored module — comment must survive apply.
name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "Stale entry to remove."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    // No source — `ghost` is in_spec_only.
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/foo.rs"), "// empty\n").unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    approve_and_set_actions(
        &path,
        &[(
            "spec-only-0",
            "remove-from-spec",
            Some("Confirmed unused; removing."),
            None,
        )],
    );

    let r = run_apply(&target, "Foo");
    assert!(r.success, "apply failed; stderr={}", r.stderr);

    let new_text = std::fs::read_to_string(&module_path).unwrap();
    assert!(
        new_text.contains("# Hand-authored module"),
        "comment lost; got\n{new_text}"
    );
    assert!(
        !new_text.contains("ghost"),
        "ghost not removed; got\n{new_text}"
    );

    let audit = std::fs::read_to_string(audit_path(&target, "Foo")).unwrap();
    assert!(audit.contains("status: applied"));
    assert!(audit.contains("approver: test@example.com"));
    assert!(audit.contains("applied_at:"));

    // Proposal file is gone after apply.
    assert!(!path.exists(), "proposal should be removed after apply");
}

#[test]
fn apply_add_appends_new_items_entry() {
    let target = tmp_dir("add");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    std::fs::write(
        &module_path,
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render() -> u32 { 0 }\npub fn helper() -> u32 { 1 }\n",
    )
    .unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    let proposed = yaml_map(&[
        ("kind", serde_yaml::Value::String("fn".into())),
        ("name", serde_yaml::Value::String("helper".into())),
        (
            "description",
            serde_yaml::Value::String("Public helper used downstream.".into()),
        ),
        (
            "satisfies",
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("DEMO-2".into())]),
        ),
    ]);
    approve_and_set_actions(
        &path,
        &[(
            "source-only-0",
            "add-to-spec",
            Some("Contractual helper; documenting."),
            Some(proposed),
        )],
    );

    let r = run_apply(&target, "Foo");
    assert!(
        r.success,
        "apply failed; stderr={}\nstdout={}",
        r.stderr, r.stdout
    );
    let new_text = std::fs::read_to_string(&module_path).unwrap();
    assert!(
        new_text.contains("name = \"helper\""),
        "new entry not found:\n{new_text}"
    );
    assert!(new_text.contains("DEMO-2"));
    // Pinned key order: kind first.
    let helper_block = new_text
        .split("[[items]]")
        .find(|s| s.contains("helper"))
        .expect("helper block");
    let kind_pos = helper_block.find("kind = ").unwrap();
    let name_pos = helper_block.find("name = ").unwrap();
    let desc_pos = helper_block.find("description = ").unwrap();
    assert!(
        kind_pos < name_pos && name_pos < desc_pos,
        "key order wrong: {helper_block}"
    );
}

#[test]
fn apply_update_replaces_matching_items_entry() {
    let target = tmp_dir("update");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    std::fs::write(
        &module_path,
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Old description."
satisfies = ["DEMO-1"]
signature_hint = "fn render(x: u64) -> u32"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render(x: u32) -> u32 { x }\n",
    )
    .unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    let proposed = yaml_map(&[
        ("kind", serde_yaml::Value::String("fn".into())),
        ("name", serde_yaml::Value::String("render".into())),
        (
            "description",
            serde_yaml::Value::String("Render the frame.".into()),
        ),
        (
            "satisfies",
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("DEMO-1".into())]),
        ),
        (
            "signature_hint",
            serde_yaml::Value::String("fn render(x: u32) -> u32".into()),
        ),
    ]);
    approve_and_set_actions(
        &path,
        &[(
            "drift-0",
            "update-spec",
            Some("Source signature is correct; spec hint was stale."),
            Some(proposed),
        )],
    );

    let r = run_apply(&target, "Foo");
    assert!(r.success, "apply failed; stderr={}", r.stderr);
    let new_text = std::fs::read_to_string(&module_path).unwrap();
    assert!(new_text.contains("Render the frame."));
    assert!(!new_text.contains("Old description."));
    assert!(new_text.contains("fn render(x: u32) -> u32"));
}

#[test]
fn apply_rename_preserves_trace_and_reconciles() {
    // F-011: `refill` was renamed to `apply_refill` in code. A `rename`
    // action rewrites only the spec item's name, preserving its DEMO-1
    // `satisfies` trace + description — and the next diff then pairs the
    // renamed item with the source symbol, no durable skip needed.
    let target = tmp_dir("rename");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    std::fs::write(
        &module_path,
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "refill"
kind = "fn"
description = "Refill the bucket against the clock."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn apply_refill() -> u32 { 0 }\n",
    )
    .unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    // spec-only-0 = old name `refill`; source-only-0 = new name `apply_refill`.
    approve_and_set_actions(
        &path,
        &[
            (
                "spec-only-0",
                "rename",
                Some("Renamed refill -> apply_refill in code; preserving DEMO-1 trace."),
                None,
            ),
            (
                "source-only-0",
                "skip",
                Some("Accounted for by the paired rename."),
                None,
            ),
        ],
    );
    set_rename_to(&path, "spec-only-0", "apply_refill");

    let r = run_apply(&target, "Foo");
    assert!(
        r.success,
        "apply failed; stderr={}\nstdout={}",
        r.stderr, r.stdout
    );
    assert!(
        r.stdout.contains("renamed: 1"),
        "expected renamed count in report; stdout=\n{}",
        r.stdout
    );

    let new_text = std::fs::read_to_string(&module_path).unwrap();
    assert!(
        new_text.contains("name = \"apply_refill\""),
        "new name missing:\n{new_text}"
    );
    assert!(
        !new_text.contains("name = \"refill\""),
        "old name still present:\n{new_text}"
    );
    // The trace and description survive the rename — the whole point of F-011.
    assert!(
        new_text.contains("DEMO-1"),
        "satisfies trace lost:\n{new_text}"
    );
    assert!(
        new_text.contains("Refill the bucket against the clock."),
        "description lost:\n{new_text}"
    );

    // The rename reconciles without a durable skip: re-diffing now pairs the
    // renamed spec item with the source `apply_refill` → clean exit 0.
    let d = run_diff(&target, "Foo");
    assert!(
        d.success,
        "expected clean diff after rename; exit={:?} stdout=\n{}",
        d.exit_code, d.stdout
    );
}

#[test]
fn apply_rename_without_rename_to_errors_item_315() {
    let target = tmp_dir("rename-missing");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    std::fs::write(
        &module_path,
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "refill"
kind = "fn"
description = "x."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn apply_refill() -> u32 { 0 }\n",
    )
    .unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    // rename action but no `rename_to` set → ITEM-315.
    approve_and_set_actions(
        &path,
        &[
            (
                "spec-only-0",
                "rename",
                Some("oops, forgot the new name"),
                None,
            ),
            ("source-only-0", "skip", Some("paired"), None),
        ],
    );

    let r = run_apply(&target, "Foo");
    assert_eq!(
        r.exit_code,
        Some(1),
        "expected refusal; stderr={}",
        r.stderr
    );
    assert!(r.stderr.contains("ITEM-315"), "stderr={}", r.stderr);
}

#[test]
fn apply_skip_is_a_noop() {
    let target = tmp_dir("skip");
    bootstrap_lib(&target);
    let module_path = target.join("specs/modules/Foo.toml");
    let original = r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render."
satisfies = ["DEMO-1"]
"#;
    std::fs::write(&module_path, original).unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render() -> u32 { 0 }\npub fn helper() -> u32 { 1 }\n",
    )
    .unwrap();

    run_propose(&target, "Foo");
    let path = proposal_path(&target, "Foo");
    approve_and_set_actions(
        &path,
        &[(
            "source-only-0",
            "skip",
            Some("Internal helper; spec stays clean."),
            None,
        )],
    );

    let r = run_apply(&target, "Foo");
    assert!(r.success, "apply failed; stderr={}", r.stderr);
    let new_text = std::fs::read_to_string(&module_path).unwrap();
    assert_eq!(new_text, original, "skip should leave file untouched");
    assert!(audit_path(&target, "Foo").exists());
}
