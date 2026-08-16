//! Raw source handling for the `.dotfile` language.
//!
//! This crate owns the byte-level foundation every other component builds on:
//! checked [`ByteRange`] spans, strict repository-relative [`RepoPath`]s,
//! immutable [`SourceText`], the shared [`LineIndex`] converting between raw
//! byte offsets, normative one-based Unicode-scalar coordinates, and LSP
//! UTF-8/UTF-16 positions, the frozen [`Diagnostic`] record, and the source
//! version bootstrap reader.
//!
//! The representations follow ADR 0002 (span and offset representation) and
//! ADR 0006 (supported version windows): raw `u64` byte offsets are canonical,
//! line/column coordinates are derived, never independently scanned.

mod bootstrap;
mod diagnostic;
mod line_index;
mod path;
mod range;
mod text;
mod utf8;

pub use bootstrap::{Bootstrap, PROFILES_PATH, SOURCE_VERSION_1, SourceVersion, read_bootstrap};
pub use diagnostic::{
    Diagnostic, DiagnosticSink, Fix, FixEdit, Scope, Severity, Span, Stage, sort_diagnostics,
};
pub use line_index::{LineCol, LineIndex, PositionEncoding};
pub use path::{PathError, RepoPath};
pub use range::ByteRange;
pub use text::SourceText;
pub use utf8::decode_utf8;

/// Stable identifier of one source file within an analysis session.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(pub u32);
