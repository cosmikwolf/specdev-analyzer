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
