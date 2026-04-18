use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::validate::{self, Inputs};

#[derive(Parser)]
#[command(
    name = "specdev-analyzer",
    version,
    about = "Domain-agnostic analyzer for TOML specs."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate spec files against their JSON Schema and run graph checks.
    Validate {
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Explicit path to the domain config (domain.toml). When omitted
        /// the analyzer looks for a sibling `specdev/domains/<d>/domain.toml`.
        #[arg(long)]
        domain: Option<PathBuf>,
        /// Suppress ANSI colors.
        #[arg(long)]
        no_color: bool,
    },
    /// Emit canonical YAML extraction for codegen.
    Extract {
        #[arg(short, long)]
        output: String,
        paths: Vec<String>,
    },
    /// Compare extracted specs against source code for drift.
    Audit {
        #[arg(long)]
        source: String,
        #[arg(long)]
        extracted: String,
    },
    /// Print the resolved context (domain, language, schema paths) for a project.
    Resolve {
        #[arg(long, default_value = ".")]
        project: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate {
            project,
            domain,
            no_color,
        } => run_validate(project, domain, no_color),
        Command::Extract { output, paths } => {
            eprintln!("specdev-analyzer extract — not yet implemented (Phase 5).");
            eprintln!("output={output}, paths={}", paths.len());
            ExitCode::SUCCESS
        }
        Command::Audit { source, extracted } => {
            eprintln!("specdev-analyzer audit — not yet implemented (Phase 8).");
            eprintln!("source={source}, extracted={extracted}");
            ExitCode::SUCCESS
        }
        Command::Resolve { project } => {
            eprintln!("specdev-analyzer resolve — not yet implemented (Phase 7).");
            eprintln!("project={project}");
            ExitCode::SUCCESS
        }
    }
}

fn run_validate(
    project: PathBuf,
    domain: Option<PathBuf>,
    no_color: bool,
) -> ExitCode {
    let inputs: Inputs = match validate::resolve_inputs(&project, domain.as_deref()) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    let report = match validate::run(&inputs) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    let (errors, warnings, infos) = report.counts();
    let rendered = if no_color {
        report.render_plain()
    } else {
        report.render_colored()
    };
    print!("{rendered}");
    eprintln!(
        "{} error(s), {} warning(s), {} info(s)",
        errors, warnings, infos
    );
    if report.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
