//! Thin binary entry point. All CLI definition and dispatch lives in
//! [`specdev_analyzer::cli`] so downstream binaries (the visualizer) can
//! compose the analyzer's subcommands with their own.

use std::process::ExitCode;

fn main() -> ExitCode {
    specdev_analyzer::cli::run()
}
