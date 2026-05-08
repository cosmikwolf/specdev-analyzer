//! Audio-plugin silence-and-log skeleton support.
//!
//! The audio-plugin profile flips `skeleton.supported: true` with a
//! `silence_and_log` audio_path_stub policy. The mechanical skeleton
//! emitter runs to completion (no SKEL-001 gate); the silence-and-log
//! body is authored into the codegen prose so the LLM-driven `gen`
//! path can produce it on top of the mechanical skeleton.
//!
//! Coverage:
//!   1. Manifest dispatch: audio-plugin (`supported: true`,
//!      `audio_path_stub: silence_and_log`) does NOT raise SKEL-001.
//!   2. `Profile::load` round-trips the audio_path_stub field via the
//!      typed manifest surface.
//!   3. `is_audio_path` on items[] is accepted by the inlined items
//!      schema (validates clean against an items[] entry with the
//!      flag set, validates clean without it).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    manifest::{AudioPathStub, Profile},
    skeleton,
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
        "specdev-audio-plugin-skel-{tag}-{}-{}",
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

fn fresh_audio_plugin(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: "skel_audio".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

// ===========================================================================
// 1 — `gen --skeleton` against an audio-plugin project does NOT raise
//     SKEL-001 (the supported gate now lets the emit pipeline run).
//
//     `cargo check` at the end of the pipeline may fail (the empty
//     skeleton has no nih-plug dependency wired up), surfacing as
//     SKEL-102 and rolling back. Either Ok or SKEL-102 is acceptable
//     here — what we're guarding against is the SKEL-001 regression.
// ===========================================================================
#[test]
fn audio_plugin_skeleton_does_not_raise_skel_001() {
    let (_target, layout) = fresh_audio_plugin("no-skel-001");
    let result = skeleton::run_skeleton(&layout);
    if let Err(err) = result {
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("SKEL-001"),
            "audio-plugin must not raise SKEL-001 once skeleton.supported flips on; got {msg}"
        );
    }
}

// ===========================================================================
// 2 — Manifest round-trip: the audio-plugin profile.yaml's
//     `audio_path_stub: silence_and_log` parses into the typed
//     `AudioPathStub::SilenceAndLog` variant.
// ===========================================================================
#[test]
fn audio_plugin_manifest_carries_silence_and_log_stub() {
    let manifest_path = phase6_plugin()
        .join("profiles")
        .join("audio-plugin")
        .join("profile.yaml");
    let profile = Profile::load(&manifest_path)
        .expect("audio-plugin profile.yaml loads cleanly");

    let skeleton = profile
        .skeleton
        .expect("audio-plugin has a skeleton section");
    assert!(
        skeleton.supported,
        "audio-plugin must declare skeleton.supported: true"
    );
    assert_eq!(
        skeleton.audio_path_stub,
        Some(AudioPathStub::SilenceAndLog),
        "audio_path_stub must be silence_and_log"
    );
}

// ===========================================================================
// 3 — `is_audio_path` on items[] is accepted by the inlined items
//     schema. Bootstrap an audio-plugin project, write a module spec
//     with a fn item carrying `is_audio_path = true`, and confirm
//     validate finds no schema errors on that field.
// ===========================================================================
#[test]
fn items_is_audio_path_validates_clean() {
    let (target, _initial) = fresh_audio_plugin("is-audio-path");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[plugin]
name = "Skel Audio"
vendor = "ExampleCo"
version = "0.1.0"
category = "instrument"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Voice.toml"),
        r#"name = "Voice"
layer = "dsp_node"
description = "Single voice render entry point."

[[items]]
kind = "fn"
name = "render_block"
description = "Render one block of samples into the output buffer."
satisfies = ["REQ-DSP-1"]
is_audio_path = true

[[items]]
kind = "fn"
name = "reset_voice"
description = "Reset envelope and oscillator phase between notes."
satisfies = ["REQ-DSP-2"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-DSP-1"
description = "Render block of audio samples."

[[requirement]]
id = "REQ-DSP-2"
description = "Reset voice state between notes."
"#,
    )
    .unwrap();

    let layout = LayoutContext::discover(&target, None).unwrap();
    let report = specdev_analyzer::validate::run_project(&layout.project_root, None).unwrap();
    let schema_errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, specdev_analyzer::Severity::Error))
        .filter(|f| {
            let s = format!("{f:?}");
            s.contains("is_audio_path")
        })
        .collect();
    assert!(
        schema_errors.is_empty(),
        "items[] is_audio_path must validate clean; got {schema_errors:#?}"
    );
}
