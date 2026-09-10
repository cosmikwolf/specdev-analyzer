//! The compile-gate registry (`gates:` in the inlined `profile.yaml`) as
//! codegen reads it: the presence check and its refusal text.
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

use anyhow::Result;

use crate::layout::LayoutContext;
use crate::manifest::{Gate, Profile};

/// Check that the inlined profile declares at least one module-scope
/// gate. `Ok(None)` when it does. `Ok(Some(message))` is the refusal to
/// print; the caller exits non-zero. `resolve_plugin` is called only
/// when the inlined registry is missing, to read the plugin's copy.
pub fn refuse_without_module_gate(
    layout: &LayoutContext,
    resolve_plugin: &dyn Fn() -> Result<PathBuf>,
) -> Result<Option<String>> {
    let inlined_path = layout.specdev_root.join("profile.yaml");
    let inlined = Profile::load_inlined(&layout.specdev_root)?;

    let subject = match &inlined {
        Some(p) if p.gates_declared() => return Ok(None),
        Some(p) if !p.gates.is_empty() => {
            // Batch-only registry: legal YAML, vacuous `verified`.
            let labels: Vec<String> = p.gates.iter().map(|g| format!("`{}`", g.label)).collect();
            return Ok(Some(format!(
                "gen: {} declares gates {} but none with `scope: module`. \
                 \"every declared module-scope gate passed\" would hold over zero gates \
                 for every module forever, so codegen refuses. Add one gate with \
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
                "gen: {subject}, but the plugin's profile at {} declares a gate registry. \
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
        "gen: {subject}, and {plugin_state}. Codegen needs at least one gate with \
         `scope: module` before it runs — a current pass row from every module-scope \
         gate is what makes a module `verified`. Write a registry into {} with this \
         shape (label is an opaque name; each step is one argv list, run in order in \
         the project root, the first non-zero exit fails the gate) and re-run:\n\n\
         gates:\n  - label: <name>\n    scope: module\n    steps:\n      - [<executable>, <argument>, ...]\n",
        inlined_path.display()
    )))
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
