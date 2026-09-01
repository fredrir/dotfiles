#![forbid(unsafe_code)]

pub mod host;
pub mod socket;
pub mod ssh;

pub use host::{Host, MUX_PORT, PROBE, Route, best};
