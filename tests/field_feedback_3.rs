//! Third round of field feedback from a firmware workspace: ids across the
//! workspace and member tiers, and workspace open questions that name a
//! requirement id.

use crate::ci;

use ci::tree;

fn decision(id: &str, title: &str) -> String {
    format!("[[decision]]\nid = \"{id}\"\ntitle = \"{title}\"\nstatus = \"accepted\"\ntext = \"Why.\"\n")
}

fn open_question(id: &str, affects: &[&str]) -> String {
    let list: Vec<String> = affects.iter().map(|a| format!("\"{a}\"")).collect();
    format!(
        "[[open_question]]\nid = \"{id}\"\ntitle = \"Question {id}\"\nstatus = \"open\"\ntext = \"?\"\naffects = [{}]\n",
        list.join(", ")
    )
}

/// A `rust-workspace` root with two `library-api` members, `one` and `two`.
fn workspace(tag: &str) -> (ci::Tree, std::path::PathBuf, std::path::PathBuf) {
    let root = tree(tag, "rust-workspace", &[]);
    let spec = root.read("spec.toml");
    let start = spec
        .find("\n[[workspace.members]]")
        .map(|i| i + 1)
        .expect("template lists a member");
    root.write(
        "spec.toml",
        &format!(
            "{}[[workspace.members]]\nname = \"one\"\npath = \"crates/one\"\n\n\
             [[workspace.members]]\nname = \"two\"\npath = \"crates/two\"\n",
            &spec[..start]
        ),
    );
    let mut dirs = Vec::new();
    for name in ["one", "two"] {
        let member = tree(&format!("{tag}-{name}"), "library-api", &[]);
        let dest = root.root.join("crates").join(name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::rename(&member.root, &dest).unwrap();
        dirs.push(dest);
    }
    let emit = root.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let two = dirs.pop().unwrap();
    (root, dirs.pop().unwrap(), two)
}

fn member(dir: &std::path::Path, plugin: &std::path::Path) -> ci::Tree {
    ci::Tree {
        root: dir.to_path_buf(),
        plugin: plugin.to_path_buf(),
    }
}

/// A member decision or open question with a workspace-tier id is an error,
/// and the member's own entry is the one its context shows.
#[test]
fn a_member_id_may_not_repeat_a_workspace_id() {
    let (root, one, _) = workspace("tier-ids");
    root.write(
        "specs/decisions/DEC-001.toml",
        &decision("DEC-001", "Root decision"),
    )
    .write(
        "specs/open_questions/OQ-001.toml",
        &open_question("OQ-001", &[]).replace("affects = []\n", ""),
    );
    let m = member(&one, &root.plugin);
    m.write("specs/decisions/DEC-001.toml", &decision("DEC-001", "Member decision"))
        .write("specs/decisions/DEC-002.toml", &decision("DEC-002", "Only here"))
        .write(
            "specs/modules/guided.toml",
            "name = \"guided\"\nvisibility = \"crate\"\ndescription = \"Guided.\"\nguided_by = [\"DEC-001\"]\n",
        )
        .write("src/guided.rs", "pub(crate) fn f() {}\n")
        .write("specs/open_questions/OQ-001.toml", &open_question("OQ-001", &["REQ-NONE-001"]));
    let c = m.check(&[]);
    c.fires("workspace_id_collision", "DEC-001", "error")
        .says("workspace_id_collision", "DEC-001", "Root decision")
        .fires("workspace_id_collision", "OQ-001", "error")
        .silent("workspace_id_collision", "DEC-002");
    let out = m.analyzer(&["report", "decisions", "--module", "guided"]);
    let doc: serde_json::Value = serde_yaml::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["modules"][0]["decisions"][0]["title"],
        "Member decision",
        "the member's own DEC-001 guides its module:\n{}",
        ci::text(&out)
    );
}

/// A member reference to a workspace-tier decision does not resolve, and
/// the finding says why and what to do (#250).
#[test]
fn a_member_reference_to_a_workspace_decision_explains_the_tiers() {
    let (root, one, _) = workspace("tier-ref");
    root.write(
        "specs/decisions/DEC-001.toml",
        &decision("DEC-001", "Root decision"),
    );
    let m = member(&one, &root.plugin);
    m.write(
        "specs/modules/guided.toml",
        "name = \"guided\"\nvisibility = \"crate\"\ndescription = \"Guided.\"\nguided_by = [\"DEC-001\"]\n",
    )
    .write("src/guided.rs", "pub(crate) fn f() {}\n");
    let out = m.analyzer(&["validate"]);
    let text = ci::text(&out);
    assert!(
        text.contains("references unknown Decision `DEC-001`")
            && text.contains("`DEC-001` is a workspace-tier entry (workspace root: specs/decisions/DEC-001.toml)")
            && text.contains("so remove this reference"),
        "expected the cross-tier note:\n{text}"
    );
}

/// A workspace open question that names an id two members define gates
/// both; one that names an id no member defines gates nothing. Both warn at
/// the root.
#[test]
fn workspace_affects_must_name_one_members_requirement() {
    let (root, one, two) = workspace("ws-affects");
    for dir in [&one, &two] {
        member(dir, &root.plugin).write(
            "specs/requirements/REQ-SHARED-001.toml",
            &ci::requirement("REQ-SHARED-001", "planned = true\n"),
        );
    }
    member(&one, &root.plugin).write(
        "specs/requirements/REQ-ONLY-001.toml",
        &ci::requirement("REQ-ONLY-001", "planned = true\n"),
    );
    root.write(
        "specs/open_questions/OQ-WS-001.toml",
        &open_question("OQ-WS-001", &["REQ-SHARED-001"]),
    )
    .write(
        "specs/open_questions/OQ-WS-002.toml",
        &open_question("OQ-WS-002", &["REQ-GONE-001"]),
    )
    .write(
        "specs/open_questions/OQ-WS-003.toml",
        &open_question("OQ-WS-003", &["REQ-ONLY-001"]),
    );
    let c = root.check(&[]);
    c.fires("workspace_affects_ambiguous", "OQ-WS-001", "warning")
        .says(
            "workspace_affects_ambiguous",
            "OQ-WS-001",
            "crates/one, crates/two",
        )
        .fires("workspace_affects_nothing", "OQ-WS-002", "warning")
        .silent("workspace_affects_ambiguous", "OQ-WS-003")
        .silent("workspace_affects_nothing", "OQ-WS-003");
}

// ---- Requirement trees: a parent is covered by its children.

/// A built module `Built` that satisfies `REQ-LEAF-001`, which names its test.
fn tree_project(tag: &str) -> ci::Tree {
    let t = tree(tag, "firmware-layered", &[]);
    t.write(
        "specs/modules/Built.toml",
        &ci::fw_module(
            "Built",
            "satisfies = [\"REQ-LEAF-001\", \"REQ-LEAF-002\"]\n",
        ),
    )
    .write(
        "src/built.rs",
        "pub fn built() {}\n#[test]\nfn built_works() {}\n",
    )
    .write(
        "specs/requirements/REQ-TOP-001.toml",
        &ci::requirement("REQ-TOP-001", ""),
    )
    .write(
        "specs/requirements/REQ-LEAF-001.toml",
        &ci::requirement(
            "REQ-LEAF-001",
            "satisfies = [\"REQ-TOP-001\"]\nverified_by = [\"built_works\"]\n",
        ),
    )
    // A second parent with a child that has no test.
    .write(
        "specs/requirements/REQ-TOP-002.toml",
        &ci::requirement("REQ-TOP-002", ""),
    )
    .write(
        "specs/requirements/REQ-LEAF-002.toml",
        &ci::requirement(
            "REQ-LEAF-002",
            "satisfies = [\"REQ-TOP-002\", \"REQ-TOP-001\"]\n",
        ),
    );
    t
}

#[test]
fn a_parent_is_covered_by_its_children() {
    let t = tree_project("rollup");
    let c = t.check(&[]);
    c.silent("requirements_have_satisfiers", "REQ-TOP-001")
        .silent("requirements_have_tests", "REQ-TOP-001")
        .silent("requirements_have_satisfiers", "REQ-TOP-002")
        .silent("requirements_have_tests", "REQ-TOP-002")
        // The defect is reported once, on the child.
        .fires("requirements_have_tests", "REQ-LEAF-002", "error");
    let cov = t.analyzer(&["report", "coverage"]);
    let doc: serde_json::Value = serde_yaml::from_slice(&cov.stdout).unwrap();
    assert!(!doc["orphans"].to_string().contains("REQ-TOP-"), "{doc}");
}

#[test]
fn a_cycle_of_parents_is_an_error_and_exempts_nothing() {
    let t = tree_project("cycle");
    t.write(
        "specs/requirements/REQ-CYC-001.toml",
        &ci::requirement("REQ-CYC-001", "satisfies = [\"REQ-CYC-002\"]\n"),
    )
    .write(
        "specs/requirements/REQ-CYC-002.toml",
        &ci::requirement("REQ-CYC-002", "satisfies = [\"REQ-CYC-001\"]\n"),
    );
    let c = t.check(&[]);
    c.fires("requirement_parents_acyclic", "REQ-CYC-001", "error")
        .fires("requirements_have_satisfiers", "REQ-CYC-001", "error")
        .fires("requirements_have_satisfiers", "REQ-CYC-002", "error");
}

#[test]
fn a_parent_whose_children_are_all_retired_is_a_leaf_again() {
    let t = tree_project("retired-children");
    t.write(
        "specs/decisions/DEC-T-001.toml",
        &decision("DEC-T-001", "Moot"),
    )
    .write(
        "specs/requirements/REQ-TOP-003.toml",
        &ci::requirement("REQ-TOP-003", ""),
    )
    .write(
        "specs/requirements/retired/REQ-OLD-003.toml",
        &ci::requirement(
            "REQ-OLD-003",
            "satisfies = [\"REQ-TOP-003\"]\nretired_by = \"DEC-T-001\"\n",
        ),
    );
    t.check(&[])
        .fires("requirements_have_satisfiers", "REQ-TOP-003", "error")
        .fires("requirements_have_tests", "REQ-TOP-003", "error");
}

#[test]
fn report_requirements_shows_the_tree() {
    let t = tree_project("report-tree");
    let out = t.analyzer(&["report", "requirements"]);
    assert!(out.status.success(), "{}", ci::text(&out));
    let doc: serde_json::Value = serde_yaml::from_slice(&out.stdout).unwrap();
    let roots: Vec<&str> = doc["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(roots, ["REQ-TOP-001", "REQ-TOP-002"], "{doc}");
    let top1 = &doc["requirements"][0];
    let kids: Vec<&str> = top1["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(kids, ["REQ-LEAF-001", "REQ-LEAF-002"], "{doc}");
    // A two-parent node is shown under both, and says so.
    assert_eq!(
        top1["children"][1]["parents"],
        serde_json::json!(["REQ-TOP-001", "REQ-TOP-002"]),
        "{doc}"
    );
    assert_eq!(
        top1["children"][0]["satisfied_by"],
        serde_json::json!(["Built"]),
        "{doc}"
    );
    let text = t.analyzer(&["report", "requirements", "--format", "text"]);
    let s = String::from_utf8_lossy(&text.stdout);
    assert!(s.contains("REQ-TOP-001\n  REQ-LEAF-001"), "{s}");
}

/// #251: an accepted decision that reaches no module is reported as info,
/// and a `guided_by` that names a superseded decision is a warning.
#[test]
fn decisions_that_guide_nothing_are_reported() {
    let t = tree("dec-reach", "library-api", &[]);
    t.write(
        "specs/decisions/DEC-001.toml",
        &decision("DEC-001", "Linked"),
    )
    .write(
        "specs/decisions/DEC-002.toml",
        &decision("DEC-002", "Unlinked"),
    )
    .write(
        "specs/decisions/DEC-003.toml",
        &decision("DEC-003", "Old").replace("\"accepted\"", "\"superseded\""),
    )
    .write(
        "specs/decisions/DEC-004.toml",
        &format!("{}supersedes = [\"DEC-003\"]\n", decision("DEC-004", "New")),
    )
    .write(
        "specs/modules/guided.toml",
        "name = \"guided\"\nvisibility = \"crate\"\ndescription = \"Guided.\"\n\
             guided_by = [\"DEC-001\", \"DEC-003\", \"DEC-004\"]\n",
    )
    .write("src/guided.rs", "pub(crate) fn f() {}\n");
    t.check(&[])
        .fires("decision_reaches_nothing", "DEC-002", "info")
        .silent("decision_reaches_nothing", "DEC-001")
        .silent("decision_reaches_nothing", "DEC-003")
        .fires(
            "decision_reference_superseded",
            "guided -> DEC-003",
            "warning",
        )
        .silent("decision_reference_superseded", "guided -> DEC-001");
}
