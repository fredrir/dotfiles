//! Peer snapshot cache (plan §12.1, ADR 009 §3): verbatim round-trip and
//! replacement over `remote_cache`. Lineage policy (conflict/stale/rollback)
//! is decided by the caller BEFORE storing — the registry never judges.

use dmux::model::{HostUid, RegistryUid};
use dmux::registry::{PeerCache, Registry, now_rfc3339};
use uuid::Uuid;

use crate::util::{open, scratch};

fn head(reg: &Registry) -> u64 {
    reg.authority_head().unwrap().revision
}

#[test]
fn peer_cache_round_trips_verbatim_and_replaces_on_store() {
    let s = scratch();
    let mut reg = open(&s.config);
    let h2 = HostUid(Uuid::new_v4());
    reg.enroll_host(h2, None).unwrap();
    assert!(reg.peer_cache(h2).unwrap().is_none());

    let checkpoint = PeerCache {
        registry_uid: RegistryUid(Uuid::new_v4()),
        authority_revision: 41,
        authority_head_hash: "sha256:aaaa".into(),
        snapshot_json: serde_json::json!({
            "spaces": [{ "space_no": 1, "name": "proj" }],
        }),
        fetched_at: now_rfc3339(),
    };
    let before = head(&reg);
    reg.store_peer_cache(h2, &checkpoint).unwrap();
    assert_eq!(head(&reg), before, "cache writes never advance the chain");
    assert_eq!(reg.peer_cache(h2).unwrap().unwrap(), checkpoint);

    // A newer checkpoint replaces the row (one checkpoint per peer).
    let newer = PeerCache {
        authority_revision: 42,
        authority_head_hash: "sha256:bbbb".into(),
        snapshot_json: serde_json::json!({ "spaces": [] }),
        ..checkpoint.clone()
    };
    reg.store_peer_cache(h2, &newer).unwrap();
    assert_eq!(reg.peer_cache(h2).unwrap().unwrap(), newer);
    let rows: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM remote_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);

    // A checkpoint needs an enrolled host row (FK).
    let ghost = HostUid(Uuid::new_v4());
    assert!(reg.store_peer_cache(ghost, &newer).is_err());

    // Forgetting the peer retains its cached history (plan §12.2).
    reg.forget_host(h2).unwrap();
    assert_eq!(reg.peer_cache(h2).unwrap().unwrap(), newer);
}
