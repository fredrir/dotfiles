//! Single-use attach tokens (plan §12.1, ADR 009 §3): atomic guarded
//! redemption (two concurrent redeemers, exactly one winner), typed
//! expiry/replay/unknown, hash-only storage, audit retention.

use std::sync::Barrier;
use std::time::{Duration, SystemTime};

use dmux::error::ErrorCode;
use dmux::model::{ServerEpoch, SpaceUid};
use dmux::registry::sha256::sha256_hex;
use dmux::registry::{
    AttachRedemption, AttachTokenSpec, Registry, RegistryError, now_rfc3339, rfc3339_utc,
};
use uuid::Uuid;

use crate::util::{open, scratch};

fn head(reg: &Registry) -> u64 {
    reg.authority_head().unwrap().revision
}

fn token_spec(reg: &Registry, token: &str, expires_at: String) -> AttachTokenSpec {
    AttachTokenSpec {
        token_hash: sha256_hex(token.as_bytes()),
        request_uid: Uuid::new_v4(),
        host_uid: reg.identity().unwrap().host_uid,
        space_uid: SpaceUid(Uuid::now_v7()),
        server_epoch: ServerEpoch(Uuid::new_v4()),
        route: "archie-usb".into(),
        attach_argv: vec![
            "tmux".into(),
            "-L".into(),
            "dmux".into(),
            "attach".into(),
            "-t".into(),
            "=proj".into(),
        ],
        issued_at: now_rfc3339(),
        expires_at,
    }
}

fn future(secs: u64) -> String {
    rfc3339_utc(SystemTime::now() + Duration::from_secs(secs))
}

#[test]
fn issue_and_redeem_round_trip_only_the_hash_is_stored() {
    let s = scratch();
    let mut reg = open(&s.config);
    let token = "an-opaque-single-use-token";
    let spec = token_spec(&reg, token, future(120));

    let before = head(&reg);
    reg.issue_attach_token(&spec).unwrap();
    assert_eq!(head(&reg), before, "tokens never advance the chain");

    // The token itself appears nowhere in the database.
    let n: i64 = reg
        .raw_connection()
        .query_row(
            "SELECT count(*) FROM attach_tokens WHERE token_hash = ?1",
            [&spec.token_hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);

    let redeemed = match reg
        .redeem_attach_token(&sha256_hex(token.as_bytes()), &now_rfc3339())
        .unwrap()
    {
        AttachRedemption::Redeemed(r) => r,
        other => panic!("expected Redeemed, got {other:?}"),
    };
    assert_eq!(redeemed.request_uid, spec.request_uid);
    assert_eq!(redeemed.host_uid, spec.host_uid);
    assert_eq!(redeemed.space_uid, spec.space_uid);
    assert_eq!(redeemed.server_epoch, spec.server_epoch);
    assert_eq!(redeemed.route, spec.route);
    assert_eq!(redeemed.attach_argv, spec.attach_argv);
    assert!(!redeemed.redeemed_at.is_empty());
    assert_eq!(head(&reg), before, "redemption never advances the chain");
}

#[test]
fn replay_is_typed_and_the_row_is_retained_for_audit() {
    let s = scratch();
    let mut reg = open(&s.config);
    let spec = token_spec(&reg, "once", future(120));
    reg.issue_attach_token(&spec).unwrap();

    let now = now_rfc3339();
    assert!(matches!(
        reg.redeem_attach_token(&spec.token_hash, &now).unwrap(),
        AttachRedemption::Redeemed(_)
    ));
    assert!(matches!(
        reg.redeem_attach_token(&spec.token_hash, &now).unwrap(),
        AttachRedemption::Replayed
    ));
    let state: String = reg
        .raw_connection()
        .query_row(
            "SELECT state FROM attach_tokens WHERE token_hash = ?1",
            [&spec.token_hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "redeemed");
}

#[test]
fn expiry_is_typed_marked_and_never_deleted() {
    let s = scratch();
    let mut reg = open(&s.config);
    let spec = token_spec(
        &reg,
        "late",
        rfc3339_utc(SystemTime::now() - Duration::from_secs(60)),
    );
    reg.issue_attach_token(&spec).unwrap();

    let now = now_rfc3339();
    assert!(matches!(
        reg.redeem_attach_token(&spec.token_hash, &now).unwrap(),
        AttachRedemption::Expired
    ));
    // The row is marked expired and retained.
    let state: String = reg
        .raw_connection()
        .query_row(
            "SELECT state FROM attach_tokens WHERE token_hash = ?1",
            [&spec.token_hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "expired");
    assert!(matches!(
        reg.redeem_attach_token(&spec.token_hash, &now).unwrap(),
        AttachRedemption::Expired
    ));

    // Boundary: expires_at == now is already expired (strict >).
    let boundary = token_spec(&reg, "boundary", now.clone());
    reg.issue_attach_token(&boundary).unwrap();
    assert!(matches!(
        reg.redeem_attach_token(&boundary.token_hash, &now).unwrap(),
        AttachRedemption::Expired
    ));
}

#[test]
fn unknown_hashes_and_duplicate_issues_are_typed() {
    let s = scratch();
    let mut reg = open(&s.config);
    assert!(matches!(
        reg.redeem_attach_token(&sha256_hex(b"never-issued"), &now_rfc3339())
            .unwrap(),
        AttachRedemption::Unknown
    ));

    let spec = token_spec(&reg, "dup", future(120));
    reg.issue_attach_token(&spec).unwrap();
    // Same hash again.
    let err = reg.issue_attach_token(&spec).unwrap_err();
    assert!(matches!(err, RegistryError::AttachTokenExists { .. }));
    assert_eq!(err.error_code(), ErrorCode::IdentityConflict);
    // Same request UID under a different hash.
    let mut reused = token_spec(&reg, "other-token", future(120));
    reused.request_uid = spec.request_uid;
    let err = reg.issue_attach_token(&reused).unwrap_err();
    assert_eq!(err.error_code(), ErrorCode::IdentityConflict);
}

#[test]
fn two_concurrent_redeemers_produce_exactly_one_redeemed() {
    let s = scratch();
    let mut reg = open(&s.config);
    let spec = token_spec(&reg, "contended", future(120));
    reg.issue_attach_token(&spec).unwrap();
    drop(reg);

    let barrier = Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let config = s.config.clone();
                let hash = spec.token_hash.clone();
                let barrier = &barrier;
                scope.spawn(move || {
                    // A real second client: its own connection to the same
                    // database file.
                    let mut reg = Registry::open(config).unwrap();
                    barrier.wait();
                    reg.redeem_attach_token(&hash, &now_rfc3339()).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });

    let redeemed = outcomes
        .iter()
        .filter(|o| matches!(o, AttachRedemption::Redeemed(_)))
        .count();
    let replayed = outcomes
        .iter()
        .filter(|o| matches!(o, AttachRedemption::Replayed))
        .count();
    assert_eq!(redeemed, 1, "exactly one winner: {outcomes:?}");
    assert_eq!(replayed, 1, "the loser sees a typed replay: {outcomes:?}");
}
