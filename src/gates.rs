//! The compile-gate registry (`gates:` in the inlined `profile.yaml`):
//! the YAML renderer that writes the block to paste, and the runner that
//! executes one declared gate.
//!
//! A gate's result is its exit code. Results are not stored: the project's
//! CI runs the gates and the test suite, and the analyzer keeps no record of
//! a past run.

use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::layout::LayoutContext;
use crate::manifest::{Gate, Profile};
use crate::path::{eval_template, Scope};

// ---------------------------------------------------------------------------
// The runner: `specdev-analyzer gate --label <l> [--module <m>]…`.
// ---------------------------------------------------------------------------

/// What one `gate` invocation established.
pub struct GateRun {
    pub label: String,
    /// Every step exited zero.
    pub pass: bool,
    /// The process exit code the command must return: `0` on a pass, else
    /// the failing step's own exit code (`1` when a signal killed it, so
    /// "died" is never reported as success).
    pub code: i32,
}

/// Run one declared gate.
///
/// The steps run in the **project root** (a workspace member's own root),
/// with the **inherited environment** and **`stdin` closed**: a first-run
/// tool that prompts must fail, not wait forever. The first non-zero exit
/// stops the gate. Output is captured and printed: everything on a failure,
/// and a step's `stderr` on a pass when it is non-empty. Both go to **this
/// process's stderr**, which keeps the first error in one stream whether the
/// tool wrote it to `stdout` (`tsc`) or `stderr` (`cargo`).
///
/// A step whose executable **cannot start**, or a token that resolves to
/// nothing, is an `Err`; a step that **starts and exits non-zero** is
/// `Ok` with `pass: false`.
///
/// No timeout in v1.
pub fn run_gate(layout: &LayoutContext, label: &str) -> Result<GateRun> {
    let profile = Profile::load_inlined(&layout.specdev_root)?
        .ok_or_else(|| anyhow!("gate: no inlined profile.yaml"))?;
    let gate = profile
        .gates
        .iter()
        .find(|g| g.label == label)
        .ok_or_else(|| {
            let declared: Vec<String> = profile
                .gates
                .iter()
                .map(|g| format!("`{}`", g.label))
                .collect();
            anyhow!(
                "gate: `{label}` is not declared in {}. Declared: {}. \
                 The label is the registry's string, verbatim.",
                layout.specdev_root.join("profile.yaml").display(),
                declared.join(", ")
            )
        })?;

    // Resolve every token before running anything.
    let steps = resolve_steps(layout, gate)?;

    let mut pass = true;
    let mut code = 0;
    for (index, argv) in steps.iter().enumerate() {
        let shown = argv.join(" ");
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&layout.project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                anyhow!(
                    "gate `{label}`: step {} of {} could not start: `{shown}`: {e}. \
                     Install the tool or fix `PATH`, then re-run.",
                    index + 1,
                    steps.len()
                )
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            if !stderr.trim().is_empty() {
                eprintln!("gate `{label}` step {} (`{shown}`) stderr:", index + 1);
                eprint!("{stderr}");
            }
            continue;
        }
        pass = false;
        // A step that failed must never report success: a signal leaves
        // no code at all, and 0 with a non-success status is not a state
        // any platform we target produces.
        code = output.status.code().filter(|c| *c != 0).unwrap_or(1);
        eprintln!(
            "gate `{label}`: step {} of {} failed (exit {code}): `{shown}`",
            index + 1,
            steps.len()
        );
        if !stdout.trim().is_empty() {
            eprintln!("--- stdout ---");
            eprint!("{stdout}");
        }
        if !stderr.trim().is_empty() {
            eprintln!("--- stderr ---");
            eprint!("{stderr}");
        }
        break;
    }

    Ok(GateRun {
        label: label.to_string(),
        pass,
        code,
    })
}

/// Resolve `${root.<table>.<field>}` tokens in every argv element against
/// `specs/architecture.toml`. The document is read only when a token is
/// present, so a project with neither still runs its gate. A missing
/// field is an error (`eval_template`); a field that resolves to nothing
/// is an error here — an empty argv element is never what the profile
/// meant.
fn resolve_steps(layout: &LayoutContext, gate: &Gate) -> Result<Vec<Vec<String>>> {
    let needs_tokens = gate
        .steps
        .iter()
        .flatten()
        .any(|element| element.contains("${"));
    let document = if needs_tokens {
        Some(
            crate::reduction::load_toml_as_json(&layout.architecture_toml_path).with_context(
                || {
                    format!(
                        "gate `{}`: a step carries a `${{…}}` token, which resolves against {}",
                        gate.label,
                        layout.architecture_toml_path.display()
                    )
                },
            )?,
        )
    } else {
        None
    };

    let mut out = Vec::with_capacity(gate.steps.len());
    for step in &gate.steps {
        let mut argv = Vec::with_capacity(step.len());
        for element in step {
            if !element.contains("${") {
                argv.push(element.clone());
                continue;
            }
            let document = document.as_ref().expect("token implies a loaded document");
            let scope = Scope {
                root: document,
                current: Some(document),
                parent: None,
            };
            let resolved = eval_template(element, &scope).with_context(|| {
                format!(
                    "gate `{}`: resolving `{element}` against {}",
                    gate.label,
                    layout.architecture_toml_path.display()
                )
            })?;
            if resolved.trim().is_empty() {
                bail!(
                    "gate `{}`: `{element}` resolved to nothing against {}. A token \
                     resolves to a value or it is an error — never to an empty argument.",
                    gate.label,
                    layout.architecture_toml_path.display()
                );
            }
            argv.push(resolved);
        }
        out.push(argv);
    }
    Ok(out)
}
