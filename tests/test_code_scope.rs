//! Code that exists only for tests, as `check` must read it.
//!
//! 1. A test made by a local `macro_rules!` macro is discovered: the macro's
//!    rule body holds the test attribute, and each call names its test by its
//!    first identifier after any attributes. An `ignore` attribute in the call
//!    makes the test pending. A call of a macro with no test attribute is not
//!    a test.
//! 2. A `use` inside code compiled only for tests (`#[cfg(test)]`,
//!    `#[cfg(all(…, test, …))]`, a `#[test]` fn) is not a dependency of the
//!    module: no `internal_edges_declared`. `#[cfg(not(test))]` and plain code
//!    still count.
//! 3. A bare `verified_by` name that matches more than one test names the
//!    candidates in its finding.

use crate::ci;

use ci::{fw_module, requirement, tree, Tree};

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

const MACRO_TESTS: &str = r#"pub fn built() {}

#[cfg(test)]
mod tests {
    macro_rules! menu_test {
        ($(#[$attr:meta])* $name:ident, $n:expr) => {
            $(#[$attr])*
            #[test]
            fn $name() {
                assert!($n > 0);
            }
        };
    }

    macro_rules! not_a_test {
        ($name:ident) => {
            fn $name() {}
        };
    }

    menu_test!(menu_one, 1);
    menu_test!(#[ignore = "later"] menu_two, 2);
    not_a_test!(helper_three);
}
"#;

#[test]
fn tests_made_by_a_local_macro_are_discovered() {
    let t = tree("macro-tests", "firmware-layered", &[]);
    t.write(
        "specs/modules/BuiltOk.toml",
        &fw_module(
            "BuiltOk",
            "satisfies = [\"REQ-M-001\", \"REQ-M-002\", \"REQ-M-003\"]\n",
        ),
    )
    .write("src/built_ok.rs", MACRO_TESTS);
    verified(&t, "REQ-M-001", &["menu_one"]);
    verified(&t, "REQ-M-002", &["menu_two"]);
    verified(&t, "REQ-M-003", &["helper_three"]);

    let report = ci::text(&t.analyzer(&["report", "tests"]));
    assert!(
        report.contains("identity: menu_one"),
        "menu_one not discovered:\n{report}"
    );
    assert!(
        report.contains("identity: menu_two"),
        "menu_two not discovered:\n{report}"
    );
    assert!(
        !report.contains("helper_three"),
        "a call of a macro with no test attribute is not a test:\n{report}"
    );
    assert!(
        !report.contains("identity: ignore"),
        "the attribute is not the name:\n{report}"
    );
    let two = report
        .split("identity: menu_two")
        .nth(1)
        .unwrap_or_default();
    assert!(
        two.lines().take(3).any(|l| l.trim() == "pending: true"),
        "an `ignore` attribute in the call makes the test pending:\n{report}"
    );

    t.check(&[])
        .silent("verified_by_resolves", "REQ-M-001")
        .silent("verified_by_resolves", "REQ-M-002")
        .fires("verified_by_resolves", "REQ-M-003", "error");
}

#[test]
fn imports_in_test_only_code_are_not_edges() {
    let t = tree("test-imports", "firmware-layered", &[]);
    t.write("specs/modules/Back.toml", &fw_module("Back", ""))
        .write("src/back.rs", "pub fn run() {}\n")
        .write("specs/modules/TestMod.toml", &fw_module("TestMod", ""))
        .write(
            "src/test_mod.rs",
            "pub fn a() {}\n\n#[cfg(test)]\nmod tests {\n    use crate::back::run;\n    fn helper() {\n        run();\n    }\n}\n",
        )
        .write("specs/modules/TestAll.toml", &fw_module("TestAll", ""))
        .write(
            "src/test_all.rs",
            "pub fn a() {}\n\n#[cfg(all(test, feature = \"x\"))]\nmod t {\n    use crate::back::run;\n}\n",
        )
        .write("specs/modules/TestFn.toml", &fw_module("TestFn", ""))
        .write(
            "src/test_fn.rs",
            "pub fn a() {}\n\n#[test]\nfn case() {\n    use crate::back::run;\n    run();\n}\n",
        )
        .write("specs/modules/NotTest.toml", &fw_module("NotTest", ""))
        .write(
            "src/not_test.rs",
            "pub fn a() {}\n\n#[cfg(not(test))]\nmod n {\n    use crate::back::run;\n}\n",
        )
        .write("specs/modules/Plain.toml", &fw_module("Plain", ""))
        .write("src/plain.rs", "use crate::back::run;\npub fn a() { run() }\n");
    t.check(&[])
        .silent("internal_edges_declared", "TestMod")
        .silent("internal_edges_declared", "TestAll")
        .silent("internal_edges_declared", "TestFn")
        .fires("internal_edges_declared", "NotTest", "error")
        .fires("internal_edges_declared", "Plain", "error");

    // `report imports` still shows a test-only import, marked.
    let imports = ci::text(&t.analyzer(&["report", "imports"]));
    assert!(
        imports.contains("test_only: true"),
        "the test-only import is not marked:\n{imports}"
    );
}

#[test]
fn an_ambiguous_test_name_names_its_candidates() {
    let t = tree("ambiguous-name", "firmware-layered", &[]);
    t.write(
        "specs/modules/BuiltOk.toml",
        &fw_module("BuiltOk", "satisfies = [\"REQ-A-001\"]\n"),
    )
    .write("specs/modules/Other.toml", &fw_module("Other", ""))
    .write(
        "src/built_ok.rs",
        "#[cfg(test)]\nmod t {\n    #[test]\n    fn dup() {}\n}\n",
    )
    .write(
        "src/other.rs",
        "#[cfg(test)]\nmod t {\n    #[test]\n    fn dup() {}\n}\n",
    );
    verified(&t, "REQ-A-001", &["dup"]);
    let c = t.check(&[]);
    c.fires("verified_by_resolves", "REQ-A-001", "error");
    assert!(
        c.raw.contains("BuiltOk::dup") && c.raw.contains("Other::dup"),
        "the finding must name the candidates:\n{}",
        c.raw
    );
}
