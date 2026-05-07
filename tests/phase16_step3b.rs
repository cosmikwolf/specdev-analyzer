//! Phase 16 §8.1 step 3b — `cargo_toml.bin.gate` interpreter.
//!
//! Per doc 21 §3.4 + §3.2 (`BinGate` enum): the audio-plugin profile's
//! `[[bin]]` row is gated by `cargo_toml.bin.gate.kind = architecture_array_contains`
//! pointing at `[plugin].formats` containing `"standalone"`. Step 3b
//! moves the evaluator from a hardcoded literal check to a closed-enum
//! `BinGate` interpreter; backward-compat fallback (manifest absent)
//! retains the legacy literal check. **Byte-for-byte parity** between
//! the two paths is asserted across both gate-true and gate-false
//! architecture configurations.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase16-step3b-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn fresh_audio_plugin(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: "myfx".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn arch_path(target: &Path) -> PathBuf {
    target.join("specs/architecture.toml")
}

fn manifest_path(target: &Path) -> PathBuf {
    target.join("specs/specdev/profile.yaml")
}

/// Rewrite architecture.toml with the given formats array. The rest of
/// the audio-plugin minimum-viable shape (plugin identity, audio bus,
/// runtime) is preserved as a bare-but-valid skeleton — enough to
/// satisfy schema validation, not used by cargo-toml's gate code path.
fn write_arch_with_formats(target: &Path, formats: &[&str]) {
    let formats_arr = formats
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"[plugin]
name     = "myfx"
vendor   = "ExampleCo"
version  = "0.1.0"
category = "instrument"
formats  = [{formats_arr}]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "default"
"#
    );
    fs::write(arch_path(target), body).unwrap();
}

// ===========================================================================
// 1 — Gate hit: formats contains "standalone" → emit [[bin]]
// ===========================================================================

#[test]
fn gate_hit_emits_standalone_bin() {
    let (target, _layout) = fresh_audio_plugin("gate-hit");
    write_arch_with_formats(&target, &["vst3", "clap", "standalone"]);
    let layout = LayoutContext::discover(&target, None).unwrap();
    let out = render(&layout).unwrap().text;
    assert!(out.contains("[[bin]]"), "gate hit must emit [[bin]]:\n{out}");
    assert!(
        out.contains(r#"name = "myfx-standalone""#),
        "bin name must be `<project>-standalone`:\n{out}"
    );
    assert!(
        out.contains(r#"path = "src/bin/standalone.rs""#),
        "bin path must be src/bin/standalone.rs:\n{out}"
    );
}

// ===========================================================================
// 2 — Gate miss: formats lacks "standalone" → no [[bin]]
// ===========================================================================

#[test]
fn gate_miss_skips_standalone_bin() {
    let (target, _layout) = fresh_audio_plugin("gate-miss");
    write_arch_with_formats(&target, &["vst3", "clap"]);
    let layout = LayoutContext::discover(&target, None).unwrap();
    let out = render(&layout).unwrap().text;
    assert!(
        !out.contains("[[bin]]"),
        "gate miss must NOT emit [[bin]]:\n{out}"
    );
    assert!(
        !out.contains("src/bin/standalone.rs"),
        "gate miss must NOT reference src/bin/standalone.rs:\n{out}"
    );
    // Sanity: [lib] cdylib still present.
    assert!(out.contains(r#"crate-type = ["cdylib"]"#));
}

// ===========================================================================
// 3 — Gate miss: missing [plugin] table entirely → no [[bin]]
// ===========================================================================

#[test]
fn gate_miss_when_plugin_table_absent() {
    let (target, _layout) = fresh_audio_plugin("gate-no-plugin");
    fs::write(
        arch_path(&target),
        r#"[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "default"
"#,
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    // The audio-plugin module/architecture schema would normally reject
    // this (missing required [plugin] section), but cargo_toml::render
    // doesn't re-validate the schema — it tolerates the absent table
    // and the gate evaluates to false (the safe default).
    let out = render(&layout).unwrap().text;
    assert!(
        !out.contains("[[bin]]"),
        "gate miss when [plugin] absent must NOT emit [[bin]]:\n{out}"
    );
}

// ===========================================================================
// 4 — Byte parity: gate-hit case, manifest path == fallback path
// ===========================================================================

#[test]
fn parity_gate_hit_manifest_vs_fallback() {
    let (target, _layout) = fresh_audio_plugin("parity-hit");
    write_arch_with_formats(&target, &["vst3", "clap", "standalone"]);

    let layout = LayoutContext::discover(&target, None).unwrap();
    let manifest_out = render(&layout).unwrap().text;

    // Strip manifest → fallback path, same architecture.
    fs::remove_file(manifest_path(&target)).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_out = render(&layout).unwrap().text;

    assert_eq!(
        manifest_out, fallback_out,
        "gate-hit: manifest-driven and fallback paths must produce identical output"
    );
}

// ===========================================================================
// 5 — Byte parity: gate-miss case, manifest path == fallback path
// ===========================================================================

#[test]
fn parity_gate_miss_manifest_vs_fallback() {
    let (target, _layout) = fresh_audio_plugin("parity-miss");
    write_arch_with_formats(&target, &["vst3", "clap"]);

    let layout = LayoutContext::discover(&target, None).unwrap();
    let manifest_out = render(&layout).unwrap().text;

    fs::remove_file(manifest_path(&target)).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_out = render(&layout).unwrap().text;

    assert_eq!(
        manifest_out, fallback_out,
        "gate-miss: manifest-driven and fallback paths must produce identical output"
    );
}
