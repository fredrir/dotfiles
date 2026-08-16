//! Typed `.dotfile` v1 domains.
//!
//! This crate is the boundary between the generic, lossless syntax tree and
//! later repository semantics.  It classifies canonical repository paths,
//! validates the immediate schema of every M2-owned domain, lowers into an
//! owned and tolerant HIR, evaluates file-local lexical bindings, and keeps
//! a bidirectional source map.  Cross-file reference resolution deliberately
//! remains outside this crate.

mod classifier;
mod format;
mod hir;
mod lower;
mod validate;

pub use classifier::{
    ClassificationError, ClassifiedPath, Domain, DomainClassifier, DomainLocation, GroupLayout,
    GroupLayoutEntry, PathClassification, classify_static,
};
pub use format::{
    FormatContainerRule, FormatContext, FormatEntryKind, FormatOrder, FormatPublishedKind,
    FormatSchema, attribute_allowed, attribute_order, entry_allowed, expected_entry_shape,
    format_schema, order_for, published_entry_allowed, resource_kind_order,
};
pub use hir::*;
pub use lower::{LoweringError, lower_path};
