//! Profile-declared paired files.
//!
//! A profile may ship config files that a project needs exactly once —
//! `tsconfig.json`, `next.config.ts`, `.gitignore`, an `app/.gitkeep`
//! that makes a directory exist before the first gate. They are declared
//! as a top-level `paired_files:` list in `profile.yaml`, read from the
//! inlined `<specdev_root>/templates/` copy at manifest time, and written
//! by the shared manifest runner **only when absent**. After that first
//! write they are externally owned: never rewritten, never gen-recorded.
//! (`next build` rewrites `tsconfig.json` on its first run, so this is
//! the only ownership category that file can live in.)
//!
//! Language-neutral by construction: the runner merges these rows with
//! the rust arm's `[[bin]]` paired stub (`cargo_toml.rs`) and applies one
//! write loop to both. The only token substituted in a body is
//! `{project_name}` — the same closed grammar as the paired stub, so a
//! JSON or TypeScript brace in a template is never a token.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::layout::LayoutContext;
use crate::manifest::Profile;

/// The declared `paired_files` targets as project-root-relative paths, in
/// declared order, with no template read at all.
///
/// The gate key hashes these targets (see
/// [`crate::gen_lineage::profile_hash`]): what the gate ran against is the
/// config file on disk, not the template it was first copied from, and a
/// template file that has since gone missing must not fail the hash.
/// Same target validation as [`paired_files_from_profile`], because a
/// target that escapes the project root must never be read.
pub fn paired_targets(layout: &LayoutContext) -> Result<Vec<PathBuf>> {
    let Some(profile) = Profile::load_inlined(&layout.specdev_root)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(profile.paired_files.len());
    for row in &profile.paired_files {
        out.push(check_target(&row.target)?);
    }
    Ok(out)
}

/// A `paired_files` target must be a relative path inside the project
/// root. Returns it as a `PathBuf`.
fn check_target(target: &str) -> Result<PathBuf> {
    let path = Path::new(target);
    let escapes = path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes || target.trim().is_empty() {
        bail!(
            "profile.yaml paired_files target `{target}` must be a relative path inside \
             the project root (no leading `/`, no `..`)"
        );
    }
    Ok(path.to_path_buf())
}

/// Read the inlined profile's `paired_files` rows into `(target, body)`
/// pairs. `Ok(empty)` when there is no inlined profile or the profile
/// declares none. Errors: a target that is absolute or escapes the
/// project root (`..`), or a template file that cannot be read.
pub fn paired_files_from_profile(layout: &LayoutContext) -> Result<Vec<(PathBuf, String)>> {
    let Some(profile) = Profile::load_inlined(&layout.specdev_root)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(profile.paired_files.len());
    for row in &profile.paired_files {
        let target = check_target(&row.target)?;
        let template_path = layout.specdev_root.join(&row.template_file);
        let body = std::fs::read_to_string(&template_path).with_context(|| {
            format!(
                "reading paired_files template at {} (referenced from profile.yaml)",
                template_path.display()
            )
        })?;
        out.push((
            target,
            body.replace("{project_name}", &layout.spec.project.name),
        ));
    }
    Ok(out)
}

/// Two writers, one path: refuse before anything is written when a
/// paired target is also the manifest file, a gen-owned sibling
/// (`pnpm-workspace.yaml`), or another paired row (the rust `[[bin]]`
/// stub included). The error names the path and both writers.
pub fn refuse_collisions(
    paired: &[(PathBuf, String)],
    manifest_filename: &str,
    siblings: &[(String, String)],
) -> Result<()> {
    let mut writers: BTreeMap<PathBuf, &str> = BTreeMap::new();
    writers.insert(PathBuf::from(manifest_filename), "the manifest emitter");
    for (rel, _) in siblings {
        writers.insert(PathBuf::from(rel), "a gen-owned sibling file");
    }
    for (rel, _) in paired {
        if let Some(other) = writers.insert(rel.clone(), "a paired file") {
            bail!(
                "profile.yaml paired_files target `{}` is also written by {other}; \
                 two writers for one path is refused",
                rel.display()
            );
        }
    }
    Ok(())
}
