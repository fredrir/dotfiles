//! `dmux ls` with the Wez-first gate on (plan §7.1, §16.1).
//!
//! The binary's legacy `list` module still answers with the gate off, so this
//! is the only caller of `inventory::reconcile` and the `output` renderers.
//! Deprecated flags keep their exact legacy payload on stdout and put their
//! migration hint on stderr, because scripts compare stdout byte for byte.
//!
//! Owned by the P4 resolver/output agent (plan §19.3).

use crate::error::ExitStatus;
use crate::model::Backend;
use crate::output::OutputFormat;

/// False while the body below is still `todo!()`. The binary consults this so
/// a machine with the canary flag already exported -- this one -- keeps the
/// legacy behaviour instead of panicking in the user's shell. The agent that
/// implements the body flips it here, in its own module, so nobody has to
/// reopen main.rs to land a verb.
pub const IMPLEMENTED: bool = false;

/// The parsed `ls` surface. The binary owns the flag spelling and the gate;
/// everything below the gate is decided here. The default is the plain
/// local listing bare `dmux` falls back to on a pipe.
#[derive(Default)]
pub struct LsArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    /// Declared as conflicting with `host`, but clap enforces that only when
    /// both follow the subcommand: `dmux --host h ls --all-hosts` arrives
    /// here with both set and has to be refused here.
    pub all_hosts: bool,
    pub backend: Option<Backend>,
    pub tree: bool,
    /// Deprecated `--json`: the bare legacy row array, never the envelope.
    pub json: bool,
    /// Deprecated `--tmux` / `--wez`, both replaced by `--backend`.
    pub only_tmux: bool,
    pub only_wez: bool,
    /// Hidden legacy: one name per line, for the shell wrappers.
    pub names: bool,
}

pub fn run(format: Option<OutputFormat>, args: LsArgs) -> ExitStatus {
    let _ = (format, args);
    todo!("P4: reconciled listing, envelope, and --tree")
}
