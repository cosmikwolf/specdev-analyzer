//! The installed-analyzer version check: a binary older than the plugin's
//! `shared/min-analyzer-version` warns on stderr, on any command, without
//! changing the command's exit code or stdout.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const THIS: &str = env!("CARGO_PKG_VERSION");

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "specdev-analyzer-version-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A directory that passes as a plugin root, with `min` (if any) as its
/// minimum analyzer version.
fn plugin(root: &Path, name: &str, min: Option<&str>) -> PathBuf {
    let p = root.join(name);
    fs::create_dir_all(p.join(".claude-plugin")).unwrap();
    fs::write(p.join(".claude-plugin/plugin.json"), "{}").unwrap();
    if let Some(min) = min {
        fs::create_dir_all(p.join("shared")).unwrap();
        fs::write(p.join("shared/min-analyzer-version"), format!("{min}\n")).unwrap();
    }
    p
}

/// Run `validate` on an empty directory — a command that fails the same way
/// with or without the check — under a controlled plugin environment.
fn run(
    root: &Path,
    flag: Option<&Path>,
    specdev_env: Option<&Path>,
    claude_env: Option<&Path>,
) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"));
    cmd.env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT");
    if let Some(p) = specdev_env {
        cmd.env("SPECDEV_PLUGIN_ROOT", p);
    }
    if let Some(p) = claude_env {
        cmd.env("CLAUDE_PLUGIN_ROOT", p);
    }
    if let Some(p) = flag {
        cmd.arg("--plugin-root").arg(p);
    }
    let empty = root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let out = cmd
        .arg("validate")
        .arg("--project")
        .arg(&empty)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn older_binary_warns_and_changes_nothing_else() {
    let root = tmp("older");
    let newer = plugin(&root, "newer", Some("999.0.0"));
    let (base_code, base_out, _) = run(&root, None, None, None);
    let (code, out, err) = run(&root, None, Some(&newer), None);
    assert!(err.contains("[ANALYZER-VERSION-001]"), "{err}");
    assert!(
        err.contains(&format!(
            "specdev-analyzer {THIS} is older than the 999.0.0 the specdev plugin needs"
        )),
        "{err}"
    );
    assert!(err.contains("analyzer-install.md"), "{err}");
    let warning: Vec<_> = err
        .lines()
        .filter(|l| l.contains("ANALYZER-VERSION"))
        .collect();
    assert_eq!(warning.len(), 1, "one line\n{err}");
    assert_eq!(
        err.matches("ANALYZER-VERSION").count(),
        1,
        "one warning\n{err}"
    );
    assert_eq!(code, base_code, "the warning never changes the exit code");
    assert_eq!(out, base_out, "the warning never touches stdout");
}

#[test]
fn equal_or_newer_binary_and_absent_file_are_silent() {
    let root = tmp("silent");
    for (name, min) in [
        ("equal", Some(THIS)),
        ("older-min", Some("0.0.1")),
        ("no-file", None),
    ] {
        let p = plugin(&root, name, min);
        let (_, _, err) = run(&root, None, Some(&p), None);
        assert!(!err.contains("ANALYZER-VERSION"), "{name}: {err}");
    }
    // No plugin root named at all.
    let (_, _, err) = run(&root, None, None, None);
    assert!(!err.contains("ANALYZER-VERSION"), "{err}");
}

#[test]
fn unparsable_file_is_its_own_warning() {
    let root = tmp("garbage");
    let p = plugin(&root, "garbage", Some("v0.2"));
    let (_, _, err) = run(&root, None, Some(&p), None);
    assert!(err.contains("[ANALYZER-VERSION-002]"), "{err}");
    assert!(err.contains("read `v0.2`"), "{err}");
}

#[test]
fn flag_wins_over_env_and_a_non_plugin_path_is_skipped() {
    let root = tmp("precedence");
    let newer = plugin(&root, "newer", Some("999.0.0"));
    let same = plugin(&root, "same", Some(THIS));

    // --plugin-root is read first.
    let (_, _, err) = run(&root, Some(&newer), Some(&same), None);
    assert!(err.contains("[ANALYZER-VERSION-001]"), "{err}");
    let (_, _, err) = run(&root, Some(&same), Some(&newer), None);
    assert!(!err.contains("ANALYZER-VERSION"), "{err}");

    // A given --plugin-root that is not a plugin does not fall through to
    // the environment: that would check a plugin the command does not use.
    let not_plugin = root.join("not-a-plugin");
    fs::create_dir_all(&not_plugin).unwrap();
    let (_, _, err) = run(&root, Some(&not_plugin), Some(&newer), None);
    assert!(!err.contains("ANALYZER-VERSION"), "{err}");

    // SPECDEV_PLUGIN_ROOT naming a non-plugin directory falls through to
    // CLAUDE_PLUGIN_ROOT, and reports no plugin-root error of its own.
    let (_, _, err) = run(&root, None, Some(&not_plugin), Some(&newer));
    assert!(err.contains("[ANALYZER-VERSION-001]"), "{err}");
}

#[test]
fn version_flag_warns_too() {
    // `--version` is answered before any subcommand runs; it is also the
    // command a user runs to ask "am I current?".
    let root = tmp("version-flag");
    let newer = plugin(&root, "newer", Some("999.0.0"));
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .env("SPECDEV_PLUGIN_ROOT", &newer)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("specdev-analyzer {THIS}\n")
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("[ANALYZER-VERSION-001]"), "{err}");
}
