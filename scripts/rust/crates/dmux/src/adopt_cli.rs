//! `dmux adopt NATIVE_REF` — the only ordinary adoption entry point
//! (plan §10.3, §7.4).
//!
//! The token is opaque: it is re-resolved in a fresh complete owner scan
//! under the adoption lease and never handed to a backend as a command
//! string. Wez adoption stays refused until the fenced atomic primitive is
//! available; tmux adoption stamps markers and binds.
//!
//! Owned by the P6 adoption agent (plan §19.3).

use crate::error::ExitStatus;
use crate::output::OutputFormat;

/// False while the body below is still `todo!()`. The binary consults this so
/// a machine with the canary flag already exported -- this one -- keeps the
/// legacy behaviour instead of panicking in the user's shell. The agent that
/// implements the body flips it here, in its own module, so nobody has to
/// reopen main.rs to land a verb.
pub const IMPLEMENTED: bool = false;

pub struct AdoptArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    /// `native:<backend>:<base64url-no-padding>`, parsed by
    /// [`crate::output::parse_native_ref`].
    pub native_ref: String,
    /// Logical name for the adopted Space; the native name when omitted.
    pub name: Option<String>,
}

pub fn adopt(format: Option<OutputFormat>, args: AdoptArgs) -> ExitStatus {
    let _ = (format, args);
    todo!("P6: re-resolve the token under the adoption lease, then bind")
}
