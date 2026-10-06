//! The CLI's own `--help` is user-facing documentation with no compiler
//! behind it. Two of its strings hand-enumerate the languages the schema
//! enums admit, and went stale when a language was admitted and nobody
//! swept them; others went on naming removed commands and files.
//!
//! The language tests drive every claim off the embedded `spec.schema.yaml`
//! `language` enum, so admitting a language turns them red until the
//! prose follows. The removed-terms test runs the lint's list over every
//! subcommand's long help.

use std::path::Path;

use clap::CommandFactory;
use specdev_analyzer::cli::Cli;

/// Every language the project-spec schema admits, in declaration order.
fn admitted_languages() -> Vec<String> {
    let embedded = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/embedded");
    let spec: serde_json::Value =
        serde_yaml::from_str(&std::fs::read_to_string(embedded.join("spec.schema.yaml")).unwrap())
            .unwrap();
    spec.pointer("/properties/project/properties/language/enum")
        .expect("spec.schema.yaml language enum")
        .as_array()
        .expect("language enum is an array")
        .iter()
        .map(|v| v.as_str().expect("enum member is a string").to_string())
        .collect()
}

fn long_help_for(subcommand: &str) -> String {
    Cli::command()
        .find_subcommand_mut(subcommand)
        .unwrap_or_else(|| panic!("no `{subcommand}` subcommand"))
        .render_long_help()
        .to_string()
}

/// Read the doc comment that opens with `marker` out of a source file and
/// return it as one line. Doc comments are invisible to the type system,
/// so the text itself is the only thing left to assert against.
fn doc_block_containing(rel_path: &str, marker: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_path);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel_path}: {e}"));
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("`{marker}` not found in {rel_path}"));
    let mut block = String::new();
    for line in &lines[start..] {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("///") {
            break;
        }
        block.push_str(trimmed.trim_start_matches("///").trim());
        block.push(' ');
    }
    block
}

/// `--language` takes any value the enums admit, and its help says so by
/// example. The example list must therefore hold every admitted value.
#[test]
fn language_flag_help_names_every_admitted_language() {
    let help = long_help_for("bootstrap");
    for lang in admitted_languages() {
        assert!(
            help.contains(&format!("`{lang}`")),
            "`bootstrap --help` does not name `{lang}`, which the \
             spec.schema.yaml language enum admits.\n--- rendered help ---\n{help}"
        );
    }
}

/// The same sentence is duplicated on the struct field the flag parses
/// into. Rustdoc renders it; nothing compiles it. Keep both in step.
#[test]
fn bootstrap_inputs_language_docstring_names_every_admitted_language() {
    let block = doc_block_containing("src/bootstrap.rs", "Project language; must match");
    for lang in admitted_languages() {
        assert!(
            block.contains(&format!("`{lang}`")),
            "`BootstrapInputs::language`'s docstring does not name `{lang}`.\n\
             --- doc block ---\n{block}"
        );
    }
}

/// The removed-terms pattern, read from the lint script so the list has one
/// home. The script holds it on one line as `REMOVED_TERMS_RE='…'`.
fn removed_terms_pattern() -> String {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/scripts/agnosticism-lint.sh");
    let text = std::fs::read_to_string(&script).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("REMOVED_TERMS_RE='"))
        .expect("agnosticism-lint.sh defines REMOVED_TERMS_RE on one line");
    line.trim_start_matches("REMOVED_TERMS_RE='")
        .trim_end_matches('\'')
        .to_string()
}

/// Every command's long help, keyed by its path (`report coverage`).
fn every_long_help() -> Vec<(String, String)> {
    fn walk(cmd: &mut clap::Command, path: String, out: &mut Vec<(String, String)>) {
        out.push((path.clone(), cmd.render_long_help().to_string()));
        let names: Vec<String> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .filter(|n| n != "help")
            .collect();
        for name in names {
            let sub = cmd.find_subcommand_mut(&name).unwrap();
            walk(sub, format!("{path} {name}").trim().to_string(), out);
        }
    }
    let mut out = Vec::new();
    let mut root = Cli::command();
    root.build();
    walk(&mut root, String::new(), &mut out);
    out
}

/// The help is the first text a user reads, so it must not name a removed
/// command, file or concept. The pattern is the lint's list, plus a bare
/// `codegen`: the analyzer generates no code, while a field name such as
/// `codegen_include` is still current and does not match. `grep -E` runs the
/// pattern, as in the lint, so both read it with the same engine.
#[test]
fn long_help_names_no_removed_term() {
    let pattern = format!("{}|\\bcodegen\\b", removed_terms_pattern());
    let helps = every_long_help();
    assert!(
        helps.len() > 15,
        "expected every subcommand; got {}",
        helps.len()
    );
    let mut hits = Vec::new();
    for (path, help) in &helps {
        let mut child = std::process::Command::new("grep")
            .args(["-nE", "-e", &pattern])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("grep runs");
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(help.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        match out.status.code() {
            Some(0) => hits.push(format!(
                "--- `{path}` ---\n{}",
                String::from_utf8_lossy(&out.stdout)
            )),
            Some(1) => {}
            other => panic!("grep failed on `{path}` ({other:?}): pattern {pattern}"),
        }
    }
    assert!(
        hits.is_empty(),
        "removed terms in long help:\n{}",
        hits.join("\n")
    );
}
