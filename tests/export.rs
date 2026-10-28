//! `export`: the spec's decisions, requirements and open questions as
//! Markdown under `[export] path`, for publication beside the code. What
//! these tests hold fixed:
//!
//! 1. The layout (`decisions/<root>/<ID>.md`, `requirements/<root>.md`,
//!    `open_questions/<root>.md`), the anchors and the links.
//! 2. `omit` leaves fields out.
//! 3. The output is deterministic, and `export --check` agrees with it.
//! 4. A path outside the code root is refused.
//! 5. `export` removes the files the spec no longer produces.

use crate::ci;

use ci::{text, tree, Tree};

const ROOT: &str = "checked_intent_fixture";

fn with_export(t: &Tree, table: &str) {
    let spec = t.read("spec.toml");
    t.write("spec.toml", &format!("{spec}\n[export]\n{table}"));
}

fn fixture(tag: &str) -> Tree {
    let t = tree(tag, "library-api", &[]);
    t.write(
        "specs/decisions/DEC-001.toml",
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"Parse in one pass\"\nstatus = \"accepted\"\ntext = \"The parser reads the input once.\"\ncontext = \"Inputs are large.\"\nsource_ref = \"Spec §4, quoted text\"\n",
    )
    .write(
        "specs/decisions/DEC-002.toml",
        "[[decision]]\nid = \"DEC-002\"\ntitle = \"Parse lazily\"\nstatus = \"accepted\"\ntext = \"The parser reads on demand.\"\nsupersedes = [\"DEC-001\"]\n",
    )
    .write(
        "specs/requirements/REQ-PARSE-001.toml",
        "[[requirement]]\nid = \"REQ-PARSE-001\"\ntext = \"The parser accepts a | in a field.\"\nnature = \"module\"\ntest_pending = true\ndecided_by = [\"DEC-002\"]\nsource_ref = \"a quoted sentence\"\n",
    )
    .write(
        "specs/requirements/REQ-PARSE-002.toml",
        "[[requirement]]\nid = \"REQ-PARSE-002\"\ntext = \"Line one.\\nLine two.\"\nnature = \"module\"\ntest_pending = true\n",
    )
    .write(
        "specs/open_questions/OQ-001.toml",
        "[[open_question]]\nid = \"OQ-001\"\ntitle = \"Which encoding?\"\nstatus = \"open\"\ntext = \"Unknown.\"\naffects = [\"REQ-PARSE-001\"]\n",
    );
    t
}

#[test]
fn export_writes_the_layout_with_anchors_and_links() {
    let t = fixture("export-layout");
    with_export(&t, "path = \"docs/spec\"\n");
    let out = t.analyzer(&["export"]);
    assert!(out.status.success(), "{}", text(&out));

    let dec1 = t.read(&format!("docs/spec/decisions/{ROOT}/DEC-001.md"));
    assert!(
        dec1.starts_with("<a id=\"dec-001\"></a>\n\n# DEC-001 — Parse in one pass\n"),
        "{dec1}"
    );
    assert!(
        dec1.contains("**Superseded by:** [DEC-002](../../decisions/"),
        "{dec1}"
    );
    assert!(dec1.contains("## Context\n\nInputs are large."), "{dec1}");
    let dec2 = t.read(&format!("docs/spec/decisions/{ROOT}/DEC-002.md"));
    assert!(
        dec2.contains(&format!(
            "**Requirements:** [REQ-PARSE-001](../../requirements/{ROOT}.md#req-parse-001)"
        )),
        "{dec2}"
    );

    let reqs = t.read(&format!("docs/spec/requirements/{ROOT}.md"));
    assert!(
        reqs.contains("<a id=\"req-parse-001\"></a>REQ-PARSE-001"),
        "{reqs}"
    );
    assert!(
        reqs.contains("accepts a \\| in a field"),
        "a `|` is escaped in a cell:\n{reqs}"
    );
    assert!(reqs.contains("Line one.<br>Line two."), "{reqs}");
    assert!(
        reqs.contains(&format!("[DEC-002](../decisions/{ROOT}/DEC-002.md)")),
        "{reqs}"
    );
    assert!(reqs.contains("*pending*"), "{reqs}");

    let oqs = t.read(&format!("docs/spec/open_questions/{ROOT}.md"));
    assert!(oqs.contains("<a id=\"oq-001\"></a>OQ-001"), "{oqs}");
    assert!(oqs.contains("[REQ-PARSE-001](../requirements/"), "{oqs}");
}

#[test]
fn omit_leaves_fields_out() {
    let t = fixture("export-omit");
    with_export(
        &t,
        "path = \"docs/spec\"\nentities = [\"decision\", \"requirement\"]\n[export.omit]\nrequirement = [\"source_ref\"]\ndecision = [\"source_ref\", \"context\"]\n",
    );
    let out = t.analyzer(&["export"]);
    assert!(out.status.success(), "{}", text(&out));
    let reqs = t.read(&format!("docs/spec/requirements/{ROOT}.md"));
    assert!(!reqs.contains("a quoted sentence"), "{reqs}");
    let dec1 = t.read(&format!("docs/spec/decisions/{ROOT}/DEC-001.md"));
    assert!(
        !dec1.contains("quoted text") && !dec1.contains("Context"),
        "{dec1}"
    );
    assert!(
        !t.root.join("docs/spec/open_questions").exists(),
        "open questions not in `entities`"
    );
}

#[test]
fn export_is_deterministic_and_check_agrees() {
    let t = fixture("export-determinism");
    with_export(&t, "path = \"docs/spec\"\n");
    assert!(t.analyzer(&["export"]).status.success());
    let first = t.read(&format!("docs/spec/requirements/{ROOT}.md"));
    assert!(t.analyzer(&["export"]).status.success());
    assert_eq!(first, t.read(&format!("docs/spec/requirements/{ROOT}.md")));
    let out = t.analyzer(&["export", "--check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));

    t.write(
        "specs/requirements/REQ-PARSE-002.toml",
        "[[requirement]]\nid = \"REQ-PARSE-002\"\ntext = \"Changed.\"\nnature = \"module\"\ntest_pending = true\n",
    );
    let out = t.analyzer(&["export", "--check"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("requirements/{ROOT}.md: differs")),
        "{}",
        text(&out)
    );
}

#[test]
fn a_path_outside_the_code_root_is_refused() {
    let t = fixture("export-escape");
    with_export(&t, "path = \"../public\"\n");
    let out = t.analyzer(&["export"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("EXPORT-002"), "{}", text(&out));
}

#[test]
fn export_removes_files_the_spec_no_longer_produces() {
    let t = fixture("export-prune");
    with_export(&t, "path = \"docs/spec\"\n");
    t.write(&format!("docs/spec/decisions/{ROOT}/DEC-099.md"), "old\n")
        .write("docs/spec/README.md", "kept: not in an owned folder\n");
    let out = t.analyzer(&["export"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!t
        .root
        .join(format!("docs/spec/decisions/{ROOT}/DEC-099.md"))
        .exists());
    assert!(t.root.join("docs/spec/README.md").exists());
}

#[test]
fn check_reports_a_stale_export() {
    let t = fixture("export-stale");
    with_export(&t, "path = \"docs/spec\"\n");
    assert!(t.analyzer(&["export"]).status.success());
    let c = t.check(&[]);
    assert!(
        !c.ran().iter().any(|f| f.rule == "export_current"),
        "fresh export: {:?}",
        c.ran()
    );
    t.write(
        "specs/requirements/REQ-PARSE-002.toml",
        "[[requirement]]\nid = \"REQ-PARSE-002\"\ntext = \"Changed.\"\nnature = \"module\"\ntest_pending = true\n",
    );
    let c = t.check(&[]);
    let hit = c.ran().iter().find(|f| f.rule == "export_current");
    assert!(
        hit.is_some_and(|f| f.severity == "error"),
        "stale export: {:?}",
        c.ran()
    );
}

#[test]
fn cited_ids_resolve_are_published_and_current() {
    let t = fixture("cited-ids");
    // DEC-001 is superseded by DEC-002.
    t.write(
        "specs/decisions/DEC-001.toml",
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"Parse in one pass\"\nstatus = \"superseded\"\ntext = \"The parser reads the input once.\"\n",
    );
    t.write(
        "src/lib.rs",
        "//! Implements REQ-PARSE-001 (per DEC-002).\n// REQ-PARSE-009 does not exist; DEC-001 is superseded.\n// UTF-8 and ABC-1 are not ids of this project.\npub fn f() {}\n",
    );
    let c = t.check(&[]);
    c.fires("cited_ids_resolve", "REQ-PARSE-009", "warning")
        .says("cited_ids_current", "DEC-001", "DEC-002")
        .silent("cited_ids_resolve", "REQ-PARSE-001")
        .silent("cited_ids_resolve", "UTF-8")
        .silent("cited_ids_resolve", "ABC-1");
    assert!(
        !c.ran().iter().any(|f| f.rule == "cited_ids_published"),
        "no export, nothing to publish: {:?}",
        c.ran()
    );

    // An export without requirements: a cited requirement cannot be opened.
    with_export(&t, "path = \"docs/spec\"\nentities = [\"decision\"]\n");
    assert!(t.analyzer(&["export"]).status.success());
    t.check(&[])
        .fires("cited_ids_published", "REQ-PARSE-001", "error")
        .silent("cited_ids_published", "DEC-002");
}

#[test]
fn a_decision_record_validates_and_is_never_exported() {
    let t = fixture("export-record");
    t.write(
        "specs/decisions/DEC-003.toml",
        "[[decision]]\nid = \"DEC-003\"\ntitle = \"Keep notes apart\"\nstatus = \"accepted\"\ntext = \"Process notes live in record.\"\nrecord = \"Accepted by the user on 2026-10-27.\"\n",
    );
    let out = t.analyzer(&["validate"]);
    assert!(
        !text(&out).contains("SCHEMA") && !text(&out).contains("DEC-003"),
        "`record` is a decision field:\n{}",
        text(&out)
    );
    with_export(&t, "path = \"docs/spec\"\n");
    assert!(t.analyzer(&["export"]).status.success());
    let dec = t.read(&format!("docs/spec/decisions/{ROOT}/DEC-003.md"));
    assert!(dec.contains("Process notes live in record."), "{dec}");
    assert!(
        !dec.contains("Accepted by the user") && !dec.contains("Record"),
        "{dec}"
    );
}
