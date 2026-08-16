use serde::Serialize;
use serde_json::{Map, Value};

use crate::{ByteRange, LineIndex, RepoPath, SourceText};

/// Analysis stage owning a diagnostic code. Stage order is the first
/// diagnostic sort key and is frozen by the diagnostics contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(into = "&'static str")]
pub enum Stage {
    Lex,
    Parse,
    Schema,
    Theme,
    Resolve,
    Graph,
    Discovery,
    Deploy,
    Lock,
    Bind,
    Observe,
    Apply,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lex => "lex",
            Self::Parse => "parse",
            Self::Schema => "schema",
            Self::Theme => "theme",
            Self::Resolve => "resolve",
            Self::Graph => "graph",
            Self::Discovery => "discovery",
            Self::Deploy => "deploy",
            Self::Lock => "lock",
            Self::Bind => "bind",
            Self::Observe => "observe",
            Self::Apply => "apply",
        }
    }

    /// The frozen stage order used for diagnostic sorting.
    pub fn order(self) -> u8 {
        match self {
            Self::Lex => 1,
            Self::Parse => 2,
            Self::Schema => 3,
            Self::Theme => 4,
            Self::Resolve => 5,
            Self::Graph => 6,
            Self::Discovery => 7,
            Self::Deploy => 8,
            Self::Lock => 9,
            Self::Bind => 10,
            Self::Observe => 11,
            Self::Apply => 12,
        }
    }
}

impl From<Stage> for &'static str {
    fn from(stage: Stage) -> Self {
        stage.as_str()
    }
}

/// The only two diagnostic severities. Informational state is shown through
/// hover, hints, and command output, never as a third severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum Severity {
    Error,
    Warning,
}

/// Where a diagnostic applies: authored source, a generated artifact, or
/// machine state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Source,
    Generated,
    Machine,
}

/// A source span in the frozen record shape: raw zero-based half-open byte
/// range plus derived one-based Unicode-scalar coordinates (pseudo-scalar
/// columns for malformed UTF-8, per ADR 0002).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Span {
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u64,
    pub start_column: u64,
    pub end_line: u64,
    pub end_column: u64,
}

impl Span {
    /// Derives the full record from a raw byte range and the shared line
    /// index.
    pub fn new(path: &RepoPath, range: ByteRange, text: &SourceText, index: &LineIndex) -> Self {
        let bytes = text.as_bytes();
        let start = index.line_col(bytes, range.start());
        let end = index.line_col(bytes, range.end());
        Self {
            path: path.as_str().to_owned(),
            start_byte: range.start(),
            end_byte: range.end(),
            start_line: start.line,
            start_column: start.column,
            end_line: end.line,
            end_column: end.column,
        }
    }
}

/// One edit of a structured fix-it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FixEdit {
    pub path: String,
    pub range: FixEditRange,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FixEditRange {
    pub start_byte: u64,
    pub end_byte: u64,
}

/// A structured fix-it. `applicability` is `safe` or `preview_only`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Fix {
    pub title: String,
    pub applicability: String,
    pub edits: Vec<FixEdit>,
}

/// The frozen diagnostic record. Optional `detail` and `fix` are omitted
/// when absent; every other field is always present, using the contract's
/// empty-value rules.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub scope: Scope,
    pub stage: Stage,
    pub severity: Severity,
    pub summary: String,
    pub remedy: String,
    pub primary_span: Span,
    pub related_spans: Vec<Span>,
    pub semantic_identity: String,
    pub provenance: Vec<String>,
    pub expected: Map<String, Value>,
    pub actual: Map<String, Value>,
    pub secret_redacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

impl Diagnostic {
    /// Builds a source-scope diagnostic with the contract's empty-value
    /// defaults. Structured data and detail are added by the emitter.
    pub fn new(
        code: &'static str,
        stage: Stage,
        severity: Severity,
        summary: impl Into<String>,
        remedy: impl Into<String>,
        primary_span: Span,
    ) -> Self {
        Self {
            code: code.to_owned(),
            scope: Scope::Source,
            stage,
            severity,
            summary: summary.into(),
            remedy: remedy.into(),
            primary_span,
            related_spans: Vec::new(),
            semantic_identity: String::new(),
            provenance: Vec::new(),
            expected: Map::new(),
            actual: Map::new(),
            secret_redacted: false,
            detail: None,
            fix: None,
        }
    }

    pub fn with_detail(mut self, detail: &'static str) -> Self {
        self.detail = Some(detail.to_owned());
        self
    }

    pub fn with_expected(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.expected.insert(key.to_owned(), value.into());
        self
    }

    pub fn with_actual(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.actual.insert(key.to_owned(), value.into());
        self
    }
}

/// Canonical diagnostic ordering: stage, path bytes, primary start byte,
/// then code.
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        left.stage
            .order()
            .cmp(&right.stage.order())
            .then_with(|| {
                left.primary_span
                    .path
                    .as_bytes()
                    .cmp(right.primary_span.path.as_bytes())
            })
            .then_with(|| {
                left.primary_span
                    .start_byte
                    .cmp(&right.primary_span.start_byte)
            })
            .then_with(|| left.code.cmp(&right.code))
    });
}

/// Maximum lexer and parser diagnostics retained per file (ADR 0001).
pub const RETAINED_DIAGNOSTIC_LIMIT: usize = 512;

/// Collects diagnostics for one file, enforcing the retained-diagnostic
/// limit. When the limit is reached, further diagnostics are counted and
/// suppressed, and the last retained diagnostic reports the suppressed count
/// in structured data. The cap never changes compiler validity or ordering.
pub struct DiagnosticSink<'a> {
    path: &'a RepoPath,
    text: &'a SourceText,
    index: &'a LineIndex,
    retained: Vec<Diagnostic>,
    suppressed: u64,
}

impl<'a> DiagnosticSink<'a> {
    pub fn new(path: &'a RepoPath, text: &'a SourceText, index: &'a LineIndex) -> Self {
        Self {
            path,
            text,
            index,
            retained: Vec::new(),
            suppressed: 0,
        }
    }

    pub fn span(&self, range: ByteRange) -> Span {
        Span::new(self.path, range, self.text, self.index)
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        if self.retained.len() < RETAINED_DIAGNOSTIC_LIMIT - 1 {
            self.retained.push(diagnostic);
            return;
        }
        self.suppressed += 1;
        if self.suppressed == 1 {
            let span = diagnostic.primary_span.clone();
            let marker = Diagnostic::new(
                "parse/syntax",
                Stage::Parse,
                Severity::Error,
                "further diagnostics were suppressed",
                "reduce the number of syntax errors in this file",
                span,
            )
            .with_detail("resource_limit")
            .with_actual("suppressed_diagnostics", self.suppressed);
            self.retained.push(marker);
        } else if let Some(marker) = self.retained.last_mut() {
            marker
                .actual
                .insert("suppressed_diagnostics".to_owned(), self.suppressed.into());
        }
    }

    pub fn suppressed(&self) -> u64 {
        self.suppressed
    }

    /// The retained diagnostics in canonical order.
    pub fn finish(mut self) -> Vec<Diagnostic> {
        sort_diagnostics(&mut self.retained);
        self.retained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (RepoPath, SourceText, LineIndex) {
        let path = RepoPath::new("fixture.dotfile").unwrap();
        let text = SourceText::from("foo\nbar\n");
        let index = LineIndex::new(text.as_bytes());
        (path, text, index)
    }

    #[test]
    fn span_derives_coordinates() {
        let (path, text, index) = fixture();
        let span = Span::new(&path, ByteRange::new(4, 7, 8).unwrap(), &text, &index);
        assert_eq!(span.path, "fixture.dotfile");
        assert_eq!(
            (
                span.start_line,
                span.start_column,
                span.end_line,
                span.end_column
            ),
            (2, 1, 2, 4)
        );
    }

    #[test]
    fn sorting_is_canonical() {
        let (path, text, index) = fixture();
        let range = ByteRange::new(0, 3, 8).unwrap();
        let mut diagnostics = vec![
            Diagnostic::new(
                "parse/syntax",
                Stage::Parse,
                Severity::Error,
                "p",
                "r",
                Span::new(&path, range, &text, &index),
            ),
            Diagnostic::new(
                "lex/token",
                Stage::Lex,
                Severity::Error,
                "l",
                "r",
                Span::new(&path, ByteRange::new(4, 5, 8).unwrap(), &text, &index),
            ),
        ];
        sort_diagnostics(&mut diagnostics);
        assert_eq!(diagnostics[0].code, "lex/token");
        assert_eq!(diagnostics[1].code, "parse/syntax");
    }

    #[test]
    fn sink_caps_and_reports_suppressed_count() {
        let (path, text, index) = fixture();
        let mut sink = DiagnosticSink::new(&path, &text, &index);
        let range = ByteRange::new(0, 1, 8).unwrap();
        for _ in 0..600 {
            sink.push(Diagnostic::new(
                "lex/token",
                Stage::Lex,
                Severity::Error,
                "x",
                "r",
                Span::new(&path, range, &text, &index),
            ));
        }
        let diagnostics = sink.finish();
        assert_eq!(diagnostics.len(), RETAINED_DIAGNOSTIC_LIMIT);
        let last = diagnostics.last().unwrap();
        assert_eq!(last.code, "parse/syntax");
        assert_eq!(last.detail.as_deref(), Some("resource_limit"));
        assert_eq!(
            last.actual
                .get("suppressed_diagnostics")
                .and_then(Value::as_u64),
            Some(89)
        );
    }

    #[test]
    fn serializes_the_frozen_record_shape() {
        let (path, text, index) = fixture();
        let diagnostic = Diagnostic::new(
            "lex/token",
            Stage::Lex,
            Severity::Error,
            "summary",
            "remedy",
            Span::new(&path, ByteRange::new(0, 3, 8).unwrap(), &text, &index),
        );
        let value = serde_json::to_value(&diagnostic).unwrap();
        let object = value.as_object().unwrap();
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "actual",
                "code",
                "expected",
                "primary_span",
                "provenance",
                "related_spans",
                "remedy",
                "scope",
                "secret_redacted",
                "semantic_identity",
                "severity",
                "stage",
                "summary"
            ]
        );
        assert!(!object.contains_key("detail"));
        assert!(!object.contains_key("fix"));
        assert_eq!(object["stage"], "lex");
        assert_eq!(object["scope"], "source");
        assert_eq!(object["severity"], "Error");
    }
}
