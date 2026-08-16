//! dmux contract library — the frozen provider-neutral contracts from
//! `docs/dmux-wezterm-first-plan.md` and `docs/adr/dmux/`.
//!
//! This library target is additive (P1/W1): the `dmux` binary in `main.rs`
//! still runs the legacy modules unchanged, because the P1 gate is "the 116
//! baseline tests stay green with no output change". Later phases implement
//! against these types and retire the legacy seams.
//!
//! Ownership (plan §19): `backend` and the conformance harness stay
//! root-owned; `model`/`refs`/`error` transfer to the identity agent and
//! `remote::protocol` to the remote agent at the recorded W1→W2 handoff.

pub mod backend;
pub mod bootstrap;
pub mod connect_cli;
pub mod error;
pub mod gui;
pub mod gui_cli;
pub mod gui_lifecycle;
pub mod history;
pub mod inventory;
pub mod locks;
pub mod model;
pub mod new_cli;
pub mod operations;
pub mod output;
pub mod policy;
pub mod recovery;
pub mod refs;
pub mod registry;
pub mod remote;
pub mod resolve;
pub mod runtime;
