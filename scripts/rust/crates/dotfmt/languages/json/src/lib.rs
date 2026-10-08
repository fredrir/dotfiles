#![forbid(unsafe_code)]

mod commented;
pub mod config;
pub mod dialect;
mod format;
mod number;
mod parse;
pub mod render;
pub mod repair;
mod value;

pub use format::{Formatted, format};

#[cfg(test)]
#[path = "../tests/unit/format_tests.rs"]
mod format_tests;

#[cfg(test)]
#[path = "../tests/unit/number_tests.rs"]
mod number_tests;

#[cfg(test)]
#[path = "../tests/unit/parse_tests.rs"]
mod parse_tests;

#[cfg(test)]
#[path = "../tests/unit/render_tests.rs"]
mod render_tests;

#[cfg(test)]
#[path = "../tests/unit/commented_tests.rs"]
mod commented_tests;
