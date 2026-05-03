use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    diagnostics::report_error,
    cargo_toml as cargo_toml_mod, drift, extract, inline,
    layout::LayoutContext,
    merge,
    plugin_root::{self, PluginRootInputs},
    prompt, reduction, resolve, validate,
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
    /// Extract structured items (top-level Rust constructs) from each module's
    /// source file. Output is the substrate Phase 14's items[] sync diffs
    /// against; v1 consumers are `verified_by_resolves` and the `/spec-reverse`
    /// skill (Phase 11b).
    ExtractItems {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Only extract one module (exact name match). Without this flag,
        /// every module under `specs/modules/` is processed.
        #[arg(long)]
        module: Option<String>,
    },
    /// Pre-flight surface for the Claude-mediated `/spec-reverse` skill (11b).
    /// Reports the blocks currently inlined in the project, validates the
    /// `--into` arg if provided, and emits coarse ambiguity warnings when
    /// multiple blocks are inlined without scoping. Phase 10 surface only —
    /// actual TOML emission is Claude's job in the skill.
    Reverse {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Comma-separated list of block names to scope reverse output to.
        /// e.g. `--into embassy,stm32h5-rs`. If absent and 2+ blocks are
        /// inlined, the pre-flight emits an ambiguity warning suggesting
        /// the user pass `--into`.
        #[arg(long)]
        into: Option<String>,
    },
    /// Phase 14 three-stage agentic sync. `items diff <module>` is
    /// stage 1: deterministic detection of drift between spec items[]
    /// and tree-sitter-extracted source items. `items propose` (14-F)
    /// and `items apply` (14-G) layer on top.
    Items {
        #[command(subcommand)]
        action: ItemsAction,
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
    /// Emit a deterministic Cargo.toml derived from the project's
    /// spec.toml + architecture.toml + module-level external_deps.
    /// Phase 6.7 commitment: every Cargo dep is captured in the spec;
    /// this subcommand renders the OUTPUT half. Lex-sorted deps;
    /// stable across runs. Conflict warnings on stderr.
    CargoToml {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Write Cargo.toml here. Without it, prints to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Assemble the codegen prompt for one module. Reads the inlined
    /// blocks/bridges' codegen prose plus architecture + module specs
    /// and concatenates them deterministically. Plugin's `spec-gen`
    /// skill pipes this to Claude. Output is byte-stable across runs.
    Prompt {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module name (matches `specs/modules/<Module>.toml`).
        #[arg(long)]
        module: String,
    },
    /// Observational drift report — classify each inlined file as
    /// clean / plugin-moved / locally-edited / both. Pure reporter.
    Drift {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Subcommand)]
enum ItemsAction {
    /// Stage 1 — deterministic detection. Walks `specs/modules/<module>.toml`
    /// items[] and the resolved source file, joins on `(kind, name)`,
    /// and emits a structured drift report (in_spec_only / in_source_only /
    /// both with optional signature_drift). No AI involvement. YAML to
    /// stdout. Exit 0 when clean, 1 on drift, 2 on hard error.
    Diff {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module name (matches `specs/modules/<module>.toml`).
        #[arg(long)]
        module: String,
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
        Command::ExtractItems { project, module } => {
            run_extract_items(project, spec_path.as_deref(), module.as_deref())
        }
        Command::Reverse { project, into } => {
            run_reverse(project, spec_path.as_deref(), into.as_deref())
        }
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
        Command::Prompt { project, module } => run_prompt(project, spec_path.as_deref(), module),
        Command::CargoToml { project, output } => {
            run_cargo_toml(project, spec_path.as_deref(), output)
        }
        Command::Drift { project } => run_drift(project, spec_path.as_deref(), plugin_flag),
        Command::Items { action } => match action {
            ItemsAction::Diff { project, module } => {
                run_items_diff(project, spec_path.as_deref(), module)
            }
        },
    }
}

fn run_cargo_toml(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    output: Option<PathBuf>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match cargo_toml_mod::render(&layout) {
        Ok(rendered) => {
            for w in &rendered.warnings {
                eprintln!("warning: {w}");
            }
            match output {
                Some(path) => match std::fs::write(&path, &rendered.text) {
                    Ok(()) => {
                        eprintln!("wrote Cargo.toml to {}", path.display());
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("failed to write {}: {e}", path.display());
                        ExitCode::from(2)
                    }
                },
                None => {
                    print!("{}", rendered.text);
                    ExitCode::SUCCESS
                }
            }
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(1)
        }
    }
}

fn run_prompt(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: String,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match prompt::assemble_module_prompt(&layout, &module) {
        Ok(s) => {
            print!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(1)
        }
    }
}

fn run_validate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    strict: bool,
    no_color: bool,
) -> ExitCode {
    // Discover layout once so we can both run the pipeline and produce
    // the positive-confirmation summary against the same project state.
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let report = match validate::run(&layout) {
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
    // Positive-confirmation tail (F-005): surface per-category counts so
    // the user can sanity-check that newly authored files were picked
    // up, even when `validate` finds nothing wrong. Printed before the
    // findings count so the eye lands on it next to the summary block.
    let summary = validate::summarize(&layout);
    eprintln!("{}", summary.render());
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

fn run_extract_items(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module_filter: Option<&str>,
) -> ExitCode {
    use specdev_analyzer::items::{extract, source_map, ExtractionReport, ModuleItems};
    use specdev_analyzer::lang;

    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };

    let language = &layout.spec.project.language;
    let adapter = match lang::adapter_for(language) {
        Some(a) => a,
        None => {
            eprintln!(
                "[ITEMS-001] no language adapter for `{language}`; supported: rust. \
                 Phase 10 ships Rust only — Phase 9 adds C, Phase 14-C ships C items."
            );
            return ExitCode::from(2);
        }
    };

    let source_root = layout.spec.paths.source.as_str();

    let mut modules_out: Vec<ModuleItems> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    let inventory = layout.enumerate();
    for module_path in &inventory.modules {
        // Module filename stem is the spec module name (per Phase 1's
        // inventory convention: `specs/modules/<Name>.toml`).
        let name: String = match module_path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if let Some(f) = module_filter {
            if name != f {
                continue;
            }
        }

        let source_file =
            source_map::resolve_source_path(&name, adapter.as_ref(), &project, source_root);
        let items = match &source_file {
            Some(p) => match extract::extract_items_from_path(p, adapter.as_ref()) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(format!("{name}: {e}"));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };

        modules_out.push(ModuleItems {
            module_name: name,
            source_file,
            items,
        });
    }

    let report = ExtractionReport {
        modules: modules_out,
    };

    match serde_yaml::to_string(&report) {
        Ok(yaml) => print!("{yaml}"),
        Err(e) => {
            eprintln!("[ITEMS-002] serialization error: {e}");
            return ExitCode::from(2);
        }
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("[ITEMS-003] {e}");
        }
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run_reverse(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    into: Option<&str>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };

    // Enumerate inlined block names: each immediate subdirectory of
    // <specs_dir>/schemas/inlined/blocks/ whose name maps to a block.
    let mut inlined_blocks: Vec<String> = Vec::new();
    if layout.inlined_blocks_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&layout.inlined_blocks_dir) {
            for e in entries.flatten() {
                if let Ok(ft) = e.file_type() {
                    if ft.is_dir() {
                        if let Some(name) = e.file_name().to_str() {
                            inlined_blocks.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    inlined_blocks.sort();

    // Parse --into.
    let into_blocks: Vec<String> = into
        .map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Validate --into entries against inlined blocks.
    let mut errors: Vec<String> = Vec::new();
    for b in &into_blocks {
        if !inlined_blocks.contains(b) {
            errors.push(format!(
                "[REVERSE-001] --into block `{b}` is not inlined; \
                 inlined blocks: {:?}. Run `specdev-analyzer inline {b}` \
                 first, or remove the entry.",
                inlined_blocks
            ));
        }
    }

    // Pre-flight gating:
    // - 0 inlined blocks AND no --into → fatal: caller can't reverse.
    // - 0 inlined blocks AND --into present → also fatal (--into validation already errored).
    // - 1 inlined block, no --into → fine; coarse warning skipped.
    // - 2+ inlined blocks, no --into → coarse ambiguity warning.
    let mut warnings: Vec<String> = Vec::new();
    if inlined_blocks.is_empty() && into_blocks.is_empty() {
        errors.push(
            "[REVERSE-002] no blocks inlined and no --into specified; \
             reverse engineering requires a chosen vocabulary. Either run \
             `specdev-analyzer inline <block>` first, or pass --into \
             <block,...> naming the blocks whose vocabulary should govern \
             the output."
                .to_string(),
        );
    } else if inlined_blocks.len() >= 2 && into_blocks.is_empty() {
        warnings.push(format!(
            "[REVERSE-003] {} blocks inlined ({:?}) and --into not \
             specified; source constructs may map ambiguously across \
             blocks. Pass --into <block,...> to scope, or accept that the \
             /spec-reverse skill will use READMEs from all inlined blocks \
             as prompt context.",
            inlined_blocks.len(),
            inlined_blocks
        ));
    }

    // Effective vocabulary: --into if specified, else all inlined.
    let vocab: Vec<String> = if !into_blocks.is_empty() {
        into_blocks
    } else {
        inlined_blocks.clone()
    };

    #[derive(serde::Serialize)]
    struct ReverseReport<'a> {
        inlined_blocks: &'a [String],
        vocabulary: &'a [String],
        warnings: &'a [String],
        errors: &'a [String],
    }
    let report = ReverseReport {
        inlined_blocks: &inlined_blocks,
        vocabulary: &vocab,
        warnings: &warnings,
        errors: &errors,
    };

    match serde_yaml::to_string(&report) {
        Ok(yaml) => print!("{yaml}"),
        Err(e) => {
            eprintln!("[REVERSE-099] serialization error: {e}");
            return ExitCode::from(2);
        }
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("{e}");
        }
        return ExitCode::from(1);
    }
    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("{w}");
        }
    }
    ExitCode::SUCCESS
}

fn run_items_diff(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: String,
) -> ExitCode {
    use specdev_analyzer::items::sync;
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let report = match sync::diff_module(&layout, &module) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    let clean = report.is_clean();
    match serde_yaml::to_string(&report) {
        Ok(yaml) => print!("{yaml}"),
        Err(e) => {
            eprintln!("[ITEM-103] serialization error: {e}");
            return ExitCode::from(2);
        }
    }
    if clean {
        ExitCode::SUCCESS
    } else {
        // Drift is informational, not an error — exit 1 so the caller
        // (skill or shell pipeline) can branch on it without parsing
        // YAML, but keep stderr empty so the structured output remains
        // the load-bearing signal.
        ExitCode::from(1)
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
    let bridge_offenders = match inline::lint_plugin_bridges(&plugin) {
        Ok(o) => o,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let block_offenders = match inline::lint_plugin_blocks(&plugin) {
        Ok(o) => o,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };

    // Bridge offenders are name-only strings (single finding code is
    // implicit — `BRIDGE-LEX-001`); we prefix at print time.
    for o in &bridge_offenders {
        eprintln!("[BRIDGE-LEX-001] {o}");
    }
    // Block offenders are pre-prefixed with their `BLOCK-LINT-NNN` code
    // (different convention because there are 7 block codes vs 1 bridge
    // code — per-line code visibility matters more for blocks).
    for o in &block_offenders {
        eprintln!("{o}");
    }

    let total = bridge_offenders.len() + block_offenders.len();
    if total == 0 {
        eprintln!("plugin lint OK at {}", plugin.display());
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{total} plugin lint finding(s) ({} bridge, {} block)",
            bridge_offenders.len(),
            block_offenders.len()
        );
        ExitCode::from(1)
    }
}
