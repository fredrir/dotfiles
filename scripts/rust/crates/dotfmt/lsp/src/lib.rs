#![forbid(unsafe_code)]
#![allow(clippy::mutable_key_type)]

mod actions;
mod analysis;
mod client;
mod diagnostics;
mod documents;
mod edits;
mod server;
mod settings;
mod tsc;
mod uri;
mod workspace;

pub use server::{RangesFormatting, RangesFormattingParams, run, serve};
