use super::*;

fn latency(millis: &[u64]) -> Latency {
    let mut round_trips: Vec<Duration> =
        millis.iter().map(|&ms| Duration::from_millis(ms)).collect();
    round_trips.sort_unstable();
    Latency { round_trips }
}

#[test]
fn the_statistics_describe_the_samples() {
    let latency = latency(&[4, 1, 2, 3]);
    assert_eq!(latency.samples(), 4);
    assert_eq!(latency.min(), Duration::from_millis(1));
    assert_eq!(latency.max(), Duration::from_millis(4));
    assert_eq!(
        latency.mean(),
        Duration::from_millis(2) + Duration::from_micros(500)
    );
    assert_eq!(latency.percentile(50.0), Duration::from_millis(2));
    assert_eq!(latency.percentile(99.0), Duration::from_millis(4));
}

#[test]
fn a_percentile_of_one_sample_is_that_sample() {
    let latency = latency(&[7]);
    assert_eq!(latency.percentile(0.0), Duration::from_millis(7));
    assert_eq!(latency.percentile(50.0), Duration::from_millis(7));
    assert_eq!(latency.percentile(100.0), Duration::from_millis(7));
}

#[test]
fn no_samples_is_zero_rather_than_a_panic() {
    let latency = latency(&[]);
    assert_eq!(latency.min(), Duration::ZERO);
    assert_eq!(latency.max(), Duration::ZERO);
    assert_eq!(latency.mean(), Duration::ZERO);
    assert_eq!(latency.percentile(50.0), Duration::ZERO);
}

#[test]
fn parallel_streams_add_their_bytes_and_share_their_time() {
    let combined = combine(&[
        Counted {
            bytes: 1_000,
            elapsed: Duration::from_millis(900),
        },
        Counted {
            bytes: 3_000,
            elapsed: Duration::from_millis(1_000),
        },
    ]);
    assert_eq!(combined.bytes, 4_000);
    assert_eq!(combined.elapsed, Duration::from_millis(1_000));
}

#[test]
fn combining_nothing_is_not_a_division_by_zero() {
    assert_eq!(combine(&[]).bits_per_second(), 0.0);
}
