//! A spec file that does not parse is a counted finding, not an abort (#196).
//!
//! The reduction stops at the first file it cannot parse. `validate` used to
//! propagate that error: exit 2, the raw parse message, no count line, and
//! every finding already collected — `LAYOUT-005` for the same file included —
//! dropped. It now reports `REDUCE-001` (the graph was not built, so no graph
//! check ran), keeps the rest of the report, prints the count line, and exits
//! 1 like any other error finding.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn project(tag: &str) -> PathBuf {
    let target = std::env::temp_dir()
        .join(format!(
            "specdev-validate-parse-{tag}-{}",
            std::process::id()
        ))
        .join("proj");
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    std::fs::create_dir_all(target.join("specs/modules")).unwrap();
    target
}

fn validate(target: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .args(["validate", "--no-color", "--project"])
        .arg(target)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap(), text)
}

fn count_line(text: &str) -> &str {
    text.lines()
        .find(|l| l.contains("error(s),"))
        .unwrap_or_else(|| panic!("no count line:\n{text}"))
}

#[test]
fn unparsable_module_is_counted_and_the_rest_of_the_report_survives() {
    let target = project("module");
    std::fs::write(target.join("specs/modules/Broken.toml"), "name = \n").unwrap();
    // A second module with a schema error of its own: its finding must
    // still be in the report.
    std::fs::write(
        target.join("specs/modules/Other.toml"),
        "name = \"Other\"\nnot_a_field = 1\n",
    )
    .unwrap();

    let (code, text) = validate(&target);
    assert_eq!(code, 1, "an unparsable file is an error finding\n{text}");
    assert!(
        text.contains("[LAYOUT-005]") && text.contains("Broken.toml"),
        "{text}"
    );
    assert!(text.contains("[REDUCE-001]"), "{text}");
    assert!(text.contains("no graph check ran"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("[SCHEMA]") && l.contains("Other.toml")),
        "the other module's findings are kept\n{text}"
    );
    let counts = count_line(&text);
    assert!(!counts.starts_with("0 error(s)"), "{counts}");
}

#[test]
fn unparsable_decisions_file_is_counted_too() {
    // Not only modules: every file the reduction reads stops it the same way.
    let target = project("decisions");
    std::fs::create_dir_all(target.join("specs/decisions")).unwrap();
    std::fs::write(target.join("specs/decisions/DEC-001.toml"), "[[decision]\n").unwrap();

    let (code, text) = validate(&target);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("[REDUCE-001]"), "{text}");
    assert!(text.contains("DEC-001.toml"), "{text}");
    count_line(&text);
}
