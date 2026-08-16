//! Stable previous/current Space history (SpaceUid-keyed) and legacy
//! name-based conversion (plan §9.2, §17 step 11).

use dmux::history::{ConvertDropReason, History, LegacyEntry, convert_legacy_entries};
use dmux::model::{HostUid, SpaceUid};
use uuid::Uuid;

fn host() -> HostUid {
    HostUid(Uuid::new_v4())
}

fn space() -> SpaceUid {
    SpaceUid(Uuid::now_v7())
}

#[test]
fn record_attach_shifts_current_into_previous_and_toggles() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let h = host();
    let (a, b) = (space(), space());

    history.record_attach(h, a).unwrap();
    assert_eq!(history.current(h), Some(a));
    assert_eq!(history.previous(h), None);

    history.record_attach(h, b).unwrap();
    assert_eq!(history.current(h), Some(b));
    assert_eq!(history.previous(h), Some(a));

    // Reattaching the current Space moves nothing.
    history.record_attach(h, b).unwrap();
    assert_eq!(history.previous(h), Some(a));

    // The toggle round trip: going back makes the old current previous.
    history.record_attach(h, a).unwrap();
    assert_eq!(history.current(h), Some(a));
    assert_eq!(history.previous(h), Some(b));
}

#[test]
fn history_is_identity_keyed_and_per_host() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let (h1, h2) = (host(), host());
    let (a, b, c) = (space(), space(), space());

    history.record_attach(h1, a).unwrap();
    history.record_attach(h1, b).unwrap();
    history.record_attach(h2, c).unwrap();

    // Host slots are independent.
    assert_eq!(history.previous(h1), Some(a));
    assert_eq!(history.current(h2), Some(c));
    assert_eq!(history.previous(h2), None);
    // An unknown host has no history.
    assert_eq!(history.previous(host()), None);
}

#[test]
fn history_persists_across_instances_and_survives_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let h = host();
    let (a, b) = (space(), space());
    {
        let history = History::new(dir.path());
        history.record_attach(h, a).unwrap();
        history.record_attach(h, b).unwrap();
    }
    // A fresh handle reads the persisted document.
    let history = History::new(dir.path());
    assert_eq!(history.current(h), Some(b));
    assert_eq!(history.previous(h), Some(a));

    // Corrupt content never panics; best-effort state starts fresh.
    std::fs::write(dir.path().join(dmux::history::HISTORY_FILE), b"{ not json").unwrap();
    let history = History::new(dir.path());
    assert_eq!(history.previous(h), None);
    history.record_attach(h, a).unwrap();
    assert_eq!(history.current(h), Some(a));
}

#[test]
fn legacy_conversion_unambiguous_converts_ambiguous_and_missing_warn_and_drop() {
    let unique = space();
    let entries = vec![
        LegacyEntry {
            key: "macie".into(),
            name: "dotfiles".into(),
        },
        LegacyEntry {
            key: "archie".into(),
            name: "dup".into(),
        },
        LegacyEntry {
            key: "archie:current".into(),
            name: "gone".into(),
        },
    ];
    let dup_uid = space();
    let (converted, warnings) = convert_legacy_entries(&entries, |name| match name {
        "dotfiles" => Some((unique, 1)),
        "dup" => Some((dup_uid, 2)), // cross-backend duplicate
        _ => None,
    });

    assert_eq!(converted, vec![("macie".to_string(), unique)]);
    assert_eq!(warnings.len(), 2);
    assert_eq!(warnings[0].name, "dup");
    assert_eq!(
        warnings[0].reason,
        ConvertDropReason::Ambiguous { candidates: 2 }
    );
    assert_eq!(warnings[1].name, "gone");
    assert_eq!(warnings[1].reason, ConvertDropReason::Missing);
}

#[test]
fn legacy_conversion_zero_count_is_missing_not_converted() {
    let uid = space();
    let entries = vec![LegacyEntry {
        key: "a".into(),
        name: "phantom".into(),
    }];
    // A lookup that returns a UID with count 0 must not be trusted.
    let (converted, warnings) = convert_legacy_entries(&entries, |_| Some((uid, 0)));
    assert!(converted.is_empty());
    assert_eq!(warnings[0].reason, ConvertDropReason::Missing);
}
