//! `validate` prints one `PLUGIN-AHEAD-001` info line when the named
//! plugin holds newer copies of files the project inlined or bootstrapped.
//! Decided by content hash (drift's classifier), never by version string.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn fixture_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-plugin-ahead-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// A private copy of the fixture plugin and a project bootstrapped from it.
fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let root = tmp(tag);
    let plugin = root.join("plugin");
    copy_dir(&fixture_plugin(), &plugin);
    let project = root.join("proj");
    fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &plugin,
        &BootstrapInputs {
            profile: "nextjs-app".into(),
            language: "typescript".into(),
            project_name: "demo".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    (plugin, project)
}

/// `validate` stderr+stdout lines naming the notice. `flag` is passed as
/// `--plugin-root`; `env` as `SPECDEV_PLUGIN_ROOT`.
fn notices(project: &Path, flag: Option<&Path>, env: Option<&Path>) -> Vec<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"));
    cmd.env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT");
    if let Some(p) = env {
        cmd.env("SPECDEV_PLUGIN_ROOT", p);
    }
    if let Some(p) = flag {
        cmd.arg("--plugin-root").arg(p);
    }
    let out = cmd
        .args(["--no-color", "validate", "--project"])
        .arg(project)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .filter(|l| l.contains("PLUGIN-AHEAD-001"))
        .map(str::to_string)
        .collect()
}

fn append(path: &Path, line: &str) {
    let mut s = fs::read_to_string(path).unwrap();
    s.push_str(line);
    fs::write(path, s).unwrap();
}

#[test]
fn a_current_project_is_silent() {
    let (plugin, project) = setup("current");
    assert!(notices(&project, Some(&plugin), None).is_empty());
    assert!(notices(&project, None, Some(&plugin)).is_empty());
}

#[test]
fn a_file_the_plugin_moved_is_announced_once() {
    let (plugin, project) = setup("moved");
    append(
        &plugin.join("profiles/nextjs-app/schemas/decisions.schema.yaml"),
        "\n# moved upstream\n",
    );
    let flag = notices(&project, Some(&plugin), None);
    assert_eq!(flag.len(), 1, "{flag:?}");
    assert!(
        flag[0].contains("1 inlined or profile file(s) are behind"),
        "{}",
        flag[0]
    );
    assert!(flag[0].contains("/spec-drift"), "{}", flag[0]);
    assert!(!flag[0].contains("also changed"), "{}", flag[0]);
    // The environment names the same plugin: same line.
    assert_eq!(notices(&project, None, Some(&plugin)), flag);
}

#[test]
fn a_file_changed_on_both_sides_is_counted_and_named() {
    let (plugin, project) = setup("both");
    append(
        &plugin.join("profiles/nextjs-app/profile.yaml"),
        "\n# moved upstream\n",
    );
    append(
        &project.join("specs/specdev/profile.yaml"),
        "\n# local edit\n",
    );
    let n = notices(&project, Some(&plugin), None);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(
        n[0].contains("(1 also changed in this project)"),
        "{}",
        n[0]
    );
}

#[test]
fn no_named_plugin_means_no_notice() {
    let (plugin, project) = setup("none");
    append(
        &plugin.join("profiles/nextjs-app/schemas/decisions.schema.yaml"),
        "\n# moved upstream\n",
    );
    assert!(notices(&project, None, None).is_empty());
}
