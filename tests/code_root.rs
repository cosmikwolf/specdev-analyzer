//! `[paths] code`: a code root apart from the spec root.
//!
//! The specs live in one folder and the code in another (a sibling folder
//! here; a git submodule in the field). What each test holds fixed:
//!
//! 1. A module `source_file`, `[paths].source`, `[paths].tests` and the
//!    language's `tests/` root all resolve against the code root, and a
//!    test file identity is relative to it — so no spec value changes when
//!    a project splits.
//! 2. Nothing code-side is read from the spec folder.
//! 3. Imports resolve in the code root, so `depends_on` is still checked.
//! 4. Printed code paths (`items diff`, `extract-items`, `report tests`)
//!    are relative to the code root.
//! 5. `code` must be relative.

use crate::ci;

use ci::{fw_module, requirement, text, tree, Tree};

/// A firmware tree whose code root is the sibling folder `../code`.
fn split(tag: &str) -> Tree {
    let t = tree(tag, "firmware-layered", &[]);
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"../code\"\n", 1),
    );
    t
}

fn verified(t: &Tree, id: &str, names: &[&str]) {
    let list = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    t.write(
        &format!("specs/requirements/{id}.toml"),
        &requirement(id, &format!("verified_by = [{list}]\n")),
    );
}

#[test]
fn sources_and_tests_resolve_in_the_code_root() {
    let t = split("code-root-resolve");
    t.write(
        "specs/modules/Explicit.toml",
        &fw_module(
            "Explicit",
            "source_file = \"src/explicit.rs\"\nsatisfies = [\"REQ-C-001\"]\n",
        ),
    )
    .write(
        "specs/modules/Convention.toml",
        &fw_module("Convention", ""),
    )
    .write(
        "../code/src/explicit.rs",
        "pub fn e() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn in_source_case() {}\n}\n",
    )
    .write("../code/src/convention.rs", "pub fn c() {}\n")
    .write(
        "../code/tests/a.rs",
        "#[test]\nfn integration_case() {}\n#[test]\nfn dup() {}\n",
    )
    .write("../code/tests/b.rs", "#[test]\nfn dup() {}\n")
    .write(
        "../code/bench/tests/bench.rs",
        "#[test]\nfn bench_case() {}\n",
    )
    // In the spec folder: never read as code.
    .write("tests/spec_side.rs", "#[test]\nfn spec_side_case() {}\n");
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ntests = [\"bench/tests\"]\n", 1),
    );
    verified(
        &t,
        "REQ-C-001",
        &[
            "in_source_case",
            "integration_case",
            "bench_case",
            "tests/a.rs::dup",
        ],
    );
    verified(&t, "REQ-C-002", &["spec_side_case"]);
    t.check(&[])
        .silent("source_resolves", "Explicit")
        .silent("source_resolves", "Convention")
        .silent("verified_by_resolves", "REQ-C-001")
        .fires("verified_by_resolves", "REQ-C-002", "error");
}

#[test]
fn imports_resolve_in_the_code_root() {
    let t = split("code-root-imports");
    t.write("specs/modules/Uses.toml", &fw_module("Uses", ""))
        .write(
            "specs/modules/Undeclared.toml",
            &fw_module("Undeclared", ""),
        )
        .write("../code/src/uses.rs", "pub fn f() {}\n")
        .write(
            "../code/src/undeclared.rs",
            "use crate::uses::f;\npub fn g() { f() }\n",
        );
    t.check(&[]).silent("source_resolves", "Undeclared").fires(
        "internal_edges_declared",
        "Undeclared",
        "error",
    );
}

#[test]
fn printed_code_paths_are_relative_to_the_code_root() {
    let t = split("code-root-print");
    t.write(
        "specs/modules/Explicit.toml",
        &fw_module("Explicit", "source_file = \"src/explicit.rs\"\n"),
    )
    .write(
        "../code/src/explicit.rs",
        "pub fn e() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
    );
    for args in [
        &["items", "diff", "--module", "Explicit"][..],
        &["extract-items"][..],
        &["report", "tests"][..],
    ] {
        let out = t.analyzer(args);
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(
            s.contains("src/explicit.rs") && !s.contains("code/src") && !s.contains(".."),
            "{args:?}: code path not relative to the code root:\n{}",
            text(&out)
        );
    }
}

#[test]
fn an_absolute_code_root_is_refused() {
    let t = tree("code-root-absolute", "firmware-layered", &[]);
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"/abs/code\"\n", 1),
    );
    let out = t.analyzer(&["validate"]);
    assert_ne!(out.status.code(), Some(0), "{}", text(&out));
    assert!(text(&out).contains("code"), "{}", text(&out));
}
