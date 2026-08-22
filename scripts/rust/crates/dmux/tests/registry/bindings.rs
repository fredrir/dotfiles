//! ADR 012 WS-A.8 (review finding #18): `native_bindings.server_epoch` was
//! INSERTed at create/adopt and never SELECTed, so every adapter fence built
//! on it compared a value synthesised by the caller. The column is now read
//! back through `current_binding_epoch` and refreshed — as observation
//! metadata, never identity — through `observe_binding_epoch`.

use dmux::model::{ServerEpoch, SpaceUid};
use dmux::registry::{NativeBindingSpec, NativeKind, RegistryError};
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

#[test]
fn the_recorded_binding_epoch_is_readable_and_refreshed_only_on_the_current_row() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let first = ServerEpoch(Uuid::new_v4());

    let bound = reserve(&mut reg, "bound", instance);
    reg.finalize_create(
        bound.space_uid,
        bound.operation_uid,
        &NativeBindingSpec {
            native_token: "$7".into(),
            native_kind: NativeKind::TmuxSessionId,
            server_epoch: Some(first),
        },
    )
    .unwrap();
    assert_eq!(
        reg.current_binding_epoch(bound.space_uid).unwrap(),
        Some(first)
    );

    // A binding recorded without an epoch reads back as None, not an error.
    let unepoched = reserve(&mut reg, "unepoched", instance);
    finalize(&mut reg, &unepoched, "$8");
    assert_eq!(
        reg.current_binding_epoch(unepoched.space_uid).unwrap(),
        None
    );

    // No current binding at all is typed not-found, for reads and refreshes.
    let reserved = reserve(&mut reg, "reserved-only", instance);
    assert!(matches!(
        reg.current_binding_epoch(reserved.space_uid),
        Err(RegistryError::NotFound { .. })
    ));
    assert!(matches!(
        reg.observe_binding_epoch(reserved.space_uid, first),
        Err(RegistryError::NotFound { .. })
    ));
    assert!(matches!(
        reg.current_binding_epoch(SpaceUid(Uuid::new_v4())),
        Err(RegistryError::NotFound { .. })
    ));

    // A refresh rewrites the current row's epoch and observation time and
    // advances no authority revision: it is observation, not identity.
    let head = reg.authority_head().unwrap();
    let second = ServerEpoch(Uuid::new_v4());
    reg.observe_binding_epoch(bound.space_uid, second).unwrap();
    assert_eq!(
        reg.current_binding_epoch(bound.space_uid).unwrap(),
        Some(second)
    );
    assert_eq!(
        reg.current_binding_epoch(unepoched.space_uid).unwrap(),
        None
    );
    assert_eq!(reg.authority_head().unwrap(), head);
    let (token, observed_at): (String, Option<String>) = reg
        .raw_connection()
        .query_row(
            "SELECT native_token, observed_at FROM native_bindings \
             WHERE space_uid = ?1 AND binding_state = 'current'",
            [bound.space_uid.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(token, "$7", "the native token is never touched");
    assert!(observed_at.is_some());
}
