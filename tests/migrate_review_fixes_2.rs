//! Migration fixes from the second review: what each test holds fixed.
//!
//! 1. Unit snapshots lose their `codegen/` files with the inlined
//!    `codegen/`; `drift` classifies the unit afterwards.
//! 2. The unit step writes a missing file and keeps the other snapshot
//!    bytes, so a plugin move still shows.
//! 3. `profile.yaml` edits keep the file valid.
//! 4. A root run finishes every member, and a unit missing from the plugin
//!    makes it exit 1.
//! 5. The fold keeps headers and comments.
//! 6. A replaced unrecorded schema shows the lines it loses.
//! 7. Other shipped files that differ get a "left as is" line.
//! 8. The tombstone's first line is matched by its whole shape.

#[path = "common/checked_intent.rs"]
mod ci;

use ci::{text, tree, tree_v1, Tree};

fn migrate(t: &Tree, extra: &[&str]) -> std::process::Output {
    let plugin = t.plugin.to_str().unwrap().to_string();
    let mut args = vec!["migrate", "--plugin-root", plugin.as_str()];
    args.extend_from_slice(extra);
    t.analyzer(&args)
}

fn drift(t: &Tree) -> String {
    let out = t.analyzer(&["drift", "--plugin-root", t.plugin.to_str().unwrap()]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A 1.x tree with `egui` inlined, whose snapshot still lists a codegen file
/// (as every tree inlined before code generation was removed does).
fn v1_with_unit(tag: &str) -> Tree {
    let t = tree_v1(tag, "library-api");
    let out = t.analyzer(&["inline", "egui"]);
    assert!(out.status.success(), "{}", text(&out));
    t.write("specs/specdev/upstream/blocks/egui/codegen/rust/README.md", "# egui codegen prose\n");
    t.write("specs/specdev/codegen/rust/blocks/egui/README.md", "# egui codegen prose\n");
    t
}

#[test]
fn unit_snapshots_lose_their_codegen_files() {
    let t = v1_with_unit("snap-codegen");
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!t.root.join("specs/specdev/upstream/blocks/egui/codegen").exists());
    let d = drift(&t);
    assert!(!d.contains("DRIFT-INVARIANT"), "{d}");
    assert!(d.contains("name: egui"), "the unit is classified:\n{d}");
    assert!(text(&out).contains("egui"), "the report names the unit:\n{}", text(&out));
}

#[test]
fn a_missing_unit_file_is_written_and_the_other_snapshot_bytes_stay() {
    let plugin = ci::plugin_with_block("snap-keep", "probe", "title: probe\ntype: object\n", "kinds: []\nrelations: []\n");
    let t = ci::tree_from("snap-keep", &plugin, "library-api", "rust", &["probe"]);
    // The plugin moves the schema, and the project loses the reduction.
    std::fs::write(plugin.join("blocks/probe/schema.yaml"), "title: probe\ntype: object\ndescription: later\n").unwrap();
    std::fs::remove_file(t.root.join("specs/specdev/reductions/blocks/probe.yaml")).unwrap();
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(t.root.join("specs/specdev/reductions/blocks/probe.yaml").is_file(), "the missing file is written");
    let snap = t.read("specs/specdev/upstream/blocks/probe/schema.yaml");
    assert!(!snap.contains("later"), "the snapshot kept the inline-time bytes:\n{snap}");
    let d = drift(&t);
    assert!(d.contains("PluginMoved"), "the schema move still shows:\n{d}");
}

#[test]
fn profile_yaml_edits_keep_the_file_valid() {
    let t = tree_v1("profile-valid", "firmware-layered");
    let rel = "specs/specdev/profile.yaml";
    let p = t
        .read(rel)
        .replacen("  bin:\n", "# binary target below\n  bin:\n", 1)
        .replacen("    scope: module\n", "    scope: module   # one row per module\n", 1);
    assert!(p.contains("# binary target below") && p.contains("# one row per module"), "fixture:\n{p}");
    t.write(rel, &p);
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let after = t.read(rel);
    let v: serde_yaml::Value = serde_yaml::from_str(&after).unwrap_or_else(|e| panic!("{e}\n{after}"));
    assert!(v.get("cargo_toml").is_none() && !after.contains("scope:"), "{after}");
    let val = t.analyzer(&["validate", "-q"]);
    assert!(!text(&val).contains("MANIFEST-001"), "{}", text(&val));
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
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

fn workspace(tag: &str) -> (Tree, Tree) {
    let root = tree(tag, "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root.read("spec.toml").replace(
            "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
            "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n",
        ),
    );
    let frozen = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_trees/library-api");
    copy_dir(&frozen, &root.root.join("crates/a"));
    let a = Tree { root: root.root.join("crates/a"), plugin: root.plugin.clone() };
    (root, a)
}

#[test]
fn a_root_run_finishes_its_members_every_time() {
    let (root, a) = workspace("ws-finish");
    assert!(migrate(&root, &[]).status.success());
    std::fs::remove_file(a.root.join("scripts/specdev-gates.sh")).unwrap();
    let out = migrate(&root, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(a.root.join("scripts/specdev-gates.sh").is_file(), "the second run writes the member's script");
}

#[test]
fn a_unit_missing_from_the_plugin_in_a_member_exits_1() {
    let (root, a) = workspace("ws-missing");
    let l = a.read("specs/specdev/lineage.toml");
    a.write(
        "specs/specdev/lineage.toml",
        &format!("{l}\n[blocks.ghost]\norigin = \"plugin\"\norigin_version = \"0.0.0\"\ninlined_at = \"2026-01-01T00:00:00Z\"\ncontent_hash = \"sha256:00\"\n"),
    );
    let out = migrate(&root, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("ghost"), "{}", text(&out));
}

const HEADER: &str = "# Our decisions: one per choice, reviewed weekly.\n\n";

#[test]
fn the_fold_keeps_headers_and_comments() {
    let t = tree_v1("fold-keep", "library-api");
    assert!(migrate(&t, &[]).status.success());
    let tomb = t.read("specs/decisions.toml");
    let readme_before = t.read("specs/decisions/README.md");
    // A merge brings back a combined file with its own header and a comment.
    t.write(
        "specs/decisions.toml",
        &format!("{tomb}{HEADER}# merged note: keep this\n[[decision]]\nid = \"DEC-901\"\ntitle = \"t\"\nstatus = \"accepted\"\ntext = \"x\"\n"),
    );
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let readme = t.read("specs/decisions/README.md");
    assert!(readme.starts_with(&readme_before), "the README keeps its text");
    assert!(readme.contains("Our decisions: one per choice"), "{readme}");
    let all = format!("{readme}{}", t.read("specs/decisions/DEC-901.toml"));
    assert!(all.contains("merged note: keep this"), "the comment is kept:\n{all}");
}

#[test]
fn a_replaced_unrecorded_schema_shows_what_it_loses() {
    let t = tree_v1("case-c-lines", "library-api");
    let rel = "specs/specdev/schemas/requirements.schema.yaml";
    t.write(rel, &format!("{}# local rule: ids are uppercase\n", t.read(rel)));
    let l = t.read("specs/specdev/lineage.toml");
    t.write("specs/specdev/lineage.toml", &l[..l.find("[profile_files]").unwrap()]);
    let dry = text(&migrate(&t, &["--dry-run"]));
    assert!(dry.contains("# local rule: ids are uppercase"), "{dry}");
    let out = text(&migrate(&t, &[]));
    assert!(out.contains("git diff") && out.contains("only if your work was committed"), "{out}");
}

#[test]
fn other_shipped_files_that_differ_get_a_left_as_is_line() {
    let t = tree_v1("left-as-is", "firmware-layered");
    let l = t.read("specs/specdev/lineage.toml");
    t.write("specs/specdev/lineage.toml", &l[..l.find("[profile_files]").unwrap()]);
    // Nothing in it that 2.0 removes, and not the plugin's copy.
    t.write("specs/architecture.toml", "# a local note\n[platform]\ntarget_triple = \"thumbv7em-none-eabihf\"\n");
    let out = text(&migrate(&t, &["--dry-run"]));
    assert!(out.contains("left specs/architecture.toml as is"), "{out}");
}

#[test]
fn the_tombstone_first_line_is_matched_by_its_whole_shape() {
    use specdev_analyzer::entries::{is_tombstone, TOMBSTONE_SECOND_LINE};
    let good = format!(
        "MOVED to specs/decisions/ by specdev-analyzer migrate — upgrade specdev-analyzer to 0.3.0 or later\n{TOMBSTONE_SECOND_LINE}\n"
    );
    assert!(is_tombstone(&good));
    let extra = format!(
        "MOVED to specs/decisions/ by specdev-analyzer migrate — upgrade specdev-analyzer to 0.3.0 or later, and more\n{TOMBSTONE_SECOND_LINE}\n"
    );
    assert!(!is_tombstone(&extra));
    let two = format!(
        "MOVED to specs/requirements/ and specs/sources/ by specdev-analyzer migrate — upgrade specdev-analyzer to 0.3.0 or later\r\n{TOMBSTONE_SECOND_LINE}\r\n"
    );
    assert!(is_tombstone(&two));
}

/// A tree with no `[profile_files]` gets its first baseline from `migrate`:
/// every profile file as `migrate` leaves it, so `drift` classifies them
/// all (no invariant) and a later edit shows.
#[test]
fn an_unrecorded_tree_gets_a_full_baseline() {
    let t = tree_v1("first-baseline", "firmware-layered");
    let l = t.read("specs/specdev/lineage.toml");
    t.write("specs/specdev/lineage.toml", &l[..l.find("[profile_files]").unwrap()]);
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let d = drift(&t);
    assert!(!d.contains("DRIFT-INVARIANT"), "{d}");
    t.write("specs/architecture.toml", &format!("{}\n# later edit\n", t.read("specs/architecture.toml")));
    let d = drift(&t);
    assert!(d.contains("LocallyEdited"), "a later edit shows:\n{d}");
}

/// The first baseline must not call a local edit "unedited": a later
/// `migrate --refresh` would then overwrite it.
#[test]
fn the_first_baseline_never_hides_a_local_edit_from_refresh() {
    let t = tree_v1("baseline-refresh", "firmware-layered");
    let l = t.read("specs/specdev/lineage.toml");
    t.write("specs/specdev/lineage.toml", &l[..l.find("[profile_files]").unwrap()]);
    let edited = format!("{}\n# our architecture note\n", t.read("specs/architecture.toml"));
    t.write("specs/architecture.toml", &edited);
    assert!(migrate(&t, &[]).status.success());
    // `migrate` edits the file precisely (removed keys); that is the base.
    let edited = t.read("specs/architecture.toml");
    let d = drift(&t);
    let arch = d.split("path: specs/architecture.toml").nth(1).unwrap_or("");
    assert!(arch.trim_start().starts_with("category: LocallyEdited"), "{d}");
    let out = migrate(&t, &["--refresh"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(t.read("specs/architecture.toml"), edited, "refresh overwrote a local edit");
}
