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
//! 6. Gates run in the code root: the runner, the spec-side script (after
//!    `check`, one `cd`) and the compile-only script in the code root, which
//!    names neither the specs nor the tool. Without a split, neither the
//!    `cd` nor the compile script appears.
//! 7. `validate` reports a stale or missing compile script.
//! 8. A workspace root's compile script skips members whose code is not
//!    inside its code root; its spec-side script still runs every member.
//! 9. A missing code root is one error, and the code-side checks are
//!    skipped.
//! 10. A code root in its own git repository: `check` names the commit it
//!     read, and warns on local changes and on a submodule that is not at
//!     the commit the spec repository records.

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

// ---------------------------------------------------------------------------
// Gates with a separate code root.
// ---------------------------------------------------------------------------

const SCRIPT: &str = "scripts/specdev-gates.sh";
const COMPILE: &str = "../code/scripts/compile-gates.sh";

/// Replace the inlined profile's `gates:` block with `gates`.
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

/// A `library-api` tree with code root `../code` and one gate that records
/// the folder it ran in.
fn split_with_gate(tag: &str) -> Tree {
    let t = tree(tag, "library-api", &[]);
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"../code\"\n", 1),
    );
    set_gates(
        &t,
        "gates:\n  - label: where\n    steps:\n      - [/bin/sh, -c, \"pwd -P > where.txt\"]\n",
    );
    std::fs::create_dir_all(t.root.join("../code")).unwrap();
    let out = t.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "gate --emit:\n{}", text(&out));
    t
}

fn sh(from: &std::path::Path, script: &std::path::Path) -> std::process::Output {
    std::process::Command::new("sh")
        .arg(script)
        .current_dir(from)
        .env("SPECDEV_ANALYZER", env!("CARGO_BIN_EXE_specdev-analyzer"))
        .output()
        .expect("run sh")
}

fn ran_in(dir: &std::path::Path) -> bool {
    dir.join("where.txt").is_file()
}

#[test]
fn every_gate_path_runs_in_the_code_root() {
    let t = split_with_gate("code-root-gates");
    let code = t.root.join("../code").canonicalize().unwrap();
    let lab = t.read(SCRIPT);
    let check = lab.find(" check\n").expect("check line");
    let cd = lab.find("cd '../code' || exit 2\n").expect("cd line");
    let gate = lab.find("gate where").expect("gate line");
    assert!(check < cd && cd < gate, "check, cd, then gates:\n{lab}");

    let public = t.read(COMPILE);
    for absent in ["specdev", "specs/", " check"] {
        assert!(
            !public.contains(absent),
            "`{absent}` in the compile script:\n{public}"
        );
    }
    assert!(public.contains("gate where"), "{public}");
    assert!(t
        .read("../code/.gitattributes")
        .contains("scripts/compile-gates.sh text eol=lf"));

    // The runner, the spec-side script and the compile script, each started
    // from an unrelated folder, all run the gate in the code root.
    let out = t.analyzer(&["gate", "--label", "where"]);
    assert!(
        out.status.success() && ran_in(&code),
        "runner:\n{}",
        text(&out)
    );
    std::fs::remove_file(code.join("where.txt")).unwrap();
    let out = sh(&std::env::temp_dir(), &t.root.join(SCRIPT));
    assert!(
        out.status.success() && ran_in(&code),
        "lab script:\n{}",
        text(&out)
    );
    std::fs::remove_file(code.join("where.txt")).unwrap();
    let out = sh(
        std::path::Path::new("/"),
        &code.join("scripts/compile-gates.sh"),
    );
    assert!(
        out.status.success() && ran_in(&code),
        "compile script:\n{}",
        text(&out)
    );
    assert!(!ran_in(&t.root), "nothing ran in the spec folder");
}

#[test]
fn without_a_split_there_is_no_cd_and_no_compile_script() {
    let t = tree("code-root-unsplit", "library-api", &[]);
    let out = t.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!t.read(SCRIPT).contains("\ncd '"), "{}", t.read(SCRIPT));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("compile-gates"));
    assert!(!t.root.join("scripts/compile-gates.sh").exists());
}

#[test]
fn validate_reports_a_stale_or_missing_compile_script() {
    let t = split_with_gate("code-root-gate-validate");
    let v = |t: &Tree| text(&t.analyzer(&["validate"]));
    assert!(!v(&t).contains("GATE-SCRIPT"), "{}", v(&t));
    t.write(COMPILE, "#!/bin/sh\nexit 0\n");
    let s = v(&t);
    assert!(
        s.contains("GATE-SCRIPT-001") && s.contains("compile-gates.sh"),
        "{s}"
    );
    std::fs::remove_file(t.root.join(COMPILE)).unwrap();
    let s = v(&t);
    assert!(
        s.contains("GATE-SCRIPT-002") && s.contains("compile-gates.sh"),
        "{s}"
    );
}

#[test]
fn a_workspace_compile_script_skips_members_whose_code_is_elsewhere() {
    let root = tree("code-root-ws", "rust-workspace", &[]);
    let spec = root.read("spec.toml").replace(
        "[[workspace.members]]\nname = \"placeholder-member\"\npath = \"crates/placeholder-member\"\n",
        "[[workspace.members]]\nname = \"split\"\npath = \"members/split\"\n\n\
         [[workspace.members]]\nname = \"lab\"\npath = \"members/lab\"\n",
    );
    root.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"code\"\n", 1),
    );
    let member = |name: &str, code: &str| {
        format!(
            "spec_version = \"2.0\"\n\n[project]\nname = \"{name}\"\nprofile = \"library-api\"\n\
             language = \"rust\"\nmax_parallel_agents = 1\n\n[paths]\nspecs = \"specs\"\n\
             source = \"src\"\ncode = \"{code}\"\n"
        )
    };
    root.write(
        "members/split/spec.toml",
        &member("split", "../../code/crates/split"),
    )
    .write("members/lab/spec.toml", &member("lab", "."));
    std::fs::create_dir_all(root.root.join("code")).unwrap();
    let out = root.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "{}", text(&out));
    let public = root.read("code/scripts/compile-gates.sh");
    assert!(
        public.contains("(cd 'crates/split' && sh scripts/compile-gates.sh)"),
        "{public}"
    );
    assert!(
        !public.contains("lab"),
        "a lab-only member is skipped:\n{public}"
    );
    // The spec-side script still runs every member.
    let lab = root.read(SCRIPT);
    assert!(
        lab.contains("members/split") && lab.contains("members/lab"),
        "{lab}"
    );
}

// ---------------------------------------------------------------------------
// A missing code root, and the state of its git repository.
// ---------------------------------------------------------------------------

#[test]
fn a_missing_code_root_is_one_error() {
    let t = split("code-root-missing");
    t.write(
        "specs/modules/Explicit.toml",
        &fw_module("Explicit", "source_file = \"src/explicit.rs\"\n"),
    );
    t.check(&[])
        .fires("LAYOUT-007", "", "error")
        .says("LAYOUT-007", "", "git submodule update --init")
        .silent("source_resolves", "Explicit")
        .exit_is(1);
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed:\n{}", text(&out));
}

#[test]
fn a_submodule_code_root_reports_its_commit_and_state() {
    let t = tree("code-root-submodule", "firmware-layered", &[]);
    let upstream = t.root.join("../upstream");
    std::fs::create_dir_all(upstream.join("src")).unwrap();
    std::fs::write(upstream.join("src/explicit.rs"), "pub fn e() {}\n").unwrap();
    git(&upstream, &["init", "-q", "-b", "main"]);
    git(&upstream, &["add", "-A"]);
    git(&upstream, &["commit", "-q", "-m", "code"]);
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"code\"\n", 1),
    )
    .write(
        "specs/modules/Explicit.toml",
        &fw_module("Explicit", "source_file = \"src/explicit.rs\"\n"),
    );
    git(&t.root, &["init", "-q", "-b", "main"]);
    let up = upstream.canonicalize().unwrap();
    git(
        &t.root,
        &["submodule", "add", "-q", up.to_str().unwrap(), "code"],
    );
    git(&t.root, &["add", "-A"]);
    git(&t.root, &["commit", "-q", "-m", "specs"]);

    t.check(&[])
        .fires("code_root_commit", "", "info")
        .silent("source_resolves", "Explicit")
        .silent("code_root_clean", "")
        .silent("code_root_recorded", "");

    let code = t.root.join("code");
    std::fs::write(code.join("src/new.rs"), "pub fn n() {}\n").unwrap();
    t.check(&[])
        .fires("code_root_clean", "", "warning")
        .silent("code_root_recorded", "");

    git(&code, &["add", "-A"]);
    git(&code, &["commit", "-q", "-m", "ahead"]);
    t.check(&[])
        .silent("code_root_clean", "")
        .fires("code_root_recorded", "", "warning")
        .says("code_root_recorded", "", "records");
}

#[test]
fn a_code_root_in_the_same_repository_reports_nothing() {
    let t = tree("code-root-same-repo", "firmware-layered", &[]);
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ncode = \"code\"\n", 1),
    )
    .write("code/src/lib.rs", "pub fn l() {}\n");
    t.commit_all("one repository");
    t.check(&[])
        .silent("code_root_commit", "")
        .silent("code_root_clean", "")
        .silent("LAYOUT-007", "");
}
