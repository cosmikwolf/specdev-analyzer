use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{extract, resolve, validate::{self, Inputs}};

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
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Explicit path to the domain config (domain.toml).
        #[arg(long)]
        domain: Option<PathBuf>,
        /// Output directory for canonical YAML files.
        #[arg(short, long)]
        output: PathBuf,
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
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Plugin root. Defaults to $CLAUDE_PLUGIN_ROOT.
        #[arg(long)]
        plugin_root: Option<PathBuf>,
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
        Command::Extract {
            project,
            domain,
            output,
        } => run_extract(project, domain, output),
        Command::Audit { source, extracted } => {
            eprintln!("specdev-analyzer audit — not yet implemented (Phase 8).");
            eprintln!("source={source}, extracted={extracted}");
            ExitCode::SUCCESS
        }
        Command::Resolve { project, plugin_root } => run_resolve(project, plugin_root),
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

fn run_resolve(project: PathBuf, plugin_root: Option<PathBuf>) -> ExitCode {
    let plugin_root = plugin_root.or_else(|| {
        std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(PathBuf::from)
    });
    let ctx = match resolve::resolve(&project, plugin_root.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    print!("{}", resolve::render(&ctx));
    ExitCode::SUCCESS
}

fn run_extract(
    project: PathBuf,
    domain: Option<PathBuf>,
    output: PathBuf,
) -> ExitCode {
    let inputs: Inputs = match validate::resolve_inputs(&project, domain.as_deref()) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    let result = match extract::run(&inputs) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(1);
        }
    };
    match extract::write(&result, &output) {
        Ok(paths) => {
            eprintln!("wrote {} file(s) to {}", paths.len(), output.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(2)
        }
    }
}
