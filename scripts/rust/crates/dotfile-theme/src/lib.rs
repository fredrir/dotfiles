//! Immediate typed schemas for `.dotfile` v1 theme sources.
//!
//! This crate validates and owns one theme file at a time. It intentionally
//! does not discover files, merge profile overrides, resolve palette/role
//! references, perform cross-profile checks, or join renderer maps. Those are
//! semantic compiler operations built on the source-ordered, spanned models
//! exposed here.

mod hir;
mod lower;
mod model;

pub use hir::*;
pub use lower::{
    ClassifiedThemePath, ThemeLowering, ThemeLoweringError, ThemePathError, ThemeValidationFailure,
    ValidatedThemeFile, classify_theme_path, lower_parsed_theme_file, lower_schema_theme_file,
    lower_theme_file,
};
pub use model::*;

#[cfg(test)]
mod tests;
