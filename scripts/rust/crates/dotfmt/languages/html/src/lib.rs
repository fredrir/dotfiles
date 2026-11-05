#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod format;
mod tailwind;

pub use config::Config;
pub use dialect::Dialect;
pub use format::format;
