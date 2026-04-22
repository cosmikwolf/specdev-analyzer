//! Package discovery, qualification, and reference resolution.
//!
//! When a project declares `[[packages]]` in `spec.toml`, modules live
//! under `specs/modules/<package>/<Module>.toml` and references across
//! packages may be qualified as `<package>::<Module>`. This module owns:
//!
//! * walking the modules directory to pair each file with its derived
//!   package (or `None` for top-level flat modules),
//! * parsing and resolving qualified / unqualified references,
//! * rewriting every reference in a project's in-memory JSON to fully-
//!   qualified form so downstream graph primitives stay string-compare-
//!   based and unchanged.
//!
//! For single-package (flat) projects — no `[[packages]]` declared — all
//! behavior degrades to "no qualification, global namespace," which is
//! byte-stable with pre-Phase-13 output.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::PackageSection;
use crate::domain::Severity;
use crate::report::Finding;

/// A module file discovered under `specs/modules/`, paired with its derived
/// package (the first subdirectory component under `modules/`), or `None`
/// if the file is directly under `modules/`.
#[derive(Debug, Clone)]
pub struct ModuleFile {
    pub path: PathBuf,
    pub package: Option<String>,
}

/// Return the fully-qualified name for a module:
/// `<package>::<name>` when packaged, just `<name>` otherwise.
pub fn qualify(package: Option<&str>, name: &str) -> String {
    match package {
        Some(pkg) => format!("{pkg}::{name}"),
        None => name.to_string(),
    }
}

/// Parse a reference string into its `(package, module)` parts. `None`
/// for `package` means the reference is unqualified.
pub fn parse_ref(s: &str) -> (Option<&str>, &str) {
    match s.split_once("::") {
        Some((p, m)) => (Some(p), m),
        None => (None, s),
    }
}

/// Walk `modules_dir` and return every `.toml` file paired with its
/// package (derived from the first subdirectory component). Deeper
/// nesting (two or more levels under `modules_dir`) produces a
/// `MOD_DEPTH` finding and the file is skipped.
pub fn collect_module_files(
    modules_dir: &Path,
    project_root: &Path,
) -> (Vec<ModuleFile>, Vec<Finding>) {
    let mut files = Vec::new();
    let mut findings = Vec::new();
    if !modules_dir.is_dir() {
        return (files, findings);
    }
    walk(
        modules_dir,
        modules_dir,
        project_root,
        &mut files,
        &mut findings,
    );
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (files, findings)
}

fn walk(
    root: &Path,
    dir: &Path,
    project_root: &Path,
    files: &mut Vec<ModuleFile>,
    findings: &mut Vec<Finding>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            // Only one level of nesting is permitted. If `dir` is already
            // one level deep under `root`, descending further is a
            // MOD_DEPTH error.
            if dir == root {
                walk(root, &path, project_root, files, findings);
            } else {
                // Report every .toml under the too-deep subtree as
                // MOD_DEPTH and do not recurse.
                collect_too_deep(&path, project_root, findings);
            }
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let package = if dir == root {
            None
        } else {
            dir.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        };
        files.push(ModuleFile { path, package });
    }
}

fn collect_too_deep(dir: &Path, project_root: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            collect_too_deep(&path, project_root, findings);
        } else if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let rel = path
                .strip_prefix(project_root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            findings.push(
                Finding::new(
                    "MOD_DEPTH",
                    Severity::Error,
                    "module file nested deeper than one level under `specs/modules/`; \
                     flatten or declare the extra grouping as a package",
                )
                .with_file(rel),
            );
        }
    }
}

/// Check each discovered module file against the declared packages and
/// emit:
///   * PKG040 — module under `specs/modules/<X>/` where `X` is not a
///     declared package (treat as unpackaged downstream).
///   * PKG041 — module directly under `specs/modules/` in a project that
///     has `[[packages]]` declared.
pub fn package_placement_findings(
    files: &[ModuleFile],
    packages: &[PackageSection],
    project_root: &Path,
    severity_pkg040: Severity,
    severity_pkg041: Severity,
) -> Vec<Finding> {
    let declared: HashSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let has_packages = !packages.is_empty();
    let mut out = Vec::new();
    for f in files {
        let rel = f
            .path
            .strip_prefix(project_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| f.path.display().to_string());
        match &f.package {
            Some(pkg) if !declared.contains(pkg.as_str()) => {
                out.push(
                    Finding::new(
                        "PKG040",
                        severity_pkg040,
                        format!(
                            "module under `specs/modules/{pkg}/` but `{pkg}` is not declared in `[[packages]]`; treated as unpackaged"
                        ),
                    )
                    .with_file(rel),
                );
            }
            None if has_packages => {
                out.push(
                    Finding::new(
                        "PKG041",
                        severity_pkg041,
                        "module directly under `specs/modules/` in a multi-package project — likely a forgotten move into a package subdirectory",
                    )
                    .with_file(rel),
                );
            }
            _ => {}
        }
    }
    out
}

/// Check each declared package's `source_path` for existence. Emits PKG042
/// per missing path.
pub fn package_source_path_findings(
    packages: &[PackageSection],
    project_root: &Path,
    severity: Severity,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for p in packages {
        let candidate = project_root.join(&p.source_path);
        if !candidate.exists() {
            out.push(Finding::new(
                "PKG042",
                severity,
                format!(
                    "package `{}` declares source_path `{}` which does not exist on disk",
                    p.name, p.source_path
                ),
            ));
        }
    }
    out
}

/// Index of "bare module name" → "packages that contain this module." Used
/// to resolve unqualified references (same-package first, global fallback,
/// error on global ambiguity).
pub struct NameIndex {
    /// For each bare module name, the packages it appears in. `None` as
    /// a key entry means "top-level / unpackaged."
    by_name: HashMap<String, Vec<Option<String>>>,
    /// Set of declared package names (strings).
    declared: HashSet<String>,
    /// Whether the project declared any packages at all (drives PKG044
    /// ambiguity semantics for flat projects).
    has_packages: bool,
}

impl NameIndex {
    pub fn build(modules: &[(&Value, Option<&str>)], packages: &[PackageSection]) -> Self {
        let mut by_name: HashMap<String, Vec<Option<String>>> = HashMap::new();
        for (m, pkg) in modules {
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            by_name
                .entry(name)
                .or_default()
                .push(pkg.map(|s| s.to_string()));
        }
        Self {
            by_name,
            declared: packages.iter().map(|p| p.name.clone()).collect(),
            has_packages: !packages.is_empty(),
        }
    }

    /// Resolve a reference string under the given referrer package, returning
    /// the fully-qualified form (`<pkg>::<mod>` or just `<mod>`).
    pub fn resolve(
        &self,
        reference: &str,
        referrer_package: Option<&str>,
    ) -> Result<String, ResolveError> {
        let (pkg_part, mod_part) = parse_ref(reference);
        match pkg_part {
            Some(pkg) => {
                if !self.declared.contains(pkg) {
                    return Err(ResolveError::UnknownPackage(pkg.to_string()));
                }
                let entries = self.by_name.get(mod_part).cloned().unwrap_or_default();
                if !entries.iter().any(|p| p.as_deref() == Some(pkg)) {
                    return Err(ResolveError::UnknownModule {
                        package: Some(pkg.to_string()),
                        module: mod_part.to_string(),
                    });
                }
                Ok(qualify(Some(pkg), mod_part))
            }
            None => {
                let entries = self.by_name.get(mod_part).cloned().unwrap_or_default();
                if entries.is_empty() {
                    return Err(ResolveError::UnknownModule {
                        package: None,
                        module: mod_part.to_string(),
                    });
                }
                // Prefer same package as the referrer.
                if let Some(rp) = referrer_package {
                    if entries.iter().any(|p| p.as_deref() == Some(rp)) {
                        return Ok(qualify(Some(rp), mod_part));
                    }
                }
                // Global fallback. If there's a single candidate, use it.
                if entries.len() == 1 {
                    let pkg = entries[0].clone();
                    return Ok(qualify(pkg.as_deref(), mod_part));
                }
                // Ambiguous across packages. PKG044 territory. Only flag
                // as ambiguous if the project actually has packages — an
                // unpackaged project with duplicate names fails uniqueness
                // elsewhere.
                if self.has_packages {
                    let mut candidates: Vec<String> =
                        entries.iter().filter_map(|p| p.clone()).collect();
                    candidates.sort();
                    Err(ResolveError::Ambiguous {
                        module: mod_part.to_string(),
                        candidates,
                    })
                } else {
                    Ok(mod_part.to_string())
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ResolveError {
    UnknownPackage(String),
    UnknownModule {
        package: Option<String>,
        module: String,
    },
    Ambiguous {
        module: String,
        candidates: Vec<String>,
    },
}

impl ResolveError {
    pub fn into_finding(self, rule: &str, severity: Severity, file: Option<String>) -> Finding {
        let msg = match &self {
            ResolveError::UnknownPackage(pkg) => {
                format!("reference targets unknown package `{pkg}`")
            }
            ResolveError::UnknownModule { package, module } => match package {
                Some(p) => format!("module `{p}::{module}` does not exist"),
                None => format!("module `{module}` does not exist"),
            },
            ResolveError::Ambiguous { module, candidates } => format!(
                "unqualified reference `{module}` is ambiguous across packages [{}]; qualify as `<package>::{module}`",
                candidates.join(", ")
            ),
        };
        let mut f = Finding::new(rule, severity, msg);
        if let Some(file) = file {
            f = f.with_file(file);
        }
        f
    }
}

/// Normalize all `depends_on[]` references in `modules` to fully-qualified
/// form. Each module's referrer package is provided by `pkg_of(name)`.
/// Returns findings for unresolved or ambiguous references.
///
/// `severity_pkg043` drives UnknownPackage/UnknownModule; `severity_pkg044`
/// drives Ambiguous.
pub fn normalize_depends_on(
    modules: &mut [Value],
    packages: &[PackageSection],
    pkg_by_name: &BTreeMap<String, Option<String>>,
    severity_pkg043: Severity,
    severity_pkg044: Severity,
) -> Vec<Finding> {
    let view: Vec<(&Value, Option<&str>)> = modules
        .iter()
        .map(|m| {
            let name = m.get("name").and_then(Value::as_str).unwrap_or("");
            let pkg = pkg_by_name.get(name).and_then(|o| o.as_deref());
            (m, pkg)
        })
        .collect();
    let index = NameIndex::build(&view, packages);

    let mut findings = Vec::new();
    for m in modules.iter_mut() {
        let name = m
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let referrer_package = pkg_by_name.get(&name).and_then(|o| o.clone());
        let Some(deps) = m.get_mut("depends_on").and_then(Value::as_array_mut) else {
            continue;
        };
        for dep in deps.iter_mut() {
            let Some(s) = dep.as_str() else { continue };
            match index.resolve(s, referrer_package.as_deref()) {
                Ok(qualified) => {
                    *dep = Value::String(qualified);
                }
                Err(e @ ResolveError::Ambiguous { .. }) => {
                    findings.push(e.into_finding(
                        "PKG044",
                        severity_pkg044,
                        Some(format!("module `{name}`")),
                    ));
                }
                Err(e) => {
                    findings.push(e.into_finding(
                        "PKG043",
                        severity_pkg043,
                        Some(format!("module `{name}`")),
                    ));
                }
            }
        }
    }
    findings
}

/// Normalize `architecture.connections[].from` / `.to` endpoints of the form
/// `Module.port` or `<package>::Module.port`, rewriting the module component
/// to fully-qualified form.
pub fn normalize_architecture(
    architecture: &mut Value,
    modules: &[Value],
    packages: &[PackageSection],
    pkg_by_name: &BTreeMap<String, Option<String>>,
    severity_pkg043: Severity,
    severity_pkg044: Severity,
) -> Vec<Finding> {
    let view: Vec<(&Value, Option<&str>)> = modules
        .iter()
        .map(|m| {
            let name = m.get("name").and_then(Value::as_str).unwrap_or("");
            let pkg = pkg_by_name.get(name).and_then(|o| o.as_deref());
            (m, pkg)
        })
        .collect();
    let index = NameIndex::build(&view, packages);

    let mut findings = Vec::new();
    let Some(conns) = architecture
        .get_mut("connections")
        .and_then(Value::as_array_mut)
    else {
        return findings;
    };

    for conn in conns.iter_mut() {
        for key in ["from", "to"] {
            let Some(endpoint) = conn.get(key).and_then(Value::as_str) else {
                continue;
            };
            let endpoint = endpoint.to_string();
            // Split "Module.port" or "<pkg>::Module.port"
            let (module_ref, rest) = match endpoint.split_once('.') {
                Some((m, r)) => (m.to_string(), format!(".{r}")),
                None => (endpoint.clone(), String::new()),
            };
            match index.resolve(&module_ref, None) {
                Ok(qualified) => {
                    if let Some(obj) = conn.as_object_mut() {
                        obj.insert(key.to_string(), Value::String(format!("{qualified}{rest}")));
                    }
                }
                Err(e @ ResolveError::Ambiguous { .. }) => {
                    findings.push(e.into_finding(
                        "PKG044",
                        severity_pkg044,
                        Some(format!("architecture.connections[].{key}")),
                    ));
                }
                Err(e) => {
                    findings.push(e.into_finding(
                        "PKG043",
                        severity_pkg043,
                        Some(format!("architecture.connections[].{key}")),
                    ));
                }
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pkg(name: &str, source_path: &str) -> PackageSection {
        PackageSection {
            name: name.to_string(),
            source_path: source_path.to_string(),
        }
    }

    #[test]
    fn qualify_builds_form() {
        assert_eq!(qualify(Some("core"), "Foo"), "core::Foo");
        assert_eq!(qualify(None, "Foo"), "Foo");
    }

    #[test]
    fn parse_splits_on_double_colon() {
        assert_eq!(parse_ref("core::Foo"), (Some("core"), "Foo"));
        assert_eq!(parse_ref("Foo"), (None, "Foo"));
        // No dot-split concern here — dots belong to port endpoints, not
        // module references.
    }

    fn modules_view<'a>(
        modules: &'a [Value],
        pkgs: &'a [Option<&'a str>],
    ) -> Vec<(&'a Value, Option<&'a str>)> {
        modules.iter().zip(pkgs.iter().copied()).collect()
    }

    #[test]
    fn resolve_unpackaged_project() {
        let modules = vec![json!({"name": "Foo"}), json!({"name": "Bar"})];
        let pkgs: Vec<Option<&str>> = vec![None, None];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &[]);
        assert_eq!(idx.resolve("Foo", None).unwrap(), "Foo");
        assert!(matches!(
            idx.resolve("Missing", None),
            Err(ResolveError::UnknownModule { .. })
        ));
    }

    #[test]
    fn resolve_same_package_preferred() {
        let modules = vec![
            json!({"name": "Config"}),
            json!({"name": "Config"}),
            json!({"name": "Scanner"}),
        ];
        let pkgs = [Some("core"), Some("audio"), Some("audio")];
        let packages = vec![pkg("core", "crates/core"), pkg("audio", "crates/audio")];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &packages);
        // Referrer in "audio" resolves bare `Config` to audio::Config.
        assert_eq!(
            idx.resolve("Config", Some("audio")).unwrap(),
            "audio::Config"
        );
        // Referrer in "core" resolves to core::Config.
        assert_eq!(idx.resolve("Config", Some("core")).unwrap(), "core::Config");
    }

    #[test]
    fn resolve_qualified_exact_match() {
        let modules = vec![json!({"name": "Config"}), json!({"name": "Config"})];
        let pkgs = [Some("core"), Some("audio")];
        let packages = vec![pkg("core", "crates/core"), pkg("audio", "crates/audio")];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &packages);
        assert_eq!(
            idx.resolve("audio::Config", Some("core")).unwrap(),
            "audio::Config"
        );
    }

    #[test]
    fn resolve_qualified_unknown_package() {
        let modules = vec![json!({"name": "Config"})];
        let pkgs = [Some("core")];
        let packages = vec![pkg("core", "crates/core")];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &packages);
        assert!(matches!(
            idx.resolve("nope::Config", Some("core")),
            Err(ResolveError::UnknownPackage(_))
        ));
    }

    #[test]
    fn resolve_qualified_unknown_module_in_known_package() {
        let modules = vec![json!({"name": "Config"})];
        let pkgs = [Some("core")];
        let packages = vec![pkg("core", "crates/core"), pkg("audio", "crates/audio")];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &packages);
        assert!(matches!(
            idx.resolve("audio::Config", None),
            Err(ResolveError::UnknownModule { .. })
        ));
    }

    #[test]
    fn resolve_ambiguous_unqualified_cross_package() {
        let modules = vec![json!({"name": "Config"}), json!({"name": "Config"})];
        let pkgs = [Some("core"), Some("audio")];
        let packages = vec![pkg("core", "crates/core"), pkg("audio", "crates/audio")];
        let idx = NameIndex::build(&modules_view(&modules, &pkgs), &packages);
        // Referrer is unpackaged, so no same-package preference kicks in.
        let err = idx.resolve("Config", None).unwrap_err();
        match err {
            ResolveError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates, vec!["audio".to_string(), "core".to_string()]);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn normalize_depends_on_rewrites_to_qualified() {
        let mut modules = vec![
            json!({"name": "A", "depends_on": ["B"]}),
            json!({"name": "B", "depends_on": []}),
        ];
        let packages = vec![pkg("core", "crates/core")];
        let mut pkg_by_name = BTreeMap::new();
        pkg_by_name.insert("A".to_string(), Some("core".to_string()));
        pkg_by_name.insert("B".to_string(), Some("core".to_string()));
        let findings = normalize_depends_on(
            &mut modules,
            &packages,
            &pkg_by_name,
            Severity::Error,
            Severity::Error,
        );
        assert!(findings.is_empty());
        assert_eq!(modules[0]["depends_on"][0], "core::B");
    }

    #[test]
    fn normalize_depends_on_emits_pkg043_for_unknown() {
        let mut modules = vec![json!({"name": "A", "depends_on": ["core::Ghost"]})];
        let packages = vec![pkg("core", "crates/core")];
        let mut pkg_by_name = BTreeMap::new();
        pkg_by_name.insert("A".to_string(), Some("core".to_string()));
        let findings = normalize_depends_on(
            &mut modules,
            &packages,
            &pkg_by_name,
            Severity::Error,
            Severity::Error,
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "PKG043");
    }

    #[test]
    fn normalize_depends_on_emits_pkg044_for_ambiguous() {
        let mut modules = vec![
            json!({"name": "A", "depends_on": ["Config"]}),
            json!({"name": "Config"}),
            json!({"name": "Config"}),
        ];
        let packages = vec![pkg("core", "crates/core"), pkg("audio", "crates/audio")];
        let mut pkg_by_name = BTreeMap::new();
        // A is top-level unpackaged — no same-package preference kicks in.
        pkg_by_name.insert("A".to_string(), None);
        pkg_by_name.insert("Config".to_string(), Some("core".to_string()));
        // Second Config wins the map lookup; we need both entries but the map
        // collapses; that's fine because NameIndex uses the raw module list.
        pkg_by_name.insert("Config".to_string(), Some("audio".to_string()));
        let findings = normalize_depends_on(
            &mut modules,
            &packages,
            &pkg_by_name,
            Severity::Error,
            Severity::Error,
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "PKG044");
    }

    #[test]
    fn normalize_architecture_qualifies_endpoints() {
        let modules = vec![json!({"name": "A"}), json!({"name": "B"})];
        let mut arch = json!({
            "connections": [
                { "from": "A.out", "to": "B.in" }
            ]
        });
        let packages = vec![pkg("core", "crates/core")];
        let mut pkg_by_name = BTreeMap::new();
        pkg_by_name.insert("A".to_string(), Some("core".to_string()));
        pkg_by_name.insert("B".to_string(), Some("core".to_string()));
        let findings = normalize_architecture(
            &mut arch,
            &modules,
            &packages,
            &pkg_by_name,
            Severity::Error,
            Severity::Error,
        );
        assert!(findings.is_empty());
        assert_eq!(arch["connections"][0]["from"], "core::A.out");
        assert_eq!(arch["connections"][0]["to"], "core::B.in");
    }

    #[test]
    fn flat_project_stays_byte_stable() {
        let mut modules = vec![
            json!({"name": "A", "depends_on": ["B"]}),
            json!({"name": "B"}),
        ];
        let mut pkg_by_name = BTreeMap::new();
        pkg_by_name.insert("A".to_string(), None);
        pkg_by_name.insert("B".to_string(), None);
        let findings = normalize_depends_on(
            &mut modules,
            &[],
            &pkg_by_name,
            Severity::Error,
            Severity::Error,
        );
        assert!(findings.is_empty());
        // Unpackaged — depends_on is unchanged.
        assert_eq!(modules[0]["depends_on"][0], "B");
    }
}
