use std::path::PathBuf;
use std::process::ExitCode;

use crate::lineage::Lineage;
use crate::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml as cargo_toml_mod, context as context_mod,
    diagnostics::report_error,
    drift, extract, inline,
    layout::LayoutContext,
    manifest::profile as profile_mod,
    merge, migrate, package_json as package_json_mod,
    plugin_root::{self, PluginRootInputs},
    prompt, prose_lint, pubspec_yaml as pubspec_yaml_mod, reduction, reference_lint, resolve,
    validate,
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
        /// Scope checks to the consumer named here.
        /// Accepts `schema`, `strict-gen`, or `cargo-toml`.
        /// Schema-tagged checks always run regardless. Without the flag,
        /// every check fires (legacy behavior).
        #[arg(long = "for")]
        scope_for: Option<String>,
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
    /// Emit the project's build manifest, dispatching on
    /// `[project].language`: Cargo.toml for rust, pubspec.yaml for dart,
    /// package.json for typescript. Other admitted languages have no
    /// emitter and are refused by name.
    /// Every dep is captured in the spec; this subcommand renders the
    /// OUTPUT half. Lex-sorted deps; stable across runs. Conflict
    /// warnings on stderr.
    Manifest {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Write the manifest here. Without it (or --write), prints to stdout.
        #[arg(short, long, conflicts_with = "write")]
        output: Option<PathBuf>,
        /// Write to the language's canonical filename in the project root
        /// (Cargo.toml / pubspec.yaml / package.json) — lets callers stay
        /// language-neutral.
        #[arg(long)]
        write: bool,
        /// Overwrite a manifest that `gen-lineage.toml` has no row for.
        ///
        /// Without it, `--write` refuses to replace a build manifest this
        /// tool did not write, because that file is hand-authored and
        /// carries fields the spec has no place for. Pass this only when
        /// the file at the target is genuinely disposable.
        #[arg(long)]
        accept_edits: bool,
    },
    /// Thin alias of `manifest` for rust-language projects (Cargo.toml
    /// emission). Errors on non-rust projects — use `manifest` there.
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
    /// Spec→codegen handoff. Orchestrated by the `/spec-gen` skill, which
    /// reads the shared preamble once (`--preamble`) then each module's
    /// tail (`--module <N> --tail`) and emits source; the analyzer
    /// assembles the codegen prompt deterministically. `--module <N>`
    /// alone still writes the whole single-module prompt (back-compat).
    Gen {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module(s) to assemble for. Repeatable. With `--preamble`, an
        /// empty list unions over every module under `specs/modules/`;
        /// otherwise the named set is unioned. With `--tail` (or default
        /// whole-prompt mode), exactly one module is required. Default
        /// whole-prompt mode writes the assembled codegen prompt for one
        /// module to stdout (equivalent to `prompt --module <Name>`, plus
        /// the codegen gate).
        #[arg(long)]
        module: Vec<String>,
        /// F-006: emit only the shared, project-invariant preamble
        /// (Tier 1 conventions + architecture; Tier 2 block/bridge prose
        /// unioned across `--module` or all modules). Gate-free — pure
        /// shared prose. `/spec-gen` emits this once per run, then loops
        /// `--module <N> --tail` per module. Mutually exclusive with `--tail`.
        #[arg(long, conflicts_with = "tail")]
        preamble: bool,
        /// F-006: emit only the per-module tail (Tier 3: decisions/OQ,
        /// project context, test items, module spec) for the single
        /// `--module`. Enforces the codegen gate. Mutually exclusive with
        /// `--preamble`.
        #[arg(long)]
        tail: bool,
    },
    /// Record SHA-256 hashes of generated files in
    /// `specs/specdev/gen-lineage.toml`. Invoked by the `/spec-gen`
    /// skill after emission — the analyzer cannot know which files the
    /// codegen prose prescribed, so the skill passes them explicitly.
    /// Re-recording a path replaces its entry. `report gen-drift`
    /// classifies recorded files later.
    GenRecord {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module the files were generated for. Omit for project-level
        /// emissions (Cargo.toml).
        #[arg(long)]
        module: Option<String>,
        /// Emitted file paths (absolute, or relative to cwd). Each must
        /// resolve inside the project root.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Record a file even when gen-drift reports it as `edited`.
        ///
        /// Without this, re-recording an edited file is refused: recording is
        /// what makes a file `clean`, so it would make the hand-edit the new
        /// baseline and gen-drift could never report it again. Pass this only
        /// when the current bytes ARE the intended baseline — a kept edit, or
        /// a formatter pass that belongs to emission.
        #[arg(long)]
        accept_edits: bool,
    },
    /// Derive per-module files from the profile's `derived_files` rules
    /// (route shims that re-export a module body) and record them in
    /// `specs/specdev/gen-lineage.toml` as `derived` rows. Run by the
    /// `/spec-gen` skill after the manifest and before lineage recording,
    /// for every module on every gen. Without `--write` every line is a
    /// preview. One line per path: `wrote` / `kept` / `edited` /
    /// `conflict` / `deleted` / `skipped`. A hand-edited target
    /// (`edited`) or an unrecorded file at a target (`conflict`) is never
    /// overwritten without `--accept-edits`. Exit 0 when nothing is held
    /// back; 3 when any `edited` / `conflict` line remains (every other
    /// target was still written, with or without `--write` the same); 1 an
    /// error stopped the run (the refusals — two modules on one target,
    /// unreachable rules, a stale inlined profile — come before any write);
    /// 2 the command could not start.
    Derive {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Write the derived files, delete stale ones and record the rows.
        #[arg(long)]
        write: bool,
        /// Overwrite `edited` and `conflict` targets and delete stale
        /// files that carry a hand edit. Run by hand after such a line;
        /// the codegen skill never passes it.
        #[arg(long)]
        accept_edits: bool,
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
    /// Record a compile-gate result in `specs/specdev/gen-lineage.toml`.
    /// Invoked by the `/spec-gen` skill AFTER the compile gate (Step 8) —
    /// never before it, because the gate itself writes the lockfile that
    /// becomes part of the result's key. Writes one batch row (the
    /// whole-tree result) plus one row per `--module`; rows replace by
    /// `(module, label)`. `report gen-plan` reads them to decide which
    /// `unchanged` modules still need a re-gate.
    GenRecordGate {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Module(s) generated in this run. Repeatable. Omit to record a
        /// batch row only.
        #[arg(long)]
        module: Vec<String>,
        /// Opaque gate label (e.g. `cargo-check`, `flutter-analyze`,
        /// `next-build`). The analyzer never interprets it.
        #[arg(long)]
        label: String,
        /// The gate passed.
        #[arg(long, conflicts_with = "fail", required_unless_present = "fail")]
        pass: bool,
        /// The gate failed. Every named module and the batch get a fail
        /// row, so none of them can be skipped as verified next run.
        #[arg(long, conflicts_with = "pass", required_unless_present = "pass")]
        fail: bool,
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
    /// Classify every file recorded in `gen-lineage.toml` as clean
    /// (bytes match the recorded hash), edited (generated then
    /// hand-edited), or missing (recorded but gone). Pure reporter;
    /// hash-level only — it detects THAT a file changed, not what.
    /// Exit 0 when clean, 1 on drift, 2 on hard error.
    GenDrift {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Classify every module under `specs/modules/` for incremental
    /// `/spec-gen --all`: `new` (no lineage), `unchanged` (clean output +
    /// matching input hash → skip), `stale` (clean output, inputs moved →
    /// regenerate), `edited` (output hand-edited → skip + warn), or
    /// `missing` (output gone → regenerate). Deterministic; no judgment.
    /// YAML to stdout. Exit 0 on success, 2 on hard error.
    GenPlan {
        /// Project root (directory the analyzer searches from). Defaults to cwd.
        #[arg(long, default_value = ".")]
        project: PathBuf,
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
            scope_for,
        } => run_validate(
            project,
            spec_path.as_deref(),
            strict,
            no_color,
            quiet,
            scope_for,
        ),
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
        Command::Prompt { project, module } => run_prompt(project, spec_path.as_deref(), module),
        Command::Manifest {
            project,
            output,
            write,
            accept_edits,
        } => run_manifest(
            project,
            spec_path.as_deref(),
            output,
            write,
            false,
            accept_edits,
        ),
        Command::CargoToml { project, output } => {
            run_manifest(project, spec_path.as_deref(), output, false, true, false)
        }
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
        Command::Gen {
            project,
            module,
            preamble,
            tail,
        } => run_gen(
            project,
            spec_path.as_deref(),
            plugin_flag,
            module,
            preamble,
            tail,
        ),
        Command::GenRecord {
            project,
            module,
            files,
            accept_edits,
        } => run_gen_record(project, spec_path.as_deref(), module, files, accept_edits),
        Command::Derive {
            project,
            write,
            accept_edits,
        } => run_derive(
            project,
            spec_path.as_deref(),
            plugin_flag,
            write,
            accept_edits,
        ),
        Command::Gate {
            project,
            label,
            module,
        } => run_gate(project, spec_path.as_deref(), plugin_flag, &label, module),
        Command::GenRecordGate {
            project,
            module,
            label,
            pass,
            fail: _,
        } => run_gen_record_gate(project, spec_path.as_deref(), module, &label, pass),
        Command::Report { action } => match action {
            ReportAction::Coverage {
                project,
                format,
                output,
            } => run_report_coverage(project, spec_path.as_deref(), &format, output),
            ReportAction::GenDrift { project } => {
                run_report_gen_drift(project, spec_path.as_deref())
            }
            ReportAction::GenPlan { project } => run_report_gen_plan(project, spec_path.as_deref()),
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

/// Shared `manifest` / `cargo-toml` runner. Dispatches on
/// `[project].language` — language dispatch is in-code by design
/// (languages are universal infrastructure; everything profile-shaped
/// flows through the runtime manifests instead).
/// `require_rust` is the `cargo-toml` alias's guard:
/// the legacy name only ever emitted Cargo.toml, so a non-rust project
/// gets a pointer to `manifest` instead of a surprising pubspec.
fn run_manifest(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    output: Option<PathBuf>,
    write: bool,
    require_rust: bool,
    accept_edits: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let language = layout.spec.project.language.clone();
    if require_rust && language != "rust" {
        eprintln!(
            "error: this project's language is `{language}`; `cargo-toml` emits \
             Cargo.toml for rust projects only — use `specdev-analyzer manifest`"
        );
        return ExitCode::from(2);
    }

    // (rendered manifest text, warnings, paired extra files written only
    // when absent, gen-owned sibling files overwritten on every write,
    // canonical filename)
    let (text, warnings, extra_files, sibling_files, default_filename) = match language.as_str() {
        "rust" => match cargo_toml_mod::render(&layout) {
            Ok(r) => (r.text, r.warnings, r.extra_files, Vec::new(), "Cargo.toml"),
            Err(e) => {
                report_error(&e);
                return ExitCode::from(1);
            }
        },
        "dart" => match pubspec_yaml_mod::render(&layout) {
            Ok(r) => (r.text, r.warnings, Vec::new(), Vec::new(), "pubspec.yaml"),
            Err(e) => {
                report_error(&e);
                return ExitCode::from(1);
            }
        },
        "typescript" => match package_json_mod::render(&layout) {
            Ok(r) => (
                r.text,
                r.warnings,
                Vec::new(),
                r.sibling_files,
                "package.json",
            ),
            Err(e) => {
                report_error(&e);
                return ExitCode::from(1);
            }
        },
        other => {
            eprintln!(
                "error: manifest emission for language `{other}` is not \
                 implemented; supported: rust, dart, typescript"
            );
            return ExitCode::from(1);
        }
    };

    for w in &warnings {
        eprintln!("warning: {w}");
    }

    // Profile-declared `paired_files` — language-neutral, written
    // once when absent through the same loop as the rust `[[bin]]` stub.
    let mut extra_files = extra_files;
    match specdev_analyzer::paired_files::paired_files_from_profile(&layout) {
        Ok(mut rows) => extra_files.append(&mut rows),
        Err(e) => {
            report_error(&e);
            return ExitCode::from(1);
        }
    }
    if let Err(e) = specdev_analyzer::paired_files::refuse_collisions(
        &extra_files,
        default_filename,
        &sibling_files,
    ) {
        report_error(&e);
        return ExitCode::from(1);
    }

    // Refuse to replace a build manifest this tool did not write.
    //
    // The manifest is pure output, rewritten on every `--write`. That is
    // correct for a project specdev generated, and wrong for a project that
    // adopted specdev over a tree it already had: there the file on disk is
    // hand-authored, carries fields the spec has no place for (`version`,
    // `authors`, a `postinstall` script, a `resolutions` block), and this
    // write discards them with no warning and exit 0.
    //
    // The matrix is `derive`'s, and the order matters — identical bytes are
    // tested BEFORE the lineage row, so a repeat `--write` with nothing to
    // record (the analyzer driven directly, or a `/spec-gen` run interrupted
    // between Step 6 and Step 6.5) is not a conflict. Only bytes that both
    // differ from our render AND have no lineage row belong to the user:
    //
    //   absent                         -> write
    //   identical to the render        -> write (nothing to lose)
    //   recorded                       -> write (ours; the regenerate loop)
    //   different AND unrecorded       -> REFUSE
    //
    // Covers the primary manifest AND every gen-owned sibling
    // (`pnpm-workspace.yaml`) under `--write`. The siblings were previously
    // left out on the grounds that they render wholly from
    // `[workspace].members[]` and are documented as overwritten every write.
    // That reasoning holds only for a tree this tool created: in a brownfield
    // monorepo the sibling is hand-authored, carries members and settings the
    // spec never saw, and the first `--write` discarded it — the same
    // data-loss shape the primary manifest was guarded against, on the same
    // command, in the same run.
    //
    // Every file the write would touch is checked BEFORE any of them is
    // written. A refusal after a partial write leaves a tree that neither the
    // spec nor the user authored.
    //
    // `--output <path>` names a destination explicitly and is the caller's
    // business, the same class as a shell redirection.
    if write && !accept_edits {
        let mut guarded: Vec<(PathBuf, &[u8])> =
            vec![(PathBuf::from(default_filename), text.as_bytes())];
        for (sib_rel, sib_body) in &sibling_files {
            guarded.push((PathBuf::from(sib_rel), sib_body.as_bytes()));
        }
        for (rel, rendered) in guarded {
            let display_name = rel.display().to_string();
            let abs = layout.project_root.join(&rel);
            let on_disk = match std::fs::read(&abs) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    eprintln!("error: reading {}: {e}", abs.display());
                    return ExitCode::from(2);
                }
            };
            if let Some(bytes) = on_disk {
                if bytes != rendered {
                    match specdev_analyzer::gen_lineage::is_recorded(&layout.specdev_root, &rel) {
                        Ok(true) => {}
                        Ok(false) => {
                            eprintln!(
                                "error: [MANIFEST-003] refusing to overwrite {display_name}: \
                                 unrecorded file at target\n\n\
                                 It has no row in specs/specdev/gen-lineage.toml, so this tool \
                                 did not write it, and its bytes differ from what the spec \
                                 renders. Overwriting would discard whatever it declares that \
                                 the spec does not yet carry.\n\n\
                                 If the file is yours to keep: fold what it declares into the \
                                 spec ([[external_deps]] in architecture.toml for project-wide \
                                 dependencies, external_deps[] in a module for module-scoped \
                                 ones; [workspace].members[] for workspace membership), then \
                                 delete it and re-run.\n\
                                 If it is genuinely disposable: re-run with --accept-edits."
                            );
                            return ExitCode::from(1);
                        }
                        Err(e) => {
                            report_error(&e);
                            return ExitCode::from(2);
                        }
                    }
                }
            }
        }
    }

    let target = output.or_else(|| write.then(|| layout.project_root.join(default_filename)));
    match target {
        Some(path) => match std::fs::write(&path, &text) {
            Ok(()) => {
                eprintln!("wrote {default_filename} to {}", path.display());
                let project_root = layout.project_root.as_path();
                // Gen-owned sibling files (e.g. the pnpm workspace
                // root's `pnpm-workspace.yaml`) are output like the
                // manifest itself: written beside it and overwritten on
                // every `--write`. They are skipped under `--output`,
                // which redirects the manifest text alone.
                if write {
                    for (rel, body) in &sibling_files {
                        let dst = project_root.join(rel);
                        if let Err(e) = std::fs::write(&dst, body) {
                            eprintln!("failed to write {}: {e}", dst.display());
                            return ExitCode::from(2);
                        }
                        eprintln!("wrote {rel} to {}", dst.display());
                    }
                } else if !sibling_files.is_empty() {
                    let names: Vec<&str> = sibling_files.iter().map(|(r, _)| r.as_str()).collect();
                    eprintln!(
                        "note: sibling file(s) {} not written under --output; use --write",
                        names.join(", ")
                    );
                }
                // Paired files — the rust `[[bin]]` stub (F-016) and the
                // profile's `paired_files` rows. Written only
                // when absent: an existing file is the user's and is
                // never overwritten. Gated on `--write` like the
                // siblings — `--output <file>` redirects the manifest
                // text alone and must not touch the project root.
                if write {
                    let mut kept: Vec<String> = Vec::new();
                    for (rel, body) in &extra_files {
                        let dst = project_root.join(rel);
                        if dst.exists() {
                            kept.push(rel.display().to_string());
                            continue;
                        }
                        if let Some(parent) = dst.parent() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                eprintln!("failed to create {}: {e}", parent.display());
                                return ExitCode::from(2);
                            }
                        }
                        if let Err(e) = std::fs::write(&dst, body) {
                            eprintln!("failed to write {}: {e}", dst.display());
                            return ExitCode::from(2);
                        }
                        eprintln!("wrote paired file {}", dst.display());
                    }
                    if !kept.is_empty() {
                        eprintln!(
                            "kept {} existing paired file(s): {}",
                            kept.len(),
                            kept.join(", ")
                        );
                    }
                } else if !extra_files.is_empty() {
                    eprintln!(
                        "note: {} paired file(s) not written under --output; use --write",
                        extra_files.len()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("failed to write {}: {e}", path.display());
                ExitCode::from(2)
            }
        },
        None => {
            print!("{text}");
            ExitCode::SUCCESS
        }
    }
}

fn run_prompt(project: PathBuf, spec_path: Option<&std::path::Path>, module: String) -> ExitCode {
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
            // Could not assemble: 2, the same class as `gen`'s (#165).
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_validate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    strict: bool,
    no_color: bool,
    quiet: bool,
    scope_for: Option<String>,
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
    let filter = match scope_for {
        None => None,
        Some(s) => match s.parse::<validate::Capability>() {
            Ok(cap) => Some(cap),
            Err(e) => {
                eprintln!("error: --for: {e}");
                return ExitCode::from(2);
            }
        },
    };
    let report = match validate::run_for(&layout, filter) {
        Ok(r) => r,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
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

/// `/spec-gen` orchestration entry point. The analyzer assembles the
/// codegen prompt deterministically; Claude consumes it and emits
/// source. With `--module <Name>`, writes the assembled prompt to
/// stdout (equivalent to `prompt --module <Name>`); without it, prints
/// a routing hint pointing at the right invocation.
///
/// Exit codes follow the rest of this file: 0 prompt printed; 1 the spec
/// was evaluated and codegen is refused (an open question, no module-scope
/// gate declared); 2 the command could not run (usage, an unreadable or
/// unparsable file, no plugin root).
fn run_gen(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
    module: Vec<String>,
    preamble: bool,
    tail: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    // The gate-registry check for the two gated paths below. The plugin
    // root is read only when the inlined profile declares no registry
    // (to print the block to paste); resolve it lazily.
    let layout_ref = &layout;
    let resolve_plugin = move || resolve_plugin_for_project(layout_ref, plugin_flag.clone());
    let registry_refusal = |layout: &LayoutContext| -> Option<ExitCode> {
        match specdev_analyzer::gates::refuse_without_module_gate(layout, &resolve_plugin, "gen") {
            Ok(None) => None,
            Ok(Some(message)) => {
                eprintln!("{message}");
                Some(ExitCode::from(1))
            }
            Err(e) => {
                report_error(&e);
                Some(ExitCode::from(2))
            }
        }
    };

    // F-006 `--preamble`: shared Tier 1 + Tier 2 prose, gate-free. An empty
    // `--module` set unions over every module under specs/modules/; a named
    // set scopes the union (the single-module and fan-out-subset cases).
    if preamble {
        let modules = if module.is_empty() {
            match specdev_analyzer::prompt::list_module_names(&layout) {
                Ok(m) => m,
                Err(e) => {
                    report_error(&e);
                    return ExitCode::from(2);
                }
            }
        } else {
            module
        };
        return match specdev_analyzer::prompt::assemble_preamble(&layout, &modules) {
            Ok(s) => {
                print!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(2)
            }
        };
    }

    // F-006 `--tail`: per-module Tier 3, gate enforced. Exactly one module.
    if tail {
        let name = match module.as_slice() {
            [one] => one.clone(),
            _ => {
                eprintln!(
                    "`gen --tail` requires exactly one `--module <Name>`; got {}.",
                    module.len()
                );
                return ExitCode::from(2);
            }
        };
        // Project-wide precondition first (one fix serves every module),
        // then the per-module open-question gate.
        if let Some(code) = registry_refusal(&layout) {
            return code;
        }
        match specdev_analyzer::prompt::check_codegen_gate(&layout, &name) {
            Ok(Some(refusal)) => {
                eprint!("{}", refusal.message);
                return ExitCode::from(1);
            }
            Ok(None) => {}
            Err(e) => {
                report_error(&e);
                return ExitCode::from(2);
            }
        }
        return match specdev_analyzer::prompt::assemble_tail(&layout, &name) {
            Ok(s) => {
                print!("{s}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(2)
            }
        };
    }

    // Default whole-prompt mode (back-compat single-module). Requires
    // exactly one module. The codegen gate refuses if the module is
    // reachable through any open OpenQuestion (validate also surfaces this
    // as a `codegen_unblocked` warning, but gen is the hard refusal).
    match module.as_slice() {
        [name] => {
            let name = name.clone();
            if let Some(code) = registry_refusal(&layout) {
                return code;
            }
            match specdev_analyzer::prompt::check_codegen_gate(&layout, &name) {
                Ok(Some(refusal)) => {
                    eprint!("{}", refusal.message);
                    return ExitCode::from(1);
                }
                Ok(None) => {}
                Err(e) => {
                    report_error(&e);
                    return ExitCode::from(2);
                }
            }
            match specdev_analyzer::prompt::assemble_module_prompt(&layout, &name) {
                Ok(s) => {
                    print!("{s}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    report_error(&e);
                    ExitCode::from(2)
                }
            }
        }
        [] => {
            eprintln!(
                "codegen is orchestrated by the `/spec-gen` skill — the analyzer assembles\n\
                 the codegen prompt deterministically; Claude emits source. Pass `--module\n\
                 <Name>` for one module's whole prompt, `--preamble` for the shared preamble\n\
                 once, or `--module <Name> --tail` for one module's tail. Or run\n\
                 `specdev-analyzer prompt --module <Name>` directly and pipe to /spec-gen.",
            );
            ExitCode::from(2)
        }
        _ => {
            eprintln!(
                "`gen --module` (whole-prompt mode) takes exactly one module; got {}. \
                 Use `--preamble` to union multiple modules into the shared preamble.",
                module.len()
            );
            ExitCode::from(2)
        }
    }
}

fn run_gen_record(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    module: Option<String>,
    files: Vec<PathBuf>,
    accept_edits: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let record = if accept_edits {
        specdev_analyzer::gen_lineage::record_files_accepting_edits
    } else {
        specdev_analyzer::gen_lineage::record_files
    };
    match record(&layout, module.as_deref(), &files, &generated_at) {
        Ok(recorded) => {
            for path in &recorded {
                println!("recorded {path}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_derive(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
    write: bool,
    accept_edits: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    // The plugin root is read only when the inlined profile declares no
    // rules (to catch a stale inlined profile); resolve it lazily.
    let layout_ref = &layout;
    let resolve = move || resolve_plugin_for_project(layout_ref, plugin_flag.clone());
    let opts = specdev_analyzer::derive::DeriveOptions {
        write,
        accept_edits,
    };
    match specdev_analyzer::derive::run_derive(&layout, &resolve, opts) {
        Ok(report) => {
            for line in &report.lines {
                println!("{}", line.render());
            }
            // 3, not 1: the run completed and every other target was
            // written, but some were held back. 1 means an error stopped
            // the run; a caller must not read the two the same way.
            match report.held() {
                0 => ExitCode::SUCCESS,
                n => {
                    eprintln!(
                        "derive: {n} target(s) held back (`edited` / `conflict` above); \
                         resolve by hand or run `derive --write --accept-edits`"
                    );
                    ExitCode::from(3)
                }
            }
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(1)
        }
    }
}

fn run_gate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    plugin_flag: Option<PathBuf>,
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
    // The plugin root is read only when the inlined registry declares no
    // module-scope gate, to print the block to paste; resolve it lazily.
    let layout_ref = &layout;
    let resolve = move || resolve_plugin_for_project(layout_ref, plugin_flag.clone());
    match specdev_analyzer::gates::run_gate(&layout, &resolve, label, &modules) {
        Ok(run) => {
            for row in &run.record.pruned {
                match &row.module {
                    Some(m) => println!("pruned {} module {m} (label undeclared)", row.label),
                    None => println!("pruned {} batch (label undeclared)", row.label),
                }
            }
            for row in &run.record.written {
                let verdict = if row.pass { "pass" } else { "fail" };
                match &row.module {
                    Some(m) => println!("gate {} {verdict} module {m} key {}", row.label, row.key),
                    None => println!("gate {} {verdict} batch key {}", row.label, row.key),
                }
            }
            if run.pass {
                ExitCode::SUCCESS
            } else {
                // The gate's own exit code, per the decision. A tool that
                // exits 2 is then indistinguishable from this command's
                // own exit-2 classes by code alone — the rows on stdout
                // are what separate "ran and failed" from "did not run".
                ExitCode::from(u8::try_from(run.code).unwrap_or(1))
            }
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_gen_record_gate(
    project: PathBuf,
    spec_path: Option<&std::path::Path>,
    modules: Vec<String>,
    label: &str,
    pass: bool,
) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match specdev_analyzer::gen_lineage::record_gate(&layout, &modules, label, pass) {
        Ok(rows) => {
            for row in &rows {
                let verdict = if row.pass { "pass" } else { "fail" };
                match &row.module {
                    Some(m) => println!("gate {} {verdict} module {m} key {}", row.label, row.key),
                    None => println!("gate {} {verdict} batch key {}", row.label, row.key),
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_report_gen_drift(project: PathBuf, spec_path: Option<&std::path::Path>) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    match specdev_analyzer::gen_lineage::run_gen_drift(&layout) {
        Ok(report) => match report.to_yaml() {
            Ok(yaml) => {
                print!("{yaml}");
                if report.is_clean() {
                    ExitCode::SUCCESS
                } else {
                    // Drift present → exit 1 so the skill can branch
                    // without parsing the YAML.
                    ExitCode::from(1)
                }
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(2)
            }
        },
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
        }
    }
}

fn run_report_gen_plan(project: PathBuf, spec_path: Option<&std::path::Path>) -> ExitCode {
    let layout = match LayoutContext::discover(&project, spec_path) {
        Ok(l) => l,
        Err(e) => {
            report_error(&e);
            return ExitCode::from(2);
        }
    };
    // gen-plan is multi-valued (one verdict per module); the skill parses
    // the YAML to drive skip/regen. Exit reflects success/failure only, not
    // the plan's contents (cf. gen-drift's 0/1 clean/drift split).
    match specdev_analyzer::gen_lineage::run_gen_plan(&layout) {
        Ok(report) => match report.to_yaml() {
            Ok(yaml) => {
                print!("{yaml}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                report_error(&e);
                ExitCode::from(2)
            }
        },
        Err(e) => {
            report_error(&e);
            ExitCode::from(2)
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
