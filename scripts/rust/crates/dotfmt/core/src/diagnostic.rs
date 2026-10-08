use std::fmt::{self, Display};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Configuration,
    Syntax,
    Io,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            path: None,
            line: None,
            column: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_location(mut self, line: usize, column: Option<usize>) -> Self {
        self.line = Some(line);
        self.column = column;
        self
    }

    pub fn config(path: impl Into<PathBuf>, line: usize, message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Configuration, message)
            .with_path(path)
            .with_location(line, None)
    }
}

impl Display for Diagnostic {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(output, "{}:", path.display())?;
        }
        if let Some(line) = self.line {
            write!(output, "{line}:")?;
            if let Some(column) = self.column {
                write!(output, "{column}:")?;
            }
        }
        if self.path.is_some() || self.line.is_some() {
            write!(output, " ")?;
        }
        output.write_str(&self.message)
    }
}

impl std::error::Error for Diagnostic {}
