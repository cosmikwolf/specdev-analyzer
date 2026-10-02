//! A workspace moves to `spec_version = "2.0"` as one.
//!
//! What each test holds fixed:
//!
//! 1. `migrate` at the root moves the root and every listed member; a
//!    refusal in any one of them writes nothing anywhere.
//! 2. `migrate` in a member refuses and names the root.
//! 3. A command in a member whose format major differs from the root's
//!    refuses; a directory the root does not list stays standalone.

#[path = "common/checked_intent.rs"]
mod ci;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ci::{text, tree, Tree};

const DEC_100: &str = "[[decision]]\nid = \"DEC-100\"\ntitle = \"Workspace choice\"\nstatus = \"accepted\"\ntext = \"One toolchain.\"\n";

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

/// A `1.x` workspace: a `rust-workspace` root (bootstrapped, then set back
/// to the `1.x` layout) with one combined decision file, and two members
/// copied from the frozen `1.x` library-api tree.
fn v1_workspace(tag: &str) -> Tree {
    let root = tree(tag, "rust-workspace", &[]);
    let spec = root
        .read("spec.toml")
        .replacen("spec_version = \"2.0\"", "spec_version = \"1.0\"", 1)
        .replace(
            "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
            "[[workspace.members]]\nname = \"a\"\npath = \"crates/a\"\n\n[[workspace.members]]\nname = \"b\"\npath = \"crates/b\"\n",
        );
    assert!(spec.contains("crates/b"), "{spec}");
    root.write("spec.toml", &spec);
    let red = "specs/specdev/reductions/project.yaml";
    let r = root
        .read(red)
        .replace("\"specs/decisions/*.toml\"", "\"specs/decisions.toml\"")
        .replace("\"specs/open_questions/*.toml\"", "\"specs/open_questions.toml\"");
    root.write(red, &r);
    root.write("specs/decisions.toml", DEC_100);
    let frozen = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_trees/library-api");
    for m in ["a", "b"] {
        copy_dir(&frozen, &root.root.join("crates").join(m));
    }
    let v = Tree { root: root.root.join("crates/a"), plugin: root.plugin.clone() }.analyzer(&["validate", "-q"]);
    assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "member a must load in 1.x:\n{}", text(&v));
    root
}

fn member(root: &Tree, m: &str) -> Tree {
    Tree { root: root.root.join("crates").join(m), plugin: root.plugin.clone() }
}

fn migrate(t: &Tree) -> std::process::Output {
    let plugin = t.plugin.to_str().unwrap().to_string();
    t.analyzer(&["migrate", "--plugin-root", plugin.as_str()])
}

fn version(t: &Tree) -> String {
    let doc: toml::Table = t.read("spec.toml").parse().unwrap();
    doc["spec_version"].as_str().unwrap().to_string()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().to_path_buf(), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn migrate_at_the_root_moves_the_root_and_every_member() {
    let root = v1_workspace("ws-mig-all");
    let out = migrate(&root);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(version(&root), "2.0");
    assert!(root.read("specs/decisions/DEC-100.toml").contains(DEC_100));
    for m in ["a", "b"] {
        let t = member(&root, m);
        assert_eq!(version(&t), "2.0", "member {m}");
        assert!(t.root.join("scripts/specdev-gates.sh").is_file(), "member {m} has its gate script");
        let v = t.analyzer(&["validate", "-q"]);
        assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "member {m}:\n{}", text(&v));
    }
    assert!(root.read("scripts/specdev-gates.sh").contains("crates/a"));
}

#[test]
fn a_refusal_in_one_member_writes_nothing_anywhere() {
    let root = v1_workspace("ws-mig-refuse");
    let b = member(&root, "b");
    let rel = "specs/specdev/schemas/requirements.schema.yaml";
    b.write(rel, &format!("{}# a local edit\n", b.read(rel)));
    let before = snapshot(&root.root);
    let out = migrate(&root);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("MIGRATE-014") && text(&out).contains("crates/b"), "{}", text(&out));
    assert_eq!(before, snapshot(&root.root), "a refused workspace migrate wrote to disk");
}

#[test]
fn migrate_in_a_member_refuses_and_names_the_root() {
    let root = v1_workspace("ws-mig-member");
    let a = member(&root, "a");
    let before = snapshot(&root.root);
    let out = migrate(&a);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("MIGRATE-015") && all.contains(root.root.to_str().unwrap()), "{all}");
    assert_eq!(before, snapshot(&root.root));
}

#[test]
fn a_member_at_another_major_than_its_root_refuses_every_command() {
    let root = v1_workspace("ws-major");
    assert!(migrate(&root).status.success());
    let a = member(&root, "a");
    a.write("spec.toml", &a.read("spec.toml").replacen("spec_version = \"2.0\"", "spec_version = \"1.0\"", 1));
    for args in [&["validate"][..], &["check"][..], &["report", "tests"][..]] {
        let out = a.analyzer(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}:\n{}", text(&out));
        assert!(text(&out).contains("FORMAT-002"), "{args:?}:\n{}", text(&out));
    }
    // A directory the root does not list is a project of its own.
    let stray = Tree { root: root.root.join("tools/stray"), plugin: root.plugin.clone() };
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_trees/library-api"), &stray.root);
    let out = stray.analyzer(&["validate", "-q"]);
    assert!(!text(&out).contains("FORMAT-002"), "{}", text(&out));
}
