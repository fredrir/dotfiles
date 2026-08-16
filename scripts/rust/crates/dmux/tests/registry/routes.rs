//! Route records (plan §12.3, ADR 009 §3): upsert keyed on
//! (host_uid, transport, endpoint), priority ordering, enable/disable, and
//! diagnostic outcome recording — with the revision-advance policy pinned.

use dmux::error::ErrorCode;
use dmux::model::HostUid;
use dmux::registry::{NetworkClass, Registry, RouteSpec, Transport};
use uuid::Uuid;

use crate::util::{open, scratch};

fn head(reg: &Registry) -> u64 {
    reg.authority_head().unwrap().revision
}

fn spec(host: HostUid, transport: Transport, endpoint: &str, priority: i64) -> RouteSpec {
    RouteSpec {
        host_uid: host,
        transport,
        endpoint: endpoint.into(),
        username: Some("fredrir".into()),
        wez_domain: None,
        network_class: NetworkClass::Usb,
        priority,
        required_capability: None,
        trust_fingerprint: Some("SHA256:abc".into()),
        enabled: true,
    }
}

#[test]
fn upsert_is_keyed_on_host_transport_endpoint() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();

    let before = head(&reg);
    let id1 = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie", 10))
        .unwrap();
    assert_eq!(head(&reg), before + 1, "new route advances the revision");

    // Mark a diagnostic outcome, then upsert the same key with changes:
    // same route_id, fields replaced, diagnostics preserved.
    reg.record_route_outcome(id1, "complete").unwrap();
    let mut update = spec(h2, Transport::Openssh, "archie", 5);
    update.username = Some("fh".into());
    update.trust_fingerprint = Some("SHA256:def".into());
    let id_again = reg.upsert_route(&update).unwrap();
    assert_eq!(id_again, id1);
    let routes = reg.routes_for(h2).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].priority, 5);
    assert_eq!(routes[0].username.as_deref(), Some("fh"));
    assert_eq!(routes[0].trust_fingerprint.as_deref(), Some("SHA256:def"));
    assert_eq!(routes[0].last_outcome.as_deref(), Some("complete"));

    // An identical spec is a pure no-op: same id, no revision advance.
    let before = head(&reg);
    assert_eq!(reg.upsert_route(&update).unwrap(), id1);
    assert_eq!(head(&reg), before);

    // A different endpoint (or transport) is a different route.
    let id2 = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie-ts", 20))
        .unwrap();
    let id3 = reg
        .upsert_route(&spec(h2, Transport::WezSsh, "archie", 30))
        .unwrap();
    assert_ne!(id1, id2);
    assert_ne!(id1, id3);
    assert_eq!(reg.routes_for(h2).unwrap().len(), 3);
}

#[test]
fn routes_for_returns_priority_order_including_disabled_rows() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();

    let slow = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie-ts", 20))
        .unwrap();
    let fast = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie-usb", 10))
        .unwrap();
    reg.set_route_enabled(slow, false).unwrap();

    let routes = reg.routes_for(h2).unwrap();
    assert_eq!(
        routes.iter().map(|r| r.route_id).collect::<Vec<_>>(),
        vec![fast, slow],
        "lower priority first"
    );
    assert!(routes[0].enabled);
    assert!(!routes[1].enabled, "disabled rows stay visible");
}

#[test]
fn upsert_requires_an_enrolled_host() {
    let s = scratch();
    let mut reg = open(&s.config);
    let ghost = HostUid(Uuid::new_v4());
    let err = reg
        .upsert_route(&spec(ghost, Transport::Openssh, "nowhere", 1))
        .unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);

    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();
    reg.upsert_route(&spec(h2, Transport::Openssh, "archie", 1))
        .unwrap();
    reg.forget_host(h2).unwrap();
    let err = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie2", 2))
        .unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound, "tombstoned host");
}

#[test]
fn enable_flag_flips_advance_the_revision_and_no_ops_do_not() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();
    let id = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie", 10))
        .unwrap();

    let before = head(&reg);
    reg.set_route_enabled(id, false).unwrap();
    assert_eq!(head(&reg), before + 1, "a flip is an authority mutation");
    assert!(!reg.routes_for(h2).unwrap()[0].enabled);

    reg.set_route_enabled(id, false).unwrap();
    assert_eq!(head(&reg), before + 1, "same-value set is a no-op");

    let err = reg.set_route_enabled(99_999, true).unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}

#[test]
fn outcome_recording_is_diagnostics_and_never_advances_the_chain() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();
    let id = reg
        .upsert_route(&spec(h2, Transport::Openssh, "archie", 10))
        .unwrap();

    let before = head(&reg);
    reg.record_route_outcome(id, "timeout").unwrap();
    reg.record_route_outcome(id, "complete").unwrap();
    assert_eq!(head(&reg), before, "outcomes never advance the chain");

    let row = &reg.routes_for(h2).unwrap()[0];
    assert_eq!(row.last_outcome.as_deref(), Some("complete"));
    assert!(row.last_outcome_at.is_some());

    let err = reg.record_route_outcome(99_999, "complete").unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}
