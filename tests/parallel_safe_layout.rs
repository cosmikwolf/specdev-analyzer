//! The parallel-safe project layout (`spec_version = "2.0"`) and the
//! `migrate` step that moves a `1.x` project into it.
//!
//! These tests state the target before the code exists. They fail on a
//! `1.x`-only analyzer and pass one by one as the layout lands:
//!
//! - one file per decision, requirement, source and open question
//!   (`specs/decisions/<id>.toml`, `specs/requirements/<id>.toml`,
//!   `specs/sources/<id>.toml`, `specs/open_questions/<id>.toml`);
//! - each moved file replaced by a tombstone an old analyzer cannot read;
//! - generation lineage split per module
//!   (`specs/specdev/gen-lineage/<Module>.toml`, `_project.toml`).
//!
//! What each test holds fixed:
//!
//! 1. `gen` still refuses on an open question after the move. Measured
//!    before this layout existed: with the files split by hand, `gen`
//!    generated code at exit 0 because the refusal read
//!    `specs/open_questions.toml` directly.
//! 2. The graph is the same before and after `migrate`, with provenance
//!    stripped (provenance names the source file, which moves).
//! 3. `validate` is clean after `migrate`.
//! 4. `report gen-plan` and `report gen-drift` say the same before and after.
//! 5. `migrate` keeps each entry's bytes and moves the header comment.
//! 6. `migrate` is idempotent and `--dry-run` writes nothing.
//! 7. The released `1.x` analyzer refuses a migrated tree loudly (ignored by
//!    default: it needs that binary, named by `SPECDEV_FIELD_ANALYZER`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    prompt, reduction,
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
        "specdev-parallel-safe-layout-{tag}-{}-{}",
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

const DECISIONS_HEADER: &str = "# Project decisions. One [[decision]] per choice.\n\n";

const DEC_001: &str = r#"[[decision]]
id = "DEC-001"
title = "Parse in one pass"
status = "accepted"
text = """
Parse the input in one pass.
The second line keeps its bytes.
"""
"#;

const DEC_002: &str = r#"[[decision]]
id = "DEC-002"
title = "Default for OQ-001"
status = "accepted"
text = "Working default for OQ-001."
defaults_for = ["OQ-001"]
"#;

const SRC_DOC: &str = r#"[[source]]
id = "layout-doc"
kind = "derived"
title = "Layout fixture source"
"#;

const REQ_001: &str = r#"[[requirement]]
id = "LAY-001"
text = "The parser shall parse."
nature = "module"
source = ["layout-doc"]
decided_by = ["DEC-001"]
"#;

const REQ_002: &str = r#"[[requirement]]
id = "LAY-002"
text = "The writer shall write the gated thing."
nature = "module"
source = ["layout-doc"]
"#;

const OQ_001: &str = r#"[[open_question]]
id = "OQ-001"
title = "Committed default"
status = "default_committed"
text = "Working assumption."
default_via = "DEC-002"
verify = "Run the parser on the sample."
affects = ["LAY-001"]
"#;

const OQ_002: &str = r#"[[open_question]]
id = "OQ-002"
title = "Open blocker"
status = "open"
text = "Unknown; blocks the writer."
affects = ["LAY-002"]
"#;

/// A `1.0` library-api project: two modules, one of them blocked by the
/// open question OQ-002, and one recorded generated file for the other.
fn build_v1_project(tag: &str) -> PathBuf {
    let target = tmp_workspace(tag).join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &plugin_root(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "layout_fixture".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let specs = target.join("specs");
    std::fs::write(
        specs.join("decisions.toml"),
        format!("{DECISIONS_HEADER}{DEC_001}\n{DEC_002}"),
    )
    .unwrap();
    std::fs::write(
        specs.join("requirements.toml"),
        format!("{SRC_DOC}\n{REQ_001}\n{REQ_002}"),
    )
    .unwrap();
    std::fs::write(
        specs.join("open_questions.toml"),
        format!("{OQ_001}\n{OQ_002}"),
    )
    .unwrap();
    std::fs::write(
        specs.join("modules/parser.toml"),
        "name = \"parser\"\nvisibility = \"crate\"\ndescription = \"Parses.\"\nsatisfies = [\"LAY-001\"]\nguided_by = [\"DEC-001\"]\n",
    )
    .unwrap();
    std::fs::write(
        specs.join("modules/writer.toml"),
        "name = \"writer\"\nvisibility = \"crate\"\ndescription = \"Writes; blocked by OQ-002.\"\nsatisfies = [\"LAY-002\"]\n",
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/parser.rs"), "pub(crate) fn parse() {}\n").unwrap();
    let rec = analyzer(
        &target,
        &["gen-record", "--module", "parser", "src/parser.rs"],
    );
    assert!(
        rec.status.success(),
        "fixture gen-record failed:\n{}",
        text(&rec)
    );
    target
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

fn migrate(project: &Path) -> Output {
    let root = plugin_root();
    let out = analyzer(
        project,
        &["migrate", "--plugin-root", root.to_str().unwrap()],
    );
    assert!(out.status.success(), "migrate failed:\n{}", text(&out));
    // Without this, a `migrate` that moves nothing passes every "unchanged
    // across migrate" assertion below.
    assert_eq!(
        spec_version(project),
        "2.0",
        "migrate did not move the project to 2.0:\n{}",
        text(&out)
    );
    out
}

/// Every regular file under `root`, relative path → bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The reduction graph as a value with every `provenance` removed and every
/// sequence sorted, so that moving entries between files compares equal.
fn graph_without_provenance(project: &Path) -> serde_json::Value {
    fn normalize(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                map.remove("provenance");
                map.values_mut().for_each(normalize);
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(normalize);
                items.sort_by_key(|i| i.to_string());
            }
            _ => {}
        }
    }
    let (graph, _report) = reduction::run_project(project, None).expect("reduction::run_project");
    let yaml = reduction::to_yaml(&graph).unwrap();
    let mut value: serde_json::Value = serde_yaml::from_str(&yaml).unwrap();
    normalize(&mut value);
    value
}

fn spec_version(project: &Path) -> String {
    let raw = std::fs::read_to_string(project.join("spec.toml")).unwrap();
    let doc: toml::Value = toml::from_str(&raw).unwrap();
    doc["spec_version"].as_str().unwrap().to_string()
}

fn assert_tombstone(project: &Path, old: &str, new_dir: &str) {
    let raw = std::fs::read_to_string(project.join(old))
        .unwrap_or_else(|e| panic!("{old} should remain as a tombstone: {e}"));
    let first = raw.lines().next().unwrap_or("");
    assert!(
        first.starts_with(&format!("MOVED to {new_dir}"))
            && first.contains("specdev-analyzer migrate"),
        "{old}: first line is not the tombstone text:\n{first}"
    );
    assert!(
        toml::from_str::<toml::Value>(&raw).is_err(),
        "{old}: a tombstone must not be valid TOML, or an old analyzer reads it as empty"
    );
}

// ---------------------------------------------------------------------------

/// Passes on a `1.x` analyzer too: the fixture is sound before any move, so
/// a failure in a test below is the migration's, not the fixture's.
#[test]
fn fixture_is_sound_in_the_1x_layout() {
    let project = build_v1_project("baseline");
    let out = analyzer(&project, &["validate"]);
    assert!(
        out.status.success() && text(&out).contains("0 error(s)"),
        "the 1.x fixture does not validate clean:\n{}",
        text(&out)
    );
    let layout = LayoutContext::discover(&project, None).unwrap();
    let refusal = prompt::check_codegen_gate(&layout, "writer").unwrap();
    assert_eq!(refusal.map(|r| r.oq_id).as_deref(), Some("OQ-002"));
    assert!(prompt::check_codegen_gate(&layout, "parser")
        .unwrap()
        .is_none());
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
}

#[test]
fn gen_still_refuses_on_an_open_question_after_migrate() {
    let project = build_v1_project("oq-gate");
    migrate(&project);

    assert_eq!(spec_version(&project), "2.0");
    assert!(project.join("specs/open_questions/OQ-002.toml").is_file());

    let layout = LayoutContext::discover(&project, None).expect("discover a 2.0 project");
    let refusal = prompt::check_codegen_gate(&layout, "writer")
        .expect("gate")
        .expect("writer must stay refused: OQ-002 is open");
    assert_eq!(refusal.oq_id, "OQ-002");
    assert!(
        prompt::check_codegen_gate(&layout, "parser")
            .expect("gate")
            .is_none(),
        "parser is reachable only through a default_committed question"
    );

    let out = analyzer(&project, &["gen", "--module", "writer"]);
    assert!(!out.status.success(), "gen must refuse:\n{}", text(&out));
    assert!(
        text(&out).contains("Refusing to generate code for module `writer`"),
        "refusal text missing:\n{}",
        text(&out)
    );
}

#[test]
fn migrate_keeps_the_graph_and_validates_clean() {
    let project = build_v1_project("graph");
    let before = graph_without_provenance(&project);
    migrate(&project);
    let after = graph_without_provenance(&project);
    assert_eq!(before, after, "the graph changed across migrate");

    let out = analyzer(&project, &["validate"]);
    assert!(
        out.status.success() && text(&out).contains("0 error(s)"),
        "validate is not clean after migrate:\n{}",
        text(&out)
    );
}

#[test]
fn migrate_keeps_gen_plan_and_gen_drift() {
    let project = build_v1_project("gen-reports");
    let plan_before = analyzer(&project, &["report", "gen-plan"]);
    let drift_before = analyzer(&project, &["report", "gen-drift"]);
    migrate(&project);
    let plan_after = analyzer(&project, &["report", "gen-plan"]);
    let drift_after = analyzer(&project, &["report", "gen-drift"]);

    assert_eq!(
        text(&plan_before),
        text(&plan_after),
        "report gen-plan changed"
    );
    assert_eq!(
        text(&drift_before),
        text(&drift_after),
        "report gen-drift changed"
    );
    assert!(project
        .join("specs/specdev/gen-lineage/parser.toml")
        .is_file());
    assert_tombstone(
        &project,
        "specs/specdev/gen-lineage.toml",
        "specs/specdev/gen-lineage/",
    );
}

#[test]
fn migrate_splits_entries_keeps_their_bytes_and_leaves_tombstones() {
    let project = build_v1_project("split");
    migrate(&project);

    let read = |rel: &str| {
        std::fs::read_to_string(project.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    };
    for (rel, entry) in [
        ("specs/decisions/DEC-001.toml", DEC_001),
        ("specs/decisions/DEC-002.toml", DEC_002),
        ("specs/sources/layout-doc.toml", SRC_DOC),
        ("specs/requirements/LAY-001.toml", REQ_001),
        ("specs/requirements/LAY-002.toml", REQ_002),
        ("specs/open_questions/OQ-001.toml", OQ_001),
        ("specs/open_questions/OQ-002.toml", OQ_002),
    ] {
        assert!(
            read(rel).contains(entry),
            "{rel} does not keep the entry's bytes:\n{}",
            read(rel)
        );
    }
    assert!(
        read("specs/decisions/README.md").contains(DECISIONS_HEADER.trim()),
        "the decisions.toml header comment did not move to specs/decisions/README.md"
    );

    assert_tombstone(&project, "specs/decisions.toml", "specs/decisions/");
    assert_tombstone(&project, "specs/requirements.toml", "specs/requirements/");
    assert_tombstone(
        &project,
        "specs/open_questions.toml",
        "specs/open_questions/",
    );
}

#[test]
fn migrate_is_idempotent_and_dry_run_writes_nothing() {
    let project = build_v1_project("idempotent");

    let before = snapshot(&project);
    let root = plugin_root();
    let dry = analyzer(
        &project,
        &[
            "migrate",
            "--dry-run",
            "--plugin-root",
            root.to_str().unwrap(),
        ],
    );
    assert!(dry.status.success(), "dry run failed:\n{}", text(&dry));
    assert!(
        String::from_utf8_lossy(&dry.stdout).contains("spec_version"),
        "the dry run does not name the format step:\n{}",
        text(&dry)
    );
    assert_eq!(before, snapshot(&project), "--dry-run wrote to disk");

    migrate(&project);
    assert_eq!(spec_version(&project), "2.0");
    let once = snapshot(&project);
    migrate(&project);
    assert_eq!(
        once,
        snapshot(&project),
        "a second migrate changed the tree"
    );
}

#[test]
#[ignore = "needs the released 1.x analyzer: SPECDEV_FIELD_ANALYZER=<path> cargo test -- --ignored"]
fn released_1x_analyzer_refuses_a_migrated_tree_loudly() {
    let field = std::env::var("SPECDEV_FIELD_ANALYZER")
        .expect("set SPECDEV_FIELD_ANALYZER to the released 1.x specdev-analyzer binary");
    let project = build_v1_project("field-refuses");
    migrate(&project);

    for args in [&["validate"][..], &["gen", "--module", "parser"][..]] {
        let out = Command::new(&field)
            .args(args)
            .current_dir(&project)
            .env("SPECDEV_PLUGIN_ROOT", plugin_root())
            .output()
            .expect("run the field analyzer");
        assert!(
            !out.status.success(),
            "the field analyzer accepted a migrated tree on {args:?}:\n{}",
            text(&out)
        );
    }
}
