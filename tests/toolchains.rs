//! The spec declares the project's tool. A profile with `toolchains:`
//! gives each tool's gates and lockfiles; `gate` runs the declared tool's
//! gates after the common ones, and `check` holds the tree to the tool.

use crate::ci;

use ci::tree;

const TOOLCHAINS: &str = "
toolchains:
  uv:
    lockfiles: [uv.lock]
    gates:
      - label: uv-tests
        steps:
          - [echo, uv-ran]
  poetry:
    lockfiles: [poetry.lock]
    gates:
      - label: poetry-tests
        steps:
          - [echo, poetry-ran]
";

/// A `library-api` tree whose profile gives gates per tool, declaring `tool`.
fn project(tag: &str, tool: Option<&str>) -> ci::Tree {
    let t = tree(tag, "library-api", &[]);
    let profile = t.read("specs/specdev/profile.yaml");
    t.write(
        "specs/specdev/profile.yaml",
        &format!("{profile}{TOOLCHAINS}"),
    );
    if let Some(tool) = tool {
        let spec = t.read("spec.toml");
        let spec = spec.replacen(
            "language = \"rust\"\n",
            &format!("language = \"rust\"\ntoolchain = \"{tool}\"\n"),
            1,
        );
        t.write("spec.toml", &spec);
    }
    t
}

#[test]
fn an_undeclared_tool_is_a_warning_and_the_gates_refuse_to_guess() {
    let t = project("tc-none", None);
    let c = t.check(&[]);
    assert!(
        c.ran()
            .iter()
            .any(|f| f.rule == "toolchain_declared" && f.severity == "warning"),
        "{}",
        c.raw
    );
    let out = t.analyzer(&["gate", "--label", "uv-tests"]);
    assert!(!out.status.success());
    assert!(
        ci::text(&out).contains("declare one in spec.toml"),
        "{}",
        ci::text(&out)
    );
}

#[test]
fn the_declared_tool_picks_its_gates() {
    let t = project("tc-uv", Some("uv"));
    let out = t.analyzer(&["gate", "--label", "uv-tests"]);
    assert!(out.status.success(), "{}", ci::text(&out));
    let other = t.analyzer(&["gate", "--label", "poetry-tests"]);
    assert!(
        !other.status.success(),
        "another tool's gate must not be declared"
    );
    let emit = t.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let script = t.read("scripts/specdev-gates.sh");
    assert!(
        script.contains("uv-ran") && !script.contains("poetry-ran"),
        "{script}"
    );
}

#[test]
fn another_tools_lockfile_is_reported() {
    let t = project("tc-lock", Some("uv"));
    t.write("poetry.lock", "# poetry\n");
    let c = t.check(&[]);
    let hit = c
        .ran()
        .iter()
        .find(|f| f.rule == "toolchain_matches_tree" && f.severity == "warning")
        .unwrap_or_else(|| panic!("no lockfile warning:\n{}", c.raw));
    assert!(
        hit.message.contains("`poetry.lock` is a `poetry` lockfile"),
        "{}",
        hit.message
    );
    t.write("uv.lock", "# uv\n");
    std::fs::remove_file(t.root.join("poetry.lock")).unwrap();
    let c = t.check(&[]);
    assert!(
        !c.ran().iter().any(|f| f.rule == "toolchain_matches_tree"),
        "{}",
        c.raw
    );
}

#[test]
fn an_unknown_tool_is_an_error() {
    let t = project("tc-bad", Some("conda"));
    let c = t.check(&[]);
    assert!(
        c.ran().iter().any(|f| f.rule == "toolchain_declared"
            && f.severity == "error"
            && f.message.contains("poetry, uv")),
        "{}",
        c.raw
    );
}
