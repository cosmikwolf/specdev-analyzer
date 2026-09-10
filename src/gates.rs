//! The compile-gate registry (`gates:` in the inlined `profile.yaml`):
//! the presence check with its refusal text, the YAML renderer that
//! writes the block to paste, and the runner that executes one declared
//! gate and records the result.
//!
//! A gate result is recorded per `(module, label)` (see
//! [`crate::gen_lineage`]); `verified` for a module means a current pass
//! row from **every** declared module-scope gate. That reading is only
//! meaningful when at least one module-scope gate is declared — over
//! zero gates it would hold for every module forever, which is the skip
//! bug the registry exists to remove. So codegen refuses to assemble a
//! module prompt for a project that declares none, in `gen --tail` and
//! whole-prompt `gen --module`, the two paths every codegen mode uses
//! (single-module and `--force` runs never consult `report gen-plan`).
//!
//! The refusal is written to be pasted from: when the plugin's copy of
//! the same profile declares a registry, the message prints that block
//! (the inlined profile is stale); when it does not — a user-authored
//! profile — the message prints the minimal shape to write. Nothing is
//! generated unverified; nothing is skipped.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::gen_lineage::{record_gate_run, GateRecord};
use crate::layout::LayoutContext;
use crate::manifest::{Gate, GateScope, Profile};
use crate::path::{eval_template, Scope};

/// Check that the inlined profile declares at least one module-scope
/// gate. `Ok(None)` when it does. `Ok(Some(message))` is the refusal to
/// print; the caller exits non-zero. `resolve_plugin` is called only
/// when the inlined registry is missing, to read the plugin's copy.
/// `caller` names the command in the message (`gen`, `gate`) — both
/// entrances enforce the one precondition and both print the same fix.
pub fn refuse_without_module_gate(
    layout: &LayoutContext,
    resolve_plugin: &dyn Fn() -> Result<PathBuf>,
    caller: &str,
) -> Result<Option<String>> {
    let inlined_path = layout.specdev_root.join("profile.yaml");
    let inlined = Profile::load_inlined(&layout.specdev_root)?;

    let subject = match &inlined {
        Some(p) if p.gates_declared() => return Ok(None),
        Some(p) if !p.gates.is_empty() => {
            // Batch-only registry: legal YAML, vacuous `verified`.
            let labels: Vec<String> = p.gates.iter().map(|g| format!("`{}`", g.label)).collect();
            return Ok(Some(format!(
                "{caller}: {} declares gates {} but none with `scope: module`. \
                 \"every declared module-scope gate passed\" would hold over zero gates \
                 for every module forever, so `{caller}` refuses. Add one gate with \
                 `scope: module` (the check that runs per module and writes one row per \
                 module in the run) and re-run.",
                inlined_path.display(),
                labels.join(", ")
            )));
        }
        Some(_) => format!("{} declares no `gates:`", inlined_path.display()),
        None => format!("{} does not exist", inlined_path.display()),
    };

    let plugin = resolve_plugin()?;
    let plugin_profile = plugin
        .join("profiles")
        .join(&layout.spec.project.profile)
        .join("profile.yaml");
    if plugin_profile.is_file() {
        let shipped = Profile::load(&plugin_profile)?;
        if shipped.gates_declared() {
            return Ok(Some(format!(
                "{caller}: {subject}, but the plugin's profile at {} declares a gate registry. \
                 The inlined profile is stale; without the registry no module here can \
                 read `verified`, and nothing this run wrote would be gated. Paste this \
                 block into {} (or re-bootstrap) and re-run:\n\n{}\n\
                 Labels are opaque; steps are argv lists run in order in the project root.",
                plugin_profile.display(),
                inlined_path.display(),
                render_gates_block(&shipped.gates),
            )));
        }
    }
    let plugin_state = if plugin_profile.is_file() {
        format!(
            "the plugin's profile at {} declares no module-scope gate either",
            plugin_profile.display()
        )
    } else {
        format!(
            "no plugin profile exists at {} to copy one from",
            plugin_profile.display()
        )
    };
    Ok(Some(format!(
        "{caller}: {subject}, and {plugin_state}. This project needs at least one gate with \
         `scope: module` before it runs — a current pass row from every module-scope \
         gate is what makes a module `verified`. Write a registry into {} with this \
         shape (label is an opaque name; each step is one argv list, run in order in \
         the project root, the first non-zero exit fails the gate) and re-run:\n\n\
         gates:\n  - label: <name>\n    scope: module\n    steps:\n      - [<executable>, <argument>, ...]\n",
        inlined_path.display()
    )))
}

// ---------------------------------------------------------------------------
// The runner: `specdev-analyzer gate --label <l> [--module <m>]…`.
// ---------------------------------------------------------------------------

/// What one `gate` invocation established.
pub struct GateRun {
    pub label: String,
    pub scope: GateScope,
    /// Every step exited zero.
    pub pass: bool,
    /// The process exit code the command must return: `0` on a pass, else
    /// the failing step's own exit code (`1` when a signal killed it, so
    /// "died" is never reported as success).
    pub code: i32,
    /// The rows written and the rows pruned, in one lineage save.
    pub record: GateRecord,
}

/// Run one declared gate and record its result.
///
/// The steps run in the **project root** (a workspace member's own root —
/// the codegen skill already walks per member), with the **inherited
/// environment** and **`stdin` closed**: a first-run tool that prompts
/// must fail, not wait forever. The first non-zero exit stops the gate.
/// Output is captured and printed: everything on a failure, and a step's
/// `stderr` on a pass when it is non-empty, so the compiler warnings a
/// human reader used to see are not lost. Both go to **this process's
/// stderr**, which keeps the first error in one stream whether the tool
/// wrote it to `stdout` (`tsc`) or `stderr` (`cargo`); the row lines the
/// caller prints stay on `stdout`.
///
/// The key is hashed **after** the run (the gate rewrites the lockfile,
/// and a first `next build` rewrites a paired target).
///
/// Failure classes, which differ in what they leave behind:
///
/// - a step's executable **cannot start**, a token resolves to nothing,
///   or the registry is refused → `Err`, **no row**. The gate did not
///   run, so the older pass rows for the same inputs stay true.
/// - a step **starts and exits non-zero**, including a network failure in
///   an install step → `Ok` with `pass: false` and fail rows. It ran; a
///   cheap re-gate is the safe direction.
/// - the gate ran but `gen-lineage.toml` cannot be written → `Err`, with
///   the gate's own output already printed.
///
/// No timeout in v1.
pub fn run_gate(
    layout: &LayoutContext,
    resolve_plugin: &dyn Fn() -> Result<PathBuf>,
    label: &str,
    modules: &[String],
) -> Result<GateRun> {
    // The same precondition codegen has, at the other entrance: without a
    // module-scope gate no module here can read `verified`, so a gate run
    // would write rows nothing can consume. Same four-variant message.
    if let Some(message) = refuse_without_module_gate(layout, resolve_plugin, "gate")? {
        bail!("{message}");
    }
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
                .map(|g| format!("`{}` ({})", g.label, g.scope.as_str()))
                .collect();
            anyhow!(
                "gate: `{label}` is not declared in {}. Declared: {}. \
                 The label is the registry's string, verbatim — it is also the \
                 row key, so a near miss would write rows nothing reads.",
                layout.specdev_root.join("profile.yaml").display(),
                declared.join(", ")
            )
        })?;

    // Which modules the rows are about. A module-scope gate with no
    // `--module` would write only a batch row and leave every module
    // unverified — silently doing nothing is worse than refusing.
    let row_modules: &[String] = match gate.scope {
        GateScope::Module => {
            if modules.is_empty() {
                bail!(
                    "gate: `{label}` is `scope: module` and needs at least one `--module`. \
                     A module-scope gate runs once and records one row per module in the \
                     run (the regenerate set plus the re-gate set)."
                );
            }
            check_modules_exist(layout, modules)?;
            modules
        }
        GateScope::Batch => {
            if !modules.is_empty() {
                eprintln!(
                    "gate: `{label}` is `scope: batch`; ignoring {} `--module` \
                     argument(s) — a batch row has no module.",
                    modules.len()
                );
            }
            &[]
        }
    };

    // Resolve every token before running anything: a token error must
    // leave the tree untouched and record nothing.
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
                     Nothing was recorded — the gate did not run, so the rows that \
                     stand are still true. Install the tool or fix `PATH`, then re-run.",
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

    // Every declared label, module-scope and batch alike: a row under a
    // label the registry no longer declares is dead weight in committed
    // lineage, and `gen-plan` already ignores it.
    let declared: Vec<String> = profile.gates.iter().map(|g| g.label.clone()).collect();
    let record =
        record_gate_run(layout, row_modules, label, pass, Some(&declared)).with_context(|| {
            format!(
                "gate `{label}` ran (result: {}) but its rows could not be written",
                if pass { "pass" } else { "fail" }
            )
        })?;

    Ok(GateRun {
        label: label.to_string(),
        scope: gate.scope,
        pass,
        code,
        record,
    })
}

/// A `--module` the spec set does not hold would write a row keyed on the
/// `none` input token that no reader can ever match — a silent no-op.
fn check_modules_exist(layout: &LayoutContext, modules: &[String]) -> Result<()> {
    let known = crate::prompt::list_module_names(layout)?;
    let unknown: Vec<&String> = modules.iter().filter(|m| !known.contains(m)).collect();
    if !unknown.is_empty() {
        bail!(
            "gate: no module spec under {} for {}. A row for a module that does not \
             exist is keyed on nothing and read by nothing.",
            layout.modules_dir.display(),
            unknown
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
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

/// Render a registry as the YAML block to paste under the top-level
/// `gates:` key. Argv elements are written as flow sequences; an element
/// that is not a safe plain scalar (a `${…}` token, a colon, a space) is
/// double-quoted so the block round-trips through a YAML parser.
pub fn render_gates_block(gates: &[Gate]) -> String {
    let mut out = String::from("gates:\n");
    for g in gates {
        out.push_str(&format!(
            "  - label: {}\n    scope: {}\n    steps:\n",
            yaml_scalar(&g.label),
            g.scope.as_str()
        ));
        for step in &g.steps {
            let argv: Vec<String> = step.iter().map(|a| yaml_scalar(a)).collect();
            out.push_str(&format!("      - [{}]\n", argv.join(", ")));
        }
    }
    out
}

/// A YAML scalar for one argv element: plain when every byte is in the
/// conservative safe set **and** a YAML parser reads the plain form back
/// as this string (`42`, `1e3`, `true`, `null`, `no` would come back as
/// a number, a bool or null and fail the schema's `type: string`);
/// double-quoted otherwise.
fn yaml_scalar(s: &str) -> String {
    let safe_bytes = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '=' | '+' | '-'))
        && (!s.starts_with('-') || is_flag(s));
    let reads_back_as_string = matches!(
        serde_yaml::from_str::<serde_yaml::Value>(s),
        Ok(serde_yaml::Value::String(ref v)) if v == s
    );
    if safe_bytes && reads_back_as_string {
        s.to_string()
    } else {
        let mut q = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => q.push_str("\\\""),
                '\\' => q.push_str("\\\\"),
                _ => q.push(c),
            }
        }
        q.push('"');
        q
    }
}

/// `--quiet`, `-p`: a plain scalar may start with `-` when the next
/// character is not a space (YAML 1.2 plain-scalar rule), and every
/// other byte is in the safe set.
fn is_flag(s: &str) -> bool {
    s.len() > 1
        && s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '=' | '+' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::GateScope;

    fn gate(label: &str, scope: GateScope, steps: &[&[&str]]) -> Gate {
        Gate {
            label: label.into(),
            scope,
            steps: steps
                .iter()
                .map(|s| s.iter().map(|a| a.to_string()).collect())
                .collect(),
        }
    }

    #[test]
    fn block_round_trips_through_yaml_including_tokens_and_flags() {
        let gates = vec![
            gate(
                "target-check",
                GateScope::Module,
                &[&[
                    "cargo",
                    "check",
                    "--quiet",
                    "--target",
                    "${root.platform.target_triple}",
                ]],
            ),
            gate(
                "fast",
                GateScope::Module,
                &[
                    &["pnpm", "install"],
                    &["pnpm", "exec", "vitest", "run", "--passWithNoTests"],
                ],
            ),
            gate("build", GateScope::Batch, &[&["sh", "-c", "echo a: b"]]),
        ];
        let block = render_gates_block(&gates);
        assert!(
            block.contains(
                "- [cargo, check, --quiet, --target, \"${root.platform.target_triple}\"]"
            ),
            "{block}"
        );
        assert!(block.contains("- [sh, -c, \"echo a: b\"]"), "{block}");
        let parsed: serde_yaml::Value = serde_yaml::from_str(&block).unwrap();
        let steps = &parsed["gates"][0]["steps"][0];
        assert_eq!(steps[4].as_str(), Some("${root.platform.target_triple}"));
        assert_eq!(
            parsed["gates"][1]["steps"][1][4].as_str(),
            Some("--passWithNoTests")
        );
        assert_eq!(
            parsed["gates"][2]["steps"][0][2].as_str(),
            Some("echo a: b")
        );
        assert_eq!(parsed["gates"][2]["scope"].as_str(), Some("batch"));
    }

    #[test]
    fn scalar_quoting_rules() {
        assert_eq!(yaml_scalar("cargo"), "cargo");
        assert_eq!(yaml_scalar("--no-fatal-infos"), "--no-fatal-infos");
        assert_eq!(yaml_scalar("-"), "\"-\"");
        assert_eq!(yaml_scalar("a b"), "\"a b\"");
        assert_eq!(yaml_scalar("${x}"), "\"${x}\"");
        assert_eq!(yaml_scalar("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(yaml_scalar(""), "\"\"");
        // Plain forms the analyzer's own YAML parser reads back as a
        // number, a bool or null must be quoted.
        for v in [
            "42", "1.0", "1e3", "0x1F", "true", "false", "null", "~", "-1",
        ] {
            assert_eq!(yaml_scalar(v), format!("\"{v}\""), "{v}");
        }
        // A flag-like value with digits stays plain: it reads back as a string.
        assert_eq!(yaml_scalar("--build-number=42"), "--build-number=42");
        // The invariant every element must satisfy: the rendered scalar
        // parses back to exactly this string.
        for v in [
            "42",
            "1.0",
            "true",
            "null",
            "no",
            "yes",
            "on",
            "off",
            "y",
            "n",
            "~",
            "-1",
            "-",
            "--quiet",
            "a b",
            "${x}",
            "a:b",
            "#c",
            "cargo",
            "",
            "say \"hi\"",
            "back\\slash",
        ] {
            let rendered = yaml_scalar(v);
            let back: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();
            assert_eq!(back.as_str(), Some(v), "{v} -> {rendered}");
        }
    }

    #[test]
    fn block_with_numeric_and_boolean_arguments_round_trips_as_strings() {
        let gates = vec![gate(
            "build",
            GateScope::Batch,
            &[&[
                "flutter",
                "build",
                "apk",
                "--build-number",
                "42",
                "--obfuscate",
                "true",
            ]],
        )];
        let block = render_gates_block(&gates);
        let parsed: serde_yaml::Value = serde_yaml::from_str(&block).unwrap();
        let step = &parsed["gates"][0]["steps"][0];
        assert_eq!(step[4].as_str(), Some("42"), "{block}");
        assert_eq!(step[6].as_str(), Some("true"), "{block}");
    }
}
