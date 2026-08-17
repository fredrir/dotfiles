//! Hidden managed-tmux client-hook dispatch (plan §13.1).
//!
//! tmux 3.7b exposes two deliberately different hook contexts. The three
//! `client-*` hooks carry `#{hook_client}` and it must equal the exact
//! client-scoped `#{client_name}`. The `after-select-*` command hooks leave
//! `#{hook_client}` empty, but retain the exact client-scoped name/PID/TTY
//! and active session/window/pane fields. This module closes that split
//! before handing one exact client name to the owner-side correlation API.

use std::path::PathBuf;

use clap::Args;

use crate::error::{ErrorCode, TypedError};
use crate::operations::{self, OperationEnv};
use crate::remote::attach::{TmuxHookClientClaim, refresh_controller_context_from_tmux_hook};

const DEBUG_ENV: &str = "DMUX_TMUX_HOOK_DEBUG";
const MAX_DEBUG_MESSAGE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TmuxContextRefreshArgs {
    /// Exact tmux hook name (`#{hook}`).
    #[arg(long)]
    pub event: String,

    /// Native hook client (`#{hook_client}`); empty for after-select hooks.
    #[arg(long)]
    pub hook_client: String,

    /// Client-scoped name (`#{client_name}`).
    #[arg(long)]
    pub client_name: String,

    /// Client-scoped PID (`#{client_pid}`).
    #[arg(long)]
    pub client_pid: String,

    /// Client-scoped terminal (`#{client_tty}`).
    #[arg(long)]
    pub client_tty: String,

    /// Active immutable tmux session ID (`#{session_id}`).
    #[arg(long)]
    pub session_id: String,

    /// Active immutable tmux window ID (`#{window_id}`).
    #[arg(long)]
    pub window_id: String,

    /// Active immutable tmux pane ID (`#{pane_id}`).
    #[arg(long)]
    pub pane_id: String,

    /// Managed `-L` namespace; inferred from `$TMUX` in production hooks.
    #[arg(long)]
    pub namespace: Option<String>,

    /// Test seam: directory holding registry.sqlite3.
    #[arg(long, hide = true, requires = "lock_dir")]
    pub data_dir: Option<PathBuf>,

    /// Test seam: kernel-lock/runtime directory.
    #[arg(long, hide = true, requires = "data_dir")]
    pub lock_dir: Option<PathBuf>,
}

/// Run one captured tmux hook. Feature-off and ordinary uncorrelated clients
/// are intentionally silent no-ops. Every feature-on correlated path either
/// publishes a fully revalidated marker or changes nothing.
pub fn run(args: &TmuxContextRefreshArgs) -> u8 {
    if std::env::var("DMUX_WEZ_FIRST").as_deref() != Ok("1") {
        return 0;
    }
    crate::remote::normalize_utf8_locale();
    match refresh(args) {
        Ok(()) => 0,
        // A client not launched through the dmux correlation path is normal:
        // the managed server may still have an explicit administrative
        // client. It has no authority and receives no marker.
        Err(error) if error.code == ErrorCode::NotFound => 0,
        Err(error) => {
            report_debug(&error);
            error.code.exit_status().code()
        }
    }
}

fn refresh(args: &TmuxContextRefreshArgs) -> Result<(), TypedError> {
    let exact_client = exact_client_for_event(&args.event, &args.hook_client, &args.client_name)?;
    let client_pid = args.client_pid.parse::<u32>().map_err(|_| {
        TypedError::new(
            ErrorCode::InvalidRef,
            "tmux hook client PID is not one canonical positive integer",
        )
    })?;
    if client_pid == 0 || args.client_pid != client_pid.to_string() {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "tmux hook client PID is not one canonical positive integer",
        ));
    }
    let namespace = exact_namespace(args.namespace.as_deref())?;
    let env = operation_env(args)?;
    refresh_controller_context_from_tmux_hook(
        &env,
        &TmuxHookClientClaim {
            namespace,
            hook_client: exact_client.to_string(),
            client_pid,
            client_tty: args.client_tty.clone(),
            session_id: args.session_id.clone(),
            window_id: args.window_id.clone(),
            pane_id: args.pane_id.clone(),
        },
    )?;
    Ok(())
}

fn exact_client_for_event<'a>(
    event: &str,
    hook_client: &str,
    client_name: &'a str,
) -> Result<&'a str, TypedError> {
    if client_name.is_empty() || client_name.chars().any(char::is_control) {
        return Err(invalid_hook_shape(event));
    }
    match event {
        "client-attached" | "client-session-changed" | "client-active"
            if !hook_client.is_empty() && hook_client == client_name =>
        {
            Ok(client_name)
        }
        "after-select-window" | "after-select-pane" | "session-window-changed"
            if hook_client.is_empty() =>
        {
            Ok(client_name)
        }
        "client-attached"
        | "client-session-changed"
        | "client-active"
        | "after-select-window"
        | "after-select-pane"
        | "session-window-changed" => Err(invalid_hook_shape(event)),
        _ => Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!("unsupported managed tmux hook event {event:?}"),
        )),
    }
}

fn invalid_hook_shape(event: &str) -> TypedError {
    TypedError::new(
        ErrorCode::IdentityConflict,
        format!("tmux hook event {event:?} has an absent or mixed client identity"),
    )
}

fn exact_namespace(explicit: Option<&str>) -> Result<String, TypedError> {
    let ambient = std::env::var("TMUX")
        .ok()
        .and_then(|value| operations::namespace_from_tmux_env(&value));
    match (explicit, ambient) {
        (Some(explicit), Some(ambient)) if explicit != ambient => Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "explicit tmux hook namespace differs from the invoking server",
        )),
        (Some(explicit), _) => Ok(explicit.to_string()),
        (None, Some(ambient)) => Ok(ambient),
        (None, None) => Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "managed tmux hook has no exact -L namespace",
        )),
    }
}

fn operation_env(args: &TmuxContextRefreshArgs) -> Result<OperationEnv, TypedError> {
    match (&args.data_dir, &args.lock_dir) {
        (Some(data), Some(lock)) => Ok(OperationEnv {
            db_path: data.join("registry.sqlite3"),
            lock_dir: lock.clone(),
        }),
        (None, None) => OperationEnv::production().map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("tmux hook environment: {error}"),
            )
        }),
        // clap enforces this for the binary; retain the library invariant.
        _ => Err(TypedError::new(
            ErrorCode::Usage,
            "--data-dir and --lock-dir must be supplied together",
        )),
    }
}

fn report_debug(error: &TypedError) {
    if std::env::var(DEBUG_ENV).as_deref() != Ok("1") {
        return;
    }
    let mut message = error.message.replace(['\r', '\n'], " ");
    if message.len() > MAX_DEBUG_MESSAGE_BYTES {
        let mut boundary = MAX_DEBUG_MESSAGE_BYTES;
        while !message.is_char_boundary(boundary) {
            boundary -= 1;
        }
        message.truncate(boundary);
        message.push('…');
    }
    eprintln!("dmux _tmux-context-refresh: {}: {message}", error.code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_events_require_exact_native_hook_client() {
        for event in ["client-attached", "client-session-changed", "client-active"] {
            assert_eq!(
                exact_client_for_event(event, "/dev/pts/7", "/dev/pts/7").unwrap(),
                "/dev/pts/7"
            );
            assert!(exact_client_for_event(event, "", "/dev/pts/7").is_err());
            assert!(exact_client_for_event(event, "/dev/pts/8", "/dev/pts/7").is_err());
        }
    }

    #[test]
    fn after_select_events_require_the_observed_empty_hook_client() {
        for event in [
            "after-select-window",
            "after-select-pane",
            "session-window-changed",
        ] {
            assert_eq!(
                exact_client_for_event(event, "", "/dev/pts/7").unwrap(),
                "/dev/pts/7"
            );
            assert!(exact_client_for_event(event, "/dev/pts/7", "/dev/pts/7").is_err());
        }
    }

    #[test]
    fn unknown_absent_and_control_bearing_hook_shapes_fail_closed() {
        assert!(exact_client_for_event("window-linked", "", "/dev/pts/7").is_err());
        assert!(exact_client_for_event("client-attached", "", "").is_err());
        assert!(exact_client_for_event("after-select-pane", "", "/dev/pts/7\n").is_err());
    }
}
