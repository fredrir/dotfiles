use dotfile_test_support::{load_fixtures, run_fixture};

#[test]
fn every_fixture_record_passes() {
    let fixtures = load_fixtures().unwrap();
    assert!(!fixtures.is_empty(), "no fixture records found");
    let mut failures = Vec::new();
    for fixture in &fixtures {
        if fixture.status == "planned" || fixture.status == "blocked" {
            continue;
        }
        failures.extend(run_fixture(fixture));
    }
    for failure in &failures {
        eprintln!("{failure}");
    }
    assert!(failures.is_empty(), "{} fixture failures", failures.len());
}

#[test]
fn fixture_ids_are_unique_sorted_and_match_filenames() {
    let fixtures = load_fixtures().unwrap();
    let ids: Vec<&str> = fixtures.iter().map(|fixture| fixture.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "fixture records must sort by id");
    assert!(
        fixtures
            .iter()
            .all(|fixture| fixture.id.bytes().all(|byte| byte.is_ascii()))
    );
}
