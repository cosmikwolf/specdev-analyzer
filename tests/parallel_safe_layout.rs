//! The parallel-safe project layout (`spec_version = "2.0"`) and the
//! `migrate` step that moves a `1.x` project into it.
//!
//! These tests state the target before the code exists. They fail on a
//! `1.x`-only analyzer and pass one by one as the layout lands:
//!
//! - one file per decision, requirement, source and open question
//!   (`specs/decisions/<id>.toml`, `specs/requirements/<id>.toml`,
//!   `specs/sources/<id>.toml`, `specs/open_questions/<id>.toml`);
//! - each moved file replaced by a tombstone an old analyzer cannot read.
//!
//! What each test holds fixed:
//!
//! 1. The open-question gate still refuses after the move. Measured before
//!    this layout existed: with the files split by hand, the gate passed
//!    because it read `specs/open_questions.toml` directly.
//! 2. The graph is the same before and after `migrate`, with provenance
//!    stripped (provenance names the source file, which moves).
//! 3. `validate` is clean after `migrate`.
//! 5. `migrate` keeps each entry's bytes and moves the header comment.
//! 6. `migrate` is idempotent and `--dry-run` writes nothing.
//! 7. The released `1.x` analyzer refuses a migrated tree loudly (ignored by
//!    default: it needs that binary, named by `SPECDEV_FIELD_ANALYZER`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use specdev_analyzer::{layout::LayoutContext, prompt, reduction};

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
/// open question OQ-002. The base is the frozen `1.x` tree in
/// `tests/fixtures/v1_trees/library-api` — a bootstrap writes `2.0` now.
fn build_v1_project(tag: &str) -> PathBuf {
    let target = tmp_workspace(tag).join("proj");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_trees/library-api"),
        &target,
    );

    let specs = target.join("specs");
    // Git stores no empty directory, so a checkout of the frozen tree has no
    // `specs/modules/` until something writes into it.
    std::fs::create_dir_all(specs.join("modules")).unwrap();
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
    target
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
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
/// The relation kinds the project's inlined reductions declare.
fn relation_kinds_declared(project: &Path) -> std::collections::BTreeSet<String> {
    let dir = project.join("specs/specdev/reductions");
    let mut kinds = std::collections::BTreeSet::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
        for r in doc["relations"].as_sequence().into_iter().flatten() {
            if let Some(k) = r["kind"].as_str() {
                kinds.insert(k.to_string());
            }
        }
    }
    kinds
}

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
    // What the format removes or moves on purpose: removed
    // kinds and relations, the `Architecture` entity of a deleted
    // `architecture.toml`, and the attributes `migrate` adds.
    let removed = specdev_analyzer::field_cuts::REMOVED_KINDS;
    for (list, field) in [("entities", "kind"), ("relations", "kind")] {
        if let Some(items) = value.get_mut(list).and_then(|v| v.as_array_mut()) {
            items.retain(|e| {
                let k = e.get(field).and_then(|k| k.as_str()).unwrap_or("");
                !removed.iter().any(|r| k.ends_with(r)) && !k.ends_with("Architecture")
            });
            for e in items.iter_mut() {
                if let Some(a) = e.get_mut("attrs").and_then(|a| a.as_object_mut()) {
                    for k in ["verified_by", "test_pending", "planned"] {
                        a.remove(k);
                    }
                }
            }
        }
    }
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
    // Proves the drift parser used by the re-baseline test on today's
    // output. The frozen tree is read against the 2.0 plugin: its untouched
    // `spec.toml` reads `PluginMoved`, and the edited combined ledger, which
    // the plugin no longer ships, `Both`.
    let categories = profile_file_categories(&project);
    assert_eq!(
        categories.get("spec.toml").map(String::as_str),
        Some("PluginMoved")
    );
    assert_eq!(
        categories.get("specs/decisions.toml").map(String::as_str),
        Some("Both")
    );
}

#[test]
fn the_open_question_gate_still_refuses_after_migrate() {
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

    // The refusal points the user at the file to edit. After the move the
    // old path is a tombstone.
    assert!(
        !refusal.message.contains("specs/open_questions.toml")
            && refusal.message.contains("specs/open_questions/"),
        "the refusal names the tombstone, not the new location:\n{}",
        refusal.message
    );
}

#[test]
fn migrate_keeps_the_graph_and_validates_clean() {
    let project = build_v1_project("graph");
    let before = graph_without_provenance(&project);
    let v1_kinds = relation_kinds_declared(&project);
    migrate(&project);
    let mut after = graph_without_provenance(&project);
    // An unedited reduction takes the plugin's bytes, so a relation kind the
    // plugin added since the 1.x tree may appear. Every other relation, and
    // every entity, is unchanged.
    if let Some(rels) = after["relations"].as_array_mut() {
        rels.retain(|r| {
            let kind = r["kind"].as_str().unwrap_or_default();
            v1_kinds.contains(kind) || {
                assert!(
                    relation_kinds_declared(&project).contains(kind),
                    "relation `{kind}` is in no reduction"
                );
                false
            }
        });
    }
    assert_eq!(before, after, "the graph changed across migrate");

    let out = analyzer(&project, &["validate"]);
    assert!(
        out.status.success() && text(&out).contains("0 error(s)"),
        "validate is not clean after migrate:\n{}",
        text(&out)
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

    // The parallel-agent limit is written by migrate, default 1.
    let spec: toml::Value = toml::from_str(&read("spec.toml")).unwrap();
    assert_eq!(
        spec["project"]
            .get("max_parallel_agents")
            .and_then(|v| v.as_integer()),
        Some(1),
        "migrate did not write [project].max_parallel_agents = 1"
    );
    // The bootstrap-time plugin version is renamed; the old name is gone.
    let lineage: toml::Value = toml::from_str(&read("specs/specdev/lineage.toml")).unwrap();
    assert!(
        lineage["meta"].get("bootstrapped_with").is_some()
            && lineage["meta"].get("plugin_version").is_none(),
        "lineage.toml [meta] still has plugin_version, or lacks bootstrapped_with"
    );
}

/// `drift`'s profile-file categories, path → category.
fn profile_file_categories(project: &Path) -> BTreeMap<String, String> {
    let out = analyzer(project, &["drift"]);
    let report: serde_json::Value = serde_yaml::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("drift output is not YAML ({e}):\n{}", text(&out)));
    report["profile"]["files"]
        .as_array()
        .expect("drift lists profile files")
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                f["category"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Files migrate rewrites (spec.toml, inlined reductions) are re-baselined,
/// so they do not read as hand-edited and block a later `--refresh`; files
/// it moves leave the profile-file list. Against a plugin that moved to
/// 2.0, a clean file `migrate` rewrites to the plugin's bytes becomes
/// `Clean`; no file becomes newly `LocallyEdited` or `Both`.
#[test]
fn migrate_rebaselines_the_profile_files_it_rewrites() {
    let project = build_v1_project("rebaseline");
    let before = profile_file_categories(&project);
    migrate(&project);
    let after = profile_file_categories(&project);

    for moved in [
        "specs/decisions.toml",
        "specs/requirements.toml",
        "specs/open_questions.toml",
    ] {
        assert!(
            !after.contains_key(moved),
            "{moved} is still a profile file after the move"
        );
    }
    for (path, category) in &after {
        let earlier = before.get(path).map(String::as_str).unwrap_or("Clean");
        assert!(
            category == earlier || category == "Clean",
            "{path}: {earlier} before migrate, {category} after"
        );
    }
}

/// A 1.x project gets one `info` line naming the migration; a migrated one
/// gets none.
#[test]
fn validate_names_the_migration_once_and_only_before_it() {
    let count = |project: &Path| {
        let out = analyzer(project, &["validate"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .chain(String::from_utf8_lossy(&out.stderr).lines())
            .filter(|l| l.to_lowercase().contains("info") && l.contains("migrate"))
            .count()
    };
    let project = build_v1_project("info-line");
    assert_eq!(
        count(&project),
        1,
        "a 1.x project must get exactly one info line naming migrate"
    );
    migrate(&project);
    assert_eq!(
        count(&project),
        0,
        "a migrated project must get no migration info line"
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
