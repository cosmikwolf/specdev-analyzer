use std::path::PathBuf;
use std::process::ExitCode;

use crate::lineage::Lineage;
use crate::{
    bootstrap::{self, BootstrapInputs},
    context as context_mod,
    diagnostics::report_error,
    drift, inline,
    layout::LayoutContext,
    manifest::profile as profile_mod,
    merge, migrate,
    plugin_root::{self, PluginRootInputs},
    prose_lint, reduction, reference_lint, resolve, validate,
};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

// This module was lifted verbatim out of the former `main.rs`, where it
// referred to the crate by its external name. Alias the crate root so those
// fully-qualified `specdev_analyzer::…` paths keep resolving now that the
// code lives inside the crate.
use crate as specdev_analyzer;

/// Global flags shared by every subcommand. Flattened into the analyzer's
/// own [`Cli`] and re-flattenable by downstream binaries (e.g. the
/// visualizer) that compose the analyzer's subcommands with their own.
#[derive(Args)]
pub struct GlobalArgs {
    /// Explicit path to the project's spec.toml. Overrides CWD-walking
    /// discovery. Shared by every subcommand that reads project state.
    #[arg(long, global = true)]
    pub spec_path: Option<PathBuf>,
    /// Explicit plugin root override for `bootstrap`, `inline`, and
    /// `inline-bridge`. Highest precedence in the resolution chain (see
    /// [`plugin_root`]).
    #[arg(long, global = true)]
    pub plugin_root: Option<PathBuf>,
    /// Suppress ANSI colors. Accepted by every subcommand, so a caller can
    /// pass it everywhere; only output that has color changes (today,
    /// `validate`). Per-subcommand before, it made every other subcommand
    /// exit 2 on an unknown argument (#123).
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Parser)]
#[command(
    name = "specdev-analyzer",
    version,
    about = "Domain-agnostic analyzer for TOML specs (blocks + bridges + profiles arc)."
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Validate the project layout, kind resolution, and module structure.
    Validate {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Treat missing layout entries as errors, not warnings.
        #[arg(long)]
        strict: bool,
        /// Suppress info-severity lines from the rendered output (F-006).
        /// Errors, warnings, and the summary/counts tail still print.
        #[arg(long, short = 'q')]
        quiet: bool,
    },
    /// Extract structured items (top-level language constructs) from each module's
    /// source file. Output is the substrate the items[] sync diffs run
    /// against; v1 consumers are `verified_by_resolves` and the `/spec-reverse`
    /// skill.
    ExtractItems {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Only extract one module (exact name match). Without this flag,
        /// every module under `specs/modules/` is processed.
        #[arg(long)]
        module: Option<String>,
    },
    /// Pre-flight surface for the Claude-mediated `/spec-reverse` skill.
    /// Reports the blocks currently inlined in the project, validates the
    /// `--into` arg if provided, and emits coarse ambiguity warnings when
    /// multiple blocks are inlined without scoping. Pre-flight surface only —
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
    /// Three-stage agentic sync. `items diff <module>` is
    /// stage 1: deterministic detection of drift between spec items[]
    /// and tree-sitter-extracted source items. `items propose`
    /// and `items apply` layer on top.
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
        /// Target language for codegen. The `language` enums in
        /// `spec.schema.yaml` and `profile.schema.yaml` are the source of
        /// truth and admit `rust`, `c`, `cpp`, `dart` and `typescript`;
        /// not every one of them has a manifest emitter (see `manifest`).
        #[arg(long)]
        language: String,
        /// Project name (lowercase, hyphens). Becomes `[project].name` in
        /// the generated `spec.toml`.
        #[arg(long)]
        name: String,
        /// Directory to bootstrap into. Created if it doesn't exist;
        /// fails if `<target>/specs/specdev/` is already present.
        target: PathBuf,
        /// Overwrite project files already sitting at a planned destination.
        ///
        /// Without it, bootstrap refuses and names every collision, so a tree
        /// carrying hand-authored `specs/*.toml` cannot lose them to the
        /// profile template.
        #[arg(long)]
        force: bool,
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
    /// Focused dependency cone for one module. Emits the
    /// schema files, plugin-tree READMEs, target module path, and the
    /// one-hop set of related sibling modules. Skill consumers
    /// (`/spec-design`, `/spec-edit`, `/spec-reverse`) read only the
    /// listed files instead of every module under `specs/modules/`.
    /// Read-only graph projection over the existing reduction; no new
    /// schema or graph primitives.
    Context {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module name (matches `specs/modules/<Name>.toml`). When the
        /// name doesn't resolve (cold-start `/spec-design`), the
        /// subcommand still emits schema + READMEs; sibling-module
        /// cone is empty and `status: module-not-found` is set.
        #[arg(long)]
        module: String,
        /// Write the YAML report here. Without it, prints to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Populate manifests for projects bootstrapped before the manifest
    /// formats shipped. Walks the project's lineage,
    /// resolves which profile / blocks / bridges it has, and byte-copies
    /// the missing manifests (`profile.yaml`, `templates/*`, per-block
    /// `deps.yaml`, per-bridge `contract.yaml`) from the plugin tree's
    /// HEAD. Lineage `content_hash` is updated to the new rolled-up hash;
    /// `origin` / `origin_version` / `inlined_at` stay frozen — they
    /// record the original inline, not the migration. No-op when the
    /// manifests are already present (true byte-stable idempotency).
    /// Backward-compat aid; deletion gated on the future `default-fallback
    /// removal` trigger.
    Migrate {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Print the per-unit migration plan + total file-write count
        /// without touching disk.
        #[arg(long)]
        dry_run: bool,
        /// Also rewrite profile-scaffolded files that are STALE, not just
        /// missing — schemas, templates and `profile.yaml` the plugin has
        /// moved since this project was bootstrapped (#109).
        ///
        /// Only files whose on-disk hash still matches the hash bootstrap
        /// recorded are rewritten: matching proves nobody hand-edited
        /// them. A file you have edited is reported and left alone —
        /// profile files keep no merge base, so there is nothing to merge
        /// against. Run `drift` to see them.
        #[arg(long)]
        refresh: bool,
    },
    /// Observational drift report — classify each inlined file as
    /// clean / plugin-moved / locally-edited / both. Pure reporter.
    Drift {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Reports over the validated spec. `report coverage` emits the
    /// requirements → modules matrix that the retired `gen --skeleton`
    /// HANDOFF.md used to carry as one of its sections; the subcommand
    /// reads `requirements.toml` + `specs/modules/*.toml` and emits a
    /// machine-readable matrix without source-tree emission.
    Report {
        #[command(subcommand)]
        action: ReportAction,
    },
    /// Run one declared compile gate and record its result.
    ///
    /// Looks `--label` up in the `gates:` registry of the inlined
    /// `specs/specdev/profile.yaml`, resolves any
    /// `${root.<table>.<field>}` token against `specs/architecture.toml`,
    /// and runs the gate's `steps` in order in the project root — one
    /// process per argv list, inherited environment, stdin closed, the
    /// first non-zero exit stops the gate. Output is printed: everything
    /// on a failure, a step's stderr on a pass when it is non-empty.
    /// Then it hashes the key (AFTER the run — the gate rewrites the
    /// lockfile) and writes the rows: always the batch row, plus one row
    /// per `--module` for a `scope: module` gate. Exits with the gate's
    /// own exit code; exit 2 means the gate did not run (a step could not
    /// start, a token resolved to nothing, the registry is missing) or
    /// its rows could not be written.
    Gate {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// The gate's `label` from the registry, verbatim. It is also the
        /// row key, so a near miss writes rows nothing reads — an
        /// undeclared label is refused.
        #[arg(long)]
        label: String,
        /// Module(s) this run covers. Repeatable. Required for a
        /// `scope: module` gate; ignored for a `scope: batch` gate.
        #[arg(long)]
        module: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum ReportAction {
    /// Emit the requirements → modules coverage matrix. Reads
    /// `specs/requirements.toml` for requirement ids and walks
    /// `specs/modules/*.toml` for module-level and items[]-level
    /// `satisfies` entries. Output: structured matrix plus an
    /// `orphans` list of unsatisfied requirements. Default format
    /// is YAML; `--format markdown` opts into a human-readable
    /// table.
    Coverage {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Output format: `yaml` (default) or `markdown` / `md`.
        #[arg(long, default_value = "yaml")]
        format: String,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Which decision text reaches which module's codegen prompt. Two
    /// YAML views over the function the prompt is rendered from: `modules`
    /// (per module: its decisions and open questions, why each is there,
    /// and which decision prose fields go in) and `decisions` (per
    /// decision: the modules it reaches; empty when no prompt carries it).
    /// Exit 0 on success, 2 on hard error.
    Decisions {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Limit the `modules` view to this module.
        #[arg(long)]
        module: Option<String>,
        /// Limit the `decisions` view to this decision id.
        #[arg(long)]
        decision: Option<String>,
    },
    /// Render the module dependency graph as Mermaid (default) or DOT.
    /// When `domain.toml` declares a `layer_dependency` graph check, nodes
    /// are grouped by its layer field in its declared order and every edge
    /// the check flags is drawn thick and red; otherwise the graph is not
    /// grouped. Each node shows how many modules it uses and how many use
    /// it. Deterministic, so a committed map diffs when an edge is added.
    /// Exit 0 on success, 2 on hard error.
    Graph {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Output format: `mermaid` (default) or `dot`.
        #[arg(long, default_value = "mermaid")]
        format: String,
        /// The `layer_dependency` rule to group by, when there is more
        /// than one. Default: the first declared.
        #[arg(long)]
        check: Option<String>,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum ItemsAction {
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
    /// Stage 2 scaffold — wraps `items diff` in a structured proposal
    /// document persisted under `specs/specdev/upstream/items-sync/`.
    /// The analyzer writes the deterministic skeleton (drift entries +
    /// source snippets + spec mappings); rationale + action fields are
    /// filled by the `/spec-sync-items` skill (or a human) and then
    /// signed off via `approved: true` for `items apply` to consume.
    /// No AI involvement at this stage either — the analyzer never
    /// shells to a model.
    Propose {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module name (matches `specs/modules/<module>.toml`).
        #[arg(long)]
        module: String,
        /// Override the output path. Defaults to
        /// `<specdev_root>/upstream/items-sync/<module>.proposal.yaml`.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Overwrite an existing proposal file. Without this flag,
        /// `items propose` refuses to clobber rationales the user (or
        /// Claude) has already written.
        #[arg(long)]
        clobber: bool,
    },
    /// Stage 3 — apply an approved proposal to `specs/modules/<module>.toml`.
    /// Validates the approval gate (top-level `approved: true` plus
    /// `approver` + `approved_at`); refuses any entry still on
    /// `pending`. Mutates the spec via `toml_edit` so comments and
    /// formatting outside `[[items]]` round-trip. On success, the
    /// proposal file is renamed to `<module>.applied.yaml` (with
    /// `status: applied`) and remains under
    /// `specs/specdev/upstream/items-sync/` as the audit trail.
    Apply {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module name (matches `specs/modules/<module>.toml`).
        #[arg(long)]
        module: String,
        /// Override the proposal path. Defaults to
        /// `<specdev_root>/upstream/items-sync/<module>.proposal.yaml`.
        #[arg(long)]
        proposal: Option<PathBuf>,
    },
}

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Parse the process arguments and run the analyzer CLI end-to-end.
/// This is the analyzer bin's entry point and the standalone-run path.
pub fn run() -> ExitCode {
    init_tracing();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // `--version` is where a user looks when asking "am I current?",
            // and clap answers it before `dispatch` runs its check. No
            // global flag is parsed on this path, so only the environment
            // names a plugin root.
            if e.kind() == clap::error::ErrorKind::DisplayVersion {
                if let Some(warning) = crate::analyzer_version::check(None) {
                    eprintln!("{warning}");
                }
            }
            e.exit()
        }
    };
    dispatch(cli.command, cli.globals)
}

/// Dispatch an already-parsed [`Command`] with its [`GlobalArgs`]. Downstream
/// binaries that compose the analyzer's subcommands call this directly after
/// routing their own subcommands.
pub fn dispatch(command: Command, globals: GlobalArgs) -> ExitCode {
    let spec_path = globals.spec_path;
    let plugin_flag = globals.plugin_root;
    let no_color = globals.no_color;
    if let Some(warning) = crate::analyzer_version::check(plugin_flag.as_deref()) {
        eprintln!("{warning}");
    }
    match command {
        Command::Validate {
            project,
            strict,
            quiet,
        } => run_validate(
            project,
            spec_path.as_deref(),
            plugin_flag.as_deref(),
            strict,
            no_color,
            quiet,
        ),
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
            force,
        } => run_bootstrap(plugin_flag, profile, language, name, target, force),
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
        Command::Context {
            project,
            module,
            output,
        } => run_context(project, spec_path.as_deref(), module, output),
        Command::Migrate {
            project,
            dry_run,
            refresh,
        } => run_migrate(project, spec_path.as_deref(), plugin_flag, dry_run, refresh),
        Command::Drift { project } => run_drift(project, spec_path.as_deref(), plugin_flag),
        Command::Gate {
            project,
            label,
            module,
        } => run_gate(project, spec_path.as_deref(), &label, module),
        Command::Report { action } => match action {
            ReportAction::Coverage {
                project,
                format,
                output,
            } => run_report_coverage(project, spec_path.as_deref(), &format, output),
            ReportAction::Decisions {
                project,
                module,
                decision,
            } => run_report_decisions(
                project,
                spec_path.as_deref(),
                module.as_deref(),
                decision.as_deref(),
            ),
            ReportAction::Graph {
                project,
                format,
                check,
                output,
            } => run_report_graph(
                project,
                spec_path.as_deref(),
                &format,
                check.as_deref(),
                output,
            ),
        },
        Command::Items { action } => match action {
            ItemsAction::Diff { project, module } => {
                run_items_diff(project, spec_path.as_deref(), module)
            }
            ItemsAction::Propose {
                project,
                module,
                output,
                clobber,
            } => run_items_propose(project, spec_path.as_deref(), module, output, clobber),
            ItemsAction::Apply {
                project,
                module,
                proposal,
            } => run_items_apply(project, spec_path.as_deref(), module, proposal),
        },
    }
}

fn run_validate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<&std::path::Path>,
    strict: bool,
    no_color: bool,
    quiet: bool,
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
    let mut report = match validate::run(&layout) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    if let Some(notice) = plugin_ahead_notice(&layout, plugin_flag) {
        report.push(notice);
    }
    let (errors, warnings, infos) = report.counts();
    let rendered = if no_color {
        report.render_plain_filtered(quiet)
    } else {
        report.render_colored_filtered(quiet)
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

/// One info line when the plugin the caller names (the same lookup as the
/// analyzer-version warning: `--plugin-root`, else `SPECDEV_PLUGIN_ROOT`,
/// then `CLAUDE_PLUGIN_ROOT`) holds newer copies of files this project
/// inlined or bootstrapped. Decided by content hash through `drift`'s
/// classifier, not by version strings: a plugin release that changes
/// nothing this project uses stays silent, and a file the plugin moved is
/// never missed. Silent when no plugin root is named, and when `drift`
/// cannot run — the notice must never turn into a `validate` failure.
fn plugin_ahead_notice(
    layout: &LayoutContext,
    plugin_flag: Option<&std::path::Path>,
) -> Option<specdev_analyzer::report::Finding> {
    let root = crate::analyzer_version::named_plugin_root(plugin_flag)?;
    let drift = specdev_analyzer::drift::run_drift(layout, &root).ok()?;
    let (moved, both) = drift.plugin_ahead_counts();
    if moved + both == 0 {
        return None;
    }
    let mut msg = format!(
        "{} inlined or profile file(s) are behind the plugin at {}",
        moved + both,
        root.display()
    );
    if both > 0 {
        msg.push_str(&format!(" ({both} also changed in this project)"));
    }
    msg.push_str("; run /spec-drift to see them and bring them forward");
    Some(specdev_analyzer::report::Finding::new(
        "PLUGIN-AHEAD-001",
        specdev_analyzer::domain::Severity::Info,
        msg,
    ))
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
                "[ITEMS-001] no language adapter for `{language}`; the spec's \
                 `project.language` must name a language with a tree-sitter adapter."
            );
            return ExitCode::from(2);
        }
    };

    let source_root = layout.spec.paths.source.as_str();
    // #101(b): the profile-declared floor, if any; the adapter default
    // applies when absent.
    let floor = match extract::profile_visibility_floor(&layout.specdev_root) {
        Ok(f) => f,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };

    let mut modules_out: Vec<ModuleItems> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    let inventory = layout.enumerate();
    for module_path in &inventory.modules {
        // Module filename stem is the spec module name (inventory
        // convention: `specs/modules/<Name>.toml`).
        let name: String = match module_path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if let Some(f) = module_filter {
            if name != f {
                continue;
            }
        }

        // #101(a): the module spec's `source_file` override wins over the
        // adapter's layout candidates.
        let override_path = source_map::load_source_file_override(module_path);
        let source_file = source_map::resolve_source_path(
            &name,
            override_path.as_deref(),
            adapter.as_ref(),
            &project,
            source_root,
        );
        let items = match &source_file {
            Some(p) => {
                match extract::extract_items_from_path(p, adapter.as_ref(), floor.as_deref()) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(format!("{name}: {e}"));
                        Vec::new()
                    }
                }
            }
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

    // Enumerate inlined block names from lineage.toml — the record `inline`
    // writes. (An earlier version listed subdirectories of
    // <specs_dir>/schemas/inlined/blocks/, but `inline` writes flat
    // `<block>.schema.yaml` files there, so it never saw a real block.)
    let inlined_blocks: Vec<String> = match Lineage::load(&layout.specdev_root) {
        Ok(l) => l.blocks.keys().cloned().collect(),
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };

    // #102: a profile may declare its own schema as the reverse vocabulary
    // (`reverse.vocabulary: profile`); zero inlined blocks is then info,
    // not an error. Keyed on the declaration, never on the profile name.
    let profile_vocabulary: Option<String> =
        match specdev_analyzer::manifest::Profile::load_inlined(&layout.specdev_root) {
            Ok(p) => p
                .filter(|p| {
                    p.reverse.as_ref().map(|r| r.vocabulary)
                        == Some(specdev_analyzer::manifest::ReverseVocabulary::Profile)
                })
                .map(|p| p.name),
            Err(e) => {
                report_error(&e);
                return ExitCode::from(2);
            }
        };

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
    let mut infos: Vec<String> = Vec::new();
    if inlined_blocks.is_empty() && into_blocks.is_empty() {
        if let Some(name) = &profile_vocabulary {
            infos.push(format!(
                "[REVERSE-004] no blocks inlined; profile `{name}` declares \
                 `reverse.vocabulary: profile`, so its own schema is the \
                 vocabulary. Proceed."
            ));
        } else {
            errors.push(
                "[REVERSE-002] no blocks inlined and no --into specified; \
                 reverse engineering requires a chosen vocabulary. Either run \
                 `specdev-analyzer inline <block>` first, or pass --into \
                 <block,...> naming the blocks whose vocabulary should govern \
                 the output."
                    .to_string(),
            );
        }
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

    // Effective vocabulary: --into if specified, else all inlined, else
    // the profile itself when it declares so (`profile:<name>`).
    let vocab: Vec<String> = if !into_blocks.is_empty() {
        into_blocks
    } else if inlined_blocks.is_empty() {
        profile_vocabulary
            .iter()
            .map(|n| format!("profile:{n}"))
            .collect()
    } else {
        inlined_blocks.clone()
    };

    #[derive(serde::Serialize)]
    struct ReverseReport<'a> {
        inlined_blocks: &'a [String],
        vocabulary: &'a [String],
        infos: &'a [String],
        warnings: &'a [String],
        errors: &'a [String],
    }
    let report = ReverseReport {
        inlined_blocks: &inlined_blocks,
        vocabulary: &vocab,
        infos: &infos,
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
    for i in &infos {
        eprintln!("{i}");
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

fn run_items_propose(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: String,
    output: Option<PathBuf>,
    clobber: bool,
) -> ExitCode {
    use specdev_analyzer::items::propose;
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let doc = match propose::build_proposal(&layout, &module) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    let path = output.unwrap_or_else(|| propose::proposal_path(&layout, &module));
    if let Err(e) = propose::write_proposal(&doc, &path, clobber) {
        eprintln!("{e:#}");
        return ExitCode::from(1);
    }
    let needs = doc
        .entries
        .iter()
        .filter(|e| e.recommendation.action == "pending")
        .count();
    eprintln!(
        "wrote proposal to {} (status={}, {} entr{} pending rationale)",
        path.display(),
        doc.status,
        needs,
        if needs == 1 { "y" } else { "ies" }
    );
    if doc.entries.is_empty() {
        // No drift → exit 0; the file still exists so the skill has a
        // stable artifact to read for "yes I checked, nothing to do."
        ExitCode::SUCCESS
    } else {
        // Drift present → exit 1 so the skill can branch without
        // re-parsing the YAML.
        ExitCode::from(1)
    }
}

fn run_items_apply(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: String,
    proposal: Option<PathBuf>,
) -> ExitCode {
    use specdev_analyzer::items::{apply, propose};
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let proposal_path = proposal.unwrap_or_else(|| propose::proposal_path(&layout, &module));
    if !proposal_path.exists() {
        eprintln!(
            "[ITEM-310] proposal not found at {}; run `specdev-analyzer items propose --module {}` first",
            proposal_path.display(),
            module
        );
        return ExitCode::from(2);
    }
    match apply::apply_proposal(&layout, &module, &proposal_path) {
        Ok(rep) => {
            print!("{}", rep.render());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(1)
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
    force: bool,
) -> ExitCode {
    // Bootstrap has no spec.toml yet (it's writing one), so the
    // [paths].plugin_root step in the resolution chain is bypassed.
    // The flag and env vars still apply.
    let inputs = PluginRootInputs { flag: plugin_flag };
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
    match bootstrap::run_bootstrap_with(
        &plugin,
        &bootstrap_inputs,
        bootstrap::BootstrapOptions { force },
    ) {
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

fn run_context(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: String,
    output: Option<PathBuf>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let report = match context_mod::run_context(&layout, &module) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    if let Err(e) = context_mod::emit(&report, output.as_ref()) {
        report_error(&e);
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn run_migrate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
    dry_run: bool,
    refresh: bool,
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
    let report = match migrate::run_migrate(&layout, &plugin, dry_run, refresh) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(1);
        }
    };

    let prefix = if dry_run { "would migrate" } else { "migrated" };
    let skip_phrase = "already migrated (no-op)";

    if !report.profile_files_written.is_empty() {
        for rel in &report.profile_files_written {
            eprintln!("profile shell: {prefix} {}", rel.display());
        }
    } else if report.profile_skipped {
        eprintln!("profile shell: {skip_phrase}");
    }
    let refresh_verb = if dry_run {
        "would refresh"
    } else {
        "refreshed"
    };
    for rel in &report.profile_files_refreshed {
        eprintln!(
            "profile shell: {refresh_verb} {} (was stale)",
            rel.display()
        );
    }
    if !report.profile_files_locally_edited.is_empty() {
        // Not a failure. The user's edit is the more valuable artifact and
        // there is no merge base for these files, so refresh declines and
        // hands the decision back with the command that shows the diff.
        for rel in &report.profile_files_locally_edited {
            eprintln!(
                "profile shell: KEPT {} — diverges from the hash bootstrap recorded",
                rel.display()
            );
        }
        eprintln!(
            "{} profile file(s) left as-is because you changed them; `specdev-analyzer drift` \
             classifies each one",
            report.profile_files_locally_edited.len()
        );
    }
    for rel in &report.profile_files_needs_manual {
        eprintln!(
            "profile shell: MANUAL {} — the plugin's template moved, but this file also holds \
             the `$ref` list `inline` appended; rewriting it would drop them. Re-apply the \
             upstream change by hand.",
            rel.display()
        );
    }
    if report.profile_refresh_unavailable {
        eprintln!(
            "profile shell: --refresh cannot run — this project has no [profile_files] in \
             lineage.toml, so no file can be proven untouched since bootstrap. Missing files \
             are still written; stale ones are not. (Projects bootstrapped before #109.)"
        );
    }

    for m in &report.migrated {
        let kind_label = match m.kind {
            specdev_analyzer::upstream::UnitKind::Block => "block",
            specdev_analyzer::upstream::UnitKind::Bridge => "bridge",
        };
        let mut detail = format!(
            "{kind_label} {}: {prefix} (hash {}…→{}…; +{} file)",
            m.name,
            short_hash(&m.before_hash),
            short_hash(&m.after_hash),
            m.files_written.len(),
        );
        if m.files_written.len() != 1 {
            detail = detail.replace("file)", "files)");
        }
        eprintln!("{detail}");
        for f in &m.files_written {
            eprintln!("  - {}", f.display());
        }
    }
    for s in &report.skipped {
        let kind_label = match s.kind {
            specdev_analyzer::upstream::UnitKind::Block => "block",
            specdev_analyzer::upstream::UnitKind::Bridge => "bridge",
        };
        eprintln!("{kind_label} {}: {skip_phrase}", s.name);
    }

    let total = report.total_files_written();
    if !report.missing_in_plugin.is_empty() {
        for line in &report.missing_in_plugin {
            eprintln!("{line}");
        }
        let skipped = report.skipped.len();
        eprintln!(
            "migrate: {} file(s) {} ({} skipped, {} missing in plugin tree)",
            total,
            if dry_run { "planned" } else { "written" },
            skipped,
            report.missing_in_plugin.len(),
        );
        return ExitCode::from(1);
    }

    if total == 0 && report.migrated.is_empty() {
        eprintln!("migrate: no-op (manifests already present)");
    } else {
        eprintln!(
            "migrate: {} file(s) {}",
            total,
            if dry_run { "planned" } else { "written" }
        );
    }
    ExitCode::SUCCESS
}

fn short_hash(h: &str) -> String {
    // Trim a `sha256:` prefix and clip to 8 hex chars when possible. Hash
    // strings are operator-readable; `sha256:abcdef…` prefix is noise.
    let stripped = h.strip_prefix("sha256:").unwrap_or(h);
    let take = stripped.chars().take(8).collect::<String>();
    if take.is_empty() {
        "<empty>".to_string()
    } else {
        take
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

fn run_gate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    label: &str,
    modules: Vec<String>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match specdev_analyzer::gates::run_gate(&layout, label, &modules) {
        Ok(run) => {
            println!("gate {} {}", run.label, if run.pass { "pass" } else { "fail" });
            if run.pass {
                ExitCode::SUCCESS
            } else {
                // The gate's own exit code. A tool that exits 2 is then
                // indistinguishable from this command's own exit-2 class by
                // code alone; the `gate <label> fail` line separates them.
                ExitCode::from(u8::try_from(run.code).unwrap_or(1))
            }
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_report_decisions(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: Option<&str>,
    decision: Option<&str>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let report = match specdev_analyzer::decision_report::build(&layout, module, decision) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match specdev_analyzer::decision_report::render_yaml(&report) {
        Ok(y) => {
            print!("{y}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_report_graph(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    format: &str,
    check: Option<&str>,
    output: Option<PathBuf>,
) -> ExitCode {
    use specdev_analyzer::graph_report::{self, GraphFormat};
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let format = match GraphFormat::parse(format) {
        Ok(f) => f,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let view = match graph_report::build(&layout, check) {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let body = match format {
        GraphFormat::Mermaid => graph_report::render_mermaid(&view),
        GraphFormat::Dot => graph_report::render_dot(&view),
    };
    eprintln!(
        "{} module(s), {} edge(s), {} flagged by the layer check",
        view.node_count(),
        view.edges.len(),
        view.violation_count()
    );
    match output {
        Some(path) => match std::fs::write(&path, &body) {
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
            print!("{body}");
            ExitCode::SUCCESS
        }
    }
}

fn run_report_coverage(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    format: &str,
    output: Option<PathBuf>,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let format = match specdev_analyzer::coverage::CoverageFormat::parse(format) {
        Ok(f) => f,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let report = match specdev_analyzer::coverage::build(&layout) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let body = match format {
        specdev_analyzer::coverage::CoverageFormat::Yaml => {
            match specdev_analyzer::coverage::render_yaml(&report) {
                Ok(s) => s,
                Err(e) => {
                    report_error(&e);
                    return ExitCode::from(2);
                }
            }
        }
        specdev_analyzer::coverage::CoverageFormat::Markdown => {
            specdev_analyzer::coverage::render_markdown(&report)
        }
    };
    match output {
        Some(path) => match std::fs::write(&path, &body) {
            Ok(()) => {
                eprintln!("wrote coverage report to {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("failed to write {}: {e}", path.display());
                ExitCode::from(2)
            }
        },
        None => {
            print!("{body}");
            ExitCode::SUCCESS
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
    // Reference-crate coverage + arity checks. Opt-in per unit by the presence
    // of `reference/<lang>/reference_cases.yaml`.
    let reference_offenders = match reference_lint::lint_reference_crates(&plugin) {
        Ok(o) => o,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    // Every shipped profile must declare the manifest contract it was
    // authored against (`PROFILE-LINT-001`); see #117.
    let profile_offenders = match profile_mod::lint_plugin_profiles(&plugin) {
        Ok(o) => o,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    // Inlined codegen prose must be profile-agnostic (`PROSE-PROFILE-001`):
    // a block/bridge `codegen/<lang>/README.md` must not name a profile.
    let prose_offenders = match prose_lint::lint_codegen_prose_profiles(&plugin) {
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
    // Reference offenders carry their own `REF-*-NNN` code (like block lines).
    for o in &reference_offenders {
        eprintln!("{o}");
    }
    // Profile offenders carry their own `PROFILE-LINT-NNN` code.
    for o in &profile_offenders {
        eprintln!("{o}");
    }
    // Prose offenders carry their own `PROSE-PROFILE-NNN` code.
    for o in &prose_offenders {
        eprintln!("{o}");
    }

    let total = bridge_offenders.len()
        + block_offenders.len()
        + reference_offenders.len()
        + profile_offenders.len()
        + prose_offenders.len();
    if total == 0 {
        eprintln!("plugin lint OK at {}", plugin.display());
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{total} plugin lint finding(s) ({} bridge, {} block, {} reference, {} profile, {} prose)",
            bridge_offenders.len(),
            block_offenders.len(),
            reference_offenders.len(),
            profile_offenders.len(),
            prose_offenders.len()
        );
        ExitCode::from(1)
    }
}
