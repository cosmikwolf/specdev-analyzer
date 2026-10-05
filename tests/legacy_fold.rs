//! Legacy content in a 2.0 tree, and how `migrate` folds it.
//!
//! A merge from an old branch, or an old skill, can put entries back into a
//! combined file. `validate` reports it; `migrate` folds it: an id the
//! per-entry layout lacks is added, an id it holds with the same bytes is
//! skipped, an id it holds with other bytes refuses and shows both. Nothing
//! is overwritten, and the combined file becomes a tombstone again.

use crate::ci;

use ci::{text, tree, Tree};

const DEC_1: &str = "[[decision]]\nid = \"DEC-001\"\ntitle = \"New\"\nstatus = \"accepted\"\ntext = \"From an old branch.\"\n";
const DEC_2: &str = "[[decision]]\nid = \"DEC-002\"\ntitle = \"Same\"\nstatus = \"accepted\"\ntext = \"Both sides agree.\"\n";
const DEC_2_OTHER: &str = "[[decision]]\nid = \"DEC-002\"\ntitle = \"Same\"\nstatus = \"accepted\"\ntext = \"The branch changed it.\"\n";

fn migrate(t: &Tree) -> std::process::Output {
    let plugin = t.plugin.to_str().unwrap().to_string();
    t.analyzer(&["migrate", "--plugin-root", plugin.as_str()])
}

#[test]
fn migrate_folds_new_entries_and_skips_identical_ones() {
    let t = tree("fold-ok", "library-api", &[]);
    t.write("specs/decisions/DEC-002.toml", DEC_2);
    t.write("specs/decisions.toml", &format!("{DEC_1}\n{DEC_2}"));
    let v = t.analyzer(&["validate"]);
    assert!(text(&v).contains("ENTRY-LEGACY-001"), "{}", text(&v));
    let out = migrate(&t);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(t.read("specs/decisions/DEC-001.toml"), DEC_1);
    assert_eq!(t.read("specs/decisions/DEC-002.toml"), DEC_2);
    assert!(specdev_analyzer::entries::is_tombstone(&t.read("specs/decisions.toml")));
    let v = t.analyzer(&["validate"]);
    assert!(!text(&v).contains("ENTRY-LEGACY-001") && text(&v).contains("0 error(s)"), "{}", text(&v));
}

#[test]
fn an_entry_with_other_bytes_refuses_and_shows_both() {
    let t = tree("fold-conflict", "library-api", &[]);
    t.write("specs/decisions/DEC-002.toml", DEC_2);
    t.write("specs/decisions.toml", &format!("{DEC_1}\n{DEC_2_OTHER}"));
    let out = migrate(&t);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("Both sides agree.") && all.contains("The branch changed it."), "{all}");
    assert!(!t.root.join("specs/decisions/DEC-001.toml").exists(), "a refused fold wrote a file");
    assert_eq!(t.read("specs/decisions.toml"), format!("{DEC_1}\n{DEC_2_OTHER}"));
}
