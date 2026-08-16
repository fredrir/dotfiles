use std::collections::BTreeSet;

use dotfile_test_support::{load_contract, load_fixtures, run_fixture};

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

#[test]
fn passing_status_and_manifest_claims_are_exactly_aligned() {
    let fixtures = load_fixtures().unwrap();
    let contract = load_contract("fixtures").unwrap();
    let claims = &contract["implementation_claims"];
    let implemented = claims["implemented_fixture_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let claimed_passing = claims["passing_fixture_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let status_passing = fixtures
        .iter()
        .filter(|fixture| fixture.status == "passing")
        .map(|fixture| fixture.id.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(claimed_passing, status_passing);
    assert!(claimed_passing.is_subset(&implemented));
}
