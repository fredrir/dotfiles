#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod format;
mod protect;

pub use format::format;
