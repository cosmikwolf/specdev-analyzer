//! Phase 13: skeleton-mode codegen (doc 16 §2).
//!
//! Skeleton mode emits a navigable, compilable Rust source tree from the
//! validated spec. Each `[[items]]` entry projects to one Rust item with
//! `unimplemented!("REQ-...: <one-line>")` bodies (or a kind-appropriate
//! stub for non-fn kinds). Cargo.toml is rendered via the existing
//! `cargo_toml` surface (Phase 8). After emission the analyzer runs
//! `cargo check`; on failure the entire skeleton tree is rolled back —
//! a failed `cargo check` is a "fix the spec, rerun" signal, not a valid
//! handoff artifact (doc 16 §2 invariant).
//!
//! Phase 16 §8.1 step 4: dispatch off the inlined `profile.yaml`'s
//! `skeleton.supported` flag and `skeleton.steps` ordering. Pre-D52
//! projects (no `profile.yaml`) keep the legacy `profile != "library-api"`
//! gate as a backward-compat fallback. Step 4 ships the *machinery* — the
//! step-walker + supported gate. Profile content (firmware-layered,
//! audio-plugin, rust-workspace skeletons) ships post-step-4 gated on
//! dogfood evidence.
//!
//! v1 supported profiles: library-api only. `firmware-layered`,
//! `audio-plugin`, and `rust-workspace` ship `skeleton.supported: false`
//! in their manifests; running `gen --skeleton` against them errors
//! `SKEL-001` (machinery in place; content authoring deferred).
//!
//! Error codes:
//! - `SKEL-001` — profile's manifest declares `skeleton.supported: false`
//!   (or, for pre-D52 projects, profile is not `library-api`).
//! - `SKEL-002` — source dir non-empty (skeleton refuses to clobber).
//! - `SKEL-003` — module spec missing `name` or items[] kind requires
//!   `signature_hint` that the spec didn't provide.
//! - `SKEL-004` — unknown items[] kind.
//! - `SKEL-101` — `cargo check` could not be invoked (cargo not on PATH).
//! - `SKEL-102` — `cargo check` failed on the emitted skeleton tree.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::items::sync::{parse_spec_items, SpecItem};
use crate::layout::LayoutContext;
use crate::manifest::{Profile, SkeletonStep};

/// Per-module spec view used by skeleton emission. Carries enough state
/// to drive both the source-tree emission and the HANDOFF.md report.
struct ModuleSpec {
    name: String,
    items: Vec<SpecItem>,
    satisfies: Vec<String>,
    context_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkeletonReport {
    pub modules: Vec<EmittedModule>,
    pub root_files: Vec<PathBuf>,
    pub handoff_md_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmittedModule {
    pub name: String,
    pub path: PathBuf,
    pub stub_count: usize,
    pub re_export_count: usize,
    pub item_summary: Vec<String>,
    pub satisfies: Vec<String>,
    pub context_files: Vec<String>,
}

/// Default skeleton step ordering for pre-D52 projects (no `profile.yaml`)
/// and for manifest-driven projects whose `skeleton.steps` is empty. Mirrors
/// library-api's manifest ordering — the only currently-supported skeleton
/// shape — so pre-D52 fallback and post-D52 manifest paths produce the same
/// emission order.
const DEFAULT_STEPS: [SkeletonStep; 3] = [
    SkeletonStep::EmitCargoToml,
    SkeletonStep::WalkModules,
    SkeletonStep::EmitHandoffMd,
];

/// Load `<specdev_root>/profile.yaml` if present (Phase 16 §8.1 step 4).
/// Returns `Ok(None)` for pre-D52 projects without an inlined manifest;
/// `Err` only when the manifest exists but fails to validate or parse.
/// `cargo_toml.rs` ships a parallel helper — kept module-private here to
/// keep step 4's surface tight; a shared helper is a v1.x cleanup.
fn load_inlined_profile(layout: &LayoutContext) -> Result<Option<Profile>> {
    let path = layout.specdev_root.join("profile.yaml");
    if !path.is_file() {
        return Ok(None);
    }
    Profile::load(&path).map(Some)
}

/// Emit a full-project skeleton tree under `<project>/<paths.source>/`.
/// Refuses to run if the source dir is non-empty (use a fresh project or
/// remove the existing tree first).
pub fn run_skeleton(layout: &LayoutContext) -> Result<SkeletonReport> {
    let manifest = load_inlined_profile(layout)?;
    let manifest_steps = match &manifest {
        Some(p) => match p.skeleton.as_ref() {
            Some(skel) if !skel.supported => {
                return Err(anyhow!(
                    "[SKEL-001] this profile's manifest declares \
                     `skeleton.supported: false`; gen-skeleton is not \
                     available for this project shape. Skeleton emission \
                     for shapes other than the currently-supported one is \
                     gated on follow-up authoring passes (doc 16 §7, \
                     doc 21 §8.1 step 4)."
                ));
            }
            Some(skel) => Some(skel.steps.clone()),
            None => None,
        },
        None => {
            // Pre-D52 backward-compat fallback (doc 21 §6.5). The legacy
            // gate remains until `specdev migrate` ships in step 9.
            if layout.spec.project.profile != "library-api" {
                return Err(anyhow!(
                    "[SKEL-001] skeleton mode is library-api only in v1; \
                     this project's profile is `{}`. firmware-layered skeleton \
                     support is a bounded deferral (doc 16 §7).",
                    layout.spec.project.profile
                ));
            }
            None
        }
    };

    let source_dir = layout.source_dir.clone();
    if source_dir.is_dir() {
        let visible: Vec<_> = std::fs::read_dir(&source_dir)
            .with_context(|| format!("reading {}", source_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| !n.starts_with('.'))
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect();
        if !visible.is_empty() {
            return Err(anyhow!(
                "[SKEL-002] skeleton mode refuses to clobber a non-empty \
                 source dir; {} contains {} entries. Remove the dir or \
                 emit into a fresh project.",
                source_dir.display(),
                visible.len()
            ));
        }
    } else {
        std::fs::create_dir_all(&source_dir)
            .with_context(|| format!("creating {}", source_dir.display()))?;
    }

    let mut module_paths: Vec<PathBuf> = Vec::new();
    if layout.modules_dir.is_dir() {
        for entry in std::fs::read_dir(&layout.modules_dir)
            .with_context(|| format!("reading {}", layout.modules_dir.display()))?
        {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                module_paths.push(p);
            }
        }
        module_paths.sort();
    }

    let mut modules: Vec<ModuleSpec> = Vec::new();
    for path in &module_paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        let module_name = value
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "[SKEL-003] module spec at {} missing required `name` field",
                    path.display()
                )
            })?
            .to_string();
        let label = path.display().to_string();
        let items = parse_spec_items(&value, &label)?;
        let satisfies = value
            .get("satisfies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let context_files = crate::prompt::parse_context_files(&value);
        modules.push(ModuleSpec {
            name: module_name,
            items,
            satisfies,
            context_files,
        });
    }

    let mut written: Vec<PathBuf> = Vec::new();
    let mut report_modules: Vec<EmittedModule> = Vec::new();
    let mut cargo_path: Option<PathBuf> = None;
    let mut handoff_md_path: Option<PathBuf> = None;

    let steps_owned: Vec<SkeletonStep> = manifest_steps
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(<[SkeletonStep]>::to_vec)
        .unwrap_or_else(|| DEFAULT_STEPS.to_vec());

    for step in &steps_owned {
        match step {
            SkeletonStep::EmitCargoToml => {
                cargo_path = Some(step_emit_cargo_toml(layout, &mut written)?);
            }
            SkeletonStep::WalkModules => {
                step_walk_modules(
                    &source_dir,
                    &modules,
                    &mut report_modules,
                    &mut written,
                )?;
            }
            SkeletonStep::EmitHandoffMd => {
                handoff_md_path = Some(step_emit_handoff_md(
                    layout,
                    &modules,
                    &report_modules,
                    &mut written,
                )?);
            }
        }
    }

    // The library-api skeleton steps universe today always emits all three
    // artifacts. Future profiles with different step sets land their own
    // SkeletonReport shape (or extend this one) when they're authored.
    let cargo_path = cargo_path.ok_or_else(|| {
        anyhow!(
            "[SKEL-001] manifest's `skeleton.steps` for this profile does not \
             include `emit_cargo_toml`; v1 skeleton mode requires Cargo.toml \
             emission in the step set"
        )
    })?;
    let handoff_md_path = handoff_md_path.ok_or_else(|| {
        anyhow!(
            "[SKEL-001] manifest's `skeleton.steps` for this profile does not \
             include `emit_handoff_md`; v1 skeleton mode requires HANDOFF.md \
             emission in the step set"
        )
    })?;

    let check = Command::new("cargo")
        .arg("check")
        .arg("--message-format=short")
        .current_dir(&layout.project_root)
        .output();
    match check {
        Ok(output) if output.status.success() => Ok(SkeletonReport {
            modules: report_modules,
            root_files: vec![source_dir, cargo_path],
            handoff_md_path,
        }),
        Ok(output) => {
            rollback(&written);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow!(
                "[SKEL-102] `cargo check` failed on skeleton emission. \
                 Skeleton tree rolled back. Fix the spec and rerun.\n\
                 ----- cargo check stderr -----\n{stderr}",
            ))
        }
        Err(e) => {
            rollback(&written);
            Err(anyhow!(
                "[SKEL-101] failed to invoke `cargo check`: {e}. Skeleton tree rolled back."
            ))
        }
    }
}

/// Step handler — `emit_cargo_toml`. Renders `<project>/Cargo.toml` via
/// `cargo_toml::render` and pairs each `extra_files` entry (e.g.
/// `src/bin/standalone.rs`) onto disk. Returns the Cargo.toml path so the
/// caller can carry it through to the final `SkeletonReport`. Pre-existing
/// `extra_files` paths are preserved (F-016 hand-edited bin survival).
fn step_emit_cargo_toml(
    layout: &LayoutContext,
    written: &mut Vec<PathBuf>,
) -> Result<PathBuf> {
    let cargo = crate::cargo_toml::render(layout)
        .context("rendering Cargo.toml for skeleton emission")?;
    let cargo_path = layout.project_root.join("Cargo.toml");
    let cargo_pre_existed = cargo_path.exists();
    std::fs::write(&cargo_path, &cargo.text)
        .with_context(|| format!("writing {}", cargo_path.display()))?;
    if !cargo_pre_existed {
        written.push(cargo_path.clone());
    }
    for (rel, body) in &cargo.extra_files {
        let dst = layout.project_root.join(rel);
        if dst.exists() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&dst, body)
            .with_context(|| format!("writing {}", dst.display()))?;
        written.push(dst);
    }
    Ok(cargo_path)
}

/// Step handler — `walk_modules`. Emits one `<module>.rs` per module spec
/// plus a single `lib.rs` re-exporting them. Each call appends the
/// emitted-module summary to `report_modules` so the subsequent
/// `emit_handoff_md` step can reference it.
fn step_walk_modules(
    source_dir: &std::path::Path,
    modules: &[ModuleSpec],
    report_modules: &mut Vec<EmittedModule>,
    written: &mut Vec<PathBuf>,
) -> Result<()> {
    for spec in modules {
        let snake = to_snake_case(&spec.name);
        let mod_path = source_dir.join(format!("{snake}.rs"));
        let body = emit_module_body(&spec.name, &spec.items)?;
        std::fs::write(&mod_path, body)
            .with_context(|| format!("writing {}", mod_path.display()))?;
        written.push(mod_path.clone());

        let stub_count = spec.items.iter().filter(|i| i.kind != "re_export").count();
        let re_export_count = spec.items.iter().filter(|i| i.kind == "re_export").count();
        let item_summary = spec
            .items
            .iter()
            .map(|i| format!("{}:{}", i.kind, i.name))
            .collect();
        report_modules.push(EmittedModule {
            name: spec.name.clone(),
            path: mod_path,
            stub_count,
            re_export_count,
            item_summary,
            satisfies: spec.satisfies.clone(),
            context_files: spec.context_files.clone(),
        });
    }
    let lib_path = source_dir.join("lib.rs");
    let lib_body = emit_lib_body(modules);
    std::fs::write(&lib_path, lib_body)
        .with_context(|| format!("writing {}", lib_path.display()))?;
    written.push(lib_path);
    Ok(())
}

/// Step handler — `emit_handoff_md`. Renders the per-project HANDOFF.md
/// (coverage matrix + skeleton inventory + suggested next steps) into
/// `<project>/specs/HANDOFF.md`. Returns the file path. Must run after
/// `walk_modules` because it consumes `report_modules`.
fn step_emit_handoff_md(
    layout: &LayoutContext,
    modules: &[ModuleSpec],
    report_modules: &[EmittedModule],
    written: &mut Vec<PathBuf>,
) -> Result<PathBuf> {
    let handoff_md_path = layout.specs_dir.join("HANDOFF.md");
    let handoff_body = render_handoff_md(layout, modules, report_modules)?;
    std::fs::write(&handoff_md_path, handoff_body)
        .with_context(|| format!("writing {}", handoff_md_path.display()))?;
    written.push(handoff_md_path.clone());
    Ok(handoff_md_path)
}

fn rollback(files: &[PathBuf]) {
    for f in files {
        let _ = std::fs::remove_file(f);
    }
}

fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn emit_lib_body(modules: &[ModuleSpec]) -> String {
    let mut out = String::new();
    out.push_str(
        "//! Skeleton emission (Phase 13 — doc 16 §2). Each module's body\n\
         //! contains `unimplemented!()` stubs and re-exports projected from\n\
         //! its `[[items]]` entries. See `specs/HANDOFF.md` for the\n\
         //! per-requirement coverage matrix and per-module skeleton inventory.\n\n",
    );
    for spec in modules {
        let snake = to_snake_case(&spec.name);
        out.push_str(&format!("pub mod {snake};\n"));
    }
    out
}

/// Render `specs/HANDOFF.md` per doc 16 §2: coverage matrix +
/// skeleton inventory + plugin gaps + suggested next steps.
fn render_handoff_md(
    layout: &LayoutContext,
    modules: &[ModuleSpec],
    emitted: &[EmittedModule],
) -> Result<String> {
    let mut out = String::new();
    out.push_str("# HANDOFF\n\n");
    out.push_str(&format!(
        "Generated by `specdev-analyzer gen --skeleton` — Phase 13, doc 16 §2.\n\
         Project: `{}` ({}). Regenerated on every skeleton run; do not hand-edit\n\
         outside the `## Plugin gaps` section (regenerate-clobbers all sections).\n\n",
        layout.spec.project.name, layout.spec.project.profile
    ));

    // Coverage matrix — requirements → modules.
    out.push_str("## Coverage matrix\n\n");
    let requirements = load_requirement_ids(layout)?;
    if requirements.is_empty() {
        out.push_str(
            "_No `requirements.toml` found, or it carries no `[[requirement]]` \
             entries — coverage matrix omitted._\n\n",
        );
    } else {
        out.push_str("| Requirement | Satisfied by |\n");
        out.push_str("|---|---|\n");
        for req in &requirements {
            let mut satisfiers: Vec<String> = Vec::new();
            for spec in modules {
                let module_satisfies = spec.satisfies.iter().any(|s| s == req);
                let item_satisfies =
                    spec.items.iter().any(|i| i.satisfies.iter().any(|s| s == req));
                if module_satisfies || item_satisfies {
                    satisfiers.push(format!("`{}`", spec.name));
                }
            }
            let cell = if satisfiers.is_empty() {
                "**ORPHAN** — no module satisfies this requirement.".to_string()
            } else {
                satisfiers.join(", ")
            };
            out.push_str(&format!("| `{req}` | {cell} |\n"));
        }
        out.push('\n');
    }

    // Skeleton inventory — per module.
    out.push_str("## Skeleton inventory\n\n");
    if emitted.is_empty() {
        out.push_str("_No modules emitted._\n\n");
    } else {
        for m in emitted {
            out.push_str(&format!(
                "### `{}` — `{}`\n\n",
                m.name,
                m.path
                    .strip_prefix(&layout.project_root)
                    .unwrap_or(&m.path)
                    .display()
            ));
            out.push_str(&format!(
                "- Stubs: **{}** (`unimplemented!()` items)\n\
                 - Re-exports: **{}**\n",
                m.stub_count, m.re_export_count
            ));
            if !m.satisfies.is_empty() {
                out.push_str(&format!(
                    "- Module-level satisfies: {}\n",
                    m.satisfies
                        .iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !m.context_files.is_empty() {
                out.push_str(&format!(
                    "- Context files inlined at prompt time: {}\n",
                    m.context_files
                        .iter()
                        .map(|c| format!("`{c}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            } else {
                out.push_str(
                    "- Context files: _none_ — consider adding `module.context_files` \
                     for deep-domain modules before running `--strict`.\n",
                );
            }
            if !m.item_summary.is_empty() {
                out.push_str("- Items:\n");
                for s in &m.item_summary {
                    out.push_str(&format!("  - `{s}`\n"));
                }
            }
            out.push('\n');
        }
    }

    out.push_str("## Plugin gaps\n\n");
    out.push_str(
        "_Free-form notes on plugin / spec / tooling friction encountered while\n\
         producing this skeleton. The generator overwrites this file on every\n\
         run; capture lasting feedback in `11-issues.md` (or your project's\n\
         feedback log) rather than relying on this section to persist._\n\n",
    );

    out.push_str("## Suggested next steps\n\n");
    let with_ctx: Vec<&EmittedModule> = emitted
        .iter()
        .filter(|m| !m.context_files.is_empty())
        .collect();
    let without_ctx: Vec<&EmittedModule> = emitted
        .iter()
        .filter(|m| m.context_files.is_empty())
        .collect();
    if !with_ctx.is_empty() {
        out.push_str(
            "**Modules ready for `--strict` (have `context_files`):**\n",
        );
        for m in &with_ctx {
            out.push_str(&format!("- `{}`\n", m.name));
        }
        out.push('\n');
    }
    if !without_ctx.is_empty() {
        out.push_str(
            "**Modules to add `context_files` to before running `--strict`:**\n",
        );
        for m in &without_ctx {
            out.push_str(&format!("- `{}`\n", m.name));
        }
        out.push('\n');
    }
    if with_ctx.is_empty() && without_ctx.is_empty() {
        out.push_str("_No modules; nothing to suggest yet._\n\n");
    }

    Ok(out)
}

/// Read `[[requirement]] id` entries from `specs/requirements.toml`.
/// Returns an empty list (not an error) when the file is missing — the
/// HANDOFF.md generator presents an empty coverage matrix in that case
/// rather than refusing to run.
///
/// F-014 (zfm dogfood, 2026-05-04): the array key is `requirement`
/// (singular) per the requirements.schema.yaml across all profiles, not
/// `requirements`. Pre-fix the lookup was always empty, which is what
/// produced the always-empty coverage matrix the dogfood reported.
fn load_requirement_ids(layout: &LayoutContext) -> Result<Vec<String>> {
    if !layout.requirements_toml_path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&layout.requirements_toml_path)
        .with_context(|| format!("reading {}", layout.requirements_toml_path.display()))?;
    let value: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing {}", layout.requirements_toml_path.display()))?;
    let arr = value
        .get("requirement")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = arr
        .iter()
        .filter_map(|t| {
            t.get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn emit_module_body(module_name: &str, items: &[SpecItem]) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!(
        "//! `{module_name}` — skeleton emission (Phase 13). Bodies are\n\
         //! `unimplemented!()`; see HANDOFF.md for the coverage matrix.\n\n"
    ));
    if items.is_empty() {
        out.push_str("// No items[] entries declared on this module's spec.\n");
        return Ok(out);
    }
    for item in items {
        emit_item(&mut out, item)?;
        out.push('\n');
    }
    Ok(out)
}

fn emit_item(out: &mut String, item: &SpecItem) -> Result<()> {
    if !item.description.is_empty() {
        for line in item.description.lines() {
            out.push_str(&format!("/// {line}\n"));
        }
    }
    let marker = format_marker(item);
    match item.kind.as_str() {
        "fn" => match &item.signature_hint {
            Some(hint) => {
                let sig = strip_leading_pub(hint);
                out.push_str(&format!("pub {sig} {{\n    {marker}\n}}\n"));
            }
            None => {
                out.push_str(&format!(
                    "pub fn {}() {{\n    {marker}\n}}\n",
                    item.name
                ));
            }
        },
        "struct" => match &item.signature_hint {
            Some(hint) => {
                let sig = strip_leading_pub(hint);
                if sig.contains('{') || sig.ends_with(';') {
                    out.push_str(&format!("pub {sig}\n"));
                } else {
                    out.push_str(&format!("pub {sig};\n"));
                }
            }
            None => out.push_str(&format!("pub struct {};\n", item.name)),
        },
        "enum" => match &item.signature_hint {
            Some(hint) => {
                let sig = strip_leading_pub(hint);
                out.push_str(&format!("pub {sig}\n"));
            }
            None => out.push_str(&format!(
                "pub enum {} {{\n    Unimplemented,\n}}\n",
                item.name
            )),
        },
        "trait" => match &item.signature_hint {
            Some(hint) => {
                let sig = strip_leading_pub(hint);
                out.push_str(&format!("pub {sig}\n"));
            }
            None => out.push_str(&format!("pub trait {} {{}}\n", item.name)),
        },
        "impl" => {
            let target_type = item
                .extras
                .get("target_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if target_type.is_empty() {
                return Err(anyhow!(
                    "[SKEL-003] item `{}` of kind `impl` missing `target_type`",
                    item.name
                ));
            }
            match item.extras.get("trait_ref").and_then(|v| v.as_str()) {
                Some(t) => out.push_str(&format!("impl {t} for {target_type} {{}}\n")),
                None => out.push_str(&format!("impl {target_type} {{}}\n")),
            }
        }
        "type" | "const" | "static" | "union" => match &item.signature_hint {
            Some(hint) => {
                let sig = strip_leading_pub(hint);
                if sig.ends_with(';') {
                    out.push_str(&format!("pub {sig}\n"));
                } else {
                    out.push_str(&format!("pub {sig};\n"));
                }
            }
            None => {
                return Err(anyhow!(
                    "[SKEL-003] item `{}` of kind `{}` requires `signature_hint` \
                     for skeleton mode (no defaultable form)",
                    item.name,
                    item.kind
                ));
            }
        },
        "re_export" => {
            let target = item
                .extras
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if target.is_empty() {
                return Err(anyhow!(
                    "[SKEL-003] item `{}` of kind `re_export` missing `target`",
                    item.name
                ));
            }
            let visibility = item
                .extras
                .get("visibility")
                .and_then(|v| v.as_str())
                .unwrap_or("pub");
            let vis_keyword = match visibility {
                "crate" => "pub(crate)",
                _ => "pub",
            };
            let alias = item.extras.get("alias").and_then(|v| v.as_str());
            let path = if target.contains("::") && !target.starts_with("crate::") {
                format!("crate::{target}")
            } else {
                target.to_string()
            };
            match alias {
                Some(a) => out.push_str(&format!("{vis_keyword} use {path} as {a};\n")),
                None => out.push_str(&format!("{vis_keyword} use {path};\n")),
            }
        }
        "macro" | "extern_block" => {
            out.push_str(&format!(
                "// SKELETON: kind `{}` for `{}` requires hand-implementation; \
                 see HANDOFF.md\n",
                item.kind, item.name
            ));
        }
        other => {
            return Err(anyhow!("[SKEL-004] unknown item kind `{other}`"));
        }
    }
    Ok(())
}

fn strip_leading_pub(hint: &str) -> &str {
    let h = hint.trim_start();
    h.strip_prefix("pub(crate) ")
        .or_else(|| h.strip_prefix("pub "))
        .unwrap_or(h)
}

/// Format the `unimplemented!(...)` marker per doc 16 §2.
/// Shape: `unimplemented!("REQ-001, REQ-002: <one-line>")`. When
/// `satisfies` is empty (re_export items), uses just the description.
fn format_marker(item: &SpecItem) -> String {
    let summary = first_sentence(&item.description);
    let safe_summary = summary.replace('\\', "\\\\").replace('"', "\\\"");
    if item.satisfies.is_empty() {
        format!("unimplemented!(\"{safe_summary}\")")
    } else {
        let req = item.satisfies.join(", ");
        format!("unimplemented!(\"{req}: {safe_summary}\")")
    }
}

fn first_sentence(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(idx) = trimmed.find(['.', '\n']) {
        trimmed[..idx].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(kind: &str, name: &str, sig: Option<&str>) -> SpecItem {
        SpecItem {
            kind: kind.to_string(),
            name: name.to_string(),
            description: "first sentence. more.".to_string(),
            satisfies: vec!["REQ-001".to_string()],
            signature_hint: sig.map(str::to_string),
            extras: serde_json::Map::new(),
        }
    }

    #[test]
    fn fn_without_hint_emits_zero_arg_unit_return() {
        let mut s = String::new();
        emit_item(&mut s, &item("fn", "do_it", None)).unwrap();
        assert!(s.contains("pub fn do_it() {"));
        assert!(s.contains("unimplemented!(\"REQ-001: first sentence\")"));
    }

    #[test]
    fn fn_with_hint_uses_hint_signature() {
        let mut s = String::new();
        emit_item(
            &mut s,
            &item("fn", "do_it", Some("fn do_it(x: u32) -> u32")),
        )
        .unwrap();
        assert!(s.contains("pub fn do_it(x: u32) -> u32 {"));
    }

    #[test]
    fn re_export_emits_pub_use_with_crate_prefix() {
        let mut it = item("re_export", "Foo", None);
        it.satisfies.clear();
        it.extras.insert("target".into(), json!("util::Foo"));
        it.extras.insert("visibility".into(), json!("pub"));
        let mut s = String::new();
        emit_item(&mut s, &it).unwrap();
        assert!(s.contains("pub use crate::util::Foo;"));
    }

    #[test]
    fn re_export_with_alias_and_crate_visibility() {
        let mut it = item("re_export", "F", None);
        it.satisfies.clear();
        it.extras.insert("target".into(), json!("inner::Foo"));
        it.extras.insert("visibility".into(), json!("crate"));
        it.extras.insert("alias".into(), json!("Foo2"));
        let mut s = String::new();
        emit_item(&mut s, &it).unwrap();
        assert!(s.contains("pub(crate) use crate::inner::Foo as Foo2;"));
    }

    #[test]
    fn type_without_hint_errors_skel_003() {
        let mut s = String::new();
        let err = emit_item(&mut s, &item("type", "Bytes", None)).unwrap_err();
        assert!(format!("{err:#}").contains("SKEL-003"));
    }

    #[test]
    fn const_with_hint_emits_pub_const() {
        let mut s = String::new();
        emit_item(
            &mut s,
            &item("const", "MAX", Some("const MAX: u32 = 32")),
        )
        .unwrap();
        assert!(s.contains("pub const MAX: u32 = 32;"));
    }

    #[test]
    fn snake_case_camelcase_to_snake() {
        assert_eq!(to_snake_case("Envelope"), "envelope");
        assert_eq!(to_snake_case("OperatorBank"), "operator_bank");
        assert_eq!(to_snake_case("snake_already"), "snake_already");
    }

    #[test]
    fn marker_quotes_safely() {
        let mut it = item("fn", "f", None);
        it.description = "a \"quoted\" thing.".into();
        let m = format_marker(&it);
        assert!(m.contains("\\\"quoted\\\""));
    }
}
