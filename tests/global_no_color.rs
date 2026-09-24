//! `--no-color` is a global flag: every subcommand accepts it (#123).
//!
//! It was declared on `validate` alone, so the other subcommands exited 2
//! with "unexpected argument '--no-color'" — and agents pass it by habit
//! (field entries 14, 21, 23, 24, 30; `/spec-drift` once shipped it on
//! `drift`). The walk below reads the subcommand tree from clap itself, so a
//! new subcommand is covered without touching this file.

use std::process::Command;

use clap::CommandFactory;
use specdev_analyzer::cli::Cli;

/// Every leaf subcommand path, e.g. `["report", "gen-plan"]`.
fn leaf_paths(cmd: &clap::Command, prefix: &[String], out: &mut Vec<Vec<String>>) {
    let subs: Vec<_> = cmd
        .get_subcommands()
        .filter(|s| s.get_name() != "help")
        .collect();
    if subs.is_empty() {
        out.push(prefix.to_vec());
    }
    for s in subs {
        let mut p = prefix.to_vec();
        p.push(s.get_name().to_string());
        leaf_paths(s, &p, out);
    }
}

#[test]
fn every_subcommand_accepts_no_color() {
    let mut paths = Vec::new();
    leaf_paths(&Cli::command(), &[], &mut paths);
    assert!(
        paths.len() > 20,
        "the walk found only {} subcommands",
        paths.len()
    );
    for path in paths {
        // `--no-color` before `--help`: clap rejects an unknown argument
        // before it prints help, so exit 0 here means the flag parsed.
        let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
            .args(&path)
            .args(["--no-color", "--help"])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{path:?}: {err}");
        assert!(!err.contains("unexpected argument"), "{path:?}: {err}");
    }
}

#[test]
fn an_unknown_flag_is_still_refused() {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["drift", "--not-a-flag", "--help"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
}
