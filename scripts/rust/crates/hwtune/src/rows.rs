use std::process::ExitCode;

use workstation::Style;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ok,
    Bad,
    Warn,
    Note,
}

#[derive(Debug)]
pub struct Row {
    pub kind: Kind,
    pub label: String,
    pub summary: String,
    pub details: Vec<String>,
}

impl Row {
    pub fn new(kind: Kind, label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    pub fn ok(label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(Kind::Ok, label, summary)
    }

    pub fn bad(label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(Kind::Bad, label, summary)
    }

    pub fn warn(label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(Kind::Warn, label, summary)
    }

    pub fn note(label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(Kind::Note, label, summary)
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

pub fn mark(kind: Kind, style: &Style) -> String {
    match kind {
        Kind::Ok => style.green("ok  "),
        Kind::Bad => style.red("bad "),
        Kind::Warn => style.code("33", "warn"),
        Kind::Note => style.dim("note"),
    }
}

pub fn render(rows: &[Row], style: &Style) -> String {
    let width = rows.iter().map(|row| row.label.len()).max().unwrap_or(0);
    let mut out = String::new();
    for row in rows {
        out.push_str(&format!(
            "  {}  {:<width$}  {}\n",
            mark(row.kind, style),
            row.label,
            row.summary
        ));
        for detail in &row.details {
            out.push_str(&format!("        {:<width$}  {}\n", "", style.dim(detail)));
        }
    }
    out
}

pub fn counts(rows: &[Row]) -> (usize, usize, usize) {
    let count = |kind: Kind| rows.iter().filter(|row| row.kind == kind).count();
    (count(Kind::Ok), count(Kind::Bad), count(Kind::Warn))
}

pub fn exit_code(rows: &[Row]) -> ExitCode {
    if rows.iter().any(|row| row.kind == Kind::Bad) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
#[path = "../tests/unit/rows_tests.rs"]
mod tests;
