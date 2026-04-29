//! `specdev bootstrap --profile <name> [<target>]`.
//!
//! One-shot scaffolding from a plugin profile into a fresh project tree.
//! Profiles are not inlineable and have no lineage entry of their own
//! (doc 15 §3.3) — the only lineage written here is a `[meta]` stamp
//! recording the plugin version and the profile name, which inline /
//! drift consult to interpret upstream changes.
//!
//! Layout assumed in the plugin tree (Phase 3 fixture; Phase 5 lands
//! `firmware-layered` and `library-api` for real):
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
//! Output layout matches doc 15 §4. Templates support a closed set of
//! placeholder tokens; any unrecognized `{{...}}` is a fatal error.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::lineage::{Lineage, MetaSection};

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
            out.push(bytes[i] as char);
            i += 1;
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

/// Build the per-language file plan for a profile. Returns the list of
/// (src_rel, dst_rel, substitute) tuples in the order they should be
/// copied; the order is deterministic because we hard-code it.
fn profile_plan(profile: &str, language: &str) -> Vec<ProfileFile> {
    let _ = profile; // structural; the caller already resolved <plugin>/profiles/<profile>/
    vec![
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
            src_rel: PathBuf::from("schemas/module.schema.yaml"),
            dst_rel: PathBuf::from("specs/specdev/schemas/module.schema.yaml"),
            substitute: false,
        },
        ProfileFile {
            src_rel: PathBuf::from("schemas/architecture.schema.yaml"),
            dst_rel: PathBuf::from("specs/specdev/schemas/architecture.schema.yaml"),
            substitute: false,
        },
        ProfileFile {
            src_rel: PathBuf::from("schemas/requirements.schema.yaml"),
            dst_rel: PathBuf::from("specs/specdev/schemas/requirements.schema.yaml"),
            substitute: false,
        },
        ProfileFile {
            src_rel: PathBuf::from("reductions/project.yaml"),
            dst_rel: PathBuf::from("specs/specdev/reductions/project.yaml"),
            substitute: false,
        },
        ProfileFile {
            src_rel: PathBuf::from(format!("codegen/{language}/module.conventions.md")),
            dst_rel: PathBuf::from(format!(
                "specs/specdev/codegen/{language}/module.conventions.md"
            )),
            substitute: false,
        },
    ]
}

/// Empty directories the bootstrap creates so the layout enumerator and
/// later inline operations have somewhere to land. `.gitkeep` markers keep
/// the dirs visible in git without committing fake content.
const EMPTY_DIRS: &[&str] = &[
    "specs/specdev/schemas/inlined/blocks",
    "specs/specdev/schemas/inlined/bridges",
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
    // codegen/<language>/module.conventions.md exists. Errors here name
    // the language and the path so users see the missing-codegen issue
    // before any bytes are written.
    let codegen_check = profile_dir
        .join("codegen")
        .join(&inputs.language)
        .join("module.conventions.md");
    if !codegen_check.is_file() {
        return Err(anyhow!(
            "[BOOTSTRAP-001] profile {:?} has no codegen for language {:?} (expected {})",
            inputs.profile,
            inputs.language,
            codegen_check.display(),
        ));
    }

    // Copy files per the plan. Substitute templates; verbatim-copy schema
    // and codegen content (D33 — analyzer never re-emits YAML/Markdown).
    let plan = profile_plan(&inputs.profile, &inputs.language);
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

    // Create the empty directories the project layout expects.
    for d in EMPTY_DIRS {
        let abs = inputs.target_dir.join(d);
        std::fs::create_dir_all(&abs)
            .with_context(|| format!("creating {}", abs.display()))?;
    }

    // Write the lineage [meta] stamp. Profiles get no entry of their own
    // (doc 15 §3.3); only `[meta]` records "this project was bootstrapped
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
