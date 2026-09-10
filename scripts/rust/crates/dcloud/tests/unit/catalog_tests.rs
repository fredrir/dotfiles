use super::*;

#[test]
fn raw_snapshot_cache_entries_have_the_same_browser_shape() {
    let raw = json!({"id":"snapshot-id","time":"2026-09-01T00:00:00Z","hostname":"archie","tree":"tree-id","tags":["dcloud.job:Documents","dcloud.category:documents","dcloud.label:personal","dcloud.pin"]});
    let row = normalize(raw, "vps");
    assert_eq!(row["kind"], "snapshot");
    assert_eq!(row["host"], "archie");
    assert_eq!(row["job"], "Documents");
    assert_eq!(row["labels"], json!(["personal"]));
    assert_eq!(row["pinned"], true);
}

#[test]
fn only_authoritative_refreshes_remove_stale_cache_entries() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::open(temp.path()).unwrap();
    state
        .cache_manifest("drive", "old", &json!({"id":"old"}))
        .unwrap();
    let fresh = vec![("new".to_string(), json!({"id":"new"}))];
    cache_rows(&mut state, "drive", &fresh, false).unwrap();
    assert_eq!(state.cached_manifests::<Value>("drive").unwrap().len(), 2);
    cache_rows(&mut state, "drive", &fresh, true).unwrap();
    let rows = state.cached_manifests::<Value>("drive").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "new");
    cache_rows(&mut state, "drive", &[], true).unwrap();
    assert!(state.cached_manifests::<Value>("drive").unwrap().is_empty());
}
