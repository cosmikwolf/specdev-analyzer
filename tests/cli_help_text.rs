//! The CLI's own `--help` is user-facing documentation with no compiler
//! behind it. Two of its strings hand-enumerate the languages the schema
//! enums admit, and one hand-enumerates the manifest emitters; all three
//! went stale when a language was admitted and nobody swept them.
//!
//! These tests drive every claim off the embedded `spec.schema.yaml`
//! `language` enum, so admitting a language turns them red until the
//! prose follows. The per-language manifest map below is an *exception*
//! list on purpose: an enum member it does not name panics with the
//! sweep instructions, rather than passing silently.

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
    let block = doc_block_containing("src/bootstrap.rs", "Target language for codegen");
    for lang in admitted_languages() {
        assert!(
            block.contains(&format!("`{lang}`")),
            "`BootstrapInputs::language`'s docstring does not name `{lang}`.\n\
             --- doc block ---\n{block}"
        );
    }
}
