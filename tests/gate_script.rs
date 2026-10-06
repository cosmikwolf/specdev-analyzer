//! The gate script: `scripts/specdev-gates.sh`, rendered from the inlined
//! `gates:` registry, run by CI.
//!
//! What each test holds fixed:
//!
//! 1. Parity: the runner (`gate --label`) and the script give the same exit
//!    code and run in the same directory for each case — a pass, closed
//!    stdin, a missing tool, a failing step, an unresolved token, and a
//!    project nested below the directory the script is started from.
//! 2. The script runs `check` before any gate, and stops when it fails.
//! 3. `bootstrap` and `migrate` write the script and the `.gitattributes`
//!    line; `gate --emit` rewrites it.
//! 4. `validate` reports a script that differs from the registry (error)
//!    and a missing one (warning).
//! 5. `gate --all` runs every gate; `--module` is accepted and ignored.
//! 6. A workspace root's script runs each member's script in order and
//!    stops at the first failure.

use crate::ci;

use std::path::Path;
use std::process::{Command, Output};

use ci::{text, tree, tree_v1, Tree};

const SCRIPT: &str = "scripts/specdev-gates.sh";

fn set_gates(t: &Tree, gates: &str) {
    let rel = "specs/specdev/profile.yaml";
    let body = t.read(rel);
    let head = body.split("\ngates:").next().unwrap().to_string();
    let rest = body
        .split("\ngates:")
        .nth(1)
        .map(|r| {
            r.split_once("\n\n")
                .map(|(_, after)| after.to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    t.write(rel, &format!("{head}\n{gates}\n{rest}"));
}

fn emit(t: &Tree) {
    let out = t.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "gate --emit:\n{}", text(&out));
}

fn run_script(from: &Path, script: &Path) -> Output {
    Command::new("sh")
        .arg(script)
        .current_dir(from)
        .env("SPECDEV_ANALYZER", env!("CARGO_BIN_EXE_specdev-analyzer"))
        .output()
        .expect("run sh")
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

#[test]
fn runner_and_script_agree_on_every_case() {
    let cases: &[(&str, &str, i32)] = &[
        ("ok", "[/bin/sh, -c, \"pwd -P > where.txt\"]", 0),
        ("stdin", "[/bin/sh, -c, \"read line\"]", 1),
        ("missing", "[definitely-not-a-real-binary-xyz, --go]", 2),
        ("failing", "[/bin/sh, -c, \"exit 3\"]", 3),
        (
            "token",
            "[/bin/echo, \"${root.platform.target_triple}\"]",
            2,
        ),
    ];
    for (label, step, want) in cases {
        // Nested: the project sits two levels below where the script starts.
        let t = tree(&format!("gs-parity-{label}"), "library-api", &[]);
        set_gates(
            &t,
            &format!("gates:\n  - label: {label}\n    steps:\n      - {step}\n"),
        );
        emit(&t);
        let runner = t.analyzer(&["gate", "--label", label]);
        let where_runner = std::fs::read_to_string(t.root.join("where.txt")).ok();
        let _ = std::fs::remove_file(t.root.join("where.txt"));
        let start = t.root.parent().unwrap().parent().unwrap();
        let script = run_script(start, &t.root.join(SCRIPT));
        let where_script = std::fs::read_to_string(t.root.join("where.txt")).ok();
        assert_eq!(code(&runner), *want, "runner `{label}`:\n{}", text(&runner));
        assert_eq!(code(&script), *want, "script `{label}`:\n{}", text(&script));
        assert_eq!(
            where_runner, where_script,
            "`{label}`: the two ran in different directories"
        );
        if *label == "ok" {
            let w = where_script.expect("the ok gate writes where.txt");
            assert_eq!(
                std::path::PathBuf::from(w.trim()),
                std::fs::canonicalize(&t.root).unwrap(),
                "the script runs in the project root"
            );
        }
        if *label == "token" {
            assert!(
                String::from_utf8_lossy(&script.stderr).contains("target_triple"),
                "{}",
                text(&script)
            );
        }
    }
}

#[test]
fn the_script_runs_check_first_and_stops_on_its_failure() {
    let t = tree("gs-check-first", "library-api", &[]);
    set_gates(
        &t,
        "gates:\n  - label: mark\n    steps:\n      - [/bin/sh, -c, \"touch gate-ran\"]\n",
    );
    emit(&t);
    let body = t.read(SCRIPT);
    let check_at = body.find(" check").expect("the script runs check");
    assert!(
        check_at < body.find("gate-ran").unwrap(),
        "check comes before every gate:\n{body}"
    );
    // A module with no source: `check` fails, and no gate runs.
    t.write(
        "specs/modules/ghost.toml",
        "name = \"ghost\"\nvisibility = \"crate\"\ndescription = \"x\"\n",
    );
    let out = run_script(&t.root, &t.root.join(SCRIPT));
    assert_eq!(code(&out), 1, "{}", text(&out));
    assert!(
        !t.root.join("gate-ran").exists(),
        "a gate ran after check failed"
    );
    std::fs::remove_file(t.root.join("specs/modules/ghost.toml")).unwrap();
    let out = run_script(&t.root, &t.root.join(SCRIPT));
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(t.root.join("gate-ran").exists());
}

#[test]
fn bootstrap_and_migrate_write_the_script_and_the_gitattributes_line() {
    let t = tree("gs-bootstrap", "library-api", &[]);
    assert!(t.root.join(SCRIPT).is_file(), "bootstrap writes the script");
    assert!(t
        .read(".gitattributes")
        .contains("scripts/specdev-gates.sh text eol=lf"));
    let sh = Command::new("sh")
        .args(["-n", SCRIPT])
        .current_dir(&t.root)
        .output()
        .unwrap();
    assert!(sh.status.success(), "sh -n: {}", text(&sh));

    let v1 = tree_v1("gs-migrate", "library-api");
    assert!(!v1.root.join(SCRIPT).exists());
    let plugin = v1.plugin.to_str().unwrap().to_string();
    let out = v1.analyzer(&["migrate", "--plugin-root", plugin.as_str()]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(v1.root.join(SCRIPT).is_file(), "migrate writes the script");
    assert!(v1
        .read(".gitattributes")
        .contains("scripts/specdev-gates.sh text eol=lf"));
}

#[test]
fn an_existing_gitattributes_is_not_rewritten_and_the_line_is_printed() {
    let t = tree("gs-attrs", "library-api", &[]);
    t.write(".gitattributes", "*.png binary\n");
    let out = t.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(t.read(".gitattributes"), "*.png binary\n");
    assert!(
        text(&out).contains("scripts/specdev-gates.sh text eol=lf"),
        "{}",
        text(&out)
    );
}

#[test]
fn validate_reports_a_stale_or_missing_script() {
    let t = tree("gs-validate", "library-api", &[]);
    let rules = |t: &Tree| {
        let out = t.analyzer(&["validate"]);
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    assert!(!rules(&t).contains("GATE-SCRIPT-"), "{}", rules(&t));
    set_gates(
        &t,
        "gates:\n  - label: other\n    steps:\n      - [/bin/echo, changed]\n",
    );
    let r = rules(&t);
    assert!(
        r.lines()
            .any(|l| l.starts_with("error") && l.contains("GATE-SCRIPT-001")),
        "{r}"
    );
    emit(&t);
    assert!(!rules(&t).contains("GATE-SCRIPT-"));
    std::fs::remove_file(t.root.join(SCRIPT)).unwrap();
    let r = rules(&t);
    assert!(
        r.lines()
            .any(|l| l.starts_with("warning") && l.contains("GATE-SCRIPT-002")),
        "{r}"
    );
}

#[test]
fn gate_all_runs_every_gate_and_module_is_ignored() {
    let t = tree("gs-all", "library-api", &[]);
    set_gates(
        &t,
        "gates:\n  - label: a\n    steps:\n      - [/bin/sh, -c, \"touch a-ran\"]\n  - label: b\n    steps:\n      - [/bin/sh, -c, \"touch b-ran\"]\n",
    );
    let out = t.analyzer(&["gate", "--all"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(t.root.join("a-ran").exists() && t.root.join("b-ran").exists());
    let out = t.analyzer(&["gate", "--label", "a", "--module", "anything"]);
    assert!(
        out.status.success(),
        "--module is accepted and ignored:\n{}",
        text(&out)
    );
}

#[test]
fn a_workspace_root_script_runs_each_member_in_order() {
    let root = tree("gs-ws", "rust-workspace", &[]);
    root.write(
        "spec.toml",
        &root.read("spec.toml").replace(
            "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
            "[[workspace.members]]\nname = \"one\"\npath = \"crates/one\"\n\n[[workspace.members]]\nname = \"two\"\npath = \"crates/two\"\n",
        ),
    );
    emit(&root);
    let body = root.read(SCRIPT);
    let one = body.find("crates/one").expect("member one");
    let two = body.find("crates/two").expect("member two");
    assert!(one < two, "declared order:\n{body}");
    for (m, exit) in [("one", 4), ("two", 0)] {
        root.write(
            &format!("crates/{m}/{SCRIPT}"),
            &format!("#!/bin/sh\ntouch \"$(dirname \"$0\")/../ran\"\nexit {exit}\n"),
        );
    }
    let out = run_script(&root.root, &root.root.join(SCRIPT));
    assert_eq!(code(&out), 4, "{}", text(&out));
    assert!(root.root.join("crates/one/ran").exists());
    assert!(
        !root.root.join("crates/two/ran").exists(),
        "stops at the first failing member"
    );
}
