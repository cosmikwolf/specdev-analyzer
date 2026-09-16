//! The analyzer must compile from its own checkout alone, and its embedded
//! tree-sitter queries must not drift from the plugin's.
//!
//! These two facts are in tension, which is why both are asserted here.
//!
//! **What went wrong.** `src/lang/*.rs` used to reach across the repository
//! boundary: `include_str!("../../../specdev/languages/rust/items.scm")`. That
//! resolves only when a `specdev/` checkout sits beside the analyzer's. It does
//! for a developer and it did in CI, which checks both repositories out as
//! siblings — so every gate was green. It does NOT for
//! `cargo install --git <url>`, which clones this repository and nothing else.
//! That is the command all three of the plugin's install sites tell a
//! co-worker to run, and it failed with eight `couldn't read` errors at the
//! first `include_str!`.
//!
//! The CI job named `cargo install smoke test` did not catch it because it ran
//! `cargo install --path .` with the sibling present. It modelled a developer,
//! not the user it was named for.
//!
//! **The fix.** The eight `.scm` files are committed under
//! `src/embedded/languages/`, included with crate-relative paths, exactly as
//! the eleven schemas in `src/embedded/` already are. The plugin tree stays the
//! source of truth; these are mirrors, and `embedded_queries_match_the_live_plugin_tree`
//! keeps them honest.

use std::path::{Path, PathBuf};

/// Every embedded query, as (language, file stem).
const QUERIES: &[(&str, &str)] = &[
    ("c", "audit"),
    ("cpp", "audit"),
    ("dart", "audit"),
    ("dart", "items"),
    ("rust", "audit"),
    ("rust", "items"),
    ("typescript", "audit"),
    ("typescript", "items"),
];

fn embedded_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("embedded")
        .join("languages")
}

/// The sibling plugin checkout, or `None`. Absent is a legitimate state here —
/// that is the whole point of the fix — so this test degrades rather than
/// panics, and says so out loud.
fn live_plugin() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("specdev");
    p.join("languages").is_dir().then_some(p)
}

/// Holds with or without a sibling: this is the standalone-build guarantee.
#[test]
fn every_embedded_query_is_present_and_non_empty() {
    assert!(
        !QUERIES.is_empty(),
        "empty inventory — a vacuous list reads as a clean pass"
    );
    for (lang, stem) in QUERIES {
        let f = embedded_dir().join(lang).join(format!("{stem}.scm"));
        let bytes = std::fs::read(&f)
            .unwrap_or_else(|e| panic!("embedded query missing: {} ({e})", f.display()));
        assert!(
            !bytes.is_empty(),
            "embedded query is empty: {} — an empty query matches nothing and \
             degrades extraction silently",
            f.display()
        );
    }
}

/// No `include_str!` in `src/` may escape the crate root. This is the
/// regression guard for the defect itself, and it needs no sibling.
#[test]
fn no_source_file_includes_across_the_repository_boundary() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).expect("read src") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).expect("read rs");
                for (i, line) in text.lines().enumerate() {
                    // `include_str!` / `include_bytes!` whose path climbs out of
                    // the crate. Three levels is what reached the sibling repo.
                    if line.contains("include_str!(\"../../../")
                        || line.contains("include_bytes!(\"../../../")
                    {
                        out.push(format!("{}:{}: {}", p.display(), i + 1, line.trim()));
                    }
                }
            }
        }
    }
    walk(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "{} include(s) reach outside the crate root. The crate then builds only \
         where a sibling checkout happens to exist, and `cargo install --git` \
         fails for every user:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

/// The plugin tree owns these queries; `src/embedded/languages/` mirrors them.
/// Local-only, like `every_fixture_file_matches_the_live_plugin_tree`.
#[test]
fn embedded_queries_match_the_live_plugin_tree() {
    let Some(plugin) = live_plugin() else {
        eprintln!(
            "skip: no sibling plugin checkout — drift cannot be checked here. \
             This is expected in a standalone clone and is exactly the layout \
             the embedding exists to support. CI checks both out."
        );
        return;
    };
    let mut drifted = Vec::new();
    for (lang, stem) in QUERIES {
        let rel = format!("languages/{lang}/{stem}.scm");
        let live = plugin.join(&rel);
        let mine = embedded_dir().join(lang).join(format!("{stem}.scm"));
        let live_bytes = std::fs::read(&live).unwrap_or_else(|e| {
            panic!(
                "plugin no longer ships {rel} ({e}) — the \
                 query moved or was renamed upstream; update QUERIES"
            )
        });
        let my_bytes = std::fs::read(&mine).expect("embedded query");
        if live_bytes != my_bytes {
            drifted.push(rel);
        }
    }
    assert!(
        drifted.is_empty(),
        "{} embedded quer(ies) drifted from the plugin tree. The plugin owns \
         them; re-copy from {}:\n  {}",
        drifted.len(),
        plugin.display(),
        drifted.join("\n  ")
    );
}
