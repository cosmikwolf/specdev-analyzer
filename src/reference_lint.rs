//! Plugin-author lint for reference crates.
//!
//! Two opt-in, deterministic checks over a unit's `reference/<lang>/`:
//!
//! - **`REF-COVERAGE-001`** — every name in `reference/<lang>/reference_cases.yaml`
//!   has a `reference/<lang>/src/<name>.rs`. Pure `{string → file-presence}`.
//! - **`REF-ARITY-001`** — every generic type the unit's codegen README names is
//!   used at the same generic arity as in the compiled reference source. The
//!   reference-source arities are the ground truth (they type-check); only base
//!   names present in the reference are checked, so prose-only types never flag.
//!
//! Both are agnostic: the case names are opaque strings the analyzer never
//! interprets, and the arity comparison is a *Rust-language* operation via the
//! existing tree-sitter adapter, not a domain notion. The manifest is opt-in —
//! a unit without `reference_cases.yaml` is skipped — and lives under
//! `reference/`, which `inline` never copies, so it is plugin-author CI-only
//! state (never reaches a user project).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::lang::{adapter_for, LanguageAdapter};

/// Walk `blocks/` and `bridges/` and lint any unit that ships a
/// `reference/<lang>/reference_cases.yaml`. Returns pre-coded, sorted,
/// de-duplicated offender lines (`<unit>: [CODE] message`).
pub fn lint_reference_crates(plugin_root: &Path) -> Result<Vec<String>> {
    let mut offenders = Vec::new();
    for kind_dir in ["blocks", "bridges"] {
        let base = plugin_root.join(kind_dir);
        if !base.is_dir() {
            continue;
        }
        let mut units: Vec<_> = std::fs::read_dir(&base)
            .with_context(|| format!("reading {}", base.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        units.sort();
        for unit in units {
            lint_unit(&unit, &mut offenders)?;
        }
    }
    offenders.sort();
    offenders.dedup();
    Ok(offenders)
}

fn lint_unit(unit: &Path, offenders: &mut Vec<String>) -> Result<()> {
    let unit_name = unit
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string();
    let reference = unit.join("reference");
    if !reference.is_dir() {
        return Ok(());
    }
    let mut langs: Vec<_> = std::fs::read_dir(&reference)
        .with_context(|| format!("reading {}", reference.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    langs.sort();
    for lang_dir in langs {
        let lang = lang_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let manifest = lang_dir.join("reference_cases.yaml");
        if !manifest.is_file() {
            continue; // opt-in: no manifest, no checks
        }
        let cases = read_cases(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;

        // --- REF-COVERAGE-001: each declared case has a reference source ---
        for case in &cases {
            let src = lang_dir.join("src").join(format!("{case}.rs"));
            if !src.is_file() {
                offenders.push(format!(
                    "{unit_name}: [REF-COVERAGE-001] reference_cases lists {case:?} \
                     but {} is missing",
                    src.display()
                ));
            }
        }

        // --- REF-ARITY-001: prose generic arities must match the reference ---
        // The wrap-parse used for prose fragments is Rust-shaped; for other
        // languages it simply yields nothing (no false flags), so the check is
        // Rust-effective today and degrades silently elsewhere.
        if let Some(adapter) = adapter_for(&lang) {
            let ref_arity = reference_arity_map(&lang_dir, adapter.as_ref())?;
            let readme = unit.join("codegen").join(&lang).join("README.md");
            if ref_arity.is_empty() || !readme.is_file() {
                continue;
            }
            let prose = std::fs::read_to_string(&readme)
                .with_context(|| format!("reading {}", readme.display()))?;
            for (base, arity) in prose_generic_arities(&prose, adapter.as_ref()) {
                if let Some(&expected) = ref_arity.get(&base) {
                    if arity != expected {
                        offenders.push(format!(
                            "{unit_name}: [REF-ARITY-001] codegen prose uses \
                             `{base}<..>` at generic arity {arity} but the compiled \
                             reference uses arity {expected}"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Parse `reference_cases: [..]` from the manifest into a list of opaque names.
fn read_cases(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_yaml::from_str(&text)?;
    let arr = value
        .get("reference_cases")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing `reference_cases:` array"))?;
    Ok(arr
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect())
}

/// Build `terminal-type-name → generic arity` from every reference source file
/// (the compiled ground truth). First occurrence wins; a base name with two
/// arities in one reference set is degenerate and the first is kept.
fn reference_arity_map(
    lang_dir: &Path,
    adapter: &dyn LanguageAdapter,
) -> Result<BTreeMap<String, usize>> {
    let mut map = BTreeMap::new();
    let src_dir = lang_dir.join("src");
    if !src_dir.is_dir() {
        return Ok(map);
    }
    let ext = adapter.file_extension();
    let mut files: Vec<_> = std::fs::read_dir(&src_dir)?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
        .collect();
    files.sort();
    for f in files {
        let src = std::fs::read_to_string(&f)?;
        for (base, arity) in generic_arities(&src, adapter) {
            map.entry(base).or_insert(arity);
        }
    }
    Ok(map)
}

/// Every `(terminal-name, arity)` for generic types in `src`, via tree-sitter.
fn generic_arities(src: &str, adapter: &dyn LanguageAdapter) -> Vec<(String, usize)> {
    let mut parser = Parser::new();
    if parser.set_language(&adapter.tree_sitter_language()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(src, None) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_generic_types(tree.root_node(), src, &mut out);
    out
}

fn collect_generic_types(node: Node, src: &str, out: &mut Vec<(String, usize)>) {
    // Skip generic types that parse with errors. Prose uses placeholder
    // shorthand inside generics (`Arc<Mutex<...>>`, `Arc<Atomic*>`) that is not
    // valid Rust; tree-sitter flags it, and its arity can't be trusted — so do
    // not compare it (keeps the check at zero false positives).
    if node.kind() == "generic_type" && !node.has_error() {
        if let (Some(ty), Some(args)) = (
            node.child_by_field_name("type"),
            node.child_by_field_name("type_arguments"),
        ) {
            if let Ok(text) = ty.utf8_text(src.as_bytes()) {
                // terminal segment of a possibly path-scoped name
                let base = text.rsplit("::").next().unwrap_or(text).trim();
                if !base.is_empty() {
                    out.push((base.to_string(), args.named_child_count()));
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_generic_types(child, src, out);
    }
}

/// Generic-type occurrences in README prose. Candidate `Ident<...>` fragments
/// (from fences, table cells, and inline code alike) are wrapped as type
/// aliases so a bare fragment parses, then run through the same generic walk —
/// guaranteeing the *same* arity-counting rule on both sides.
fn prose_generic_arities(prose: &str, adapter: &dyn LanguageAdapter) -> Vec<(String, usize)> {
    let candidates = extract_type_candidates(prose);
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut wrapped = String::new();
    for (i, c) in candidates.iter().enumerate() {
        wrapped.push_str(&format!("type _C{i} = {c};\n"));
    }
    generic_arities(&wrapped, adapter)
}

/// Find `Ident<...>` (optionally path-scoped) substrings with balanced angle
/// brackets. Bounded look-ahead so a stray `<` used as less-than doesn't match
/// a distant `>`. Over-collection is harmless: a fragment that doesn't parse as
/// a generic type yields nothing, and only base names present in the reference
/// are ever compared.
fn extract_type_candidates(text: &str) -> Vec<String> {
    const MAX_SPAN: usize = 240;
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'<' {
            if let Some(start) = ident_start_before(bytes, i) {
                if let Some(end) = match_angle(bytes, i, MAX_SPAN) {
                    if let Ok(frag) = std::str::from_utf8(&bytes[start..=end]) {
                        out.push(frag.to_string());
                    }
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// If the bytes immediately before `lt` form a (path-scoped) identifier ending
/// in an identifier char, return the run's start index. `::` is allowed inside.
fn ident_start_before(bytes: &[u8], lt: usize) -> Option<usize> {
    if lt == 0 || !is_ident_byte(bytes[lt - 1]) {
        return None;
    }
    let mut j = lt;
    while j > 0 && (is_ident_byte(bytes[j - 1]) || bytes[j - 1] == b':') {
        j -= 1;
    }
    // must start on an ident char (not a digit or a stray colon)
    if j < lt && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
        Some(j)
    } else {
        None
    }
}

/// Index of the `>` that closes the `<` at `open`, or `None` if unbalanced
/// within `max_span` bytes.
fn match_angle(bytes: &[u8], open: usize, max_span: usize) -> Option<usize> {
    let mut depth = 0usize;
    let end = (open + max_span).min(bytes.len());
    let mut k = open;
    while k < end {
        match bytes[k] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> std::path::PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "reflint-test-{}-{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// Build a bridge unit with a reference crate + codegen README.
    fn write_bridge(
        root: &Path,
        name: &str,
        cases: &[&str],
        src_files: &[(&str, &str)],
        readme: &str,
    ) {
        let unit = root.join("bridges").join(name);
        let refrust = unit.join("reference").join("rust");
        std::fs::create_dir_all(refrust.join("src")).unwrap();
        std::fs::create_dir_all(unit.join("codegen").join("rust")).unwrap();
        let list: String = cases.iter().map(|c| format!("  - {c}\n")).collect();
        std::fs::write(
            refrust.join("reference_cases.yaml"),
            format!("reference_cases:\n{list}"),
        )
        .unwrap();
        for (fname, body) in src_files {
            std::fs::write(refrust.join("src").join(fname), body).unwrap();
        }
        std::fs::write(
            unit.join("codegen").join("rust").join("README.md"),
            readme,
        )
        .unwrap();
    }

    #[test]
    fn green_when_cases_covered_and_arities_match() {
        let root = scratch();
        write_bridge(
            &root,
            "a+b",
            &["i2c"],
            &[("i2c.rs", "fn t(x: I2c<'static, Async, Master>) {}")],
            "task takes `I2c<'static, Async, Master>` by value.",
        );
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(offenders.is_empty(), "{offenders:?}");
    }

    #[test]
    fn coverage_flags_declared_case_without_source() {
        let root = scratch();
        write_bridge(
            &root,
            "a+b",
            &["i2c", "spi"], // spi declared, no spi.rs
            &[("i2c.rs", "fn t(x: I2c<'static, Async, Master>) {}")],
            "uses `I2c<'static, Async, Master>`.",
        );
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(
            offenders.iter().any(|o| o.contains("REF-COVERAGE-001")
                && o.contains("\"spi\"")),
            "{offenders:?}"
        );
        // i2c is covered → no coverage finding for it
        assert!(
            !offenders.iter().any(|o| o.contains("\"i2c\"")),
            "{offenders:?}"
        );
    }

    #[test]
    fn arity_flags_wrong_generic_arity_in_prose_incl_table_cell() {
        let root = scratch();
        // reference: I2c is arity 3. Prose (a table cell) says arity 2 — the
        // DPS310 bug. Must flag.
        write_bridge(
            &root,
            "a+b",
            &["i2c"],
            &[("i2c.rs", "fn t(x: I2c<'static, Async, Master>) {}")],
            "| I2C | `i2c::I2c<'static, Async>` |\n",
        );
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(
            offenders.iter().any(|o| o.contains("REF-ARITY-001")
                && o.contains("I2c")
                && o.contains("arity 2")
                && o.contains("arity 3")),
            "{offenders:?}"
        );
    }

    #[test]
    fn arity_ignores_placeholder_shorthand_in_prose() {
        let root = scratch();
        // Reference uses Arc<T> (arity 1). Prose uses placeholder shorthand
        // inside generics (`Arc<Mutex<...>>`, `Arc<Atomic*>`) that is not valid
        // Rust — the parser flags it; must NOT miscount as arity 2 and flag.
        write_bridge(
            &root,
            "a+b",
            &["editor"],
            &[("editor.rs", "fn t(x: std::sync::Arc<Foo>) {}")],
            "captures `Arc<MyParams>`, engine lives behind `Arc<Mutex<...>>`, \
             reads `Arc<Atomic*>` cells.",
        );
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(offenders.is_empty(), "{offenders:?}");
    }

    #[test]
    fn arity_ignores_prose_only_types_no_false_positive() {
        let root = scratch();
        // Reference only has Output (arity 1). Prose mentions Result<…> and
        // Mutex<…> (not in the reference) — must NOT flag.
        write_bridge(
            &root,
            "a+b",
            &["gpio"],
            &[("gpio.rs", "fn t(x: Output<'static>) {}")],
            "returns `Result<Uart, ConfigError>`; held in a \
             `Mutex<CriticalSectionRawMutex, Option<Foo>>`. Uses `Output<'static>`.",
        );
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(offenders.is_empty(), "{offenders:?}");
    }

    #[test]
    fn no_manifest_means_no_checks() {
        let root = scratch();
        let unit = root.join("bridges").join("a+b");
        std::fs::create_dir_all(unit.join("reference").join("rust").join("src")).unwrap();
        // a source file but no reference_cases.yaml → opt-out
        std::fs::write(
            unit.join("reference").join("rust").join("src").join("i2c.rs"),
            "fn t(x: I2c<'static, Async>) {}",
        )
        .unwrap();
        let offenders = lint_reference_crates(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(offenders.is_empty(), "{offenders:?}");
    }
}
