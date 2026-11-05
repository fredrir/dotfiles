#![forbid(unsafe_code)]

pub mod analyze;
pub mod config;
pub mod dialect;
mod format;
pub mod project;
mod range;
mod source;
mod tsconfig;

pub use analyze::{Options, TypeDiagnostic, analyze};
pub use config::Config;
pub use dialect::Dialect;
pub use dotfmt_core::edit::Analysis;
pub use format::format;
pub use range::expand_range;
