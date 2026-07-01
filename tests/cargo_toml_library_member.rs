//! F12-22 / F12-23 — firmware library-member support.
//!
//! A firmware-layered *binary* profile can host a workspace member that is a
//! driver/HAL-shim **library**:
//!   - F12-22: `[project].crate_shape = "library"` renders `[lib]` +
//!     `src/lib.rs` instead of `[[bin]]` + `src/main.rs`, keeping every
//!     dependency the binary shape would have emitted.
//!   - F12-23: a path `[[external_deps]]` needs no version — it renders
//!     `{ path = "..." }` with no `version` key (a versionless workspace-
//!     internal dep). Registry deps still require a version (schema-enforced).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/specs/full_plugin/_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-libmember-{tag}-{}-{}",
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

fn bootstrap_firmware(name: &str, tag: &str) -> PathBuf {
    let target = tmp_dir(tag).join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

fn render_text(target: &Path) -> String {
    let layout = LayoutContext::discover(target, None).unwrap();
    render(&layout).unwrap().text
}

#[test]
fn crate_shape_library_renders_lib_not_bin() {
    let target = bootstrap_firmware("sensor-driver", "lib");
    // Add the per-package override to spec.toml's [project].
    let spec = target.join("spec.toml");
    let body = std::fs::read_to_string(&spec).unwrap();
    let body = body.replace(
        "language = \"rust\"",
        "language = \"rust\"\ncrate_shape = \"library\"",
    );
    std::fs::write(&spec, body).unwrap();

    let out = render_text(&target);
    assert!(out.contains("[lib]"), "library member must emit [lib]:\n{out}");
    assert!(
        out.contains(r#"name = "sensor_driver""#),
        "lib name must be the hyphen->underscore crate ident:\n{out}"
    );
    assert!(
        out.contains(r#"path = "src/lib.rs""#),
        "library member crate root is src/lib.rs:\n{out}"
    );
    assert!(
        !out.contains("[[bin]]"),
        "library member must NOT emit [[bin]]:\n{out}"
    );
    // Dependencies are unchanged from the binary shape — the platform/runtime/
    // logger crates the firmware renderer emits are still present.
    assert!(out.contains("cortex-m"), "deps still emitted for a lib:\n{out}");
}

#[test]
fn default_shape_still_renders_bin() {
    // No crate_shape → firmware-layered's binary default is unchanged.
    let target = bootstrap_firmware("app", "bin");
    let out = render_text(&target);
    assert!(out.contains("[[bin]]"), "default is still [[bin]]:\n{out}");
    assert!(out.contains(r#"path = "src/main.rs""#));
    assert!(!out.contains("[lib]"), "no [lib] without the override:\n{out}");
}

#[test]
fn path_external_dep_without_version_renders_versionless() {
    let target = bootstrap_firmware("app", "pathdep");
    // Append a versionless path dep to architecture.toml.
    let arch = target.join("specs/architecture.toml");
    let mut body = std::fs::read_to_string(&arch).unwrap();
    body.push_str(
        "\n[[external_deps]]\ncrate = \"sensor-driver\"\nsource = { path = \"../sensor-driver\" }\n",
    );
    std::fs::write(&arch, body).unwrap();

    let out = render_text(&target);
    assert!(
        out.contains(r#"sensor-driver = { path = "../sensor-driver" }"#),
        "a path dep with no version renders versionless:\n{out}"
    );
    // No `version =` inside the sensor-driver dep value.
    let dep_line = out
        .lines()
        .find(|l| l.starts_with("sensor-driver = "))
        .unwrap_or("");
    assert!(
        !dep_line.contains("version"),
        "versionless path dep must carry no version key: `{dep_line}`"
    );
}
