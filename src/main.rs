use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{
    audit::{self, AuditOptions},
    bootstrap::{self, BootstrapInputs},
    diagnostics::report_error,
    drift, extract, inline,
    layout::LayoutContext,
    merge,
    plugin_root::{self, PluginRootInputs},
    reduction, resolve, validate,
};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "specdev-analyzer",
    version,
    about = "Domain-agnostic analyzer for TOML specs (blocks + bridges + profiles arc)."
)]
struct Cli {
    /// Explicit path to the project's spec.toml. Overrides CWD-walking
    /// discovery. Shared by every subcommand that reads project state.
    #[arg(long, global = true)]
    spec_path: Option<PathBuf>,
    /// Explicit plugin root override for `bootstrap`, `inline`, and
    /// `inline-bridge`. Highest precedence in the resolution chain (see
    /// [`plugin_root`]).
    #[arg(long, global = true)]
    plugin_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate the project layout, kind resolution, and module structure.
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
    Extract {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Output directory for canonical YAML files.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Compare extracted specs against source code for drift.
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
    /// Scaffold a new project from a plugin profile.
    Bootstrap {
        /// Profile name (e.g. `firmware-layered`, `library-api`).
        #[arg(long)]
        profile: String,
        /// Target language for codegen (`rust`, `c`).
        #[arg(long)]
        language: String,
        /// Project name (lowercase, hyphens). Becomes `[project].name` in
        /// the generated `spec.toml`.
        #[arg(long)]
        name: String,
        /// Directory to bootstrap into. Created if it doesn't exist;
        /// fails if `<target>/specs/specdev/` is already present.
        target: PathBuf,
    },
    /// Inline a canonical block from the plugin into the project.
    Inline {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Block name (e.g. `embassy`, `lvgl`).
        block: String,
        /// 3-way merge from the recorded upstream snapshot to the current
        /// plugin version. Required to update an already-inlined block;
        /// without it, re-inline is refused.
        #[arg(long)]
        merge: bool,
    },
    /// Inline a canonical bridge between two already-inlined blocks.
    InlineBridge {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Bridge owner name (`<a>+<b>`, lex-ordered: `a < b`).
        bridge: String,
        /// 3-way merge from the recorded upstream snapshot to the current
        /// plugin version. Required to update an already-inlined bridge.
        #[arg(long)]
        merge: bool,
    },
    /// Plugin-author lint: walk a plugin tree and check structural rules
    /// (currently: bridge directory names are lex-ordered).
    PluginLint {
        /// Plugin root directory (must contain `.claude-plugin/plugin.json`).
        #[arg(long)]
        plugin: PathBuf,
    },
    /// Observational drift report — classify each inlined file as
    /// clean / plugin-moved / locally-edited / both. Pure reporter.
    Drift {
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
    let plugin_flag = cli.plugin_root;
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
        Command::Bootstrap {
            profile,
            language,
            name,
            target,
        } => run_bootstrap(plugin_flag, profile, language, name, target),
        Command::Inline {
            project,
            block,
            merge,
        } => run_inline(project, spec_path.as_deref(), plugin_flag, block, merge),
        Command::InlineBridge {
            project,
            bridge,
            merge,
        } => run_inline_bridge(project, spec_path.as_deref(), plugin_flag, bridge, merge),
        Command::PluginLint { plugin } => run_plugin_lint(plugin),
        Command::Drift { project } => run_drift(project, spec_path.as_deref(), plugin_flag),
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
    let result = match extract::run_project(&project, spec_path) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match extract::write(&result, &output) {
        Ok(written) => {
            eprintln!("wrote {} file(s) to {}", written.len(), output.display());
            ExitCode::SUCCESS
        }
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
    let (graph, _report) = match reduction::run_project(&project, spec_path) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let yaml = match serde_yaml::to_string(&graph) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("failed to serialize graph: {e}");
            return ExitCode::from(2);
        }
    };
    match output {
        Some(path) => match std::fs::write(&path, yaml) {
            Ok(()) => {
                eprintln!("wrote graph to {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("failed to write {}: {e}", path.display());
                ExitCode::from(2)
            }
        },
        None => {
            print!("{yaml}");
            ExitCode::SUCCESS
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

fn run_bootstrap(
    plugin_flag: Option<PathBuf>,
    profile: String,
    language: String,
    name: String,
    target: PathBuf,
) -> ExitCode {
    // Bootstrap has no spec.toml yet (it's writing one), so the
    // [paths].plugin_root step in the resolution chain is bypassed.
    // The flag and env vars still apply.
    let inputs = PluginRootInputs {
        flag: plugin_flag,
    };
    let plugin = match plugin_root::resolve_for_bootstrap(&inputs) {
        Ok(p) => p,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let bootstrap_inputs = BootstrapInputs {
        profile,
        language,
        project_name: name,
        target_dir: target,
    };
    match bootstrap::run_bootstrap(&plugin, &bootstrap_inputs) {
        Ok(()) => {
            eprintln!(
                "bootstrapped {} from profile {} ({}) at {}",
                bootstrap_inputs.project_name,
                bootstrap_inputs.profile,
                bootstrap_inputs.language,
                bootstrap_inputs.target_dir.display(),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(1)
        }
    }
}

fn resolve_plugin_for_project(
    layout: &LayoutContext,
    flag: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let inputs = PluginRootInputs { flag };
    plugin_root::resolve(&inputs, &layout.spec_toml_path, &layout.spec)
}

fn run_inline(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
    block: String,
    merge_flag: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let plugin = match resolve_plugin_for_project(&layout, plugin_flag) {
        Ok(p) => p,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    if merge_flag {
        match merge::run_merge_block(&layout, &plugin, &block) {
            Ok(out) => {
                eprintln!(
                    "merged block {} (advanced to {}); re-run `specdev validate` before committing",
                    out.unit_name,
                    out.advanced_to_version.as_deref().unwrap_or("unknown"),
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(1)
            }
        }
    } else {
        match inline::run_inline_block(&layout, &plugin, &block) {
            Ok(()) => {
                eprintln!("inlined block {block}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(1)
            }
        }
    }
}

fn run_inline_bridge(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
    bridge: String,
    merge_flag: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let plugin = match resolve_plugin_for_project(&layout, plugin_flag) {
        Ok(p) => p,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    if merge_flag {
        match merge::run_merge_bridge(&layout, &plugin, &bridge) {
            Ok(out) => {
                eprintln!(
                    "merged bridge {} (advanced to {}); re-run `specdev validate` before committing",
                    out.unit_name,
                    out.advanced_to_version.as_deref().unwrap_or("unknown"),
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(1)
            }
        }
    } else {
        match inline::run_inline_bridge(&layout, &plugin, &bridge) {
            Ok(()) => {
                eprintln!("inlined bridge {bridge}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(1)
            }
        }
    }
}

fn run_drift(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let plugin = match resolve_plugin_for_project(&layout, plugin_flag) {
        Ok(p) => p,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match drift::run_drift(&layout, &plugin) {
        Ok(report) => {
            match report.to_yaml() {
                Ok(y) => print!("{y}"),
                Err(e) => {
                    eprintln!("failed to render drift report: {e}");
                    return ExitCode::from(2);
                }
            }
            if !report.invariants.is_empty() {
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

fn run_plugin_lint(plugin: PathBuf) -> ExitCode {
    match inline::lint_plugin_bridges(&plugin) {
        Ok(offenders) if offenders.is_empty() => {
            eprintln!("plugin lint OK at {}", plugin.display());
            ExitCode::SUCCESS
        }
        Ok(offenders) => {
            for o in &offenders {
                eprintln!("[BRIDGE-LEX-001] {o}");
            }
            ExitCode::from(1)
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}
