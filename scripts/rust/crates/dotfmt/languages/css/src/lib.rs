#![forbid(unsafe_code)]

mod apply;
pub mod config;
pub mod dialect;
mod format;

pub use config::Config;
pub use dialect::Dialect;
pub use format::format;
