use super::*;
use crate::bios::export;
use crate::rows::Kind;

fn spec(text: &str) -> Spec {
    crate::bios::spec::parse(text).unwrap()
}

#[test]
fn plain_keys_require_every_occurrence_to_match() {
    let export = export::parse("A [1]\nA [2]\n");
    let rows = compare(&spec("s {\n  A = 1\n}"), &export);
    assert_eq!(rows[0].kind, Kind::Bad);
    assert_eq!(rows[0].details, vec!["line 2: 2".to_string()]);
    let rows = compare(&spec("s {\n  A#2 = 2\n  A#1 = 1\n}"), &export);
    assert!(rows.iter().all(|row| row.kind == Kind::Ok));
    assert_eq!(rows[0].label, "A#2");
}

#[test]
fn missing_names_and_occurrences_are_bad() {
    let export = export::parse("A [1]\n");
    let rows = compare(&spec("s {\n  B = 1\n  A#2 = 1\n}"), &export);
    assert_eq!(rows[0].summary, "not in export");
    assert!(rows[1].summary.contains("occurrence 2 missing"));
}
