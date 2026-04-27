use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{
    audit::{self, AuditOptions},
    diagnostics::report_error,
    extract,
    layout::LayoutContext,
    reduction, resolve, validate,
};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "specdev-analyzer",
    version,
    about = "Domain-agnostic analyzer for TOML specs (Phase 1 of the blocks + bridges + profiles arc)."
)]
struct Cli {
    /// Explicit path to the project's spec.toml. Overrides CWD-walking
    /// discovery. Shared by every subcommand that reads project state.
    #[arg(long, global = true)]
    spec_path: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate the project layout and (Phase 2) module structure.
    Validate {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Treat missing layout entries as errors, not warnings.
        #[arg(long)]
        strict: bool,
        /// Suppress ANSI colors.
        #[arg(long)]
        no_color: bool,
    },
    /// Emit canonical YAML extraction for codegen.
    ///
    /// Phase 1 stub: bails until Phase 2 lands kind resolution.
    Extract {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Output directory for canonical YAML files.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Compare extracted specs against source code for drift.
    ///
    /// Phase 1 stub: bails until Phase 2 lands kind resolution + extract.
    Audit {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Only audit one module (exact name match).
        #[arg(long)]
        filter: Option<String>,
        /// Report functions/types in code that are not in the spec.
        #[arg(long)]
        show_uncovered: bool,
        /// Expand macros (cargo expand / cc -E) before parsing.
        #[arg(long)]
        expand: bool,
    },
    /// Execute the reduction and emit a universal entity/relation graph.
    ///
    /// Phase 1 stub: bails until Phase 2 lands kind resolution.
    Reduce {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Write the graph YAML to this path. Without it, prints to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print the resolved project layout for skill consumers.
    Resolve {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let spec_path = cli.spec_path;
    match cli.command {
        Command::Validate {
            project,
            strict,
            no_color,
        } => run_validate(project, spec_path.as_deref(), strict, no_color),
        Command::Extract { project, output } => run_extract(project, spec_path.as_deref(), output),
        Command::Audit {
            project,
            filter,
            show_uncovered,
            expand,
        } => run_audit(
            project,
            spec_path.as_deref(),
            filter,
            show_uncovered,
            expand,
        ),
        Command::Reduce { project, output } => run_reduce(project, spec_path.as_deref(), output),
        Command::Resolve { project } => run_resolve(project, spec_path.as_deref()),
    }
}

fn run_validate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    strict: bool,
    no_color: bool,
) -> ExitCode {
    let report = match validate::run_project(&project, spec_path) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
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
    eprintln!("{errors} error(s), {warnings} warning(s), {infos} info(s)");
    let strict_warns = strict && warnings > 0;
    if errors > 0 || strict_warns {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_extract(project: PathBuf, spec_path: Option<&std::path::Path>, output: PathBuf) -> ExitCode {
    let _ = output; // unused in Phase 1 stub
    match extract::run_project(&project, spec_path) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_audit(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    filter: Option<String>,
    show_uncovered: bool,
    expand: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let options = AuditOptions {
        show_uncovered,
        expand,
        module_filter: filter.as_deref(),
        type_map: None,
    };
    match audit::run_project(&layout, options) {
        Ok(report) => {
            print!("{}", audit::format_text(&report));
            let summary = report.summary();
            eprintln!(
                "{} match, {} missing, {} mismatch, {} uncovered",
                summary.matches, summary.missing, summary.mismatches, summary.uncovered
            );
            if summary.missing > 0 || summary.mismatches > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_reduce(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    output: Option<PathBuf>,
) -> ExitCode {
    let _ = output; // unused in Phase 1 stub
    match reduction::run_project(&project, spec_path) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_resolve(project: PathBuf, spec_path: Option<&std::path::Path>) -> ExitCode {
    match resolve::run(&project, spec_path) {
        Ok(yaml) => {
            print!("{yaml}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}
