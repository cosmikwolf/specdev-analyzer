//! The compile-gate registry (`gates:` in the inlined `profile.yaml`): the
//! runner that executes one declared gate, and the gate script CI runs.
//!
//! A gate's result is its exit code. Results are not stored: the project's
//! CI runs `scripts/specdev-gates.sh` — `check` first, then every gate —
//! and the analyzer keeps no record of a past run. The runner and the
//! script take their commands from one function, [`gate_commands`].

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
/// The steps run in the **code root** (`[paths] code`; the project root by
/// default, and a workspace member's own root),
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
    let gates = profile.gates_for(layout.spec.project.toolchain.as_deref())?;
    let gate = gates.iter().find(|g| g.label == label).ok_or_else(|| {
        let declared: Vec<String> = gates.iter().map(|g| format!("`{}`", g.label)).collect();
        anyhow!(
            "gate: `{label}` is not declared in {}. Declared: {}. \
                 The label is the registry's string, verbatim.",
            layout.specdev_root.join("profile.yaml").display(),
            declared.join(", ")
        )
    })?;

    // Resolve every token before running anything.
    let steps = gate_commands(layout, gate)?;

    let mut pass = true;
    let mut code = 0;
    for (index, argv) in steps.iter().enumerate() {
        let shown = argv.join(" ");
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&layout.code_root)
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
pub fn gate_commands(layout: &LayoutContext, gate: &Gate) -> Result<Vec<Vec<String>>> {
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
                        "gate `{}`: the step token `{}` resolves against {}",
                        gate.label,
                        gate.steps
                            .iter()
                            .flatten()
                            .find(|e| e.contains("${"))
                            .map(String::as_str)
                            .unwrap_or(""),
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

// ---------------------------------------------------------------------------
// The gate script: `scripts/specdev-gates.sh`.
// ---------------------------------------------------------------------------

/// Where the script lives, relative to the project root.
pub const SCRIPT_REL: &str = "scripts/specdev-gates.sh";

/// The `.gitattributes` line that keeps the script LF on every checkout: a
/// CRLF copy fails at once in bash and dash.
pub const GITATTRIBUTES_LINE: &str = "scripts/specdev-gates.sh text eol=lf";

/// Where the compile-only script lives, relative to the code root. It is
/// written only when `[paths] code` is not `"."`: the code tree then may be
/// a repository of its own, with no specs and no analyzer.
pub const COMPILE_SCRIPT_REL: &str = "scripts/compile-gates.sh";

/// As [`GITATTRIBUTES_LINE`], for [`COMPILE_SCRIPT_REL`] in the code root.
pub const COMPILE_GITATTRIBUTES_LINE: &str = "scripts/compile-gates.sh text eol=lf";

/// The header of [`PROLOGUE`], replaced in the compile-only script.
const HEADER: &str = r#"# Generated by specdev-analyzer from the `gates:` registry in
# specs/specdev/profile.yaml. Do not edit: `specdev-analyzer gate --emit`
# rewrites it, and `validate` reports a copy that differs from the registry.
# CI runs it: `sh scripts/specdev-gates.sh`. Set SPECDEV_ANALYZER to use an
# analyzer that is not on PATH.
"#;

/// The compile-only script's header: it names neither the specs nor the
/// tool, because the code tree it sits in may be published without them.
const COMPILE_HEADER: &str = r#"# Generated from this project's compile-gate list, which is kept with the
# project's specification. Do not edit: a copy that differs from the list is
# reported there. CI runs it: `sh scripts/compile-gates.sh`.
"#;

fn split(layout: &LayoutContext) -> bool {
    layout.code_is_split()
}

/// One gate's lines for a script, `tag` naming the script in its messages.
fn gate_lines(layout: &LayoutContext, gate: &Gate, tag: &str, out: &mut String) {
    out.push_str(&format!(
        "\necho {} >&2\n",
        shell_quote(&format!("{tag}: gate {}", gate.label))
    ));
    match gate_commands(layout, gate) {
        Ok(steps) => {
            for argv in steps {
                let words: Vec<String> = argv.iter().map(|a| shell_quote(a)).collect();
                out.push_str(&format!("run {}\n", words.join(" ")));
            }
        }
        // The script still renders; the gate fails when CI reaches it.
        Err(e) => {
            let cause = format!("{e:#}").replace('\n', " ");
            out.push_str(&format!(
                "echo {} >&2\nexit 2\n",
                shell_quote(&format!("{tag}: gate {}: {cause}", gate.label))
            ));
        }
    }
}

/// One argv element as a single-quoted POSIX shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const PROLOGUE: &str = r#"#!/bin/sh
# Generated by specdev-analyzer from the `gates:` registry in
# specs/specdev/profile.yaml. Do not edit: `specdev-analyzer gate --emit`
# rewrites it, and `validate` reports a copy that differs from the registry.
# CI runs it: `sh scripts/specdev-gates.sh`. Set SPECDEV_ANALYZER to use an
# analyzer that is not on PATH.
set -u
cd "$(dirname "$0")/.." || exit 2
exec </dev/null

# Run one step; stop at the first non-zero exit and exit with it. A step
# that could not start (126, 127) exits 2, as the runner does.
run() {
  "$@"
  status=$?
  if [ "$status" -ne 0 ]; then
    if [ "$status" -eq 126 ] || [ "$status" -eq 127 ]; then
      echo "specdev-gates: $1 could not start: install it (on a CI runner, install it in CI's workflow)" >&2
      status=2
    fi
    echo "specdev-gates: step failed (exit $status): $*" >&2
    exit "$status"
  fi
}
"#;

/// The script for this project, rendered from its inlined registry. A
/// workspace root's script runs each member's script in declared order;
/// every other project's runs `check`, then each gate's steps. `None` when
/// the project has no inlined `profile.yaml` (nothing to render from).
pub fn render_script(layout: &LayoutContext) -> Result<Option<String>> {
    let mut out = String::from(PROLOGUE);
    if let Some(ws) = &layout.spec.workspace {
        out.push_str("\n# Each member checks itself; stop at the first failure.\n");
        for m in &ws.members {
            out.push_str(&format!(
                "echo {} >&2\n(cd {} && sh {SCRIPT_REL}) || exit $?\n",
                shell_quote(&format!("specdev-gates: member {}", m.name)),
                shell_quote(&m.path)
            ));
        }
        return Ok(Some(out));
    }
    let Some(profile) = Profile::load_inlined(&layout.specdev_root)? else {
        return Ok(None);
    };
    out.push_str(
        "\necho 'specdev-gates: check' >&2\nrun \"${SPECDEV_ANALYZER:-specdev-analyzer}\" check\n",
    );
    if split(layout) {
        // `check` reads the specs here; the gates run in the code root.
        out.push_str(&format!(
            "cd {} || exit 2\n",
            shell_quote(&layout.spec.paths.code)
        ));
    }
    for gate in &profile.gates_for(layout.spec.project.toolchain.as_deref())? {
        gate_lines(layout, gate, "specdev-gates", &mut out);
    }
    Ok(Some(out))
}

/// The compile-only script for the code root: the gates, no `check`, and
/// no path into the spec folder. `None` when the code root is the project
/// root, or when there is nothing to render from. A workspace root's
/// script runs the compile script of each member whose code root lies
/// inside its own; a member whose code lives elsewhere (with its specs, for
/// example) is checked only by the spec-side script.
pub fn render_compile_script(layout: &LayoutContext) -> Result<Option<String>> {
    if !split(layout) {
        return Ok(None);
    }
    let mut out = PROLOGUE
        .replacen(HEADER, COMPILE_HEADER, 1)
        .replace("specdev-gates:", "compile-gates:");
    if let Some(ws) = &layout.spec.workspace {
        out.push_str("\n# Each member checks itself; stop at the first failure.\n");
        for m in &ws.members {
            let member = LayoutContext::discover_unchecked(&layout.project_root.join(&m.path))?;
            if !split(&member) {
                continue;
            }
            let Ok(rel) = member.code_root.strip_prefix(&layout.code_root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            out.push_str(&format!(
                "echo {} >&2\n(cd {} && sh {COMPILE_SCRIPT_REL}) || exit $?\n",
                shell_quote(&format!("compile-gates: member {}", m.name)),
                shell_quote(if rel.is_empty() { "." } else { &rel })
            ));
        }
        return Ok(Some(out));
    }
    let Some(profile) = Profile::load_inlined(&layout.specdev_root)? else {
        return Ok(None);
    };
    for gate in &profile.gates_for(layout.spec.project.toolchain.as_deref())? {
        gate_lines(layout, gate, "compile-gates", &mut out);
    }
    Ok(Some(out))
}

/// What [`emit_script`] did.
#[derive(Debug, Default)]
pub struct EmitReport {
    /// The script was written (it was absent or differed).
    pub written: bool,
    /// `.gitattributes` was created with [`GITATTRIBUTES_LINE`].
    pub gitattributes_written: bool,
    /// `.gitattributes` exists without the line; the caller prints it.
    pub gitattributes_line_missing: bool,
    /// The compile-only script in the code root, when the project has one.
    pub compile: Option<CompileEmit>,
}

/// What [`emit_script`] did in the code root.
#[derive(Debug, Default)]
pub struct CompileEmit {
    /// [`COMPILE_SCRIPT_REL`] under the code root, as `[paths] code` names it.
    pub path: String,
    pub written: bool,
    pub gitattributes_written: bool,
    pub gitattributes_line_missing: bool,
}

/// Write `body` to `path` when it differs; `true` when written.
fn write_script(path: &std::path::Path, body: &str) -> Result<bool> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(body) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(true)
}

/// Create `<dir>/.gitattributes` with `line` when absent; an existing file
/// is never rewritten. Returns `(written, line_missing)`.
fn ensure_gitattributes(dir: &std::path::Path, line: &str) -> Result<(bool, bool)> {
    let attrs = dir.join(".gitattributes");
    match std::fs::read_to_string(&attrs) {
        Ok(text) => Ok((false, !text.lines().any(|l| l.trim() == line))),
        Err(_) => {
            std::fs::write(&attrs, format!("{line}\n"))
                .with_context(|| format!("writing {}", attrs.display()))?;
            Ok((true, false))
        }
    }
}

/// Write the script and, when `.gitattributes` is absent, that file with
/// [`GITATTRIBUTES_LINE`]. An existing `.gitattributes` is never rewritten.
pub fn emit_script(layout: &LayoutContext) -> Result<EmitReport> {
    let mut report = EmitReport::default();
    let Some(body) = render_script(layout)? else {
        return Ok(report);
    };
    report.written = write_script(&layout.project_root.join(SCRIPT_REL), &body)?;
    (
        report.gitattributes_written,
        report.gitattributes_line_missing,
    ) = ensure_gitattributes(&layout.project_root, GITATTRIBUTES_LINE)?;
    if let Some(body) = render_compile_script(layout)? {
        if !layout.code_root.is_dir() {
            bail!(
                "the code root {} (`[paths] code`) does not exist; check it out first",
                layout.code_root.display()
            );
        }
        let mut c = CompileEmit {
            path: format!("{}/{COMPILE_SCRIPT_REL}", layout.spec.paths.code),
            written: write_script(&layout.code_root.join(COMPILE_SCRIPT_REL), &body)?,
            ..Default::default()
        };
        (c.gitattributes_written, c.gitattributes_line_missing) =
            ensure_gitattributes(&layout.code_root, COMPILE_GITATTRIBUTES_LINE)?;
        report.compile = Some(c);
    }
    Ok(report)
}

/// The `validate` findings for the scripts (the spec-side one and, with a
/// separate code root, the compile-only one): a copy that differs from the
/// rendering is an error (CI would check the wrong thing); a missing one in
/// a 2.0 project that has something to run is a warning.
pub fn script_findings(layout: &LayoutContext) -> Vec<crate::report::Finding> {
    if layout.spec.format_major() < 2 {
        return Vec::new();
    }
    let has_gates = layout.spec.workspace.is_some()
        || Profile::load_inlined(&layout.specdev_root)
            .ok()
            .flatten()
            .is_some_and(|p| !p.gates.is_empty() || !p.toolchains.is_empty());
    if !has_gates {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(Some(body)) = render_script(layout) {
        out.extend(compare(&layout.project_root.join(SCRIPT_REL), &body));
    }
    if layout.code_root.is_dir() {
        if let Ok(Some(body)) = render_compile_script(layout) {
            out.extend(compare(&layout.code_root.join(COMPILE_SCRIPT_REL), &body));
        }
    }
    out
}

/// `GATE-SCRIPT-001` when the copy at `path` differs from `body`,
/// `GATE-SCRIPT-002` when it is missing.
fn compare(path: &std::path::Path, body: &str) -> Option<crate::report::Finding> {
    use crate::domain::Severity;
    use crate::report::Finding;
    match std::fs::read_to_string(path) {
        Ok(on_disk) if on_disk == body => None,
        Ok(_) => Some(
            Finding::new(
                "GATE-SCRIPT-001",
                Severity::Error,
                format!(
                    "{} differs from the gates registry (or the values its tokens read); CI would run the wrong \
                     checks. Run `specdev-analyzer gate --emit`",
                    path.display()
                ),
            )
            .with_file(path.display().to_string()),
        ),
        Err(_) => Some(Finding::new(
            "GATE-SCRIPT-002",
            Severity::Warning,
            format!("{} is missing; CI has no script to run. Run `specdev-analyzer gate --emit`", path.display()),
        )),
    }
}
