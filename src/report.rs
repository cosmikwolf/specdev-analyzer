//! Analyzer findings + formatting.
//!
//! A `Finding` is a single structural or graph-check failure. `Report`
//! collects findings and knows how to render them to a terminal with
//! severity-colored prefixes.

use std::fmt::Write;

use owo_colors::OwoColorize;

use crate::domain::Severity;

/// Source-location tuple carried by findings. `line` is best-effort: the TOML
/// crate only exposes spans when loaders opt into `toml::Spanned`, which we
/// haven't threaded through every call site. Reduction-execution errors (phase
/// 14) set it explicitly; pre-reduction findings leave it `None`.
#[derive(Debug, Clone, Default)]
pub struct Provenance {
    pub source_file: String,
    pub pointer: String,
    pub line: Option<u32>,
}

impl Provenance {
    pub fn new(source_file: impl Into<String>, pointer: impl Into<String>) -> Self {
        Self {
            source_file: source_file.into(),
            pointer: pointer.into(),
            line: None,
        }
    }

    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub file: Option<String>,
    pub pointer: String,
    pub line: Option<u32>,
    pub message: String,
}

impl Finding {
    pub fn new(rule: impl Into<String>, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            severity,
            file: None,
            pointer: String::new(),
            line: None,
            message: message.into(),
        }
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_pointer(mut self, pointer: impl Into<String>) -> Self {
        self.pointer = pointer.into();
        self
    }

    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    pub fn with_provenance(mut self, prov: Provenance) -> Self {
        self.file = Some(prov.source_file);
        self.pointer = prov.pointer;
        self.line = prov.line;
        self
    }
}

#[derive(Debug, Default)]
pub struct Report {
    findings: Vec<Finding>,
}

impl Report {
    pub fn push(&mut self, f: Finding) {
        self.findings.push(f);
    }

    pub fn extend<I: IntoIterator<Item = Finding>>(&mut self, iter: I) {
        self.findings.extend(iter);
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Plain-text render (no ANSI). Used by snapshot tests.
    pub fn render_plain(&self) -> String {
        let mut out = String::new();
        for f in &self.findings {
            let sev = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            let loc = render_loc(&f.file, f.pointer.as_str(), f.line);
            let rule = if f.rule.is_empty() {
                String::new()
            } else {
                format!("[{}] ", f.rule)
            };
            let _ = writeln!(out, "{sev}: {rule}{loc}: {}", f.message);
        }
        out
    }

    /// ANSI-colored render for terminal output.
    pub fn render_colored(&self) -> String {
        let mut out = String::new();
        for f in &self.findings {
            let sev = match f.severity {
                Severity::Error => "error".red().bold().to_string(),
                Severity::Warning => "warning".yellow().bold().to_string(),
                Severity::Info => "info".blue().bold().to_string(),
            };
            let loc = render_loc(&f.file, f.pointer.as_str(), f.line);
            let rule = if f.rule.is_empty() {
                String::new()
            } else {
                format!("[{}] ", f.rule.cyan())
            };
            let _ = writeln!(out, "{sev}: {rule}{}: {}", loc.dimmed(), f.message);
        }
        out
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut e = 0;
        let mut w = 0;
        let mut i = 0;
        for f in &self.findings {
            match f.severity {
                Severity::Error => e += 1,
                Severity::Warning => w += 1,
                Severity::Info => i += 1,
            }
        }
        (e, w, i)
    }
}

/// Render the `{file}:{line}{pointer}` prefix for a finding. Line is additive:
/// when absent, output is byte-identical to the pre-phase-14 format
/// (`{file}{pointer}` or `{pointer}` alone).
fn render_loc(file: &Option<String>, pointer: &str, line: Option<u32>) -> String {
    match (file, pointer, line) {
        (Some(f), "", None) => f.clone(),
        (Some(f), ptr, None) => format!("{f}{ptr}"),
        (Some(f), "", Some(n)) => format!("{f}:{n}"),
        (Some(f), ptr, Some(n)) => format!("{f}:{n}{ptr}"),
        (None, ptr, None) => ptr.to_string(),
        (None, ptr, Some(n)) => format!(":{n}{ptr}"),
    }
}
