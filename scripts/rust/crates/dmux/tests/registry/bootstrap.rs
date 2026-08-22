//! P5 identity-slice gate tests: the bootstrap journal behind the frozen
//! `BootstrapJournal` seam (plan §11.1, ADR 004) and backend server-epoch
//! publication (ADR 001/002).

use dmux::bootstrap::{BootstrapJournal, BootstrapState, IssuedRequest};
use dmux::error::ErrorCode;
use dmux::model::{Backend, BackendInstanceUid, ServerEpoch};
use dmux::registry::{Registry, RegistryError, bootstrap_can_transition, bootstrap_is_terminal};
use uuid::Uuid;

use crate::util::{open, reserve, scratch, tmux_instance};

const ALL_STATES: [BootstrapState; 9] = [
    BootstrapState::Issued,
    BootstrapState::Spawned,
    BootstrapState::Correlated,
    BootstrapState::Acked,
    BootstrapState::Completed,
    BootstrapState::Timeout,
    BootstrapState::Orphaned,
    BootstrapState::Conflict,
    BootstrapState::Aborted,
];

const FAILURE_EXITS: [BootstrapState; 4] = [
    BootstrapState::Timeout,
    BootstrapState::Orphaned,
    BootstrapState::Conflict,
    BootstrapState::Aborted,
];

/// The frozen matrix, restated independently of the implementation:
/// issued→spawned→correlated→acked→completed; any non-terminal → any
/// failure exit; terminal states immutable; self-loops are not transitions.
fn expect_legal(from: BootstrapState, to: BootstrapState) -> bool {
    use BootstrapState::*;
    let forward = matches!(
        (from, to),
        (Issued, Spawned) | (Spawned, Correlated) | (Correlated, Acked) | (Acked, Completed)
    );
    let nonterminal = matches!(from, Issued | Spawned | Correlated | Acked);
    forward || (nonterminal && FAILURE_EXITS.contains(&to))
}

fn minimal_request(instance: BackendInstanceUid) -> IssuedRequest {
    IssuedRequest {
        request_uid: Uuid::new_v4(),
        operation_uid: None,
        space_uid: None,
        backend_instance: instance,
        server_epoch: ServerEpoch(Uuid::new_v4()),
        intended_parent: None,
        recovery_generation: None,
        manifest_node_path: None,
    }
}

/// Issue a fresh request and walk it to `target` along legal edges only.
fn drive_to(reg: &mut Registry, instance: BackendInstanceUid, target: BootstrapState) -> Uuid {
    use BootstrapState::*;
    let request = minimal_request(instance);
    let uid = request.request_uid;
    reg.bootstrap_issue(&request).unwrap();
    let depth = match target {
        Issued => 0,
        Spawned => 1,
        Correlated => 2,
        Acked => 3,
        Completed => 4,
        Timeout | Orphaned | Conflict | Aborted => {
            reg.bootstrap_state(uid, target).unwrap();
            return uid;
        }
    };
    if depth >= 1 {
        reg.bootstrap_spawned(uid, r#"{"pane_id":"8"}"#).unwrap();
    }
    if depth >= 2 {
        reg.bootstrap_correlated(uid, "g0.wz-3", "p0.wz-4").unwrap();
    }
    if depth >= 3 {
        reg.bootstrap_state(uid, Acked).unwrap();
    }
    if depth >= 4 {
        reg.bootstrap_state(uid, Completed).unwrap();
    }
    uid
}

#[test]
fn happy_path_journals_every_field_from_issue_to_completed() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    // Real spaces/operations rows so the optional foreign keys are exercised.
    let r = reserve(&mut reg, "proj", instance);
    let epoch = ServerEpoch(Uuid::new_v4());
    let request = IssuedRequest {
        request_uid: Uuid::new_v4(),
        operation_uid: Some(r.operation_uid),
        space_uid: Some(r.space_uid),
        backend_instance: instance,
        server_epoch: epoch,
        intended_parent: Some("@3".into()),
        recovery_generation: Some("gen-7".into()),
        manifest_node_path: Some("/proj/split-0".into()),
    };
    let uid = request.request_uid;

    reg.bootstrap_issue(&request).unwrap();
    let row = reg.bootstrap_request(uid).unwrap().unwrap();
    assert_eq!(row.request_uid, uid);
    assert_eq!(row.operation_uid, Some(r.operation_uid));
    assert_eq!(row.space_uid, Some(r.space_uid));
    assert_eq!(row.backend_instance, instance);
    assert_eq!(row.server_epoch, epoch);
    assert_eq!(row.intended_parent.as_deref(), Some("@3"));
    assert_eq!(row.recovery_generation.as_deref(), Some("gen-7"));
    assert_eq!(row.manifest_node_path.as_deref(), Some("/proj/split-0"));
    assert_eq!(row.returned_native_ids, None);
    assert_eq!(row.final_group_ref, None);
    assert_eq!(row.final_split_ref, None);
    assert_eq!(row.state, BootstrapState::Issued);
    assert!(!row.created_at.is_empty());
    assert_eq!(row.created_at, row.updated_at);

    reg.bootstrap_spawned(uid, r#"{"pane_id":"8","window_id":"2"}"#)
        .unwrap();
    let row = reg.bootstrap_request(uid).unwrap().unwrap();
    assert_eq!(row.state, BootstrapState::Spawned);
    assert_eq!(
        row.returned_native_ids.as_deref(),
        Some(r#"{"pane_id":"8","window_id":"2"}"#)
    );

    let (group, split) = (format!("g{}.wz-2", epoch.0), format!("p{}.wz-8", epoch.0));
    reg.bootstrap_correlated(uid, &group, &split).unwrap();
    let row = reg.bootstrap_request(uid).unwrap().unwrap();
    assert_eq!(row.state, BootstrapState::Correlated);
    assert_eq!(row.final_group_ref.as_deref(), Some(group.as_str()));
    assert_eq!(row.final_split_ref.as_deref(), Some(split.as_str()));

    reg.bootstrap_state(uid, BootstrapState::Acked).unwrap();
    assert_eq!(
        reg.bootstrap_request(uid).unwrap().unwrap().state,
        BootstrapState::Acked
    );
    reg.bootstrap_state(uid, BootstrapState::Completed).unwrap();

    // The completed row retains the full correlation evidence.
    let row = reg.bootstrap_request(uid).unwrap().unwrap();
    assert_eq!(row.state, BootstrapState::Completed);
    assert!(row.returned_native_ids.is_some());
    assert_eq!(row.final_group_ref.as_deref(), Some(group.as_str()));
    assert_eq!(row.final_split_ref.as_deref(), Some(split.as_str()));

    // Never-issued UIDs read back as None, not an error.
    assert!(reg.bootstrap_request(Uuid::new_v4()).unwrap().is_none());
}

#[test]
fn transition_matrix_is_exactly_the_frozen_state_machine() {
    // Pin the matrix size first: 4 forward edges + 4 non-terminal states
    // × 4 failure exits = 20 legal transitions out of 81 pairs.
    let legal_count = ALL_STATES
        .iter()
        .flat_map(|&from| ALL_STATES.iter().map(move |&to| (from, to)))
        .filter(|&(from, to)| bootstrap_can_transition(from, to))
        .count();
    assert_eq!(legal_count, 20);
    for &from in &ALL_STATES {
        for &to in &ALL_STATES {
            assert_eq!(
                bootstrap_can_transition(from, to),
                expect_legal(from, to),
                "{} -> {}",
                from.as_str(),
                to.as_str()
            );
        }
    }

    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // Every illegal pair is rejected typed and leaves the row untouched;
    // every legal pair succeeds from a fresh row.
    for &from in &ALL_STATES {
        let uid = drive_to(&mut reg, instance, from);
        for &to in &ALL_STATES {
            if expect_legal(from, to) {
                let fresh = drive_to(&mut reg, instance, from);
                reg.bootstrap_state(fresh, to).unwrap();
                assert_eq!(reg.bootstrap_request(fresh).unwrap().unwrap().state, to);
            } else {
                let err = reg.bootstrap_state(uid, to).unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::OperationFailed,
                    "{} -> {}",
                    from.as_str(),
                    to.as_str()
                );
                assert!(err.message.contains("illegal bootstrap transition"));
                assert!(err.message.contains(from.as_str()));
                assert_eq!(reg.bootstrap_request(uid).unwrap().unwrap().state, from);
            }
        }
    }
}

#[test]
fn terminal_states_are_immutable_through_every_entry_point() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let terminal: Vec<BootstrapState> = ALL_STATES
        .into_iter()
        .filter(|&state| bootstrap_is_terminal(state))
        .collect();
    assert_eq!(terminal.len(), 5); // completed + the four failure exits

    for &state in &terminal {
        let uid = drive_to(&mut reg, instance, state);
        let before = reg.bootstrap_request(uid).unwrap().unwrap();
        for &to in &ALL_STATES {
            let err = reg.bootstrap_state(uid, to).unwrap_err();
            assert_eq!(err.code, ErrorCode::OperationFailed);
        }
        // The dedicated payload-recording methods refuse too.
        let err = reg.bootstrap_spawned(uid, "{}").unwrap_err();
        assert_eq!(err.code, ErrorCode::OperationFailed);
        let err = reg.bootstrap_correlated(uid, "g", "p").unwrap_err();
        assert_eq!(err.code, ErrorCode::OperationFailed);
        // Nothing changed at all.
        assert_eq!(reg.bootstrap_request(uid).unwrap().unwrap(), before);
    }
}

#[test]
fn every_nonterminal_state_reaches_each_failure_exit() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    for from in [
        BootstrapState::Issued,
        BootstrapState::Spawned,
        BootstrapState::Correlated,
        BootstrapState::Acked,
    ] {
        for exit in FAILURE_EXITS {
            let uid = drive_to(&mut reg, instance, from);
            reg.bootstrap_state(uid, exit).unwrap();
            let row = reg.bootstrap_request(uid).unwrap().unwrap();
            assert_eq!(row.state, exit, "{} -> {}", from.as_str(), exit.as_str());
        }
    }
}

#[test]
fn dedicated_methods_enforce_their_position_in_the_chain() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // Correlate cannot skip the spawn record.
    let uid = drive_to(&mut reg, instance, BootstrapState::Issued);
    let err = reg.bootstrap_correlated(uid, "g", "p").unwrap_err();
    assert_eq!(err.code, ErrorCode::OperationFailed);
    assert!(err.message.contains("issued -> correlated"));

    // Spawn is recorded once; a second record is illegal.
    let uid = drive_to(&mut reg, instance, BootstrapState::Spawned);
    let err = reg.bootstrap_spawned(uid, "{}").unwrap_err();
    assert_eq!(err.code, ErrorCode::OperationFailed);

    // Unknown request UIDs are typed not-found on every mutator.
    let ghost = Uuid::new_v4();
    for err in [
        reg.bootstrap_spawned(ghost, "{}").unwrap_err(),
        reg.bootstrap_correlated(ghost, "g", "p").unwrap_err(),
        reg.bootstrap_state(ghost, BootstrapState::Aborted)
            .unwrap_err(),
    ] {
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}

#[test]
fn bootstrap_journal_never_advances_the_authority_revision() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let head = reg.authority_head().unwrap();
    drive_to(&mut reg, instance, BootstrapState::Completed);
    drive_to(&mut reg, instance, BootstrapState::Timeout);
    assert_eq!(reg.authority_head().unwrap(), head);
}

#[test]
fn concurrent_issue_of_one_request_uid_is_a_typed_identity_conflict() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let request = minimal_request(instance);
    reg.bootstrap_issue(&request).unwrap();

    // A second broker process (own connection) claiming the same UID hits
    // the primary key and surfaces typed — never a raw SQLite error.
    let mut other = open(&s.config);
    let err = other.bootstrap_issue(&request).unwrap_err();
    assert_eq!(err.code, ErrorCode::IdentityConflict);
    assert!(err.message.contains(&request.request_uid.to_string()));

    // Same-connection reissue conflicts identically, and the original row
    // is untouched (still issued, original epoch).
    let err = reg.bootstrap_issue(&request).unwrap_err();
    assert_eq!(err.code, ErrorCode::IdentityConflict);
    let row = reg.bootstrap_request(request.request_uid).unwrap().unwrap();
    assert_eq!(row.state, BootstrapState::Issued);
    assert_eq!(row.server_epoch, request.server_epoch);
}

// ---------------------------------------------------------------------------
// Backend server-epoch publication (ADR 001/002)

#[test]
fn publish_backend_server_round_trips_and_advances_the_revision_chain() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // Unpublished: the record exists, every incarnation field is None.
    let stopped = reg.backend_server(instance).unwrap();
    assert_eq!(stopped.server_epoch, None);
    assert_eq!(stopped.server_pid, None);
    assert_eq!(stopped.server_start_token, None);
    assert_eq!(stopped.socket_dev, None);
    assert_eq!(stopped.socket_ino, None);

    let before = reg.authority_head().unwrap();
    let epoch1 = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(
        instance,
        epoch1,
        Some(4321),
        Some("boot-1"),
        Some(66),
        Some(9099),
    )
    .unwrap();

    let live = reg.backend_server(instance).unwrap();
    assert_eq!(live.server_epoch, Some(epoch1));
    assert_eq!(live.server_pid, Some(4321));
    assert_eq!(live.server_start_token.as_deref(), Some("boot-1"));
    assert_eq!(live.socket_dev, Some(66));
    assert_eq!(live.socket_ino, Some(9099));

    // Epoch publication is an identity mutation: exactly one chain advance.
    let after = reg.authority_head().unwrap();
    assert_eq!(after.revision, before.revision + 1);
    assert_ne!(after.head_hash, before.head_hash);
}

#[test]
fn republishing_a_new_epoch_fully_replaces_the_old_incarnation() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let epoch1 = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(
        instance,
        epoch1,
        Some(4321),
        Some("boot-1"),
        Some(66),
        Some(9099),
    )
    .unwrap();
    let head1 = reg.authority_head().unwrap();

    // Restart: a fresh epoch with different (and partly absent) witnesses.
    let epoch2 = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(instance, epoch2, None, Some("boot-2"), Some(66), Some(9871))
        .unwrap();
    let restarted = reg.backend_server(instance).unwrap();
    assert_eq!(restarted.server_epoch, Some(epoch2));
    assert_ne!(restarted.server_epoch, Some(epoch1)); // the old epoch is gone
    assert_eq!(restarted.server_pid, None); // overwritten, not merged
    assert_eq!(restarted.server_start_token.as_deref(), Some("boot-2"));
    assert_eq!(restarted.socket_ino, Some(9871));
    let head2 = reg.authority_head().unwrap();
    assert_eq!(head2.revision, head1.revision + 1);

    // Unknown instances are typed not-found on publish and readback.
    let ghost = BackendInstanceUid(Uuid::new_v4());
    let err = reg
        .publish_backend_server(ghost, epoch2, None, None, None, None)
        .unwrap_err();
    assert!(matches!(err, RegistryError::NotFound { .. }));
    assert_eq!(err.error_code(), ErrorCode::NotFound);
    assert!(matches!(
        reg.backend_server(ghost).unwrap_err(),
        RegistryError::NotFound { .. }
    ));
    // The failed publish advanced nothing.
    assert_eq!(reg.authority_head().unwrap(), head2);
}

// ---------------------------------------------------------------------------
// Backend instance registration is exact on the endpoint (ADR 012 WS-A.12)

/// `register_backend_instance` is get-or-create on `(owner, backend)`, and
/// the "get" must not hand back the owner's one instance for a *different*
/// endpoint: every caller goes on to fence and scan under the returned uid,
/// so instance A's lock would cover a read of endpoint B (review report 07's
/// `repair_scan_wez` residual). The refusal is typed, names both endpoints,
/// and changes nothing — not the row, not the revision chain.
#[test]
fn registering_the_same_backend_at_another_endpoint_is_a_typed_refusal() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = reg
        .register_backend_instance(Backend::Wez, Some("/run/dmux/a.sock"), Some("svc"))
        .unwrap();
    let head = reg.authority_head().unwrap();

    let err = reg
        .register_backend_instance(Backend::Wez, Some("/run/dmux/b.sock"), None)
        .unwrap_err();
    match &err {
        RegistryError::EndpointMismatch {
            backend,
            instance: named,
            recorded,
            requested,
        } => {
            assert_eq!(*backend, Backend::Wez);
            assert_eq!(*named, instance);
            assert_eq!(recorded.as_deref(), Some("/run/dmux/a.sock"));
            assert_eq!(requested, "/run/dmux/b.sock");
        }
        other => panic!("expected EndpointMismatch, got {other:?}"),
    }
    assert_eq!(err.error_code(), ErrorCode::WrongBackendInstance);
    let text = err.to_string();
    assert!(
        text.contains("/run/dmux/a.sock") && text.contains("/run/dmux/b.sock"),
        "{text}"
    );
    assert_eq!(
        reg.authority_head().unwrap(),
        head,
        "a refusal advances nothing"
    );
    assert_eq!(
        reg.backend_instance_info(instance)
            .unwrap()
            .socket_path
            .as_deref(),
        Some("/run/dmux/a.sock"),
        "the recorded endpoint is untouched"
    );

    // The same endpoint, or no endpoint, is the ordinary get.
    assert_eq!(
        reg.register_backend_instance(Backend::Wez, Some("/run/dmux/a.sock"), None)
            .unwrap(),
        instance
    );
    assert_eq!(
        reg.register_backend_instance(Backend::Wez, None, None)
            .unwrap(),
        instance
    );
    assert_eq!(reg.authority_head().unwrap(), head);

    // A row registered without an endpoint stays unaddressable: a later
    // caller naming one is neither refused nor allowed to bind it here.
    let tmux = reg
        .register_backend_instance(Backend::Tmux, None, None)
        .unwrap();
    assert_eq!(
        reg.register_backend_instance(Backend::Tmux, Some("dmux"), None)
            .unwrap(),
        tmux
    );
    assert_eq!(reg.backend_instance_info(tmux).unwrap().socket_path, None);
}
