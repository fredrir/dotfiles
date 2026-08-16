//! Owner-side fenced operations. P5 delivers the tmux server-epoch
//! bootstrap (`dmux _tmux-bootstrap`, plan §11.2); P6 adds the fenced
//! create/rename/remove flows on top of the same skeleton.
//!
//! Root-owned (plan §19).

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::backend::tmux::{EpochSetOutcome, SystemRunner, TmuxProvider};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{Backend, ServerEpoch};
use crate::registry::{Registry, RegistryConfig};

/// Outcome of one `_tmux-bootstrap` run (plan §11.2). Every variant leaves
/// the registry binding equal to the server's observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxBootstrapOutcome {
    /// Fresh incarnation: epoch minted, stamped, published, verified.
    Bootstrapped { epoch: ServerEpoch },
    /// Option already equalled the registry binding for this incarnation.
    AlreadyBound { epoch: ServerEpoch },
    /// Option present but the registry knew a different incarnation or
    /// epoch: the observed binding was published, which invalidates every
    /// prior child ref minted under the old epoch (plan §11.2). Never
    /// overwrites the option.
    Rebound {
        epoch: ServerEpoch,
        previous: Option<ServerEpoch>,
    },
}

#[derive(Debug)]
pub enum BootstrapError {
    /// No running server for the namespace: nothing to bootstrap. `ls`
    /// lists a stopped namespace via inventory; the hook simply loses the
    /// race with server death.
    ServerStopped(String),
    Lock(String),
    Registry(String),
    Provider(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::ServerStopped(d) => write!(f, "tmux server stopped: {d}"),
            BootstrapError::Lock(d) => write!(f, "kernel lock: {d}"),
            BootstrapError::Registry(d) => write!(f, "registry: {d}"),
            BootstrapError::Provider(d) => write!(f, "tmux: {d}"),
        }
    }
}

/// Explicit storage/lock locations so tests inject scratch dirs; production
/// callers build this from `registry::production_db_path()` +
/// `runtime::dmux_runtime_dir()`.
pub struct OperationEnv {
    pub db_path: PathBuf,
    pub lock_dir: PathBuf,
}

impl OperationEnv {
    pub fn production() -> std::io::Result<OperationEnv> {
        let db_path = crate::registry::production_db_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no XDG data home resolvable")
        })?;
        Ok(OperationEnv {
            db_path,
            lock_dir: crate::runtime::dmux_runtime_dir()?,
        })
    }
}

/// The §11.2 epoch bootstrap for one managed tmux namespace. The exact
/// sequence, per the P5 handoffs: open registry → ensure the tmux backend
/// instance → take the authority gate (shared) and the backend-instance
/// kernel lock (exclusive) → probe server identity UNDER the lock →
/// `set_epoch_if_absent` → publish the observed binding → `verify_epoch`.
pub fn tmux_bootstrap(
    env: &OperationEnv,
    namespace: &str,
) -> Result<TmuxBootstrapOutcome, BootstrapError> {
    let mut registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(namespace), None)
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| BootstrapError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| BootstrapError::Lock(e.to_string()))?;

    let provider: TmuxProvider<SystemRunner> = TmuxProvider::new(namespace);
    // Identity under the lock, so identity and epoch bind to one incarnation.
    let identity = provider
        .server_identity(namespace)
        .map_err(|e| BootstrapError::ServerStopped(format!("{e:?}")))?;

    let minted = ServerEpoch(Uuid::new_v4());
    let outcome = provider
        .set_epoch_if_absent(namespace, minted)
        .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;
    let (observed_epoch, previous) = match outcome {
        EpochSetOutcome::Set => (minted, None),
        EpochSetOutcome::AlreadySet(existing) => {
            let record = registry
                .backend_server(instance)
                .map_err(|e| BootstrapError::Registry(e.to_string()))?;
            let same_incarnation = record.server_pid == Some(identity.pid as i64)
                && record.server_start_token.as_deref() == Some(identity.start_token.as_str());
            if same_incarnation && record.server_epoch == Some(existing) {
                // Fully bound already: adopt/no-op.
                provider
                    .verify_epoch(namespace, existing, &identity)
                    .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;
                return Ok(TmuxBootstrapOutcome::AlreadyBound { epoch: existing });
            }
            (existing, record.server_epoch)
        }
    };

    registry
        .publish_backend_server(
            instance,
            observed_epoch,
            Some(identity.pid as i64),
            Some(&identity.start_token),
            None,
            None,
        )
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;
    provider
        .verify_epoch(namespace, observed_epoch, &identity)
        .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;

    Ok(match previous {
        None if matches!(outcome, EpochSetOutcome::Set) => TmuxBootstrapOutcome::Bootstrapped {
            epoch: observed_epoch,
        },
        previous => TmuxBootstrapOutcome::Rebound {
            epoch: observed_epoch,
            previous,
        },
    })
}

/// Derive the `-L` namespace from a tmux socket path when the hook runs
/// inside the server (`$TMUX` is `<socket-path>,<pid>,<session>`): sockets
/// under the standard `tmux-<uid>` directory map to their basename; any
/// other path is not a `-L` namespace and is rejected (the caller must pass
/// `--namespace` explicitly for `-S` servers).
pub fn namespace_from_tmux_env(tmux_env: &str) -> Option<String> {
    let socket = tmux_env.split(',').next()?;
    let path = Path::new(socket);
    let parent = path.parent()?.file_name()?.to_str()?;
    if !parent.starts_with("tmux-") {
        return None;
    }
    Some(path.file_name()?.to_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_derivation_from_tmux_env() {
        assert_eq!(
            namespace_from_tmux_env("/private/tmp/tmux-501/dmux-managed,45159,0"),
            Some("dmux-managed".into())
        );
        assert_eq!(
            namespace_from_tmux_env("/tmp/tmux-1000/other,1,2"),
            Some("other".into())
        );
        // -S servers outside the standard dir are not -L namespaces.
        assert_eq!(namespace_from_tmux_env("/var/run/custom.sock,1,2"), None);
        assert_eq!(namespace_from_tmux_env(""), None);
    }
}
