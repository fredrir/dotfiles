//! P7 identity surface (ADR 009 §3): enrollment idempotence, bijective
//! alias sequencing without reuse, forget semantics, and the
//! never-rebind-a-spelling guarantee.

use dmux::error::ErrorCode;
use dmux::model::HostUid;
use dmux::registry::{HostLifecycle, NetworkClass, Registry, RegistryError, RouteSpec, Transport};
use uuid::Uuid;

use crate::util::{open, scratch};

fn head(reg: &Registry) -> u64 {
    reg.authority_head().unwrap().revision
}

fn route_spec(host: HostUid, endpoint: &str, priority: i64) -> RouteSpec {
    RouteSpec {
        host_uid: host,
        transport: Transport::Openssh,
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
fn the_local_host_is_preminted_as_alias_a_and_enroll_is_idempotent() {
    let s = scratch();
    let mut reg = open(&s.config);
    let id = reg.identity().unwrap();

    let hosts = reg.hosts().unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].host_uid, id.host_uid);
    assert_eq!(hosts[0].alias.as_deref(), Some("a"));
    assert_eq!(hosts[0].lifecycle, HostLifecycle::Enrolled);

    let before = head(&reg);
    let enrolled = reg.enroll_host(id.host_uid, None).unwrap();
    assert_eq!(enrolled.alias, "a");
    assert!(!enrolled.newly_enrolled);
    assert!(!enrolled.reactivated);
    // Pure no-op: the authority revision must not advance.
    assert_eq!(head(&reg), before);
}

#[test]
fn enrollment_allocates_the_next_alias_and_is_idempotent_by_host_uid() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    let h3 = HostUid(Uuid::new_v4());

    let before = head(&reg);
    let e2 = reg.enroll_host(h2, None).unwrap();
    assert_eq!(e2.alias, "b");
    assert!(e2.newly_enrolled);
    assert_eq!(
        head(&reg),
        before + 1,
        "enrollment is an authority mutation"
    );

    // Idempotent re-enroll: same alias, no revision advance.
    let again = reg.enroll_host(h2, None).unwrap();
    assert_eq!(again.alias, "b");
    assert!(!again.newly_enrolled);
    assert_eq!(head(&reg), before + 1);

    let e3 = reg.enroll_host(h3, None).unwrap();
    assert_eq!(e3.alias, "c");

    let found = reg.host_by_alias("b").unwrap().unwrap();
    assert_eq!(found.host_uid, h2);
    assert!(reg.host_by_alias("zz").unwrap().is_none());
}

#[test]
fn aliases_are_never_reused_and_reenrollment_reactivates_the_same_alias() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    let h3 = HostUid(Uuid::new_v4());
    let h4 = HostUid(Uuid::new_v4());

    assert_eq!(reg.enroll_host(h2, None).unwrap().alias, "b");
    assert_eq!(reg.enroll_host(h3, None).unwrap().alias, "c");
    reg.forget_host(h3).unwrap();

    // `c` stays permanently bound to h3: the next host gets `d`.
    assert_eq!(reg.enroll_host(h4, None).unwrap().alias, "d");

    // Re-enrollment is the only way back, and it restores the same alias.
    let back = reg.enroll_host(h3, None).unwrap();
    assert!(back.reactivated);
    assert!(!back.newly_enrolled);
    assert_eq!(back.alias, "c");
    let row = reg.host_by_alias("c").unwrap().unwrap();
    assert_eq!(row.host_uid, h3);
    assert_eq!(row.lifecycle, HostLifecycle::Enrolled);
    assert!(row.tombstoned_at.is_none());
}

#[test]
fn forget_refuses_the_local_host_with_a_typed_error() {
    let s = scratch();
    let mut reg = open(&s.config);
    let id = reg.identity().unwrap();

    let before = head(&reg);
    let err = reg.forget_host(id.host_uid).unwrap_err();
    assert!(
        matches!(err, RegistryError::LocalHostImmutable { host_uid } if host_uid == id.host_uid)
    );
    assert_eq!(err.error_code(), ErrorCode::Usage);
    assert_eq!(head(&reg), before, "refusal has no side effects");
    assert_eq!(reg.hosts().unwrap().len(), 1);
}

#[test]
fn forget_tombstones_refs_disables_routes_and_retains_history() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, Some("archie")).unwrap();
    reg.upsert_route(&route_spec(h2, "archie", 10)).unwrap();
    reg.upsert_route(&route_spec(h2, "archie-ts", 20)).unwrap();

    let before = head(&reg);
    reg.forget_host(h2).unwrap();
    assert_eq!(head(&reg), before + 1, "forget is an authority mutation");

    // Host row tombstoned, refs tombstoned (not deleted), no resolution.
    let row = reg
        .hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.host_uid == h2)
        .unwrap();
    assert_eq!(row.lifecycle, HostLifecycle::Tombstoned);
    assert!(row.tombstoned_at.is_some());
    assert_eq!(row.alias, None);
    assert_eq!(row.label, None);
    assert!(reg.host_by_alias("b").unwrap().is_none());
    let tombstoned: i64 = reg
        .raw_connection()
        .query_row(
            "SELECT count(*) FROM host_refs WHERE host_uid = ?1 AND state = 'tombstoned'",
            [h2.0.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tombstoned, 2, "alias and label rows retained as tombstones");

    // Routes retained but disabled.
    let routes = reg.routes_for(h2).unwrap();
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().all(|r| !r.enabled));

    // Idempotent: a second forget is a no-op that does not advance.
    reg.forget_host(h2).unwrap();
    assert_eq!(head(&reg), before + 1);

    // A never-enrolled host is typed NotFound.
    let err = reg.forget_host(HostUid(Uuid::new_v4())).unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}

#[test]
fn a_spelling_once_used_never_rebinds_to_a_different_host() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    let h3 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap(); // alias b
    reg.enroll_host(h3, None).unwrap(); // alias c

    reg.set_host_label(h2, "archie").unwrap();
    assert_eq!(
        reg.host_by_alias("b").unwrap().unwrap().label.as_deref(),
        Some("archie")
    );

    // Relabel: the old spelling becomes historical, still bound to h2.
    let before = head(&reg);
    reg.set_host_label(h2, "mac").unwrap();
    assert_eq!(head(&reg), before + 1, "labeling is an authority mutation");
    assert_eq!(
        reg.host_by_alias("b").unwrap().unwrap().label.as_deref(),
        Some("mac")
    );

    // The historical spelling never rebinds to another host.
    for spelling in ["archie", "mac", "b"] {
        let err = reg.set_host_label(h3, spelling).unwrap_err();
        assert!(
            matches!(&err, RegistryError::SpellingBound { bound_to, .. } if *bound_to == h2),
            "{spelling}: {err}"
        );
        assert_eq!(err.error_code(), ErrorCode::IdentityConflict);
    }

    // The same host may take one of its own old spellings back.
    reg.set_host_label(h2, "archie").unwrap();
    assert_eq!(
        reg.host_by_alias("b").unwrap().unwrap().label.as_deref(),
        Some("archie")
    );

    // Setting the already-current label is a no-op.
    let before = head(&reg);
    reg.set_host_label(h2, "archie").unwrap();
    assert_eq!(head(&reg), before);

    // Label validation is typed.
    let too_long = "a".repeat(33);
    for bad in ["Archie", "-x", "", "9front", too_long.as_str()] {
        let err = reg.set_host_label(h2, bad).unwrap_err();
        assert!(
            matches!(&err, RegistryError::InvalidLabel { .. }),
            "{bad:?}"
        );
        assert_eq!(err.error_code(), ErrorCode::InvalidName);
    }

    // Labeling a tombstoned or unknown host is typed NotFound.
    reg.forget_host(h3).unwrap();
    let err = reg.set_host_label(h3, "gone").unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::NotFound);
}

#[test]
fn alias_allocation_skips_spellings_already_bound_as_labels() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    let h3 = HostUid(Uuid::new_v4());

    assert_eq!(reg.enroll_host(h2, None).unwrap().alias, "b");
    // Bind the NEXT alias spelling (`c`) as h2's label.
    reg.set_host_label(h2, "c").unwrap();

    // h3 must not receive `c` — that spelling is bound to h2 forever.
    assert_eq!(reg.enroll_host(h3, None).unwrap().alias, "d");
    // And `c` resolves as nothing alias-wise (it is a label, not an alias).
    assert!(reg.host_by_alias("c").unwrap().is_none());
}

#[test]
fn enroll_with_a_conflicting_label_is_atomic() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    let h3 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, Some("archie")).unwrap();

    // The label conflict rolls back the WHOLE enrollment: no host row,
    // no alias burned... the next successful enrollment still gets `c`.
    let err = reg.enroll_host(h3, Some("archie")).unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::IdentityConflict);
    assert!(reg.hosts().unwrap().iter().all(|h| h.host_uid != h3));
    assert_eq!(reg.enroll_host(h3, None).unwrap().alias, "c");
}
