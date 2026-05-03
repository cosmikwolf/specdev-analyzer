//! Project layout discovery.
//!
//! Phase 1 of the blocks + bridges + profiles arc. [`LayoutContext`] is the
//! single source of truth for "where things live in the project" — it
//! consumes a parsed [`SpecConfig`] and derives every directory and file path
//! the analyzer needs to load. There is **no resolution chain** at analyze
//! time (doc 15 §11 invariant 5); every path is rooted at `spec.toml`'s
//! directory, never at a plugin or `.specdev/` cascade.
//!
//! [`LayoutContext::enumerate`] returns a [`LayoutInventory`] of expected
//! files with present/absent flags, suitable for emitting `LAYOUT-002`
//! present / `LAYOUT-003` missing findings during `validate`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::instrument;

use crate::config::{discover_spec_toml, SpecConfig};

#[derive(Debug, Clone)]
pub struct LayoutContext {
    /// Project root — directory containing `spec.toml`.
    pub project_root: PathBuf,
    /// Absolute path to the project's `spec.toml`.
    pub spec_toml_path: PathBuf,
    /// Parsed manifest.
    pub spec: SpecConfig,
    /// `<project_root>/<paths.specs>` — the user-visible `specs/` directory.
    pub specs_dir: PathBuf,
    /// `<project_root>/<paths.source>` — source-code directory for audit/reverse.
    pub source_dir: PathBuf,
    /// `<specs_dir>/specdev/` — machinery directory.
    pub specdev_root: PathBuf,
    /// `<specdev_root>/schemas/`
    pub schemas_dir: PathBuf,
    /// `<schemas_dir>/module.schema.yaml` — aggregator schema.
    pub module_schema_path: PathBuf,
    /// `<schemas_dir>/inlined/blocks/`
    pub inlined_blocks_dir: PathBuf,
    /// `<schemas_dir>/inlined/bridges/`
    pub inlined_bridges_dir: PathBuf,
    /// `<schemas_dir>/inlined/languages/` — Phase 14 fourth top-level
    /// plugin tier. Holds per-language item-kind vocabulary
    /// (`languages/<lang>/items.schema.yaml`) inlined at bootstrap.
    pub inlined_languages_dir: PathBuf,
    /// `<specdev_root>/reductions/`
    pub reductions_dir: PathBuf,
    /// `<reductions_dir>/project.yaml`
    pub project_reduction_path: PathBuf,
    /// `<reductions_dir>/blocks/`
    pub reductions_blocks_dir: PathBuf,
    /// `<reductions_dir>/bridges/`
    pub reductions_bridges_dir: PathBuf,
    /// `<reductions_dir>/bridges/local/`
    pub reductions_bridges_local_dir: PathBuf,
    /// `<specdev_root>/codegen/<project.language>/`
    pub codegen_dir: PathBuf,
    /// `<specdev_root>/domain.toml`
    pub domain_toml_path: PathBuf,
    /// `<specs_dir>/modules/` — user-authored module TOMLs (per profile convention).
    pub modules_dir: PathBuf,
    /// `<specs_dir>/architecture.toml` — profile-owned architecture-level
    /// authoring (firmware-layered: connections; library-api: features).
    /// Phase 5+ adds this alongside the existing module TOMLs.
    pub architecture_toml_path: PathBuf,
    /// `<specs_dir>/requirements.toml` — sources + requirements (Phase 5+).
    pub requirements_toml_path: PathBuf,
    /// `<schemas_dir>/architecture.schema.yaml` — schema validating
    /// architecture.toml.
    pub architecture_schema_path: PathBuf,
    /// `<schemas_dir>/requirements.schema.yaml` — schema validating
    /// requirements.toml.
    pub requirements_schema_path: PathBuf,
}

impl LayoutContext {
    /// Discover `spec.toml` and build a layout context. `start` is the
    /// directory the user invoked the analyzer in (or passed via `--project`);
    /// `explicit_spec_path` short-circuits the CWD walk when set.
    #[instrument(level = "info", skip(start, explicit_spec_path), fields(start = %start.display()))]
    pub fn discover(start: &Path, explicit_spec_path: Option<&Path>) -> Result<Self> {
        let spec_toml_path = discover_spec_toml(start, explicit_spec_path)?;
        let project_root = spec_toml_path
            .parent()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "spec.toml at {} has no parent directory",
                    spec_toml_path.display()
                )
            })?
            .to_path_buf();
        let spec = SpecConfig::load(&spec_toml_path)
            .with_context(|| format!("loading {}", spec_toml_path.display()))?;
        Ok(Self::from_spec(project_root, spec_toml_path, spec))
    }

    /// Build a context from a parsed manifest. Pure path arithmetic; useful
    /// for tests that want to skip discovery.
    pub fn from_spec(project_root: PathBuf, spec_toml_path: PathBuf, spec: SpecConfig) -> Self {
        let specs_dir = project_root.join(&spec.paths.specs);
        let source_dir = project_root.join(&spec.paths.source);
        let specdev_root = specs_dir.join("specdev");
        let schemas_dir = specdev_root.join("schemas");
        let module_schema_path = schemas_dir.join("module.schema.yaml");
        let inlined_blocks_dir = schemas_dir.join("inlined").join("blocks");
        let inlined_bridges_dir = schemas_dir.join("inlined").join("bridges");
        let inlined_languages_dir = schemas_dir.join("inlined").join("languages");
        let reductions_dir = specdev_root.join("reductions");
        let project_reduction_path = reductions_dir.join("project.yaml");
        let reductions_blocks_dir = reductions_dir.join("blocks");
        let reductions_bridges_dir = reductions_dir.join("bridges");
        let reductions_bridges_local_dir = reductions_bridges_dir.join("local");
        let codegen_dir = specdev_root.join("codegen").join(&spec.project.language);
        let domain_toml_path = specdev_root.join("domain.toml");
        let modules_dir = specs_dir.join("modules");
        let architecture_toml_path = specs_dir.join("architecture.toml");
        let requirements_toml_path = specs_dir.join("requirements.toml");
        let architecture_schema_path = schemas_dir.join("architecture.schema.yaml");
        let requirements_schema_path = schemas_dir.join("requirements.schema.yaml");

        Self {
            project_root,
            spec_toml_path,
            spec,
            specs_dir,
            source_dir,
            specdev_root,
            schemas_dir,
            module_schema_path,
            inlined_blocks_dir,
            inlined_bridges_dir,
            inlined_languages_dir,
            reductions_dir,
            project_reduction_path,
            reductions_blocks_dir,
            reductions_bridges_dir,
            reductions_bridges_local_dir,
            codegen_dir,
            domain_toml_path,
            modules_dir,
            architecture_toml_path,
            requirements_toml_path,
            architecture_schema_path,
            requirements_schema_path,
        }
    }

    /// Enumerate every expected layout entry with present/absent flags.
    #[instrument(level = "info", skip(self), fields(specdev_root = %self.specdev_root.display()))]
    pub fn enumerate(&self) -> LayoutInventory {
        let dir_entries = vec![
            ("specs/", &self.specs_dir),
            ("specs/specdev/", &self.specdev_root),
            ("specs/specdev/schemas/", &self.schemas_dir),
            (
                "specs/specdev/schemas/inlined/blocks/",
                &self.inlined_blocks_dir,
            ),
            (
                "specs/specdev/schemas/inlined/bridges/",
                &self.inlined_bridges_dir,
            ),
            // `inlined/languages/` is intentionally NOT in the standard
            // inventory: it is bootstrap-created for projects whose language
            // ships an items[] vocabulary, but pre-Phase-14 projects (and
            // languages without item vocabulary) bootstrap without it. The
            // aggregator MapRetriever guards on `is_dir()` itself, so a
            // missing dir is a no-op rather than a layout warning.
            ("specs/specdev/reductions/", &self.reductions_dir),
            (
                "specs/specdev/reductions/blocks/",
                &self.reductions_blocks_dir,
            ),
            (
                "specs/specdev/reductions/bridges/",
                &self.reductions_bridges_dir,
            ),
            (
                "specs/specdev/reductions/bridges/local/",
                &self.reductions_bridges_local_dir,
            ),
            ("specs/specdev/codegen/<lang>/", &self.codegen_dir),
            ("specs/modules/", &self.modules_dir),
        ];

        // `architecture.toml` + `requirements.toml` are intentionally
        // omitted from this list: they are project-authored and optional,
        // so missing-on-disk should emit `LAYOUT-005`/`LAYOUT-006` info
        // (handled by `validate::validate_aux_toml`), not the
        // `LAYOUT-003` warning this list drives. Their schemas, however,
        // are profile-shipped — missing is a real warning.
        let file_entries = vec![
            (
                "specs/specdev/schemas/module.schema.yaml",
                &self.module_schema_path,
            ),
            (
                "specs/specdev/schemas/architecture.schema.yaml",
                &self.architecture_schema_path,
            ),
            (
                "specs/specdev/schemas/requirements.schema.yaml",
                &self.requirements_schema_path,
            ),
            (
                "specs/specdev/reductions/project.yaml",
                &self.project_reduction_path,
            ),
            ("specs/specdev/domain.toml", &self.domain_toml_path),
        ];

        let directories = dir_entries
            .into_iter()
            .map(|(label, path)| LayoutEntry {
                label: label.to_string(),
                path: path.to_path_buf(),
                exists: path.is_dir(),
                kind: LayoutKind::Directory,
            })
            .collect();
        let files = file_entries
            .into_iter()
            .map(|(label, path)| LayoutEntry {
                label: label.to_string(),
                path: path.to_path_buf(),
                exists: path.is_file(),
                kind: LayoutKind::File,
            })
            .collect();

        let modules = list_files_with_extension(&self.modules_dir, "toml");
        let inlined_block_schemas = list_files_with_extension(&self.inlined_blocks_dir, "yaml")
            .into_iter()
            .chain(list_files_with_extension(&self.inlined_blocks_dir, "yml"))
            .collect();
        let inlined_bridge_schemas = list_files_with_extension(&self.inlined_bridges_dir, "yaml")
            .into_iter()
            .chain(list_files_with_extension(&self.inlined_bridges_dir, "yml"))
            .collect();
        let block_reductions = list_files_with_extension(&self.reductions_blocks_dir, "yaml");
        let bridge_reductions = list_files_with_extension(&self.reductions_bridges_dir, "yaml")
            .into_iter()
            .filter(|p| p.parent() == Some(self.reductions_bridges_dir.as_path()))
            .collect::<Vec<_>>();
        let bridge_local_reductions =
            list_files_with_extension(&self.reductions_bridges_local_dir, "yaml");

        LayoutInventory {
            directories,
            files,
            modules,
            inlined_block_schemas,
            inlined_bridge_schemas,
            block_reductions,
            bridge_reductions,
            bridge_local_reductions,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LayoutInventory {
    pub directories: Vec<LayoutEntry>,
    pub files: Vec<LayoutEntry>,
    pub modules: Vec<PathBuf>,
    pub inlined_block_schemas: Vec<PathBuf>,
    pub inlined_bridge_schemas: Vec<PathBuf>,
    pub block_reductions: Vec<PathBuf>,
    pub bridge_reductions: Vec<PathBuf>,
    pub bridge_local_reductions: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct LayoutEntry {
    pub label: String,
    pub path: PathBuf,
    pub exists: bool,
    pub kind: LayoutKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Directory,
    File,
}

fn list_files_with_extension(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_minimal_skeleton(root: &Path) {
        std::fs::create_dir_all(root.join("specs/specdev/schemas/inlined/blocks")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/schemas/inlined/bridges")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/schemas/inlined/languages")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/reductions/blocks")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/reductions/bridges/local")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/codegen/rust")).unwrap();
        std::fs::create_dir_all(root.join("specs/modules")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("spec.toml"),
            r#"
spec_version = "1.0"

[project]
name = "skel"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/schemas/module.schema.yaml"),
            "{}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/schemas/architecture.schema.yaml"),
            "{}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/schemas/requirements.schema.yaml"),
            "{}\n",
        )
        .unwrap();
        std::fs::write(root.join("specs/specdev/reductions/project.yaml"), "{}\n").unwrap();
        std::fs::write(root.join("specs/specdev/domain.toml"), "").unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "specdev-layout-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn discover_finds_spec_toml_in_cwd() {
        let root = tmp("discover");
        write_minimal_skeleton(&root);
        let ctx = LayoutContext::discover(&root, None).expect("discover");
        assert_eq!(ctx.spec.project.name, "skel");
        // discover() canonicalizes spec.toml's path; macOS rewrites
        // /var/... → /private/var/..., so compare against canonical roots.
        let expected = std::fs::canonicalize(root.join("specs")).unwrap();
        assert_eq!(ctx.specs_dir, expected);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enumerate_marks_present_dirs_and_files() {
        let root = tmp("enumerate");
        write_minimal_skeleton(&root);
        let ctx = LayoutContext::discover(&root, None).expect("discover");
        let inv = ctx.enumerate();
        for entry in &inv.directories {
            assert!(
                entry.exists,
                "expected {} to exist at {}",
                entry.label,
                entry.path.display()
            );
        }
        for entry in &inv.files {
            assert!(
                entry.exists,
                "expected {} to exist at {}",
                entry.label,
                entry.path.display()
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enumerate_lists_module_files() {
        let root = tmp("modules");
        write_minimal_skeleton(&root);
        std::fs::write(root.join("specs/modules/Foo.toml"), "name = \"Foo\"\n").unwrap();
        std::fs::write(root.join("specs/modules/Bar.toml"), "name = \"Bar\"\n").unwrap();
        let ctx = LayoutContext::discover(&root, None).expect("discover");
        let inv = ctx.enumerate();
        assert_eq!(inv.modules.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
