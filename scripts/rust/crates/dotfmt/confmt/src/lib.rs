#![forbid(unsafe_code)]

mod block;
mod conf;
pub mod config;
mod native;

pub use conf::{Mode, mode};
pub use native::format;

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;
