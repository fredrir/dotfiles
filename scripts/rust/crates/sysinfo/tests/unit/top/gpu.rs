use super::*;

#[test]
fn gpu_time_deltas_become_shares_of_the_window() {
    let before = HashMap::from([(1, 100), (2, 500)]);
    let after = HashMap::from([(1, 50_000_100), (2, 500), (3, 400_000_000)]);
    let shares = shares(&before, &after, 100_000_000);
    assert_eq!(shares[&1], 50.0);
    assert_eq!(shares[&2], 0.0, "an idle client stays listed at zero");
    assert_eq!(shares[&3], 100.0, "concurrent queues cap at one full GPU");
}

#[test]
fn rewound_counters_never_report_negative_use() {
    let shares = shares(
        &HashMap::from([(1, 900)]),
        &HashMap::from([(1, 100)]),
        1_000,
    );
    assert_eq!(shares[&1], 0.0);
}

#[test]
fn samples_average_per_pid_and_spread_over_devices() {
    let averaged = average([(7, 40), (7, 60), (8, 90)], 2);
    assert_eq!(averaged[&7], 25.0);
    assert_eq!(averaged[&8], 45.0);
    assert!(average([], 0).is_empty());
}
