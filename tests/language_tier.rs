//! `languages/<lang>/` plugin tier integration through layout +
//! bootstrap + aggregator.
//!
//! Coverage:
//!   * Bootstrap copies `languages/<inputs.language>/*.schema.yaml` into
//!     `<target>/specs/specdev/schemas/inlined/languages/<lang>/`.
//!   * `LayoutContext.inlined_languages_dir` resolves to the correct path.
//!   * Aggregator MapRetriever resolves `inlined/languages/<lang>/items.schema.yaml`.
//!   * Bootstrap with a language that has no entry under
//!     `${PLUGIN_ROOT}/languages/` succeeds without inlining the language tier.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-language-tier-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn bootstrap_inlines_languages_tier_for_rust() {
    let plugin_root = full_plugin();
    let target = tmp_dir("inlined-tier");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let inlined = target
        .join("specs/specdev/schemas/inlined/languages/rust/items.schema.yaml");
    assert!(
        inlined.is_file(),
        "expected items.schema.yaml at {}",
        inlined.display()
    );

    // Content is byte-faithful copy of the plugin tree.
    let plugin_src = plugin_root.join("languages/rust/items.schema.yaml");
    assert_eq!(
        std::fs::read(&inlined).unwrap(),
        std::fs::read(&plugin_src).unwrap(),
    );

    // Layout discover() resolves to the same path.
    let layout = LayoutContext::discover(&target, None).expect("discover");
    let expected = std::fs::canonicalize(
        target.join("specs/specdev/schemas/inlined/languages"),
    )
    .unwrap();
    assert_eq!(layout.inlined_languages_dir, expected);
}

#[test]
fn bootstrap_skips_languages_tier_when_absent() {
    let plugin_root = full_plugin();
    let target = tmp_dir("no-lang-tier");

    // Synthesize a fake language target by bootstrapping with a language
    // the full plugin has codegen for but no `languages/<lang>/` entry.
    // The fixture only ships `languages/rust/`, so a language without the
    // language tier exercises the silent-skip path. We can't bootstrap a
    // non-rust language here directly (the profile only has `codegen/rust/`),
    // so instead we rename the source language tier away, then bootstrap,
    // then restore. To keep the test hermetic, we operate on a copy of the
    // plugin tree under tmp.
    let plugin_copy = tmp_dir("plugin-copy");
    copy_dir_recursive(&plugin_root, &plugin_copy);
    // Remove the languages/rust/ tier from the copy.
    let _ = std::fs::remove_dir_all(plugin_copy.join("languages"));

    bootstrap::run_bootstrap(
        &plugin_copy,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // Languages tier dir is created (per EMPTY_DIRS) but empty.
    let dir = target.join("specs/specdev/schemas/inlined/languages");
    assert!(dir.is_dir(), "languages dir should exist");
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "languages dir should be empty; got {:?}",
        entries.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

#[test]
fn aggregator_retriever_registers_languages_prefix() {
    use serde_json::Value;

    let plugin_root = full_plugin();
    let target = tmp_dir("retriever");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let layout = LayoutContext::discover(&target, None).expect("discover");
    let docs = specdev_analyzer::aggregator::build_retriever_docs(&layout)
        .expect("retriever docs");

    // Registered under: full prefixed path, plain filename, and $id.
    let prefixed = "inlined/languages/rust/items.schema.yaml";
    assert!(
        docs.contains_key(prefixed),
        "expected {prefixed} in retriever docs"
    );
    assert!(docs.contains_key("items.schema.yaml"));
    // $id from the schema file.
    let v: &Value = docs.get(prefixed).unwrap();
    let id = v
        .get("$id")
        .and_then(Value::as_str)
        .expect("items.schema.yaml has $id");
    assert!(id.starts_with("https://specdev.dev/languages/rust/items"));
    assert!(docs.contains_key(id));
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        let target = dst.join(entry.file_name());
        if p.is_dir() {
            copy_dir_recursive(&p, &target);
        } else {
            std::fs::copy(&p, &target).unwrap();
        }
    }
}
