use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{
    audit::{self, AuditOptions},
    extract, reduction, resolve,
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
    /// Execute the domain reduction and emit a universal entity/relation graph.
    Reduce {
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Explicit path to the reduction.yaml. Defaults to the sibling
        /// `specdev/domains/<domain>/reduction.yaml` discovered via the
        /// domain-config lookup.
        #[arg(long)]
        reduction: Option<PathBuf>,
        /// Metaschema directory (holding the `*.schema.yaml` for reductions).
        /// Defaults to `<plugin_root>/schema/meta/v1`.
        #[arg(long)]
        meta_dir: Option<PathBuf>,
        /// Plugin root. Defaults to $CLAUDE_PLUGIN_ROOT.
        #[arg(long)]
        plugin_root: Option<PathBuf>,
        /// Write the graph YAML to this path. Without it, prints to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print the resolved context (domain, language, schema paths) for a project.
    Resolve {
        /// Project root (directory containing spec.toml). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Plugin root. Defaults to $CLAUDE_PLUGIN_ROOT.
        #[arg(long)]
        plugin_root: Option<PathBuf>,
        /// When spec.toml is missing, emit an informational header and exit 0
        /// instead of erroring. Used by skills that must load even before
        /// spec.toml exists (e.g. /spec-init).
        #[arg(long)]
        allow_missing: bool,
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
        Command::Reduce {
            project,
            reduction,
            meta_dir,
            plugin_root,
            output,
        } => run_reduce(project, reduction, meta_dir, plugin_root, output),
        Command::Resolve {
            project,
            plugin_root,
            allow_missing,
        } => run_resolve(project, plugin_root, allow_missing),
    }
}

fn run_validate(project: PathBuf, domain: Option<PathBuf>, no_color: bool) -> ExitCode {
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
    let plugin_root =
        plugin_root.or_else(|| std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(PathBuf::from));
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

fn run_resolve(project: PathBuf, plugin_root: Option<PathBuf>, allow_missing: bool) -> ExitCode {
    let plugin_root =
        plugin_root.or_else(|| std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(PathBuf::from));
    match resolve::resolve(&project, plugin_root.as_deref()) {
        Ok(ctx) => {
            print!("{}", resolve::render(&ctx));
            ExitCode::SUCCESS
        }
        Err(e) => {
            if allow_missing && resolve::is_missing_spec(&e) {
                print!(
                    "{}",
                    resolve::render_missing(&project, plugin_root.as_deref())
                );
                ExitCode::SUCCESS
            } else {
                eprintln!("{e:#}");
                ExitCode::from(2)
            }
        }
    }
}

fn run_extract(project: PathBuf, domain: Option<PathBuf>, output: PathBuf) -> ExitCode {
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

fn run_reduce(
    project: PathBuf,
    reduction_path: Option<PathBuf>,
    meta_dir: Option<PathBuf>,
    plugin_root: Option<PathBuf>,
    output: Option<PathBuf>,
) -> ExitCode {
    let plugin_root =
        plugin_root.or_else(|| std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(PathBuf::from));
    let ctx = match resolve::resolve(&project, plugin_root.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };

    let reduction_file = match reduction_path {
        Some(p) => p,
        None => ctx.domain_dir.path.join("reduction.yaml"),
    };
    if !reduction_file.is_file() {
        eprintln!(
            "reduction spec not found at {} — pass --reduction <path> or create it under specdev/domains/{}/",
            reduction_file.display(),
            ctx.domain
        );
        return ExitCode::from(2);
    }

    let meta_dir = match meta_dir {
        Some(p) => p,
        None => match &ctx.plugin_root {
            Some(root) => root.join("schema").join("meta").join("v1"),
            None => {
                eprintln!(
                    "cannot locate metaschema directory — pass --meta-dir <path> or set CLAUDE_PLUGIN_ROOT"
                );
                return ExitCode::from(2);
            }
        },
    };

    let spec = match reduction::load_reduction_spec(&reduction_file, &meta_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(1);
        }
    };

    let (graph, report) = match reduction::execute(&spec, &ctx.project_root) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(1);
        }
    };

    let yaml = match reduction::to_yaml(&graph) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };

    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!("creating {}: {e:#}", parent.display());
                        return ExitCode::from(2);
                    }
                }
            }
            if let Err(e) = std::fs::write(&path, &yaml) {
                eprintln!("writing {}: {e:#}", path.display());
                return ExitCode::from(2);
            }
            eprintln!("wrote reduction graph to {}", path.display());
        }
        None => print!("{yaml}"),
    }

    eprint!("{}", report.render_plain());
    let (errors, warnings, infos) = report.counts();
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
