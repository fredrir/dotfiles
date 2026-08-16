use std::collections::HashMap;

use dotfile_source::{
    ByteRange, Diagnostic, LineIndex, RepoPath, Severity, SourceText, Span, Stage, sort_diagnostics,
};
use dotfile_syntax::{Atom, Block, Cst, Element, Entry, NamedEntry, NodeKind, Value};
use unicode_normalization::UnicodeNormalization;

use crate::model::{CanonicalDecimal, Spanned, ThemeReference};

pub(super) struct Context<'a> {
    path: &'a RepoPath,
    source: &'a SourceText,
    index: &'a LineIndex,
    cst: &'a Cst,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Context<'a> {
    pub(super) fn new(
        path: &'a RepoPath,
        source: &'a SourceText,
        index: &'a LineIndex,
        cst: &'a Cst,
    ) -> Self {
        Self {
            path,
            source,
            index,
            cst,
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn finish(mut self) -> Vec<Diagnostic> {
        sort_diagnostics(&mut self.diagnostics);
        self.diagnostics
    }

    pub(super) fn span(&self, range: ByteRange) -> Span {
        Span::new(self.path, range, self.source, self.index)
    }

    pub(super) fn schema(&mut self, range: ByteRange, summary: impl Into<String>) {
        self.diagnostics.push(Diagnostic::new(
            "schema/context",
            Stage::Schema,
            Severity::Error,
            summary,
            "use the value and structure required by this theme schema",
            self.span(range),
        ));
    }

    pub(super) fn duplicate(
        &mut self,
        range: ByteRange,
        previous: &[ByteRange],
        summary: impl Into<String>,
    ) {
        let mut diagnostic = Diagnostic::new(
            "schema/duplicate",
            Stage::Schema,
            Severity::Error,
            summary,
            "remove or rename the duplicate theme entry",
            self.span(range),
        );
        diagnostic.related_spans = previous
            .iter()
            .copied()
            .map(|range| self.span(range))
            .collect();
        self.diagnostics.push(diagnostic);
    }

    pub(super) fn map_conflict(
        &mut self,
        range: ByteRange,
        previous: &[ByteRange],
        summary: impl Into<String>,
    ) {
        let mut diagnostic = Diagnostic::new(
            "theme/map",
            Stage::Theme,
            Severity::Error,
            summary,
            "remove the conflicting application-map entry",
            self.span(range),
        );
        diagnostic.related_spans = previous
            .iter()
            .copied()
            .map(|range| self.span(range))
            .collect();
        self.diagnostics.push(diagnostic);
    }

    pub(super) fn missing(&mut self, range: ByteRange, field: &str) {
        let anchor = if range.start() == 0 && range.end() == self.source.len() {
            range.end()
        } else {
            self.source
                .slice(range)
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace())
                .filter(|index| self.source.slice(range)[*index] == b'}')
                .map_or(range.end(), |index| range.start() + index as u64)
        };
        let insertion = ByteRange::at(anchor, self.source.len())
            .expect("missing-field insertion anchor is source-bounded");
        let mut diagnostic = Diagnostic::new(
            "schema/context",
            Stage::Schema,
            Severity::Error,
            format!("missing required theme field `{field}`"),
            "add the required field at this insertion point",
            self.span(insertion),
        );
        diagnostic.related_spans.push(self.span(range));
        self.diagnostics.push(diagnostic);
    }

    pub(super) fn reject_non_named(&mut self, entry: Entry<'_>) {
        self.schema(
            entry.range(),
            "this entry form is forbidden in a theme definition",
        );
    }

    pub(super) fn plain_named<'b>(&mut self, entry: Entry<'b>) -> Option<NamedEntry<'b>> {
        if matches!(entry, Entry::Error(_)) {
            return None;
        }
        let directly_poisoned = self.directly_poisoned(entry.node_id());
        let Entry::Named(named) = entry else {
            if directly_poisoned {
                return None;
            }
            self.reject_non_named(entry);
            return None;
        };
        if named.optional() {
            self.schema(
                named.range(),
                "optional entries are forbidden in a theme definition",
            );
        }
        Some(named)
    }

    pub(super) fn require_block<'b>(
        &mut self,
        entry: NamedEntry<'b>,
        field: &str,
    ) -> Option<Block<'b>> {
        match (entry.block(), entry.value()) {
            _ if entry.optional() => None,
            (Some(block), None) => Some(block),
            _ if self.directly_poisoned(entry.node_id()) => None,
            _ => {
                self.schema(
                    entry.range(),
                    format!("theme field `{field}` must be a block"),
                );
                None
            }
        }
    }

    pub(super) fn require_value<'b>(
        &mut self,
        entry: NamedEntry<'b>,
        field: &str,
    ) -> Option<Value<'b>> {
        match (entry.value(), entry.block()) {
            _ if entry.optional() => None,
            (Some(value), None) => Some(value),
            _ if self.directly_poisoned(entry.node_id()) => None,
            _ => {
                self.schema(
                    entry.range(),
                    format!("theme field `{field}` must be an assignment"),
                );
                None
            }
        }
    }

    pub(super) fn literal_string(
        &mut self,
        value: Value<'_>,
        field: &str,
    ) -> Option<Spanned<String>> {
        let Value::String(expression) = value else {
            self.schema(
                value.range(),
                format!("theme field `{field}` must be a quoted string"),
            );
            return None;
        };
        let atoms = expression.atoms();
        let [atom] = atoms.as_slice() else {
            self.schema(
                expression.range(),
                format!("theme field `{field}` must be one literal string token"),
            );
            return None;
        };
        let Atom::String { data, range, .. } = atom else {
            self.schema(
                atom.range(),
                format!("theme field `{field}` must be one literal string token"),
            );
            return None;
        };
        let Some(data) = data else {
            // The lexer owns malformed escape, interpolation, and encoding
            // diagnostics. Do not cascade a schema error from poisoned string
            // data while continuing to validate independent entries.
            return None;
        };
        if data.has_interpolation() {
            self.schema(
                *range,
                format!("interpolation is forbidden in theme field `{field}`"),
            );
            return None;
        }
        let decoded = data.decoded();
        if decoded.chars().any(is_line_break) {
            self.schema(
                *range,
                format!("theme field `{field}` must be a one-line string"),
            );
            return None;
        }
        if !decoded.nfc().eq(decoded.chars()) {
            self.schema(
                *range,
                format!("theme field `{field}` must be NFC-normalized"),
            );
            return None;
        }
        Some(Spanned::new(decoded, *range))
    }

    pub(super) fn nonempty_string(
        &mut self,
        value: Value<'_>,
        field: &str,
    ) -> Option<Spanned<String>> {
        let string = self.literal_string(value, field)?;
        if string.value.is_empty() {
            self.schema(
                string.span,
                format!("theme field `{field}` must not be empty"),
            );
            return None;
        }
        Some(string)
    }

    pub(super) fn reference(&mut self, value: Value<'_>, field: &str) -> Option<ThemeReference> {
        let Value::Reference(reference) = value else {
            self.schema(
                value.range(),
                format!("theme field `{field}` must be a bare reference"),
            );
            return None;
        };
        Some(ThemeReference {
            name: reference.name()?.to_owned(),
            span: reference.name_range().unwrap_or_else(|| reference.range()),
        })
    }

    pub(super) fn canonical_decimal(
        &mut self,
        value: Value<'_>,
        field: &str,
        constraint: DecimalConstraint,
    ) -> Option<Spanned<CanonicalDecimal>> {
        let string = self.literal_string(value, field)?;
        if !is_canonical_decimal(&string.value)
            || match constraint {
                DecimalConstraint::Positive => is_zero(&string.value),
                DecimalConstraint::ZeroToOne => !is_zero_to_one(&string.value),
            }
        {
            let expectation = match constraint {
                DecimalConstraint::Positive => "a positive canonical decimal",
                DecimalConstraint::ZeroToOne => "a canonical decimal from zero through one",
            };
            self.schema(
                string.span,
                format!("theme field `{field}` must be {expectation}"),
            );
            return None;
        }
        Some(Spanned::new(CanonicalDecimal(string.value), string.span))
    }

    pub(super) fn track_unique(
        &mut self,
        seen: &mut HashMap<String, Vec<ByteRange>>,
        key: &str,
        range: ByteRange,
        label: &str,
    ) -> bool {
        if let Some(previous) = seen.get_mut(key) {
            self.duplicate(range, previous, format!("duplicate {label} `{key}`"));
            previous.push(range);
            false
        } else {
            seen.insert(key.to_owned(), vec![range]);
            true
        }
    }

    pub(super) fn directly_poisoned(&self, node: dotfile_syntax::NodeId) -> bool {
        self.cst.children(node).iter().any(|child| match child {
            Element::Missing { .. } => true,
            Element::Node(child) => self.cst.node_kind(*child) == NodeKind::Error,
            Element::Token(_) => false,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum DecimalConstraint {
    Positive,
    ZeroToOne,
}

fn is_line_break(scalar: char) -> bool {
    matches!(
        scalar,
        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

fn is_canonical_decimal(text: &str) -> bool {
    let Some((integer, fraction)) = text.split_once('.').map_or_else(
        || Some((text, None)),
        |(integer, fraction)| Some((integer, Some(fraction))),
    ) else {
        return false;
    };
    if integer.is_empty() || !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if integer.len() > 1 && integer.starts_with('0') {
        return false;
    }
    match fraction {
        None => true,
        Some(fraction) => {
            !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
                && !fraction.ends_with('0')
        }
    }
}

fn is_zero(text: &str) -> bool {
    text == "0"
}

fn is_zero_to_one(text: &str) -> bool {
    text == "0" || text == "1" || text.starts_with("0.")
}

pub(super) fn name_spanned(entry: NamedEntry<'_>) -> Option<Spanned<String>> {
    Some(Spanned::new(
        entry.name()?.to_owned(),
        entry.name_range().unwrap_or_else(|| entry.range()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_decimal_grammar() {
        for valid in ["0", "1", "10", "0.1", "0.72", "12.345"] {
            assert!(is_canonical_decimal(valid), "{valid}");
        }
        for invalid in ["", "+1", "-1", "01", ".5", "1.", "1.0", "1e2", "0.20"] {
            assert!(!is_canonical_decimal(invalid), "{invalid}");
        }
        for valid in ["0", "1", "0.1", "0.999"] {
            assert!(is_zero_to_one(valid), "{valid}");
        }
        for invalid in ["2", "1.1", "10"] {
            assert!(!is_zero_to_one(invalid), "{invalid}");
        }
    }
}
