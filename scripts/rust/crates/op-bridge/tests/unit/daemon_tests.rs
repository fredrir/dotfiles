use super::*;

#[test]
fn known_references_round_trip_and_stay_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known.json");
    let known = vec![
        "op://Dev/a/credential".to_string(),
        "op://Dev/b/credential".to_string(),
    ];
    save_known(&path, &known).unwrap();
    assert_eq!(load_known(&path).into_iter().collect::<Vec<_>>(), known);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_missing_or_corrupt_known_file_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known.json");
    assert!(load_known(&path).is_empty());
    fs::write(&path, "not json").unwrap();
    assert!(load_known(&path).is_empty());
}
