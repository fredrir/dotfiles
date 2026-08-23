//! Host administration entry points for the root's `dmux host ls|label|
//! forget` wiring (plan §7.3, §12.2). Pure library functions returning
//! typed data/errors; rendering stays with the CLI.

use crate::error::{ErrorCode, TypedError};
use crate::model::HostUid;
use crate::operations::OperationEnv;
use crate::refs::HostToken;
use crate::registry::{HostRow, Registry, RegistryConfig, RouteRow};
use crate::resolve::resolve_enrolled_host;

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

/// The host-token shape of a HOST_REF spelling: a hyphenated HostUid, or an
/// alias/label spelling — which of the two is the resolver's question.
pub fn host_token(host_ref: &str) -> HostToken {
    match uuid::Uuid::try_parse(host_ref) {
        Ok(uid) if host_ref.len() == 36 => HostToken::Uid(HostUid(uid)),
        _ => HostToken::AliasOrLabel(host_ref.to_string()),
    }
}

/// Resolve a HOST_REF: current alias, current label, or full HostUid, by
/// the one enrolled-host rule (`resolve::resolve_enrolled_host`, ADR 012
/// WS-D.3): enrolled rows only, so a tombstoned host resolves by none of
/// its spellings; an unknown spelling is `not_found` and an ambiguous one
/// `ambiguous_target`, never a guess.
pub fn resolve_host(registry: &Registry, host_ref: &str) -> Result<HostRow, TypedError> {
    let hosts = registry.hosts().map_err(typed)?;
    let host_uid = resolve_enrolled_host(&hosts, &host_token(host_ref))?;
    hosts
        .into_iter()
        .find(|host| host.host_uid == host_uid)
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::NotFound,
                format!("no enrolled host matches {host_ref:?}"),
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HostUid;
    use uuid::Uuid;

    fn scratch() -> (tempfile::TempDir, OperationEnv) {
        let dir = tempfile::tempdir().unwrap();
        let env = OperationEnv {
            db_path: dir.path().join("registry.sqlite3"),
            lock_dir: dir.path().join("locks"),
        };
        (dir, env)
    }

    /// ADR 012 WS-D.3's follow-up: `host label|forget` and the recovery
    /// verbs' `--host` resolve through `resolve::resolve_enrolled_host`, so
    /// the three spellings answer the same row, an unknown spelling keeps
    /// its `not_found` text, and a tombstoned host resolves by none of its
    /// spellings — the old resolver still answered a forgotten host by its
    /// label or HostUid.
    #[test]
    fn host_refs_resolve_enrolled_rows_only_through_the_one_resolver() {
        let (_dir, env) = scratch();
        let peer = HostUid(Uuid::from_u128(0x5EE7));
        let alias = {
            let mut registry = open(&env).unwrap();
            registry.enroll_host(peer, Some("peer")).unwrap().alias
        };
        let registry = open(&env).unwrap();
        for spelling in [alias.as_str(), "peer", &peer.0.to_string()] {
            assert_eq!(
                resolve_host(&registry, spelling).unwrap().host_uid,
                peer,
                "{spelling}"
            );
        }
        let unknown = resolve_host(&registry, "nosuchhost").unwrap_err();
        assert_eq!(unknown.code, ErrorCode::NotFound);
        assert_eq!(unknown.message, "no enrolled host matches \"nosuchhost\"");
        assert_eq!(
            host_token(&peer.0.to_string()),
            HostToken::Uid(peer),
            "a hyphenated uuid is the HostUid form"
        );
        assert_eq!(
            host_token(&peer.0.simple().to_string()),
            HostToken::AliasOrLabel(peer.0.simple().to_string()),
            "the compact form is not ref grammar and is matched as a spelling"
        );
        drop(registry);

        let tombstoned = forget(&env, "peer", true).unwrap();
        assert_eq!(tombstoned.host_uid, peer);
        let registry = open(&env).unwrap();
        for spelling in [alias.as_str(), "peer", &peer.0.to_string()] {
            let error = resolve_host(&registry, spelling).unwrap_err();
            assert_eq!(
                error.code,
                ErrorCode::NotFound,
                "{spelling}: {}",
                error.message
            );
        }
    }
}
