#![forbid(unsafe_code)]

pub mod host;
pub mod socket;

pub use host::{Host, MUX_PORT, PROBE, Route, best};
