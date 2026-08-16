//! Canonical formatter for authored `.dotfile` version 1 sources.
//!
//! The formatter consumes the one lossless syntax tree and a path-selected
//! [`FormatSchema`].  It refuses lexer/parser errors, but deliberately does
//! not require schema lowering to succeed: unknown entries, duplicates, and
//! otherwise invalid generic syntax still receive deterministic formatting.

mod comments;
mod render;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub use dotfile_schema::{
    Domain, DomainClassifier, FormatSchema, GroupLayout, GroupLayoutEntry, PathClassification,
    format_schema,
};
use dotfile_source::{Diagnostic, RepoPath, SourceText};
use dotfile_syntax::{Parse, parse};

/// Canonical authored-source bytes and whether they differ from the input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatOutput {
    pub bytes: Vec<u8>,
    pub changed: bool,
}

/// A refusal at the formatter boundary.
#[derive(Clone, Debug)]
pub enum FormatError {
    /// The authoritative lexer/parser rejected the source.  Diagnostics are
    /// returned unchanged so callers can publish the original tight spans.
    InvalidSyntax { diagnostics: Vec<Diagnostic> },
    /// A `Parse` built for another repository path or source bytes was supplied to
    /// [`format_parsed`].
    MismatchedParse,
    /// The compiler-owned generated lock is never rewritten by this crate.
    GeneratedLockReadOnly,
    /// The encrypted YAML variable store is not `.dotfile` source.
    UnsupportedDomain { domain: Domain },
    /// The path is payload rather than `.dotfile` language input.
    UnclassifiedPath { path: RepoPath },
    /// A schema-explicit caller supplied a schema other than the one selected
    /// by the authoritative path classifier.
    SchemaMismatch {
        path: RepoPath,
        expected: Domain,
        actual: Domain,
    },
}

impl Display for FormatError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSyntax { .. } => formatter.write_str("source has lexer or parser errors"),
            Self::MismatchedParse => {
                formatter.write_str("the syntax tree was parsed from a different path or source")
            }
            Self::GeneratedLockReadOnly => formatter.write_str(
                "package.lock.dotfile is compiler-owned; regenerate it with `dotfile lock`",
            ),
            Self::UnsupportedDomain { domain } => {
                write!(
                    formatter,
                    "the {domain} domain is not authored .dotfile source"
                )
            }
            Self::UnclassifiedPath { path } => {
                write!(
                    formatter,
                    "repository path `{path}` is not .dotfile formatter input"
                )
            }
            Self::SchemaMismatch {
                path,
                expected,
                actual,
            } if expected == actual => write!(
                formatter,
                "supplied {actual} schema is not the canonical descriptor for `{path}`"
            ),
            Self::SchemaMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "repository path `{path}` selects {expected}, not supplied schema {actual}"
            ),
        }
    }
}

impl Error for FormatError {}

/// Parses and canonically formats one authored source document.
///
/// Known paths use their classified domain schema. An unknown repository
/// `.dotfile` remains valid generic language input and is formatted with
/// conservative source-order preservation. Non-`.dotfile` payloads refuse.
pub fn format_source(
    path: &RepoPath,
    source: &SourceText,
    classifier: &DomainClassifier,
) -> Result<FormatOutput, FormatError> {
    let schema = schema_for_path(path, classifier)?;
    let parsed = parse(path, source);
    format_parsed_inner(path, source, &parsed, schema)
}

/// Checked schema-explicit form of [`format_source`].
///
/// This is useful when the caller already retained the path-selected schema;
/// it can never override the classifier's authoritative domain.
pub fn format_source_with_schema(
    path: &RepoPath,
    source: &SourceText,
    classifier: &DomainClassifier,
    schema: &FormatSchema,
) -> Result<FormatOutput, FormatError> {
    verify_schema(path, classifier, schema)?;
    let parsed = parse(path, source);
    format_parsed_inner(path, source, &parsed, Some(schema))
}

/// Canonically formats a document using an already-built authoritative CST.
pub fn format_parsed(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
    classifier: &DomainClassifier,
) -> Result<FormatOutput, FormatError> {
    check_parse_identity(path, source, parsed)?;
    let schema = schema_for_path(path, classifier)?;
    format_parsed_inner(path, source, parsed, schema)
}

/// Checked schema-explicit form of [`format_parsed`].
pub fn format_parsed_with_schema(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
    classifier: &DomainClassifier,
    schema: &FormatSchema,
) -> Result<FormatOutput, FormatError> {
    check_parse_identity(path, source, parsed)?;
    verify_schema(path, classifier, schema)?;
    format_parsed_inner(path, source, parsed, Some(schema))
}

fn format_parsed_inner(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
    schema: Option<&FormatSchema>,
) -> Result<FormatOutput, FormatError> {
    check_parse_identity(path, source, parsed)?;
    if path.as_str() == "package.lock.dotfile"
        || schema.is_some_and(|schema| schema.domain == Domain::GeneratedLock)
    {
        return Err(FormatError::GeneratedLockReadOnly);
    }
    if path.as_str() == "vars.enc.yaml"
        || schema.is_some_and(|schema| schema.domain == Domain::TemplateVariables)
    {
        return Err(FormatError::UnsupportedDomain {
            domain: Domain::TemplateVariables,
        });
    }
    if parsed.has_errors() {
        return Err(FormatError::InvalidSyntax {
            diagnostics: parsed.diagnostics().to_vec(),
        });
    }

    let bytes = render::format(source, parsed, schema);
    Ok(FormatOutput {
        changed: bytes != source.as_bytes(),
        bytes,
    })
}

fn check_parse_identity(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
) -> Result<(), FormatError> {
    if parsed.was_parsed_from(path, source) {
        Ok(())
    } else {
        Err(FormatError::MismatchedParse)
    }
}

/// Returns whether authored source already equals its canonical bytes.
pub fn is_canonical(
    path: &RepoPath,
    source: &SourceText,
    parsed: &Parse,
    classifier: &DomainClassifier,
) -> Result<bool, FormatError> {
    Ok(!format_parsed(path, source, parsed, classifier)?.changed)
}

fn schema_for_path(
    path: &RepoPath,
    classifier: &DomainClassifier,
) -> Result<Option<&'static FormatSchema>, FormatError> {
    match classifier.classify(path) {
        PathClassification::Known(classified) => Ok(Some(format_schema(classified.domain))),
        PathClassification::UnknownDotfile => Ok(None),
        PathClassification::NotDotfile => Err(FormatError::UnclassifiedPath { path: path.clone() }),
    }
}

fn verify_schema(
    path: &RepoPath,
    classifier: &DomainClassifier,
    schema: &FormatSchema,
) -> Result<(), FormatError> {
    let Some(expected_schema) = schema_for_path(path, classifier)? else {
        return Err(FormatError::UnclassifiedPath { path: path.clone() });
    };
    let expected = expected_schema.domain;
    if schema != expected_schema {
        return Err(FormatError::SchemaMismatch {
            path: path.clone(),
            expected,
            actual: schema.domain,
        });
    }
    Ok(())
}
