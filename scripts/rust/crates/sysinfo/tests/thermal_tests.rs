use super::*;

#[test]
fn telemetry_excludes_slow_probes_and_samples_during_cleanup() {
    let deadline = Instant::now() + Duration::from_millis(30);
    let temperature = sample_before_deadline(deadline, |remaining| {
        assert!(remaining <= Duration::from_millis(30));
        std::thread::sleep(remaining + Duration::from_millis(1));
        Some(65.0)
    });
    assert!(temperature.is_none());
    let clock = sample_before_deadline(deadline, |_| -> Option<f64> {
        panic!("clock probe ran during cleanup");
    });
    assert!(clock.is_none());
}

#[test]
fn telemetry_keeps_values_completed_within_the_measurement_window() {
    assert_eq!(
        sample_before_deadline(Instant::now() + Duration::from_secs(5), |_| Some(65.0)),
        Some(65.0)
    );
}
