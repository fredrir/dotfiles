use super::*;

#[test]
fn a_wall_clock_far_ahead_of_the_monotonic_clock_means_a_sleep() {
    assert!(slept(Duration::from_secs(3600), Duration::from_secs(5)));
}

#[test]
fn ordinary_scheduling_jitter_is_not_a_sleep() {
    assert!(!slept(Duration::from_secs(6), Duration::from_secs(5)));
    assert!(!slept(Duration::from_secs(4), Duration::from_secs(5)));
}
