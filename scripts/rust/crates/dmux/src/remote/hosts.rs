//! Host administration entry points for the root's `dmux host ls|label|
//! forget` wiring (plan §7.3, §12.2). Pure library functions returning
//! typed data/errors; rendering stays with the CLI.

use crate::error::{ErrorCode, TypedError};
use crate::operations::OperationEnv;
use crate::registry::{HostRow, Registry, RegistryConfig, RouteRow};

fn open(env: &OperationEnv) -> Result<Registry, TypedError> {
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))
}

fn typed(e: crate::registry::RegistryError) -> TypedError {
    TypedError::new(e.error_code(), e.to_string())
}

/// One host plus its route records (`dmux host ls` lists hosts and routes
/// only; Spaces belong to `dmux ls`).
#[derive(Debug, Clone)]
pub struct HostListing {
    pub host: HostRow,
    pub routes: Vec<RouteRow>,
}

/// Enrollment-ordered hosts with their routes (tombstoned hosts included —
/// their history is retained; their routes arrive disabled).
pub fn list(env: &OperationEnv) -> Result<Vec<HostListing>, TypedError> {
    let registry = open(env)?;
    let hosts = registry.hosts().map_err(typed)?;
    let mut listings = Vec::with_capacity(hosts.len());
    for host in hosts {
        let routes = registry.routes_for(host.host_uid).map_err(typed)?;
        listings.push(HostListing { host, routes });
    }
    Ok(listings)
}

/// Resolve a HOST_REF: current alias, current label, or full HostUid.
/// Unknown/ambiguous spellings are typed errors, never guesses.
pub fn resolve_host(registry: &Registry, host_ref: &str) -> Result<HostRow, TypedError> {
    if let Some(row) = registry.host_by_alias(host_ref).map_err(typed)? {
        return Ok(row);
    }
    let hosts = registry.hosts().map_err(typed)?;
    if let Some(row) = hosts.iter().find(|h| h.label.as_deref() == Some(host_ref)) {
        return Ok(row.clone());
    }
    if let Ok(uid) = host_ref.parse::<uuid::Uuid>()
        && let Some(row) = hosts.iter().find(|h| h.host_uid.0 == uid)
    {
        return Ok(row.clone());
    }
    Err(TypedError::new(
        ErrorCode::NotFound,
        format!("no enrolled host matches {host_ref:?}"),
    ))
}

/// `dmux host label HOST_REF NEW_LABEL`. Spellings are never rebound to a
/// different HostUid (registry contract); returns the refreshed row.
pub fn label(env: &OperationEnv, host_ref: &str, new_label: &str) -> Result<HostRow, TypedError> {
    let mut registry = open(env)?;
    let host = resolve_host(&registry, host_ref)?;
    registry
        .set_host_label(host.host_uid, new_label)
        .map_err(typed)?;
    resolve_host(&registry, new_label)
}

/// `dmux host forget HOST_REF`. Requires the CALLER to have confirmed
/// (`confirmed` false returns the typed confirmation_required error and
/// changes nothing); refuses the local host `a`; tombstones the host and
/// its refs, disables its routes, retains cached history. Returns the
/// tombstoned row.
pub fn forget(env: &OperationEnv, host_ref: &str, confirmed: bool) -> Result<HostRow, TypedError> {
    let mut registry = open(env)?;
    let host = resolve_host(&registry, host_ref)?;
    let identity = registry.identity().map_err(typed)?;
    if host.host_uid == identity.host_uid {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "`a` is the local authority and can never be forgotten (plan §12.2)",
        ));
    }
    if !confirmed {
        return Err(TypedError::new(
            ErrorCode::ConfirmationRequired,
            format!(
                "forgetting host {} disables its routes and tombstones its refs; \
                 confirm to proceed",
                host.host_uid.0
            ),
        ));
    }
    registry.forget_host(host.host_uid).map_err(typed)?;
    let hosts = registry.hosts().map_err(typed)?;
    hosts
        .into_iter()
        .find(|h| h.host_uid == host.host_uid)
        .ok_or_else(|| TypedError::new(ErrorCode::OperationFailed, "host row vanished"))
}
