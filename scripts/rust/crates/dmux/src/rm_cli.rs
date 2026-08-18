//! `dmux rm` and `dmux rename` with the Wez-first gate on (plan §7.1, §10.2).
//!
//! Both are owner-only mutations: they resolve a stable Space identity first
//! and never create. In JSON mode a destructive verb without `--yes` emits
//! `output::confirmation_required` and changes nothing (plan §7.4).
//!
//! Owned by the P6 mutation agent (plan §19.3).

use crate::error::ExitStatus;
use crate::model::Backend;
use crate::output::OutputFormat;

/// False while the body below is still `todo!()`. The binary consults this so
/// a machine with the canary flag already exported -- this one -- keeps the
/// legacy behaviour instead of panicking in the user's shell. The agent that
/// implements the body flips it here, in its own module, so nobody has to
/// reopen main.rs to land a verb.
pub const IMPLEMENTED: bool = false;

pub struct RmArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    pub targets: Vec<String>,
    /// One-release compatibility escape for the old listing indices; bare
    /// digits are permanent local SpaceNo values instead (plan §17.13).
    pub rows: Vec<u64>,
    pub all: bool,
    pub backend: Option<Backend>,
    /// Legacy `-w`: one tmux window, not a Space.
    pub window: Option<String>,
    pub yes: bool,
}

pub struct RenameArgs {
    pub host: Option<String>,
    /// `dmux rename (SPACE_REF | --name OLD) NEW`: with a selector flag the
    /// grammar has one positional, so clap fills `old` with the new name and
    /// leaves `new_name` empty. Nothing else can tell the two spellings apart.
    pub old: Option<String>,
    pub new_name: Option<String>,
    pub name: Option<String>,
    pub row: Option<u64>,
    pub backend: Option<Backend>,
    pub allow_name_collision: bool,
}

pub fn remove(format: Option<OutputFormat>, args: RmArgs) -> ExitStatus {
    let _ = (format, args);
    todo!("P6: resolve, confirm, and journal the removals")
}

pub fn rename(format: Option<OutputFormat>, args: RenameArgs) -> ExitStatus {
    let _ = (format, args);
    todo!("P6: logical rename for Wez, atomic native+registry rename for tmux")
}
