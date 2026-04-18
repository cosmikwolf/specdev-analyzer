//! Analyzer findings + formatting.
//!
//! A `Finding` is a single structural or graph-check failure. `Report`
//! collects findings and knows how to render them to a terminal with
//! severity-colored prefixes.

use std::fmt::Write;

use owo_colors::OwoColorize;

use crate::domain::Severity;

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub file: Option<String>,
    pub pointer: String,
    pub message: String,
}

impl Finding {
    pub fn new(
        rule: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            rule: rule.into(),
            severity,
            file: None,
            pointer: String::new(),
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
        self.findings
            .iter()
            .any(|f| f.severity == Severity::Error)
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
            let loc = match (&f.file, f.pointer.as_str()) {
                (Some(file), "") => format!("{file}"),
                (Some(file), ptr) => format!("{file}{ptr}"),
                (None, ptr) => ptr.to_string(),
            };
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
            let loc = match (&f.file, f.pointer.as_str()) {
                (Some(file), "") => file.clone(),
                (Some(file), ptr) => format!("{file}{ptr}"),
                (None, ptr) => ptr.to_string(),
            };
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
