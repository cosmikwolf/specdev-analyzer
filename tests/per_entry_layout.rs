//! Reading the per-entry layout (`spec_version = "2.x"`): one file per
//! decision, requirement, source and open question.
//!
//! The fixture is built by hand — split files, rewritten reduction globs,
//! `spec_version = "2.0"` — which is what `migrate` produces. Holding the
//! readers to a hand-built tree keeps these tests independent of `migrate`.
//!
//! Also here: the checks a per-entry file must pass (its schema, one entry
//! per file, the file stem equal to the id), and a workspace member at
//! `crates/<m>` reading its root's decisions and open questions in the
//! root's own layout.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    prompt, reduction, validate,
};

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-per-entry-layout-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn bootstrap_at(target: &Path, profile: &str, name: &str) {
    std::fs::create_dir_all(target).unwrap();
    bootstrap::run_bootstrap(
        &plugin_root(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.to_path_buf(),
        },
    )
    .unwrap();
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn set_spec_version(project: &Path, version: &str) {
    let path = project.join("spec.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let rest = text
        .split_once('\n')
        .map(|(first, rest)| {
            assert!(first.starts_with("spec_version"), "{first}");
            rest
        })
        .unwrap();
    std::fs::write(&path, format!("spec_version = \"{version}\"\n{rest}")).unwrap();
}

/// Set the inlined project reduction to a layout. A bootstrap writes the
/// `2.0` globs, so `"2.x"` only checks them; `"1.x"` points every glob back
/// at its combined file (the Source glob at the requirements file, which
/// holds `source[]` in `1.x`).
fn set_reduction_layout(project: &Path, version: &str) {
    let path = project.join("specs/specdev/reductions/project.yaml");
    let text = std::fs::read_to_string(&path).unwrap();
    if version.starts_with('2') {
        assert!(
            ["requirements", "decisions", "open_questions"]
                .iter()
                .any(|d| text.contains(&format!("\"specs/{d}/*.toml\""))),
            "not a 2.0 reduction"
        );
        return;
    }
    let text = text
        .replace("\"specs/sources/*.toml\"", "\"specs/requirements.toml\"")
        .replace(
            "\"specs/requirements/*.toml\"",
            "\"specs/requirements.toml\"",
        )
        .replace("\"specs/decisions/*.toml\"", "\"specs/decisions.toml\"")
        .replace(
            "\"specs/open_questions/*.toml\"",
            "\"specs/open_questions.toml\"",
        );
    for dir in ["sources", "requirements", "decisions", "open_questions"] {
        assert!(
            !text.contains(&format!("\"specs/{dir}/*.toml\"")),
            "{dir} glob still named"
        );
    }
    std::fs::write(&path, text).unwrap();
}

/// A `1.x` tree from a bootstrap: the version and the combined reduction.
fn to_1x(project: &Path) {
    set_spec_version(project, "1.0");
    set_reduction_layout(project, "1.0");
}

/// A `2.0` library-api project: `parser` (guided by DEC-001) and `writer`
/// (blocked by the open question OQ-002).
fn build_per_entry_project(tag: &str) -> PathBuf {
    let project = tmp_workspace(tag).join("proj");
    bootstrap_at(&project, "library-api", "per_entry_fixture");
    let specs = project.join("specs");
    write(
        &specs.join("decisions/DEC-001.toml"),
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"Parse in one pass\"\nstatus = \"accepted\"\ntext = \"One pass.\"\n",
    );
    write(
        &specs.join("decisions/DEC-002.toml"),
        "[[decision]]\nid = \"DEC-002\"\ntitle = \"Default for OQ-001\"\nstatus = \"accepted\"\ntext = \"Working default.\"\ndefaults_for = [\"OQ-001\"]\n",
    );
    write(
        &specs.join("sources/layout-doc.toml"),
        "[[source]]\nid = \"layout-doc\"\nkind = \"derived\"\ntitle = \"Fixture source\"\n",
    );
    write(
        &specs.join("requirements/LAY-001.toml"),
        "[[requirement]]\nid = \"LAY-001\"\ntext = \"The parser shall parse.\"\nnature = \"module\"\nsource = [\"layout-doc\"]\ndecided_by = [\"DEC-001\"]\n",
    );
    write(
        &specs.join("requirements/LAY-002.toml"),
        "[[requirement]]\nid = \"LAY-002\"\ntext = \"The writer shall write.\"\nnature = \"module\"\nsource = [\"layout-doc\"]\n",
    );
    write(
        &specs.join("open_questions/OQ-001.toml"),
        "[[open_question]]\nid = \"OQ-001\"\ntitle = \"Committed default\"\nstatus = \"default_committed\"\ntext = \"Assumed.\"\ndefault_via = \"DEC-002\"\nverify = \"Run it.\"\naffects = [\"LAY-001\"]\n",
    );
    write(
        &specs.join("open_questions/OQ-002.toml"),
        "[[open_question]]\nid = \"OQ-002\"\ntitle = \"Open blocker\"\nstatus = \"open\"\ntext = \"Unknown.\"\naffects = [\"LAY-002\"]\n",
    );
    write(
        &specs.join("modules/parser.toml"),
        "name = \"parser\"\nvisibility = \"crate\"\ndescription = \"Parses.\"\nsatisfies = [\"LAY-001\"]\nguided_by = [\"DEC-001\"]\n",
    );
    write(
        &specs.join("modules/writer.toml"),
        "name = \"writer\"\nvisibility = \"crate\"\ndescription = \"Writes.\"\nsatisfies = [\"LAY-002\"]\n",
    );
    set_reduction_layout(&project, "2.0");
    project
}

fn analyzer(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(args)
        .arg("--no-color")
        .current_dir(project)
        .env("SPECDEV_PLUGIN_ROOT", plugin_root())
        .output()
        .expect("run specdev-analyzer")
}

fn text(out: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout\n{}\n--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Rule codes of every error `validate` reports.
fn error_rules(project: &Path) -> Vec<(String, String)> {
    let report = validate::run_project(project, None).expect("validate");
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .map(|f| (f.rule.clone(), f.message.clone()))
        .collect()
}

// ---------------------------------------------------------------------------

#[test]
fn a_per_entry_project_reads_every_entry() {
    let project = build_per_entry_project("reads");

    let out = analyzer(&project, &["validate"]);
    let all = text(&out);
    assert!(
        out.status.success() && all.contains("0 error(s)"),
        "validate is not clean:\n{all}"
    );
    assert!(
        all.contains("2 requirements / 1 sources")
            && all.contains("2 decisions / 2 open questions (1 open)"),
        "the summary does not count the per-entry files:\n{all}"
    );

    let (graph, _) = reduction::run_project(&project, None).unwrap();
    let yaml = reduction::to_yaml(&graph).unwrap();
    for id in [
        "DEC-001",
        "DEC-002",
        "layout-doc",
        "LAY-001",
        "LAY-002",
        "OQ-001",
        "OQ-002",
    ] {
        assert!(yaml.contains(id), "{id} did not reach the graph");
    }

    let layout = LayoutContext::discover(&project, None).unwrap();
    let module: toml::Value = toml::from_str(
        &std::fs::read_to_string(project.join("specs/modules/parser.toml")).unwrap(),
    )
    .unwrap();
    let ctx = prompt::decision_context(&layout, &module).unwrap();
    assert!(
        ctx.decisions.iter().any(|d| d.id == "DEC-001"),
        "parser's prompt does not carry DEC-001"
    );

    let coverage = analyzer(&project, &["report", "coverage"]);
    assert!(
        String::from_utf8_lossy(&coverage.stdout).contains("LAY-001"),
        "the coverage matrix does not list LAY-001:\n{}",
        text(&coverage)
    );
}

/// Measured before this layout was read: with the files split by hand, the
/// open-question gate passed — it read the combined file only.
#[test]
fn the_open_question_gate_refuses_in_the_per_entry_layout() {
    let project = build_per_entry_project("gate");
    let layout = LayoutContext::discover(&project, None).unwrap();
    let refusal = prompt::check_codegen_gate(&layout, "writer")
        .unwrap()
        .expect("writer must be refused: OQ-002 is open");
    assert_eq!(refusal.oq_id, "OQ-002");
    assert_eq!(refusal.oq_title, "Open blocker");
    assert!(
        refusal
            .message
            .contains("`specs/open_questions/OQ-002.toml`"),
        "the refusal does not name the entry's file:\n{}",
        refusal.message
    );
    assert!(prompt::check_codegen_gate(&layout, "parser")
        .unwrap()
        .is_none());
}

/// `spec_version = "2.0"` written by hand, entries still in the `1.x`
/// files. Measured before the refusal existed: `gen` generated the blocked
/// module at exit 0 and `validate` reported 0 errors — the per-entry reader
/// did not see the open question.
#[test]
fn a_2x_project_with_entries_in_1x_files_refuses_instead_of_losing_them() {
    let project = tmp_workspace("legacy").join("proj");
    bootstrap_at(&project, "library-api", "legacy_fixture");
    write(
        &project.join("specs/requirements.toml"),
        "[[source]]\nid = \"d\"\nkind = \"derived\"\ntitle = \"d\"\n\n[[requirement]]\nid = \"LAY-002\"\ntext = \"w\"\nnature = \"module\"\nsource = [\"d\"]\n",
    );
    write(
        &project.join("specs/open_questions.toml"),
        "[[open_question]]\nid = \"OQ-002\"\ntitle = \"Open blocker\"\nstatus = \"open\"\ntext = \"?\"\naffects = [\"LAY-002\"]\n",
    );
    write(
        &project.join("specs/modules/writer.toml"),
        "name = \"writer\"\nvisibility = \"crate\"\ndescription = \"Writes.\"\nsatisfies = [\"LAY-002\"]\n",
    );
    set_spec_version(&project, "2.0");

    let errors = error_rules(&project);
    for file in ["requirements.toml", "open_questions.toml"] {
        assert!(
            errors
                .iter()
                .any(|(r, m)| r == "ENTRY-LEGACY-001" && m.contains(file) && m.contains("migrate")),
            "no ENTRY-LEGACY-001 error for {file}: {errors:#?}"
        );
    }
}

#[test]
fn a_schema_error_in_one_entry_file_names_that_file() {
    let project = build_per_entry_project("schema");
    write(
        &project.join("specs/decisions/DEC-001.toml"),
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"t\"\nstatus = \"bogus_status\"\ntext = \"x\"\nnot_a_field = 1\n",
    );
    write(
        &project.join("specs/open_questions/OQ-001.toml"),
        "[[open_question]]\nid = \"OQ-001\"\ntitle = \"t\"\nstatus = \"open\"\ntext = \"x\"\n",
    );
    let errors = error_rules(&project);
    assert!(
        errors
            .iter()
            .any(|(r, m)| r == "SCHEMA-DEC" && m.starts_with("DEC-001.toml:")),
        "no SCHEMA-DEC error naming DEC-001.toml: {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|(r, m)| r == "SCHEMA-OQ" && m.starts_with("OQ-001.toml:")),
        "no SCHEMA-OQ error naming OQ-001.toml (affects is required): {errors:#?}"
    );
}

/// The combined `1.x` files get the same schema check. Before it, a bogus
/// status and an unknown field in `decisions.toml` validated at exit 0.
#[test]
fn the_combined_layout_checks_decisions_and_open_questions_against_their_schemas() {
    let project = tmp_workspace("combined-schema").join("proj");
    bootstrap_at(&project, "library-api", "combined_fixture");
    to_1x(&project);
    write(
        &project.join("specs/decisions.toml"),
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"t\"\nstatus = \"bogus_status\"\ntext = \"x\"\n",
    );
    let errors = error_rules(&project);
    assert!(
        errors
            .iter()
            .any(|(r, m)| r == "SCHEMA-DEC" && m.starts_with("decisions.toml:")),
        "no SCHEMA-DEC error: {errors:#?}"
    );
}

#[test]
fn a_file_whose_stem_is_not_its_id_is_an_error() {
    let project = build_per_entry_project("stem");
    std::fs::rename(
        project.join("specs/decisions/DEC-002.toml"),
        project.join("specs/decisions/DEC-009.toml"),
    )
    .unwrap();
    let errors = error_rules(&project);
    assert!(
        errors.iter().any(|(r, m)| r == "ENTRY-FILE-002"
            && m.contains("DEC-009.toml")
            && m.contains("`DEC-002.toml`")),
        "no ENTRY-FILE-002 error: {errors:#?}"
    );
}

#[test]
fn a_file_with_two_entries_or_another_kind_is_an_error() {
    let project = build_per_entry_project("shape");
    write(
        &project.join("specs/decisions/DEC-003.toml"),
        "[[decision]]\nid = \"DEC-003\"\ntitle = \"a\"\nstatus = \"accepted\"\ntext = \"a\"\n\n[[decision]]\nid = \"DEC-004\"\ntitle = \"b\"\nstatus = \"accepted\"\ntext = \"b\"\n",
    );
    write(
        &project.join("specs/requirements/layout-doc.toml"),
        "[[source]]\nid = \"layout-doc\"\nkind = \"derived\"\ntitle = \"misplaced\"\n",
    );
    let errors = error_rules(&project);
    for file in ["DEC-003.toml", "requirements/layout-doc.toml"] {
        assert!(
            errors
                .iter()
                .any(|(r, m)| r == "ENTRY-FILE-001" && m.contains(file)),
            "no ENTRY-FILE-001 error for {file}: {errors:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Workspaces: a member at `crates/<m>` reads its root, in the root's layout.

/// A `rust-workspace` root listing one `library-api` member at `crates/m`.
/// The member's module `m_mod` satisfies `M-001`.
fn build_workspace(tag: &str, version: &str) -> (PathBuf, PathBuf) {
    let root = tmp_workspace(tag).join("ws");
    bootstrap_at(&root, "rust-workspace", "demo-ws");
    // The member is bootstrapped (2.0) before the root names it, so the
    // bootstrap does not meet a root of another format.
    let member = root.join("crates/m");
    bootstrap_at(&member, "library-api", "m");
    std::fs::write(
        root.join("spec.toml"),
        format!(
            "spec_version = \"{version}\"\n\n[project]\nname = \"demo-ws\"\nprofile = \"rust-workspace\"\nlanguage = \"rust\"\n\n[paths]\nspecs = \"specs\"\nsource = \".\"\n\n[workspace]\n\n[[workspace.members]]\nname = \"m\"\npath = \"crates/m\"\n"
        ),
    )
    .unwrap();
    set_spec_version(&member, version);
    set_reduction_layout(&member, version);
    set_reduction_layout(&root, version);
    let reqs = "[[source]]\nid = \"m-doc\"\nkind = \"derived\"\ntitle = \"Member source\"\n\n[[requirement]]\nid = \"M-001\"\ntext = \"The member shall work.\"\nnature = \"module\"\nsource = [\"m-doc\"]\n";
    if version.starts_with('2') {
        write(
            &member.join("specs/sources/m-doc.toml"),
            "[[source]]\nid = \"m-doc\"\nkind = \"derived\"\ntitle = \"Member source\"\n",
        );
        write(
            &member.join("specs/requirements/M-001.toml"),
            "[[requirement]]\nid = \"M-001\"\ntext = \"The member shall work.\"\nnature = \"module\"\nsource = [\"m-doc\"]\n",
        );
    } else {
        write(&member.join("specs/requirements.toml"), reqs);
    }
    write(
        &member.join("specs/modules/m_mod.toml"),
        "name = \"m_mod\"\nvisibility = \"crate\"\ndescription = \"Member module.\"\nsatisfies = [\"M-001\"]\n",
    );
    (root, member)
}

fn member_module(member: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(member.join("specs/modules/m_mod.toml")).unwrap())
        .unwrap()
}

/// Before: the reader probed only the member's parent directory, so a
/// member at the documented `crates/<m>` path got no workspace decisions
/// (measured: 0).
#[test]
fn a_member_at_crates_reads_its_roots_decisions() {
    let (root, member) = build_workspace("member-decisions", "1.0");
    write(
        &root.join("specs/decisions.toml"),
        "[[decision]]\nid = \"DEC-100\"\ntitle = \"Workspace choice\"\nstatus = \"accepted\"\ntext = \"All members.\"\n",
    );
    let layout = LayoutContext::discover(&member, None).unwrap();
    let decisions = prompt::project_decisions(&layout).unwrap();
    assert!(
        decisions
            .iter()
            .any(|(id, _, _, ws)| id == "DEC-100" && *ws),
        "the member does not see the root's DEC-100: {decisions:?}"
    );
    let ctx = prompt::decision_context(&layout, &member_module(&member)).unwrap();
    assert!(
        ctx.decisions
            .iter()
            .any(|d| d.id == "DEC-100" && d.workspace),
        "m_mod's prompt does not carry the workspace decision"
    );
}

/// Before: workspace open questions reached a member's prompt only when the
/// root also held a decisions file.
#[test]
fn a_roots_open_questions_load_without_any_root_decision() {
    let (root, member) = build_workspace("member-oqs", "1.0");
    write(
        &root.join("specs/open_questions.toml"),
        "[[open_question]]\nid = \"OQ-100\"\ntitle = \"Workspace unknown\"\nstatus = \"open\"\ntext = \"?\"\naffects = [\"M-001\"]\n",
    );
    // The workspace bootstrap writes a comments-only decisions file; the
    // case under test is a root with none.
    let _ = std::fs::remove_file(root.join("specs/decisions.toml"));
    let layout = LayoutContext::discover(&member, None).unwrap();
    let ctx = prompt::decision_context(&layout, &member_module(&member)).unwrap();
    assert!(
        ctx.open_questions.iter().any(|q| q.id == "OQ-100"),
        "m_mod's prompt does not carry the workspace open question"
    );
    let refusal = prompt::check_codegen_gate(&layout, "m_mod")
        .unwrap()
        .expect("OQ-100 is open and affects M-001");
    assert_eq!(refusal.oq_id, "OQ-100");
    assert!(
        refusal
            .message
            .contains("the workspace's `specs/open_questions.toml`"),
        "{}",
        refusal.message
    );
}

#[test]
fn a_member_reads_a_per_entry_root_in_the_roots_layout() {
    let (root, member) = build_workspace("member-per-entry", "2.0");
    write(
        &root.join("specs/decisions/DEC-100.toml"),
        "[[decision]]\nid = \"DEC-100\"\ntitle = \"Workspace choice\"\nstatus = \"accepted\"\ntext = \"All members.\"\n",
    );
    write(
        &root.join("specs/open_questions/OQ-100.toml"),
        "[[open_question]]\nid = \"OQ-100\"\ntitle = \"Workspace unknown\"\nstatus = \"open\"\ntext = \"?\"\naffects = [\"M-001\"]\n",
    );
    let layout = LayoutContext::discover(&member, None).unwrap();
    let ctx = prompt::decision_context(&layout, &member_module(&member)).unwrap();
    assert!(ctx
        .decisions
        .iter()
        .any(|d| d.id == "DEC-100" && d.workspace));
    assert!(ctx.open_questions.iter().any(|q| q.id == "OQ-100"));
    let refusal = prompt::check_codegen_gate(&layout, "m_mod")
        .unwrap()
        .expect("OQ-100 is open and affects M-001");
    assert!(
        refusal
            .message
            .contains("the workspace's `specs/open_questions/OQ-100.toml`"),
        "{}",
        refusal.message
    );
}
