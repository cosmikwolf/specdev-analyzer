//! `check` gives the same plugin-ahead notice as `validate`: when the named
//! plugin holds a newer copy of a file this project inlined, one
//! `PLUGIN-AHEAD-001` info line points at `/spec-drift`. Silent when the
//! plugin matches.

use crate::ci;

use std::path::{Path, PathBuf};

use ci::{plugin_root, text, tree_from};

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

fn plugin_copy(tag: &str) -> PathBuf {
    let root = std::env::temp_dir()
        .join(format!(
            "specdev-check-plugin-ahead-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
        .join("_plugin");
    copy_dir(&plugin_root(), &root);
    root
}

#[test]
fn check_reports_a_plugin_that_moved_ahead() {
    let plugin = plugin_copy("ahead");
    let t = tree_from("ahead", &plugin, "firmware-layered", "rust", &["rtic"]);
    let quiet = t.analyzer(&["check"]);
    assert!(!text(&quiet).contains("PLUGIN-AHEAD-001"), "silent while the plugin matches:\n{}", text(&quiet));

    let g = plugin.join("blocks/rtic/guidance.md");
    let body = std::fs::read_to_string(&g).unwrap();
    std::fs::write(&g, format!("{body}- One more rule.\n")).unwrap();
    let out = t.analyzer(&["check"]);
    assert!(text(&out).contains("PLUGIN-AHEAD-001"), "check names the plugin move:\n{}", text(&out));
}
