//! Owner-side `_attach` endpoint (plan §12.1): verifies a single-use
//! token's request UID, HostUid, SpaceUid, tmux server epoch, route,
//! expiry, and replay state, then `exec`s the exact owner-generated tmux
//! attach argv. It accepts no native target and no command text from the
//! client; a failed verification exits typed without ever attaching.
//!
//! W5 root skeleton (ADR 009): entry signature frozen for the `main.rs`
//! wiring; the remote/routing agent owns the implementation.

use std::path::PathBuf;

use crate::error::ErrorCode;

#[derive(Debug, Clone)]
pub struct AttachArgs {
    pub token: String,
    pub data_dir: Option<PathBuf>,
    pub lock_dir: Option<PathBuf>,
}

/// Run the attach endpoint. On success this function does not return (it
/// replaces the process with the owner-generated attach command); every
/// failure path returns the mapped exit code after one line on stderr.
pub fn run(args: &AttachArgs) -> i32 {
    let _ = &args.token;
    eprintln!("dmux _attach: not implemented yet (P7)");
    i32::from(ErrorCode::OperationFailed.exit_status().code())
}
