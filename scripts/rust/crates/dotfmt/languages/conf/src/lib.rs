#![forbid(unsafe_code)]

mod block;
mod conf;
pub mod config;
mod format;

pub use conf::{Mode, mode};
pub use format::format;

#[cfg(test)]
#[path = "../tests/unit/format_tests.rs"]
mod format_tests;

#[cfg(test)]
#[path = "../tests/unit/block_tests.rs"]
mod block_tests;

#[cfg(test)]
#[path = "../tests/unit/layout_tests.rs"]
mod layout_tests;
