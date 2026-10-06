//! Phase 44: the flxs1 session's second round of field feedback (#226–#231).
//!
//! Each test was written red first, against the 0.3.1 behaviour.

use crate::ci;

use ci::{fw_module, tree};

const RTIC_APP: &str = r#"#![no_std]
#![no_main]
#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    #[shared] struct Shared {}
    #[local] struct Local {}
    #[init] fn init(_cx: init::Context) -> (Shared, Local) { (Shared {}, Local {}) }
    #[task(priority = 1)] async fn built(_cx: built::Context) {}
}
"#;

fn task(name: &str, extra: &str) -> String {
    format!("[[rtic.tasks]]\nname = \"{name}\"\nkind = \"software\"\npriority = 1\n{extra}\n")
}

/// #231: the entry-level `planned` exit was not discoverable. The finding for a
/// missing pack entry names it; a planned module whose source exists and which
/// has pack entries is told it can flag the entries instead; a planned module
/// with no pack entries is not.
#[test]
fn the_planned_exits_are_named_where_they_apply() {
    let t = tree("planned-exits", "firmware-layered", &["rtic", "mk20dx"]);
    let main = "source_file = \"src/main.rs\"\n";
    t.write("src/main.rs", RTIC_APP)
        .write("src/plain.rs", "pub fn plain() {}\n")
        .write(
            "specs/modules/App.toml",
            &(fw_module("App", main) + &task("built", "") + &task("not_yet", "")),
        )
        .write(
            "specs/modules/Target.toml",
            &(fw_module("Target", &format!("planned = true\n{main}")) + &task("built", "")),
        )
        .write(
            "specs/modules/Plain.toml",
            &fw_module("Plain", "planned = true\n"),
        );
    let c = t.check(&[]);
    c.fires("pack_entity_in_code", "App.not_yet", "error")
        .says(
            "pack_entity_in_code",
            "App.not_yet",
            "set `planned = true` on this entry",
        )
        .fires("stale_planned", "Target", "warning")
        .says(
            "stale_planned",
            "Target",
            "`planned = true` on those entries instead",
        )
        .fires("stale_planned", "Plain", "warning");
    let plain = c
        .ran()
        .iter()
        .find(|f| f.rule == "stale_planned" && f.entity == "Plain")
        .unwrap();
    assert!(!plain.message.contains("entries"), "{}", plain.message);
}

/// #228: a module whose code lives at another path. The finding ranks the
/// files that define the module's names (at any visibility: task functions
/// are `pub(crate)`), and stays as it was when no file defines any.
#[test]
fn source_resolves_names_the_files_that_define_the_module() {
    let t = tree("source-hint", "firmware-layered", &[]);
    t.write(
        "specs/modules/FlashTask.toml",
        &(fw_module("FlashTask", "")
            + "[[actions]]\nname = \"flash_task\"\n\n[[actions]]\nname = \"init_buffer\"\n"),
    )
    .write(
        "specs/modules/Ghost.toml",
        &(fw_module("Ghost", "") + "[[actions]]\nname = \"never_written\"\n"),
    )
    .write(
        "src/tasks/flash.rs",
        "pub(crate) async fn flash_task() {}\nfn init_buffer() {}\n",
    )
    .write(
        "src/tasks/mod.rs",
        "pub(crate) mod flash;\npub fn init_buffer() {}\n",
    );
    let c = t.check(&[]);
    c.fires("source_resolves", "FlashTask", "error")
        .says(
            "source_resolves",
            "FlashTask",
            "candidates: `src/tasks/flash.rs` defines 2 of 2 name(s) (`flash_task`, `init_buffer`); \
             `src/tasks/mod.rs` defines 1 of 2 name(s) (`init_buffer`)",
        )
        .fires("source_resolves", "Ghost", "error");
    let ghost = c
        .ran()
        .iter()
        .find(|f| f.rule == "source_resolves" && f.entity == "Ghost")
        .unwrap();
    assert!(!ghost.message.contains("candidates"), "{}", ghost.message);
}

/// A `rust-workspace` root with two `library-api` members: `good` is clean,
/// `bad` has a built module whose source does not resolve.
fn workspace(tag: &str) -> ci::Tree {
    let root = tree(tag, "rust-workspace", &[]);
    let spec = root.read("spec.toml");
    let start = spec
        .find("\n[[workspace.members]]")
        .map(|i| i + 1)
        .expect("template lists a member");
    root.write(
        "spec.toml",
        &format!(
            "{}[[workspace.members]]\nname = \"good\"\npath = \"crates/good\"\n\n\
             [[workspace.members]]\nname = \"bad\"\npath = \"crates/bad\"\n",
            &spec[..start]
        ),
    );
    for (name, module) in [("good", None), ("bad", Some("missing"))] {
        let member = tree(&format!("{tag}-{name}"), "library-api", &[]);
        let dest = root.root.join("crates").join(name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::rename(&member.root, &dest).unwrap();
        if let Some(m) = module {
            std::fs::write(
                dest.join(format!("specs/modules/{m}.toml")),
                format!("name = \"{m}\"\nvisibility = \"crate\"\ndescription = \"No code.\"\n"),
            )
            .unwrap();
        }
    }
    let emit = root.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    root
}

/// #230: `check` at a workspace root checked only the root. Its summary now
/// says what it skipped; `--members` checks every member, reports each, and
/// exits with the worst result.
#[test]
fn check_members_covers_the_workspace() {
    let t = workspace("members");
    let root_only = t.analyzer(&["check"]);
    let stderr = String::from_utf8_lossy(&root_only.stderr);
    assert_eq!(root_only.status.code(), Some(0), "{}", ci::text(&root_only));
    assert!(
        stderr.contains("root only: 2 member(s) not checked (`check --members`)"),
        "{stderr}"
    );

    let all = t.analyzer(&["check", "--members"]);
    let (out, err) = (
        String::from_utf8_lossy(&all.stdout),
        String::from_utf8_lossy(&all.stderr),
    );
    assert_eq!(all.status.code(), Some(1), "{}", ci::text(&all));
    assert!(out.contains("member crates/good: 0 error(s)"), "{out}");
    assert!(out.contains("member crates/bad: 1 error(s)"), "{out}");
    assert!(
        out.contains("module `missing` has no resolvable source"),
        "{out}"
    );
    assert!(err.contains("across the root and 2 member(s)"), "{err}");

    let yaml = t.analyzer(&["check", "--members", "--format", "yaml"]);
    let doc: serde_json::Value = serde_yaml::from_slice(&yaml.stdout).unwrap();
    let members = doc["members"].as_array().expect("a members list");
    assert_eq!(members.len(), 2, "{doc}");
    assert_eq!(members[1]["path"], "crates/bad");
    assert_eq!(members[1]["counts"]["error"], 1, "{doc}");
    assert!(
        doc["findings"].is_array(),
        "the root keeps its own shape: {doc}"
    );
}

/// The sibling plugin checkout, or `None` (a standalone clone).
fn live_plugin() -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("specdev");
    p.join("scripts/session-context.sh").is_file().then_some(p)
}

/// Run the plugin's SessionStart hook in `project` with `path` as `PATH`.
fn hook(plugin: &std::path::Path, project: &std::path::Path, path: &str) -> String {
    let out = std::process::Command::new("sh")
        .arg(plugin.join("scripts/session-context.sh"))
        .current_dir(project)
        .env_clear()
        .env("PATH", path)
        .env("CLAUDE_PLUGIN_ROOT", plugin)
        .output()
        .expect("run the hook");
    assert_eq!(out.status.code(), Some(0), "the hook always exits 0");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// #226 / D116: the hook relays the analyzer's own version warning, says when
/// the analyzer is missing, prints nothing about versions when they agree, and
/// still relays when the project's rules copy makes it skip the rules text.
#[test]
fn the_session_hook_relays_the_analyzer_version_warning() {
    let Some(plugin) = live_plugin() else {
        eprintln!("skip: no sibling plugin checkout; the hook lives there");
        return;
    };
    let base = std::env::temp_dir().join(format!(
        "specdev-hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let project = base.join("proj");
    std::fs::create_dir_all(project.join("specs/specdev")).unwrap();
    std::fs::write(project.join("spec.toml"), "").unwrap();
    // A stub analyzer that is "older": it prints the warning on stderr only.
    let stub = base.join("stub");
    std::fs::create_dir_all(&stub).unwrap();
    let stub_bin = stub.join("specdev-analyzer");
    std::fs::write(
        &stub_bin,
        "#!/bin/sh\necho 'specdev-analyzer 0.0.1'\n\
         echo 'warning: [ANALYZER-VERSION-001] specdev-analyzer 0.0.1 is older than the 9.9.9 the specdev plugin needs' >&2\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The real analyzer, whose version equals what the plugin needs.
    let real = std::path::Path::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .parent()
        .unwrap()
        .to_path_buf();
    let sys = "/usr/bin:/bin";

    let older = hook(&plugin, &project, &format!("{}:{sys}", stub.display()));
    assert!(
        older.contains("specdev: warning: [ANALYZER-VERSION-001]"),
        "{older}"
    );
    assert!(older.contains("Tell the user once"), "{older}");

    let same = hook(&plugin, &project, &format!("{}:{sys}", real.display()));
    assert!(
        !same.contains("ANALYZER-VERSION") && !same.contains("not on PATH"),
        "{same}"
    );

    let missing = hook(&plugin, &project, sys);
    assert!(
        missing.contains("specdev-analyzer is not on PATH"),
        "{missing}"
    );

    // The project's own rules copy: the rules text is skipped, the warning is not.
    std::fs::create_dir_all(project.join(".claude/rules")).unwrap();
    std::fs::write(project.join(".claude/rules/specdev.md"), "rules\n").unwrap();
    let with_copy = hook(&plugin, &project, &format!("{}:{sys}", stub.display()));
    assert!(with_copy.contains("ANALYZER-VERSION-001"), "{with_copy}");
    assert!(
        !with_copy.contains("## "),
        "no rules text when the project has its copy: {with_copy}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
