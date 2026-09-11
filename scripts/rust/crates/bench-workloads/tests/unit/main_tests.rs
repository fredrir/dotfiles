use super::*;

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(ToString::to_string).collect()
}

#[test]
fn chain_is_deterministic() {
    assert_eq!(xorshift_chain(7, 1000), xorshift_chain(7, 1000));
    assert_ne!(xorshift_chain(7, 1000), xorshift_chain(8, 1000));
}

#[test]
fn zero_threads_means_all_cores() {
    assert!(resolve_threads(0) >= 1);
    assert_eq!(resolve_threads(3), 3);
}

#[test]
fn cpu_measures_a_positive_rate() {
    let measurement = cpu_workload(2, 100_000);
    assert!(measurement.value > 0.0);
    assert_eq!(measurement.threads, 2);
}

#[test]
fn memory_measures_both_directions() {
    for op in [MemoryOp::Read, MemoryOp::Write] {
        let measurement = memory_workload(op, 1, 1);
        assert!(measurement.value > 0.0);
    }
}

#[test]
fn latency_chain_is_one_cycle_over_every_line() {
    let lines = 257;
    let buffer = latency_chain(lines, 7);
    let mut index = 0usize;
    for hop in 1..=lines {
        index = buffer[index * LINE_WORDS] as usize;
        assert!(index < lines);
        assert_eq!(
            index == 0,
            hop == lines,
            "returned to the start after {hop} hops"
        );
    }
    assert_eq!(latency_chain(lines, 7), buffer);
    assert_ne!(latency_chain(lines, 8), buffer);
}

#[test]
fn latency_reports_nanoseconds_per_dependent_load() {
    let measurement = memory_workload(MemoryOp::Latency, 1, 2);
    assert_eq!(measurement.unit, "ns");
    assert!(measurement.value > 0.0);
    assert!(measurement.detail.contains(&("passes", 2)));
    assert!(measurement.detail.contains(&("lines", 16384)));
}

#[test]
fn percentiles_use_nearest_rank() {
    let sorted: Vec<u64> = (1..=100).collect();
    assert_eq!(percentile(&sorted, 50), 50);
    assert_eq!(percentile(&sorted, 99), 99);
    assert_eq!(percentile(&sorted, 100), 100);
    assert_eq!(percentile(&[5], 99), 5);
    assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
}

#[test]
fn wake_measures_oversleep_with_and_without_load() {
    for load in [Load::None, Load::All] {
        let measurement = wake_workload(20, 200, load);
        assert_eq!(measurement.unit, "us");
        let p50 = measurement
            .detail
            .iter()
            .find(|(key, _)| *key == "p50")
            .map(|(_, value)| *value)
            .unwrap();
        let max = measurement
            .detail
            .iter()
            .find(|(key, _)| *key == "max")
            .map(|(_, value)| *value)
            .unwrap();
        assert!(p50 as f64 <= measurement.value && measurement.value <= max as f64);
        assert!(measurement.elapsed_s >= 0.004);
        let busy = measurement
            .detail
            .iter()
            .find(|(key, _)| *key == "busy_threads")
            .map(|(_, value)| *value)
            .unwrap();
        assert_eq!(busy == 0, load == Load::None);
    }
}

#[test]
fn json_shape_is_stable() {
    let measurement = Measurement {
        workload: "cpu",
        unit: "Mops/s",
        value: 12.3456,
        elapsed_s: 1.5,
        threads: 4,
        detail: vec![("iterations", 10)],
    };
    assert_eq!(
        measurement.to_json(),
        "{\"workload\":\"cpu\",\"unit\":\"Mops/s\",\"value\":12.346,\
             \"elapsed_s\":1.500,\"threads\":4,\"detail\":{\"iterations\":10}}"
    );
}

#[test]
fn rejects_unknown_flags() {
    assert!(dispatch(strings(&["cpu", "--bogus"])).is_err());
    assert!(dispatch(strings(&["memory", "--op", "sideways"])).is_err());
    assert!(dispatch(strings(&["wake", "--load", "some"])).is_err());
    assert!(dispatch(strings(&["wake", "--iterations"])).is_err());
    assert!(dispatch(strings(&["juggling"])).is_err());
}

#[test]
fn runs_tiny_latency_and_wake_workloads() {
    let latency = dispatch(strings(&["memory", "--op", "latency", "--mib", "1"]))
        .unwrap()
        .unwrap();
    assert_eq!(latency.unit, "ns");
    assert!(latency.detail.contains(&("passes", 4)));
    let wake = dispatch(strings(&[
        "wake",
        "--iterations",
        "5",
        "--sleep-us",
        "100",
        "--load",
        "none",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(wake.workload, "wake");
    assert!(wake.detail.contains(&("iterations", 5)));
}

#[test]
fn runs_a_tiny_cpu_workload() {
    let measurement = dispatch(strings(&["cpu", "--threads", "1", "--iterations", "1000"]))
        .unwrap()
        .unwrap();
    assert_eq!(measurement.workload, "cpu");
    assert!(measurement.value > 0.0);
}
