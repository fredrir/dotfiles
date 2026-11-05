#![forbid(unsafe_code)]
// LSP URIs and lsp-types values are immutable in practice; clippy sees the
// interior mutability of their internals and would reject every map key.
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
