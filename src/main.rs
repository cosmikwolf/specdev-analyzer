use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{
    audit::{self, AuditOptions},
    extract, resolve,
    validate::{self, Inputs},
};

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
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Plugin root. Defaults to $CLAUDE_PLUGIN_ROOT.
        #[arg(long)]
        plugin_root: Option<PathBuf>,
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
        Command::Audit {
            project,
            plugin_root,
            filter,
            show_uncovered,
            expand,
        } => run_audit(project, plugin_root, filter, show_uncovered, expand),
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

fn run_audit(
    project: PathBuf,
    plugin_root: Option<PathBuf>,
    filter: Option<String>,
    show_uncovered: bool,
    expand: bool,
) -> ExitCode {
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
    let options = AuditOptions {
        show_uncovered,
        expand,
        module_filter: filter.as_deref(),
        type_map: None,
    };
    let report = match audit::run_project(&ctx, options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
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
