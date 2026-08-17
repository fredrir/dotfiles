//! Stable previous/current Space history (SpaceUid-keyed) and legacy
//! name-based conversion (plan §9.2, §17 step 11).

use dmux::bootstrap::MarkerContext;
use dmux::history::{
    ConvertDropReason, GuiHistoryTarget, History, LegacyEntry, PendingGuiTransition,
    convert_legacy_entries,
};
use dmux::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceNo, SpaceUid};
use std::num::NonZeroU64;
use std::sync::{Arc, Barrier};
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
fn gui_history_records_one_cross_host_last_presented_identity() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let (h1, h2) = (host(), host());
    let (a, b) = (space(), space());

    // Ordinary terminal attachment history must not silently become a GUI
    // summon target.
    history.record_attach(h1, a).unwrap();
    assert_eq!(history.last_gui_presented(), None);

    history.record_gui_present(h1, a).unwrap();
    assert_eq!(history.previous_gui_presented(), None);
    assert_eq!(
        history.last_gui_presented(),
        Some(dmux::history::GuiHistoryTarget {
            host_uid: h1,
            space_uid: a,
        })
    );
    history.record_gui_present(h2, b).unwrap();
    assert_eq!(
        history.last_gui_presented(),
        Some(dmux::history::GuiHistoryTarget {
            host_uid: h2,
            space_uid: b,
        })
    );
    assert_eq!(history.current(h1), Some(a));
    assert_eq!(history.current(h2), Some(b));
    assert_eq!(
        history.previous_gui_presented(),
        Some(dmux::history::GuiHistoryTarget {
            host_uid: h1,
            space_uid: a,
        })
    );
    // Re-presenting the same target is idempotent and does not rotate the
    // global previous slot.
    history.record_gui_present(h2, b).unwrap();
    assert_eq!(
        history.previous_gui_presented(),
        Some(dmux::history::GuiHistoryTarget {
            host_uid: h1,
            space_uid: a,
        })
    );
}

#[test]
fn exact_gui_transition_preserves_the_visible_source_in_one_update() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let (h1, h2, stale_host) = (host(), host(), host());
    let (source, destination, stale) = (space(), space(), space());

    history.record_gui_present(stale_host, stale).unwrap();
    history
        .record_gui_transition(
            GuiHistoryTarget {
                host_uid: h1,
                space_uid: source,
            },
            GuiHistoryTarget {
                host_uid: h2,
                space_uid: destination,
            },
        )
        .unwrap();

    assert_eq!(
        history.previous_gui_presented(),
        Some(GuiHistoryTarget {
            host_uid: h1,
            space_uid: source,
        })
    );
    assert_eq!(
        history.last_gui_presented(),
        Some(GuiHistoryTarget {
            host_uid: h2,
            space_uid: destination,
        })
    );
    assert_eq!(history.current(h1), Some(source));
    assert_eq!(history.current(h2), Some(destination));

    // Repeating the already-visible destination cannot rotate the source.
    history
        .record_gui_transition(
            GuiHistoryTarget {
                host_uid: h2,
                space_uid: destination,
            },
            GuiHistoryTarget {
                host_uid: h2,
                space_uid: destination,
            },
        )
        .unwrap();
    assert_eq!(
        history.previous_gui_presented(),
        Some(GuiHistoryTarget {
            host_uid: h1,
            space_uid: source,
        })
    );
}

#[test]
fn concurrent_pending_finalizer_and_writer_do_not_lose_either_update() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(dir.path());
    let (source_host, destination_host, independent_host) = (host(), host(), host());
    let (source_space, destination_space, independent_space) = (space(), space(), space());
    let backend_instance = BackendInstanceUid(Uuid::new_v4());
    let server_epoch = ServerEpoch(Uuid::new_v4());
    let pending = PendingGuiTransition {
        tmux_client_uid: Uuid::new_v4(),
        source: GuiHistoryTarget {
            host_uid: source_host,
            space_uid: source_space,
        },
        destination: GuiHistoryTarget {
            host_uid: destination_host,
            space_uid: destination_space,
        },
        destination_backend_instance_uid: backend_instance,
        destination_marker: MarkerContext {
            host_uid: destination_host,
            space_uid: destination_space,
            space_no: SpaceNo(NonZeroU64::new(9).unwrap()),
            backend: Backend::Tmux,
            domain: Some("local".into()),
            server_epoch,
            group_ref: "g00000000-0000-0000-0000-000000000001.@0".into(),
            split_ref: "p00000000-0000-0000-0000-000000000001.%0".into(),
        },
        destination_child_kind: None,
        gui_instance: "gui-42-concurrent".into(),
        gui_pid: 42,
        gui_process_start_token: "token".into(),
        gui_pane_id: 7,
        gui_domain: "local".into(),
        expires_at: u64::MAX,
    };
    history.stage_gui_transition(pending.clone()).unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let finalizer_history = history.clone();
    let finalizer_barrier = Arc::clone(&barrier);
    let expected = pending.clone();
    let finalizer = std::thread::spawn(move || {
        finalizer_barrier.wait();
        finalizer_history
            .complete_gui_transition(&expected)
            .unwrap()
    });
    let writer_history = history.clone();
    let writer_barrier = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        writer_history
            .record_attach(independent_host, independent_space)
            .unwrap();
    });
    barrier.wait();
    assert!(finalizer.join().unwrap());
    writer.join().unwrap();

    assert_eq!(history.current(independent_host), Some(independent_space));
    assert_eq!(
        history.last_gui_presented(),
        Some(GuiHistoryTarget {
            host_uid: destination_host,
            space_uid: destination_space,
        })
    );
    assert!(
        history
            .pending_gui_transition(pending.tmux_client_uid)
            .is_none()
    );
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
