//! Owner-side `_agent` endpoint (plan §12.1): reads ONE JSON request
//! envelope on stdin, writes ONE response envelope on stdout, exits with
//! the mapped typed status. Never an interactive transport.
//!
//! W5 root skeleton (ADR 009): the entry signature below is frozen so the
//! binary wiring in `main.rs` is stable; the remote/routing agent owns the
//! implementation from the W5 dispatch record.

use std::path::PathBuf;

use crate::error::{ErrorCode, TypedError};
use crate::remote::protocol::{Envelope, PROTOCOL_VERSION};

/// Arguments the hidden `dmux _agent` subcommand collects. `data_dir` and
/// `lock_dir` are the same test seams `_tmux-bootstrap` exposes; production
/// resolves the registry through `OperationEnv::production()`.
#[derive(Debug, Clone)]
pub struct AgentArgs {
    pub protocol: u32,
    pub method: String,
    pub data_dir: Option<PathBuf>,
    pub lock_dir: Option<PathBuf>,
}

/// Run the agent endpoint. Returns the process exit code; the response
/// envelope (payload or typed error) has already been written to stdout.
pub fn run(args: &AgentArgs) -> i32 {
    // Skeleton behavior: version-gate only, every method unimplemented.
    let code = if args.protocol == PROTOCOL_VERSION {
        ErrorCode::OperationFailed
    } else {
        ErrorCode::ProtocolMismatch
    };
    let error = TypedError::new(
        code,
        format!("_agent method not implemented yet: {}", args.method),
    );
    // A skeleton cannot present authority identity; emit the bare typed
    // error as the single stdout document so callers still get one JSON
    // response. The real implementation always answers with a full
    // `Envelope` — this placeholder is replaced in P7.
    println!(
        "{}",
        serde_json::json!({ "protocol_version": PROTOCOL_VERSION, "error": error })
    );
    i32::from(code.exit_status().code())
}
