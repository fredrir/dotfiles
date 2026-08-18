//! `dmux migrate` — the one-time explicit cutover (plan §17).
//!
//! Preview is the default: without `--commit` it prints the proposed Space
//! mapping and adopts nothing. Legacy toggle history arrives as the parsed
//! state file rather than being read here, because that file belongs to the
//! binary and converts to SpaceUid only where a name is unambiguous
//! (plan §17.11).
//!
//! Owned by the P11 migration agent (plan §19.3).

use std::collections::BTreeMap;

use crate::error::ExitStatus;
use crate::output::OutputFormat;

/// False while the body below is still `todo!()`. The binary consults this so
/// a machine with the canary flag already exported -- this one -- keeps the
/// legacy behaviour instead of panicking in the user's shell. The agent that
/// implements the body flips it here, in its own module, so nobody has to
/// reopen main.rs to land a verb.
pub const IMPLEMENTED: bool = false;

pub struct MigrateArgs {
    /// Apply the previewed plan; without it nothing is adopted or stamped.
    pub commit: bool,
    pub yes: bool,
    /// `key session` lines from the legacy `dmux -` state file.
    pub previous_sessions: BTreeMap<String, String>,
}

pub fn run(format: Option<OutputFormat>, args: MigrateArgs) -> ExitStatus {
    let _ = (format, args);
    todo!("P11: scan, print the deterministic mapping, then batch-adopt")
}
