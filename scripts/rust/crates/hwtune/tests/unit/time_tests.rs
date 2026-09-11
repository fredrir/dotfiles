use super::*;

#[test]
fn rapid_sessions_get_distinct_identifiers() {
    let identifiers = (0..256)
        .map(|_| session_id("cpu"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identifiers.len(), 256);
    assert!(identifiers.iter().all(|id| !id.contains(['/', '\\'])));
}
