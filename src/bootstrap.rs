//! `specdev bootstrap --profile <name> [<target>]`.
//!
//! One-shot scaffolding from a plugin profile into a fresh project tree.
//! Profiles are not inlineable and have no lineage entry of their own —
//! the only lineage written here is a `[meta]` stamp
//! recording the plugin version and the profile name, which inline /
//! drift consult to interpret upstream changes.
//!
//! Layout assumed in the plugin tree:
//!
//! ```text
//! <plugin-root>/profiles/<name>/
//!   spec.toml.template
//!   domain.toml.template
//!   schemas/{module,architecture,requirements}.schema.yaml
//!   reductions/project.yaml
//!   codegen/<lang>/module.conventions.md
//!   README.md                 # author-facing, NOT copied
//! ```
//!
//! Templates support a closed set of
//! placeholder tokens; any unrecognized `{{...}}` is a fatal error.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tracing::info;

use crate::lineage::{Lineage, MetaSection};
use crate::manifest::{BootstrapStep, Profile};

/// CLI inputs for `bootstrap`.
#[derive(Debug, Clone)]
pub struct BootstrapInputs {
    /// Profile to scaffold (`firmware-layered`, `library-api`, …).
    pub profile: String,
    /// Target language for codegen (`rust`, `c`).
    pub language: String,
    /// Project name; conventionally lowercase + hyphens.
    pub project_name: String,
    /// Directory to bootstrap into. Conventionally the project root the
    /// user just `mkdir`'d.
    pub target_dir: PathBuf,
}

const PLACEHOLDER_PROJECT_NAME: &str = "{{project_name}}";
const PLACEHOLDER_LANGUAGE: &str = "{{language}}";
const PLACEHOLDER_PROFILE: &str = "{{profile}}";

/// Read the plugin's `plugin.json` and extract `version`. Mirrors
/// [`crate::inline::plugin_version`]; lifted here so bootstrap doesn't
/// depend on inline's private surface.
fn plugin_version(plugin_root: &Path) -> Result<String> {
    let manifest = plugin_root.join(crate::plugin_root::PLUGIN_MARKER);
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", manifest.display()))?;
    Ok(value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string())
}

fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Apply the closed-set placeholder substitution to a string. Any
/// `{{...}}` token outside the supported set fails fast — failing loudly
/// prevents a profile template from silently shipping a placeholder that
/// nothing fills.
fn substitute(text: &str, inputs: &BootstrapInputs, source_label: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find closing `}}`.
            let Some(rel_end) = text[i + 2..].find("}}") else {
                return Err(anyhow!(
                    "[BOOTSTRAP-002] {source_label}: unterminated `{{{{` placeholder",
                ));
            };
            let token_end = i + 2 + rel_end + 2;
            let token = &text[i..token_end];
            let replacement = match token {
                PLACEHOLDER_PROJECT_NAME => &inputs.project_name,
                PLACEHOLDER_LANGUAGE => &inputs.language,
                PLACEHOLDER_PROFILE => &inputs.profile,
                _ => {
                    return Err(anyhow!(
                        "[BOOTSTRAP-002] {source_label}: unknown placeholder {token:?}; supported: {PLACEHOLDER_PROJECT_NAME}, {PLACEHOLDER_LANGUAGE}, {PLACEHOLDER_PROFILE}",
                    ));
                }
            };
            out.push_str(replacement);
            i = token_end;
        } else {
            // UTF-8-safe pass-through: find the next placeholder (or end of
            // string) and copy the slice verbatim. Earlier templates
            // were ASCII-only, masking the bug; YAML schemas with em-dashes
            // and other multi-byte chars surface it.
            let next_open = text[i..].find("{{").map(|r| i + r).unwrap_or(bytes.len());
            out.push_str(&text[i..next_open]);
            i = next_open;
        }
    }
    Ok(out)
}

/// One source-file → destination-file mapping describing what bootstrap
/// copies.
struct ProfileFile {
    /// Path within `<plugin-root>/profiles/<profile>/`.
    src_rel: PathBuf,
    /// Path within `<target>/` (project tree).
    dst_rel: PathBuf,
    /// Whether to apply placeholder substitution. `*.template` files do;
    /// schema/reduction/codegen content is copied verbatim.
    substitute: bool,
}

/// Build the per-language file plan for a profile. The two routing
/// special cases (`spec.toml.template` → project root,
/// `domain.toml.template` → `specs/specdev/`) are hard-coded; everything
/// else under the profile root, `schemas/`, and `codegen/<lang>/` is
/// dynamically enumerated so a profile can ship arbitrary supporting
/// schemas (e.g. firmware-layered's `metadata.schema.yaml`), arbitrary
/// codegen-prose files (e.g. rust-workspace's `workspace.conventions.md`),
/// and arbitrary `<artifact>.toml.template` files at the profile root
/// (architecture / requirements / decisions / open_questions / future
/// additions) without touching this code.
fn profile_plan(profile_dir: &Path, language: &str) -> Result<Vec<ProfileFile>> {
    let mut plan: Vec<ProfileFile> = vec![
        ProfileFile {
            src_rel: PathBuf::from("spec.toml.template"),
            dst_rel: PathBuf::from("spec.toml"),
            substitute: true,
        },
        ProfileFile {
            src_rel: PathBuf::from("domain.toml.template"),
            dst_rel: PathBuf::from("specs/specdev/domain.toml"),
            substitute: true,
        },
        ProfileFile {
            src_rel: PathBuf::from("reductions/project.yaml"),
            dst_rel: PathBuf::from("specs/specdev/reductions/project.yaml"),
            substitute: false,
        },
    ];

    // Dynamically enumerate `*.toml.template` files at the profile
    // root. Each `<basename>.toml.template` emits to
    // `<target>/specs/<basename>.toml`. `spec.toml.template` and
    // `domain.toml.template` are skipped here (they have non-standard
    // routing handled above). The dispatch is data-driven: a profile
    // adds a new author-facing TOML artifact by dropping a template
    // file into the profile root; bootstrap picks it up automatically.
    let mut root_templates: Vec<PathBuf> = std::fs::read_dir(profile_dir)
        .with_context(|| format!("reading {}", profile_dir.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(".toml.template"))
        })
        .collect();
    root_templates.sort();
    for entry in root_templates {
        let Some(file_name) = entry.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // Routing special cases handled above.
        if file_name == "spec.toml.template" || file_name == "domain.toml.template" {
            continue;
        }
        // `<basename>.toml.template` → `specs/<basename>.toml`.
        let basename = file_name.trim_end_matches(".template");
        plan.push(ProfileFile {
            src_rel: PathBuf::from(file_name),
            dst_rel: PathBuf::from(format!("specs/{basename}")),
            substitute: true,
        });
    }

    // When the profile ships a `profile.yaml` manifest, copy it
    // byte-faithfully alongside `domain.toml`. The file is static YAML; no
    // template substitution applies. Profiles without profile.yaml are
    // skipped silently, and the analyzer falls back to its built-in default.
    if profile_dir.join("profile.yaml").is_file() {
        plan.push(ProfileFile {
            src_rel: PathBuf::from("profile.yaml"),
            dst_rel: PathBuf::from("specs/specdev/profile.yaml"),
            substitute: false,
        });
    }

    // When the profile ships a `templates/` directory (referenced from
    // profile.yaml's `paired_stub.template_file` and any future
    // template_file fields), copy each file byte-faithfully to
    // `<target>/specs/specdev/templates/`. Tokens in the file body are
    // resolved at render time (closed `{key}` grammar); bootstrap-time copy
    // is byte-faithful. Subdirectories are not walked in v1 (no shipped
    // template needs them); a flat layout keeps `specs/specdev/`
    // structurally addressable.
    let templates_dir = profile_dir.join("templates");
    if templates_dir.is_dir() {
        let mut template_entries: Vec<PathBuf> = std::fs::read_dir(&templates_dir)
            .with_context(|| format!("reading {}", templates_dir.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        template_entries.sort();
        for entry in template_entries {
            let Some(file_name) = entry.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            plan.push(ProfileFile {
                src_rel: PathBuf::from(format!("templates/{file_name}")),
                dst_rel: PathBuf::from(format!("specs/specdev/templates/{file_name}")),
                substitute: false,
            });
        }
    }

    // Dynamically enumerate `codegen/<lang>/*.md`. Lex-sorted; copied
    // verbatim. Profiles ship whatever convention files they need:
    //   - firmware-layered, library-api: `module.conventions.md`
    //   - rust-workspace: `workspace.conventions.md`
    //   - future profiles: any `*.conventions.md` (or other `*.md`)
    let codegen_lang_dir = profile_dir.join("codegen").join(language);
    if codegen_lang_dir.is_dir() {
        let mut codegen_entries: Vec<PathBuf> = std::fs::read_dir(&codegen_lang_dir)
            .with_context(|| format!("reading {}", codegen_lang_dir.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.ends_with(".md"))
            })
            .collect();
        codegen_entries.sort();
        for entry in codegen_entries {
            let Some(file_name) = entry.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            plan.push(ProfileFile {
                src_rel: PathBuf::from(format!("codegen/{language}/{file_name}")),
                dst_rel: PathBuf::from(format!(
                    "specs/specdev/codegen/{language}/{file_name}"
                )),
                substitute: false,
            });
        }
    }

    // Dynamically enumerate the profile's schemas/ directory. Every
    // `*.schema.yaml` file at the top level is copied verbatim. Profiles
    // ship whatever set they need (firmware-layered: module + architecture
    // + requirements + enums + metadata; library-api: module + architecture
    // + requirements + enums).
    let schemas_dir = profile_dir.join("schemas");
    if schemas_dir.is_dir() {
        let mut schema_entries: Vec<PathBuf> = std::fs::read_dir(&schemas_dir)
            .with_context(|| format!("reading {}", schemas_dir.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.ends_with(".schema.yaml"))
            })
            .collect();
        schema_entries.sort();
        for entry in schema_entries {
            let Some(file_name) = entry.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // `module.schema.yaml` carries the `{{language}}`
            // placeholder in its items[] $ref, so it must be substituted at
            // bootstrap. All other profile schemas are copied byte-
            // faithfully. Closed-set placeholder grammar ensures
            // unrelated `{{...}}` patterns in any schema would surface as
            // BOOTSTRAP-002, not silent corruption.
            let substitute = file_name == "module.schema.yaml";
            plan.push(ProfileFile {
                src_rel: PathBuf::from(format!("schemas/{file_name}")),
                dst_rel: PathBuf::from(format!("specs/specdev/schemas/{file_name}")),
                substitute,
            });
        }
    }

    Ok(plan)
}

/// Empty directories the bootstrap creates so the layout enumerator and
/// later inline operations have somewhere to land. `.gitkeep` markers keep
/// the dirs visible in git without committing fake content.
const EMPTY_DIRS: &[&str] = &[
    "specs/specdev/schemas/inlined/blocks",
    "specs/specdev/schemas/inlined/bridges",
    "specs/specdev/schemas/inlined/languages",
    "specs/specdev/reductions/blocks",
    "specs/specdev/reductions/bridges",
    "specs/specdev/reductions/bridges/local",
    "specs/modules",
    "src",
];

pub fn run_bootstrap(plugin_root: &Path, inputs: &BootstrapInputs) -> Result<()> {
    // Refuse if `<target>/specs/specdev/` already exists; that's the
    // signal someone bootstrapped here before. Refuses to overwrite even
    // if `<target>/` itself is non-empty for other reasons (e.g. a fresh
    // git repo with `.gitignore` only).
    let specdev_root = inputs.target_dir.join("specs").join("specdev");
    if specdev_root.exists() {
        return Err(anyhow!(
            "[BOOTSTRAP-001] {} already exists; refusing to overwrite. Remove it manually if you intend to re-bootstrap.",
            specdev_root.display(),
        ));
    }

    let profile_dir = plugin_root.join("profiles").join(&inputs.profile);
    if !profile_dir.is_dir() {
        return Err(anyhow!(
            "[BOOTSTRAP-001] profile {:?} not found at {}",
            inputs.profile,
            profile_dir.display(),
        ));
    }

    // Validate language is supported by the profile by checking that
    // `codegen/<language>/` exists with at least one `*.conventions.md`
    // file. Errors here name the language so users see the missing-
    // codegen issue before any bytes are written. (The convention file
    // name is no longer hardcoded to `module.conventions.md`; rust-workspace
    // ships `workspace.conventions.md` instead.)
    let codegen_lang_dir = profile_dir.join("codegen").join(&inputs.language);
    let has_conventions = codegen_lang_dir.is_dir()
        && std::fs::read_dir(&codegen_lang_dir)
            .map(|it| {
                it.filter_map(|r| r.ok()).any(|e| {
                    e.path()
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.ends_with(".conventions.md"))
                })
            })
            .unwrap_or(false);
    if !has_conventions {
        return Err(anyhow!(
            "[BOOTSTRAP-001] profile {:?} has no codegen for language {:?} \
             (expected at least one *.conventions.md under {})",
            inputs.profile,
            inputs.language,
            codegen_lang_dir.display(),
        ));
    }

    // Copy files per the plan. Substitute templates; verbatim-copy schema
    // and codegen content (analyzer never re-emits YAML/Markdown).
    let plan = profile_plan(&profile_dir, &inputs.language)?;
    for f in &plan {
        let src = profile_dir.join(&f.src_rel);
        if !src.is_file() {
            return Err(anyhow!(
                "[BOOTSTRAP-001] profile {:?} is missing required source file: {}",
                inputs.profile,
                src.display(),
            ));
        }
        let dst = inputs.target_dir.join(&f.dst_rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if f.substitute {
            let text = std::fs::read_to_string(&src)
                .with_context(|| format!("reading {}", src.display()))?;
            let out = substitute(&text, inputs, &src.display().to_string())?;
            std::fs::write(&dst, out).with_context(|| format!("writing {}", dst.display()))?;
        } else {
            // Byte-faithful copy.
            let bytes =
                std::fs::read(&src).with_context(|| format!("reading {}", src.display()))?;
            std::fs::write(&dst, bytes).with_context(|| format!("writing {}", dst.display()))?;
        }
    }

    // Inline the language tier. `${PLUGIN_ROOT}/languages/<lang>/`
    // holds per-language item-kind vocabulary (items.schema.yaml in v1, plus
    // items.scm tree-sitter queries). Every `*.schema.yaml` under the
    // language dir is copied verbatim into
    // `<target>/specs/specdev/schemas/inlined/languages/<lang>/`, where the
    // aggregator MapRetriever picks them up under the `inlined/languages/<lang>/`
    // prefix. The languages tier is silently skipped when the language has
    // no entry under `${PLUGIN_ROOT}/languages/` — languages without a
    // language tier (and languages whose item vocabulary hasn't been authored) bootstrap
    // without an `items[]` substrate, which is fine for v1.
    let lang_src_dir = plugin_root.join("languages").join(&inputs.language);
    if lang_src_dir.is_dir() {
        let mut lang_entries: Vec<PathBuf> = std::fs::read_dir(&lang_src_dir)
            .with_context(|| format!("reading {}", lang_src_dir.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.ends_with(".schema.yaml"))
            })
            .collect();
        lang_entries.sort();
        let lang_dst_dir = inputs
            .target_dir
            .join("specs/specdev/schemas/inlined/languages")
            .join(&inputs.language);
        if !lang_entries.is_empty() {
            std::fs::create_dir_all(&lang_dst_dir)
                .with_context(|| format!("creating {}", lang_dst_dir.display()))?;
        }
        for entry in lang_entries {
            let Some(file_name) = entry.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let dst = lang_dst_dir.join(file_name);
            let bytes = std::fs::read(&entry)
                .with_context(|| format!("reading {}", entry.display()))?;
            std::fs::write(&dst, bytes)
                .with_context(|| format!("writing {}", dst.display()))?;
        }
    }

    // Create the empty directories the project layout expects.
    for d in EMPTY_DIRS {
        let abs = inputs.target_dir.join(d);
        std::fs::create_dir_all(&abs)
            .with_context(|| format!("creating {}", abs.display()))?;
    }

    // Walk the manifest's `bootstrap.steps` and
    // dispatch each step to its handler. v1 handlers cover the closed
    // enum (`emit_workspace_root` / `emit_member_stubs` / `walk_members`);
    // only the rust-workspace shape currently authors a non-empty step
    // set. EmitWorkspaceRoot adds a defensive validation against the
    // bootstrapped spec.toml; the other two are no-op v1 placeholders
    // so the dispatch shell exists without committing to per-member
    // scaffolding semantics until dogfood evidence informs them.
    if let Some(steps) = manifest_bootstrap_steps(&specdev_root)? {
        let target_spec_toml = inputs.target_dir.join("spec.toml");
        for step in steps {
            dispatch_bootstrap_step(step, &target_spec_toml)?;
        }
    }

    // Write the lineage [meta] stamp. Profiles get no entry of their own;
    // only `[meta]` records "this project was bootstrapped
    // from plugin X at version Y with profile Z."
    let mut lineage = Lineage::default();
    lineage.meta = Some(MetaSection {
        plugin_version: plugin_version(plugin_root)?,
        bootstrapped_at: now_iso8601(),
        profile: inputs.profile.clone(),
    });
    lineage.save(&specdev_root)?;
    Ok(())
}

/// Load the just-copied `profile.yaml` from the
/// bootstrapped tree and return its `bootstrap.steps`. `Ok(None)` when the
/// manifest is absent (profiles without a manifest) or when
/// `bootstrap.steps` is empty (the closed-set non-workspace profiles —
/// library-api, firmware-layered, audio-plugin all author `steps: []`).
/// `Err` only on a malformed manifest; surfaces as `MANIFEST-001`.
fn manifest_bootstrap_steps(specdev_root: &Path) -> Result<Option<Vec<BootstrapStep>>> {
    let manifest_path = specdev_root.join("profile.yaml");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let profile = Profile::load(&manifest_path)?;
    let steps = profile
        .bootstrap
        .map(|b| b.steps)
        .unwrap_or_default();
    if steps.is_empty() {
        Ok(None)
    } else {
        Ok(Some(steps))
    }
}

/// Dispatch one `BootstrapStep` against the
/// bootstrapped target. The closed enum has three variants in v1:
///
/// * `EmitWorkspaceRoot` — defensive validation: the bootstrapped
///   `spec.toml` MUST contain a `[workspace]` table. Errors `BOOTSTRAP-002`
///   if not (catches a future profile authoring mistake where
///   `bootstrap.steps` references this step but the template doesn't ship
///   the table). Today the rust-workspace `spec.toml.template` writes the
///   table directly, so this step is a no-op pass for that profile.
/// * `EmitMemberStubs` — no-op v1. The dispatch shell exists; per-member
///   stub directory creation is future work gated on real
///   workspace-bootstrap dogfood.
/// * `WalkMembers` — no-op v1. Recursive per-member bootstrap deferred.
///
/// All three steps log via `tracing::info!` so the dispatch path is
/// observable in `--verbose` runs.
fn dispatch_bootstrap_step(step: BootstrapStep, target_spec_toml: &Path) -> Result<()> {
    match step {
        BootstrapStep::EmitWorkspaceRoot => {
            let text = std::fs::read_to_string(target_spec_toml)
                .with_context(|| format!("reading {}", target_spec_toml.display()))?;
            let parsed: toml::Value = toml::from_str(&text)
                .with_context(|| format!("parsing {}", target_spec_toml.display()))?;
            if parsed.get("workspace").is_none() {
                return Err(anyhow!(
                    "[BOOTSTRAP-002] profile manifest's `bootstrap.steps` \
                     includes `emit_workspace_root` but the bootstrapped \
                     `spec.toml` at {} has no `[workspace]` table; the \
                     profile's `spec.toml.template` must include the \
                     `[workspace]` declaration.",
                    target_spec_toml.display(),
                ));
            }
            info!(
                target: "specdev_analyzer::bootstrap",
                "bootstrap step `emit_workspace_root`: validated [workspace] table present"
            );
        }
        BootstrapStep::EmitMemberStubs => {
            info!(
                target: "specdev_analyzer::bootstrap",
                "bootstrap step `emit_member_stubs`: v1 no-op (per-member stub creation deferred)"
            );
        }
        BootstrapStep::WalkMembers => {
            info!(
                target: "specdev_analyzer::bootstrap",
                "bootstrap step `walk_members`: v1 no-op (recursive per-member bootstrap deferred)"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_known_tokens() {
        let inputs = BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: PathBuf::from("/tmp/x"),
        };
        let out = substitute(
            "name = {{project_name}}, lang = {{language}}, profile = {{profile}}",
            &inputs,
            "test",
        )
        .unwrap();
        assert_eq!(out, "name = demo, lang = rust, profile = firmware-layered");
    }

    #[test]
    fn substitute_rejects_unknown_token() {
        let inputs = BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: PathBuf::from("/tmp/x"),
        };
        let err = substitute("hello {{not_a_token}} world", &inputs, "test").unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("BOOTSTRAP-002"), "{s}");
        assert!(s.contains("not_a_token"), "{s}");
    }

    #[test]
    fn substitute_rejects_unterminated() {
        let inputs = BootstrapInputs {
            profile: "x".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: PathBuf::from("/tmp/x"),
        };
        let err = substitute("hello {{open without close", &inputs, "test").unwrap_err();
        assert!(format!("{err:#}").contains("BOOTSTRAP-002"));
    }

    #[test]
    fn substitute_passes_through_text_without_tokens() {
        let inputs = BootstrapInputs {
            profile: "x".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: PathBuf::from("/tmp/x"),
        };
        let s = "no placeholders here {single brace} or [other] punctuation";
        assert_eq!(substitute(s, &inputs, "t").unwrap(), s);
    }
}
