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
    assert!(dispatch(strings(&["juggling"])).is_err());
}

#[test]
fn runs_a_tiny_cpu_workload() {
    let measurement = dispatch(strings(&["cpu", "--threads", "1", "--iterations", "1000"]))
        .unwrap()
        .unwrap();
    assert_eq!(measurement.workload, "cpu");
    assert!(measurement.value > 0.0);
}
