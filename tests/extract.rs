//! Integration tests for the extract pipeline.

use std::path::{Path, PathBuf};

use specdev_analyzer::{extract, validate};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("analyzer crate has a parent dir")
        .to_path_buf()
}

fn domain_file() -> PathBuf {
    workspace_root().join("specdev/domains/firmware/domain.toml")
}

fn blinky_root() -> PathBuf {
    workspace_root().join("specdev/examples/blinky-a2dp")
}

fn tmp_out(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("specdev-extract-{tag}-{}", std::process::id()));
    // Ensure clean.
    let _ = std::fs::remove_dir_all(&p);
    p
}

#[test]
fn extract_blinky_writes_expected_files() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let result = extract::run(&inputs).expect("extract run");
    let out = tmp_out("files");
    let written = extract::write(&result, &out).expect("write");

    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    for expected in [
        "AudioPipeline.yaml",
        "BtA2dpSink.yaml",
        "I2sOutput.yaml",
        "StatusLed.yaml",
        "architecture.yaml",
        "requirements.yaml",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "expected {expected} in output; got {names:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn extract_is_deterministic() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");

    let out1 = tmp_out("det1");
    let out2 = tmp_out("det2");

    extract::write(&extract::run(&inputs).unwrap(), &out1).unwrap();
    extract::write(&extract::run(&inputs).unwrap(), &out2).unwrap();

    for name in [
        "AudioPipeline.yaml",
        "BtA2dpSink.yaml",
        "I2sOutput.yaml",
        "StatusLed.yaml",
        "architecture.yaml",
        "requirements.yaml",
    ] {
        let a = std::fs::read(out1.join(name)).unwrap();
        let b = std::fs::read(out2.join(name)).unwrap();
        assert_eq!(a, b, "{name} differs across runs");
    }

    let _ = std::fs::remove_dir_all(&out1);
    let _ = std::fs::remove_dir_all(&out2);
}

#[test]
fn extract_architecture_has_summary_and_connections() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let result = extract::run(&inputs).expect("extract run");
    let arch = &result.architecture;

    let modules = arch
        .get("modules")
        .and_then(|v| v.as_array())
        .expect("architecture.modules");
    assert_eq!(modules.len(), 4, "expected 4 module summaries");

    let graph = arch
        .get("dependency_graph")
        .and_then(|v| v.as_array())
        .expect("architecture.dependency_graph");
    let has_edge = graph.iter().any(|e| {
        e.get("from").and_then(|v| v.as_str()) == Some("AudioPipeline")
            && e.get("to").and_then(|v| v.as_str()) == Some("BtA2dpSink")
    });
    assert!(has_edge, "AudioPipeline → BtA2dpSink edge missing");

    let connections = arch
        .get("connections")
        .and_then(|v| v.as_array())
        .expect("architecture.connections");
    assert!(!connections.is_empty(), "connections missing");
}
