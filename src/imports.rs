//! `report imports` — packages a module's source imports but does not
//! declare.
//!
//! A manifest emitter unions every module's `external_deps` into one
//! package manifest, so a module that imports a package only a sibling
//! declares still resolves, builds and passes every gate. It breaks the
//! day the sibling drops the dependency, in a module nobody touched. This
//! report compares, per module, the packages its source file imports with
//! the packages the module may rely on:
//!
//! * its own `external_deps` (runtime entries — `test_only` ones are for
//!   test files, which this report does not read);
//! * the architecture-level `external_deps` (runtime entries);
//! * the runtime deps of the inlined blocks and bridges the module uses,
//!   decided by [`crate::prompt::detect_relevance`] — the same rule that
//!   decides which unit READMEs reach the module's codegen prompt.
//!
//! The import side comes from the language adapter: an `imports.scm`
//! query with one `@import.source` capture, and
//! [`LanguageAdapter::import_package`] to map a specifier to a package. A
//! language without the query is reported as not checked, never as clean.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::items::source_map;
use crate::lang::{self, LanguageAdapter};
use crate::layout::LayoutContext;
use crate::manifest::block_deps::BlockDeps;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct UndeclaredImport {
    pub module: String,
    /// Project-root-relative source file.
    pub file: String,
    pub package: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportsReport {
    /// `false` when the project's language ships no import query; nothing
    /// was read and `undeclared` proves nothing.
    pub checked: bool,
    pub language: String,
    /// Modules whose source file was found and read.
    pub modules_checked: usize,
    /// Modules with no source file yet (pre-implementation).
    pub modules_without_source: usize,
    pub undeclared: Vec<UndeclaredImport>,
}

impl ImportsReport {
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("serializing imports report")
    }
}

/// Run the report over every module, or over `only` when given.
pub fn run(layout: &LayoutContext, only: Option<&str>) -> Result<ImportsReport> {
    let language = layout.spec.project.language.clone();
    let mut report = ImportsReport {
        language: language.clone(),
        ..ImportsReport::default()
    };
    let Some(adapter) = lang::adapter_for(&language) else {
        return Ok(report);
    };
    let Some(query_text) = adapter.imports_query() else {
        return Ok(report);
    };
    report.checked = true;

    let modules = match only {
        Some(m) => {
            if !layout.modules_dir.join(format!("{m}.toml")).is_file() {
                return Err(anyhow!(
                    "module `{m}` not found at {}",
                    layout.modules_dir.join(format!("{m}.toml")).display()
                ));
            }
            vec![m.to_string()]
        }
        None => crate::prompt::list_module_names(layout)?,
    };

    let arch = runtime_packages(&read_toml(&layout.specs_dir.join("architecture.toml"))?);
    let mut unit_cache: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let self_package = layout.spec.project.name.clone();
    let source_root = layout.spec.paths.source.clone();

    for module in &modules {
        let module_path = layout.modules_dir.join(format!("{module}.toml"));
        let module_toml = read_toml(&module_path)?;
        let override_path = source_map::source_file_override(&module_toml).map(str::to_string);
        let Some(src) = source_map::resolve_source_path(
            module,
            override_path.as_deref(),
            adapter.as_ref(),
            &layout.project_root,
            &source_root,
        ) else {
            report.modules_without_source += 1;
            continue;
        };
        report.modules_checked += 1;

        let mut declared = runtime_packages(&module_toml);
        declared.extend(arch.iter().cloned());
        let relevance = crate::prompt::detect_relevance(layout, module)?;
        for (kind, units) in [
            ("blocks", &relevance.blocks),
            ("bridges", &relevance.bridges),
        ] {
            for unit in units {
                let key = (kind.to_string(), unit.clone());
                if !unit_cache.contains_key(&key) {
                    let pkgs = unit_packages(layout, kind, unit)?;
                    unit_cache.insert(key.clone(), pkgs);
                }
                declared.extend(unit_cache[&key].iter().cloned());
            }
        }

        let source =
            std::fs::read_to_string(&src).with_context(|| format!("reading {}", src.display()))?;
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_else(|| adapter.file_extension());
        let imported = imported_packages(&source, ext, adapter.as_ref(), query_text, &self_package)
            .with_context(|| format!("reading imports of {}", src.display()))?;
        let file = src
            .strip_prefix(&layout.project_root)
            .unwrap_or(&src)
            .to_string_lossy()
            .replace('\\', "/");
        for package in imported.difference(&declared) {
            report.undeclared.push(UndeclaredImport {
                module: module.clone(),
                file: file.clone(),
                package: package.clone(),
            });
        }
    }
    report.undeclared.sort();
    Ok(report)
}

/// The external packages one source text imports.
pub fn imported_packages(
    source: &str,
    ext: &str,
    adapter: &dyn LanguageAdapter,
    query_text: &str,
    self_package: &str,
) -> Result<BTreeSet<String>> {
    let ts_language = adapter.tree_sitter_language_for(ext);
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| anyhow!("set_language: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter parser returned None"))?;
    let query = Query::new(&ts_language, query_text)
        .map_err(|e| anyhow!("imports.scm query compile: {e}"))?;
    let Some(idx) = query.capture_index_for_name("import.source") else {
        return Err(anyhow!("imports.scm has no `@import.source` capture"));
    };
    let mut out = BTreeSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures.iter().filter(|c| c.index == idx) {
            let text = c
                .node
                .utf8_text(source.as_bytes())
                .map_err(|e| anyhow!("non-UTF-8 import specifier: {e}"))?;
            if let Some(p) = adapter.import_package(text, self_package) {
                out.insert(p);
            }
        }
    }
    Ok(out)
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    if !path.is_file() {
        return Ok(toml::Value::Table(Default::default()));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// `package` of every `[[external_deps]]` entry that is not `test_only`.
fn runtime_packages(doc: &toml::Value) -> BTreeSet<String> {
    doc.get("external_deps")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|d| {
            !d.get("test_only")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|d| d.get("package").and_then(toml::Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Package names an inlined unit's `deps.yaml` contributes (npm and pub;
/// a unit fills only its own language's list). A unit with no
/// `deps.yaml` contributes nothing.
fn unit_packages(layout: &LayoutContext, kind: &str, unit: &str) -> Result<BTreeSet<String>> {
    let path = layout.codegen_dir.join(kind).join(unit).join("deps.yaml");
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    let deps =
        BlockDeps::load(&path).with_context(|| format!("loading {kind} deps.yaml for `{unit}`"))?;
    Ok(deps
        .npm
        .iter()
        .map(|e| e.package.clone())
        .chain(deps.pub_packages.iter().map(|e| e.package.clone()))
        .collect())
}
