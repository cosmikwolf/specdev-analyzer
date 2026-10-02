//! Migration fixes found by review: what each test holds fixed.
//!
//! 1. A tombstone is the whole text `migrate` writes; entries appended to
//!    one (a merge) are legacy content, reported and folded.
//! 2. A workspace whose root and members are at different majors recovers:
//!    `migrate` at the root moves every 1.x member and folds every member.
//! 3. `bootstrap` under a workspace of another major refuses.
//! 4. A recorded profile file that is missing is not written again.
//! 5. The removal report is complete, and the warning block says to
//!    commit first.
//! 6. `gate --emit` refuses in a 1.x tree.

#[path = "common/checked_intent.rs"]
mod ci;

use std::path::Path;

use ci::{text, tree, tree_v1, Tree};

const DEC_900: &str = "[[decision]]\nid = \"DEC-900\"\ntitle = \"Merged in\"\nstatus = \"accepted\"\ntext = \"From an old branch.\"\n";

fn migrate(t: &Tree, extra: &[&str]) -> std::process::Output {
    let plugin = t.plugin.to_str().unwrap().to_string();
    let mut args = vec!["migrate", "--plugin-root", plugin.as_str()];
    args.extend_from_slice(extra);
    t.analyzer(&args)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn frozen() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_trees/library-api")
}

#[test]
fn entries_appended_to_a_tombstone_are_reported_and_folded() {
    let t = tree_v1("tomb-append", "library-api");
    assert!(migrate(&t, &[]).status.success());
    let tomb = t.read("specs/decisions.toml");
    assert!(specdev_analyzer::entries::is_tombstone(&tomb));
    t.write("specs/decisions.toml", &format!("{tomb}{DEC_900}"));
    assert!(!specdev_analyzer::entries::is_tombstone(&t.read("specs/decisions.toml")));
    let v = t.analyzer(&["validate"]);
    assert!(text(&v).contains("ENTRY-LEGACY-001"), "{}", text(&v));
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(t.read("specs/decisions/DEC-900.toml"), DEC_900);
    assert!(specdev_analyzer::entries::is_tombstone(&t.read("specs/decisions.toml")));
    assert_eq!(t.read("specs/decisions.toml"), tomb, "the tombstone is rewritten as it was");
}

/// A 2.0 root with a 1.x member (an interrupted run, or a 1.x crate added).
#[test]
fn a_2x_root_moves_its_1x_member() {
    let root = tree("mixed-2-1", "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root.read("spec.toml").replace(
            "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
            "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n",
        ),
    );
    copy_dir(&frozen(), &root.root.join("crates/a"));
    let a = Tree { root: root.root.join("crates/a"), plugin: root.plugin.clone() };
    assert!(text(&a.analyzer(&["validate"])).contains("FORMAT-002"), "the member refuses first");
    let out = migrate(&root, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(a.read("spec.toml").starts_with("spec_version = \"2.0\""), "{}", text(&out));
    let v = a.analyzer(&["validate", "-q"]);
    assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "{}", text(&v));
}

/// A 1.x root whose member is already 2.0: the root still moves.
#[test]
fn a_1x_root_with_a_2x_member_moves() {
    let root = tree("mixed-1-2", "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root
            .read("spec.toml")
            .replacen("spec_version = \"2.0\"", "spec_version = \"1.0\"", 1)
            .replace(
                "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
                "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n",
            ),
    );
    let red = "specs/specdev/reductions/project.yaml";
    root.write(
        red,
        &root
            .read(red)
            .replace("\"specs/decisions/*.toml\"", "\"specs/decisions.toml\"")
            .replace("\"specs/open_questions/*.toml\"", "\"specs/open_questions.toml\""),
    );
    copy_dir(&frozen(), &root.root.join("crates/a"));
    let a = Tree { root: root.root.join("crates/a"), plugin: root.plugin.clone() };
    assert!(migrate(&a, &[]).status.code() == Some(1), "a member refuses migrate");
    // Simulate the interrupted run: the member was written, the root not.
    let plugin = root.plugin.to_str().unwrap().to_string();
    let solo = a.root.join("spec.toml");
    let body = a.read("spec.toml");
    // Move the member alone by hiding it from the root, then restore.
    root.write("spec.toml", &root.read("spec.toml").replace("path = \"crates/a\"", "path = \"crates/hidden\""));
    let o = a.analyzer(&["migrate", "--plugin-root", plugin.as_str()]);
    assert!(o.status.success(), "{}", text(&o));
    root.write("spec.toml", &root.read("spec.toml").replace("path = \"crates/hidden\"", "path = \"crates/a\""));
    assert!(std::fs::read_to_string(&solo).unwrap() != body, "member moved to 2.0");
    let out = migrate(&root, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(root.read("spec.toml").starts_with("spec_version = \"2.0\""), "{}", text(&out));
}

#[test]
fn a_2x_root_folds_legacy_content_in_its_members() {
    let root = tree("ws-fold", "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root.read("spec.toml").replace(
            "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
            "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n",
        ),
    );
    copy_dir(&frozen(), &root.root.join("crates/a"));
    assert!(migrate(&root, &[]).status.success());
    let a = Tree { root: root.root.join("crates/a"), plugin: root.plugin.clone() };
    a.write("specs/decisions.toml", DEC_900);
    let out = migrate(&root, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(a.read("specs/decisions/DEC-900.toml"), DEC_900);
}

#[test]
fn bootstrap_under_a_workspace_of_another_major_refuses() {
    let root = tree("boot-mixed", "rust-workspace", &[]);
    root.write("spec.toml", &root.read("spec.toml").replacen("spec_version = \"2.0\"", "spec_version = \"1.0\"", 1));
    let target = root.root.join("crates/new");
    std::fs::create_dir_all(&target).unwrap();
    let err = specdev_analyzer::bootstrap::run_bootstrap(
        &root.plugin,
        &specdev_analyzer::bootstrap::BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "new".into(),
            target_dir: target.clone(),
        },
    )
    .expect_err("a 2.0 member under a 1.x workspace must refuse");
    assert!(format!("{err:#}").contains("FORMAT-002"), "{err:#}");
    assert!(!target.join("spec.toml").exists(), "nothing written");
}

#[test]
fn a_recorded_profile_file_that_is_missing_is_not_written_again() {
    let t = tree_v1("recorded-missing", "library-api");
    std::fs::remove_file(t.root.join("specs/architecture.toml")).unwrap();
    assert!(migrate(&t, &[]).status.success());
    assert!(!t.root.join("specs/architecture.toml").exists(), "the format step wrote it again");
    assert!(migrate(&t, &[]).status.success());
    assert!(!t.root.join("specs/architecture.toml").exists(), "the shell step wrote it again");
}

#[test]
fn the_report_names_every_removal_and_the_warning_says_commit_first() {
    let t = tree_v1("report-complete", "firmware-layered");
    let spec = t.read("spec.toml").replacen("language = \"rust\"\n", "language = \"rust\"\ncrate_shape = \"library\"\n", 1);
    t.write("spec.toml", &spec);
    // An edited profile.yaml with a column-0 comment right after a retired section.
    let rel = "specs/specdev/profile.yaml";
    let p = t.read(rel).replacen("\ngates:", "\n# keep: a note about the gates\ngates:", 1);
    t.write(rel, &format!("{p}# local edit\n"));
    // An unrecorded tree: no baseline, so schemas are replaced and warned.
    let lineage = t.read("specs/specdev/lineage.toml");
    t.write("specs/specdev/lineage.toml", &lineage[..lineage.find("[profile_files]").unwrap()]);
    let dry = migrate(&t, &["--dry-run"]);
    let d = text(&dry);
    assert!(d.contains("crate_shape"), "{d}");
    assert!(d.contains("cargo_toml"), "the retired section is named:\n{d}");
    assert!(d.contains("commit or stash"), "the warning block:\n{d}");
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(t.read(rel).contains("# keep: a note about the gates"), "{}", t.read(rel));
}

#[test]
fn a_workspace_dry_run_shows_the_member_steps() {
    let root = tree("ws-dry", "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root
            .read("spec.toml")
            .replacen("spec_version = \"2.0\"", "spec_version = \"1.0\"", 1)
            .replace(
                "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
                "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n",
            ),
    );
    let red = "specs/specdev/reductions/project.yaml";
    root.write(red, &root.read(red).replace("\"specs/decisions/*.toml\"", "\"specs/decisions.toml\"").replace("\"specs/open_questions/*.toml\"", "\"specs/open_questions.toml\""));
    copy_dir(&frozen(), &root.root.join("crates/a"));
    let out = migrate(&root, &["--dry-run"]);
    let d = text(&out);
    assert!(out.status.success(), "{d}");
    assert!(d.contains("crates/a") && d.contains("would write scripts/specdev-gates.sh"), "{d}");
}

#[test]
fn gate_emit_refuses_in_a_1x_tree() {
    let t = tree_v1("emit-1x", "library-api");
    let out = t.analyzer(&["gate", "--emit"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("migrate"), "{}", text(&out));
    assert!(!t.root.join("scripts/specdev-gates.sh").exists());
}

/// The unit step must not hide a plugin move: when it writes no file, it
/// leaves the unit's record and upstream snapshot alone, so `drift` still
/// reports that the plugin moved.
#[test]
fn migrate_does_not_hide_a_plugin_move_in_an_inlined_unit() {
    let plugin = ci::plugin_with_block(
        "unit-move",
        "probe",
        "$schema: \"https://json-schema.org/draft/2020-12/schema\"\ntitle: probe\ntype: object\n",
        "kinds: []\nrelations: []\n",
    );
    let t = ci::tree_from("unit-move", &plugin, "library-api", "rust", &["probe"]);
    let before: toml::Table = t.read("specs/specdev/lineage.toml").parse().unwrap();
    // The plugin moves the unit after it was inlined.
    let schema = plugin.join("blocks/probe/schema.yaml");
    std::fs::write(&schema, format!("{}description: a later plugin line\n", std::fs::read_to_string(&schema).unwrap())).unwrap();
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let after: toml::Table = t.read("specs/specdev/lineage.toml").parse().unwrap();
    assert_eq!(
        before["blocks"]["probe"]["content_hash"], after["blocks"]["probe"]["content_hash"],
        "the unit record moved to the plugin's copy while the inlined files did not"
    );
    let drift = t.analyzer(&["drift", "--plugin-root", plugin.to_str().unwrap()]);
    let d = String::from_utf8_lossy(&drift.stdout);
    assert!(d.contains("probe") && d.contains("PluginMoved"), "drift must still report the move:\n{d}");
}
