//! Pane-stamp acknowledgements (plan §10.3, W5 root addendum): epoch-scoped
//! upsert with refresh, listing for the caller-side health recompute, and
//! the static backend-instance registration read.

use dmux::error::ErrorCode;
use dmux::model::{Backend, ServerEpoch, SpaceUid};
use dmux::registry::Registry;
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

fn head(reg: &Registry) -> u64 {
    reg.authority_head().unwrap().revision
}

#[test]
fn stamps_upsert_refresh_and_are_epoch_scoped() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &r, "$1");
    let epoch1 = ServerEpoch(Uuid::new_v4());
    let epoch2 = ServerEpoch(Uuid::new_v4());

    let before = head(&reg);
    reg.record_pane_stamp(r.space_uid, epoch1, "tx-1").unwrap();
    reg.record_pane_stamp(r.space_uid, epoch1, "tx-2").unwrap();
    reg.record_pane_stamp(r.space_uid, epoch2, "tx-7").unwrap();
    assert_eq!(
        head(&reg),
        before,
        "stamps are diagnostics, never authority"
    );

    let stamps = reg.pane_stamps(r.space_uid, epoch1).unwrap();
    assert_eq!(
        stamps
            .iter()
            .map(|s| s.pane_handle.as_str())
            .collect::<Vec<_>>(),
        vec!["tx-1", "tx-2"],
        "only the requested epoch's stamps"
    );
    assert_eq!(reg.pane_stamps(r.space_uid, epoch2).unwrap().len(), 1);

    // An upsert refreshes stamped_at on the same key instead of duplicating.
    reg.raw_connection()
        .execute(
            "UPDATE pane_stamps SET stamped_at = '2000-01-01T00:00:00Z' WHERE pane_handle = 'tx-1'",
            [],
        )
        .unwrap();
    reg.record_pane_stamp(r.space_uid, epoch1, "tx-1").unwrap();
    let stamps = reg.pane_stamps(r.space_uid, epoch1).unwrap();
    assert_eq!(stamps.len(), 2);
    assert!(
        stamps[0].stamped_at > "2000-01-01T00:00:00Z".to_string(),
        "stamped_at refreshed: {}",
        stamps[0].stamped_at
    );

    // A stamp for a Space that does not exist is typed NotFound.
    let err = reg
        .record_pane_stamp(SpaceUid(Uuid::now_v7()), epoch1, "tx-9")
        .unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}

#[test]
fn backend_instance_info_returns_the_static_registration() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = reg
        .register_backend_instance(Backend::Tmux, Some("dmux"), Some("io.dmux.tmux"))
        .unwrap();

    let info = reg.backend_instance_info(instance).unwrap();
    assert_eq!(info.backend, Backend::Tmux);
    assert_eq!(info.socket_path.as_deref(), Some("dmux"));
    assert_eq!(info.service_label.as_deref(), Some("io.dmux.tmux"));
    assert_eq!(info.owner, reg.identity().unwrap().host_uid);
    assert!(!info.created_at.is_empty());

    let err = reg
        .backend_instance_info(dmux::model::BackendInstanceUid(Uuid::new_v4()))
        .unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}
