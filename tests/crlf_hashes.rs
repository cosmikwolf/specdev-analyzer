//! Recorded content hashes and line endings (#219).
//!
//! A hash recorded from 2.0 on is taken over LF bytes, so a CRLF checkout
//! of an unedited file reads clean. A record a 1.x bootstrap took over
//! CRLF bytes still matches the same file checked out with LF (the
//! read-both window), and `migrate` re-records it in the LF form.

use crate::ci;

use std::collections::BTreeMap;

use ci::{text, tree, tree_v1, Tree};

fn crlf(bytes: &[u8]) -> Vec<u8> {
    let lf: Vec<u8> = String::from_utf8_lossy(bytes).replace("\r\n", "\n").into_bytes();
    String::from_utf8(lf).unwrap().replace('\n', "\r\n").into_bytes()
}

fn categories(t: &Tree) -> BTreeMap<String, String> {
    let out = t.analyzer(&["drift"]);
    let v: serde_json::Value = serde_yaml::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}\n{}", text(&out)));
    v["profile"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap().to_string(), f["category"].as_str().unwrap().to_string()))
        .collect()
}

fn profile_records(t: &Tree) -> toml::Table {
    let l: toml::Table = t.read("specs/specdev/lineage.toml").parse().unwrap();
    l["profile_files"].as_table().unwrap().clone()
}

fn sha(bytes: &[u8]) -> String {
    specdev_analyzer::hash::content_hash(bytes)
}

/// A CRLF checkout of an unedited 2.0 tree: every profile file reads clean.
#[test]
fn a_crlf_checkout_of_an_unedited_tree_reads_clean() {
    let t = tree("crlf-checkout", "library-api", &[]);
    let before = categories(&t);
    for path in profile_records(&t).keys() {
        let p = t.root.join(path);
        let b = std::fs::read(&p).unwrap();
        std::fs::write(&p, crlf(&b)).unwrap();
    }
    let after = categories(&t);
    for (path, cat) in &after {
        assert_eq!(Some(cat), before.get(path), "{path}: {cat} on a CRLF checkout");
    }
}

/// A record taken over CRLF bytes (a 1.x bootstrap on a CRLF checkout)
/// matches the LF checkout of the same file.
#[test]
fn a_record_taken_over_crlf_bytes_matches_the_lf_file() {
    let t = tree("crlf-record", "library-api", &[]);
    let before = categories(&t);
    let lineage = t.read("specs/specdev/lineage.toml");
    let mut doc: toml_edit::DocumentMut = lineage.parse().unwrap();
    for (path, _) in profile_records(&t) {
        if path.ends_with("module.schema.yaml") || before.get(&path).map(String::as_str) != Some("Clean") {
            continue; // the aggregator compares its profile-owned part; an edited file has no bootstrap bytes
        }
        let b = std::fs::read(t.root.join(&path)).unwrap();
        doc["profile_files"][path.as_str()] = toml_edit::value(sha(&crlf(&b)));
    }
    t.write("specs/specdev/lineage.toml", &doc.to_string());
    assert_eq!(categories(&t), before);
}

/// `migrate` re-records a CRLF-form record it can prove unedited in the LF
/// form.
#[test]
fn migrate_re_records_crlf_hashes_in_the_lf_form() {
    let t = tree_v1("crlf-migrate", "library-api");
    let lineage = t.read("specs/specdev/lineage.toml");
    let mut doc: toml_edit::DocumentMut = lineage.parse().unwrap();
    let path = "specs/specdev/schemas/enums.schema.yaml";
    let b = std::fs::read(t.root.join(path)).unwrap();
    doc["profile_files"][path] = toml_edit::value(sha(&crlf(&b)));
    t.write("specs/specdev/lineage.toml", &doc.to_string());
    let plugin = t.plugin.to_str().unwrap().to_string();
    let out = t.analyzer(&["migrate", "--plugin-root", plugin.as_str()]);
    assert!(out.status.success(), "{}", text(&out));
    let rec = profile_records(&t)[path].as_str().unwrap().to_string();
    let lf = String::from_utf8_lossy(&std::fs::read(t.root.join(path)).unwrap()).replace("\r\n", "\n");
    assert_eq!(rec, sha(lf.as_bytes()), "the record is the LF-form hash");
}
