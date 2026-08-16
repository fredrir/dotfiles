//! Owner-side `_attach` endpoint (plan §12.1): the single-use-token PTY
//! attach channel. Hashes the presented token, redeems it atomically, then
//! verifies — in order — that the token was minted by THIS authority
//! (HostUid), that the Space is still active and bound, and that the
//! recorded server epoch still equals the LIVE tmux server epoch (a
//! restarted server refuses). Only then does it `exec` the exact
//! owner-recorded attach argv.
//!
//! It accepts no native target and no command text from the client — the
//! argv comes only from the redeemed record. Every refusal is one stderr
//! line plus a typed exit; a redeemed-but-refused token stays consumed
//! (single use is not negotiable).

use std::path::PathBuf;

use crate::backend::tmux::{TmuxProvider, TmuxServerIdentity};
use crate::error::ErrorCode;
use crate::model::{Backend, Lifecycle};
use crate::registry::{
    AttachRedemption, RedeemedAttach, Registry, RegistryConfig, now_rfc3339, sha256::sha256_hex,
};

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
    crate::remote::normalize_utf8_locale();
    match attach(args) {
        Ok(never) => match never {},
        Err((code, message)) => {
            eprintln!("dmux _attach: {message}");
            i32::from(code.exit_status().code())
        }
    }
}

enum Never {}

fn attach(args: &AttachArgs) -> Result<Never, (ErrorCode, String)> {
    let env = match (&args.data_dir, &args.lock_dir) {
        (Some(data), Some(lock)) => (data.join("registry.sqlite3"), lock.clone()),
        _ => {
            let env = crate::operations::OperationEnv::production()
                .map_err(|e| (ErrorCode::OperationFailed, format!("environment: {e}")))?;
            (env.db_path, env.lock_dir)
        }
    };
    let mut registry = Registry::open(RegistryConfig::new(env.0, env.1))
        .map_err(|e| (e.error_code(), format!("registry: {e}")))?;

    // Never persist or log the raw token; only its sha256 is looked up.
    let token_hash = sha256_hex(args.token.trim().as_bytes());
    let redemption = registry
        .redeem_attach_token(&token_hash, &now_rfc3339())
        .map_err(|e| (e.error_code(), format!("redeem: {e}")))?;
    let redeemed = match redemption {
        AttachRedemption::Redeemed(redeemed) => redeemed,
        AttachRedemption::Replayed => {
            return Err((
                ErrorCode::AuthFailed,
                "attach token already redeemed; replay refused".to_string(),
            ));
        }
        AttachRedemption::Expired => {
            return Err((ErrorCode::AuthFailed, "attach token expired".to_string()));
        }
        AttachRedemption::Revoked => {
            return Err((ErrorCode::AuthFailed, "attach token revoked".to_string()));
        }
        AttachRedemption::Unknown => {
            return Err((ErrorCode::AuthFailed, "unknown attach token".to_string()));
        }
    };
    verify(&registry, &redeemed)?;

    // Exec the EXACT owner-recorded argv; nothing from the client.
    let (program, argv_rest) = redeemed
        .attach_argv
        .split_first()
        .ok_or((ErrorCode::OperationFailed, "empty attach argv".to_string()))?;
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(program).args(argv_rest).exec();
    Err((
        ErrorCode::OperationFailed,
        format!("exec {program}: {error}"),
    ))
}

/// The post-redemption verification chain (plan §12.1). Any failure here
/// refuses the attach; the single-use token is already consumed.
fn verify(registry: &Registry, redeemed: &RedeemedAttach) -> Result<(), (ErrorCode, String)> {
    let identity = registry
        .identity()
        .map_err(|e| (e.error_code(), format!("identity: {e}")))?;
    if redeemed.host_uid != identity.host_uid {
        return Err((
            ErrorCode::HostIdentityChanged,
            format!(
                "token was minted for host {} but this authority is {}",
                redeemed.host_uid.0, identity.host_uid.0
            ),
        ));
    }
    let space = registry
        .space(redeemed.space_uid)
        .map_err(|e| (ErrorCode::SpaceAbsent, format!("space: {e}")))?;
    if space.lifecycle != Lifecycle::Active {
        return Err((
            ErrorCode::SpaceAbsent,
            format!(
                "space is no longer active ({})",
                lifecycle_token(space.lifecycle)
            ),
        ));
    }
    let binding = registry
        .current_binding(redeemed.space_uid)
        .map_err(|e| (e.error_code(), format!("binding: {e}")))?
        .ok_or((
            ErrorCode::SpaceAbsent,
            "space has no current native binding".to_string(),
        ))?;
    // The recorded argv targets the bound session; a rebind since issue
    // invalidates the plan.
    if redeemed.attach_argv.last() != Some(&binding.native_token) {
        return Err((
            ErrorCode::SpaceAbsent,
            "space was rebound since the plan was issued".to_string(),
        ));
    }
    // LIVE server re-probe: the published incarnation must still be running
    // and its epoch must equal the one recorded in the token. A restarted
    // server (fresh incarnation or fresh epoch) refuses.
    let info = registry
        .backend_instance_info(space.backend_instance)
        .map_err(|e| (e.error_code(), format!("instance: {e}")))?;
    if info.backend != Backend::Tmux {
        return Err((
            ErrorCode::ProviderUnavailable,
            "attach tokens are tmux-only".to_string(),
        ));
    }
    let namespace = info.socket_path.ok_or((
        ErrorCode::ProviderUnavailable,
        "managed tmux instance has no recorded namespace".to_string(),
    ))?;
    let record = registry
        .backend_server(space.backend_instance)
        .map_err(|e| (e.error_code(), format!("server record: {e}")))?;
    let published_epoch = record.server_epoch.ok_or((
        ErrorCode::BackendEpochChanged,
        "tmux server has no published epoch".to_string(),
    ))?;
    if published_epoch != redeemed.server_epoch {
        return Err((
            ErrorCode::BackendEpochChanged,
            format!(
                "token epoch {} but the published server epoch is {}",
                redeemed.server_epoch.0, published_epoch.0
            ),
        ));
    }
    let expected_identity = TmuxServerIdentity {
        pid: record
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or((
                ErrorCode::BackendEpochChanged,
                "published incarnation has no recorded pid".to_string(),
            ))?,
        start_token: record.server_start_token.clone().ok_or((
            ErrorCode::BackendEpochChanged,
            "published incarnation has no recorded start token".to_string(),
        ))?,
    };
    let provider: TmuxProvider<_> = TmuxProvider::new(namespace.clone());
    provider
        .verify_epoch(&namespace, redeemed.server_epoch, &expected_identity)
        .map_err(|e| match e {
            crate::backend::ProviderError::EpochChanged { .. }
            | crate::backend::ProviderError::WrongInstance { .. } => (
                ErrorCode::BackendEpochChanged,
                "tmux server restarted since the plan was issued".to_string(),
            ),
            other => (
                ErrorCode::ProviderUnavailable,
                format!("tmux server probe failed: {other:?}"),
            ),
        })?;
    Ok(())
}

fn lifecycle_token(lifecycle: Lifecycle) -> &'static str {
    match lifecycle {
        Lifecycle::Reserved => "reserved",
        Lifecycle::Active => "active",
        Lifecycle::Deleting => "deleting",
        Lifecycle::Deleted => "deleted",
        Lifecycle::Conflict => "conflict",
        Lifecycle::Aborted => "aborted",
    }
}
