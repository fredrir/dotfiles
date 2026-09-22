use super::*;
use serde_json::Value;

fn reading(cores: Ticks, own: u64) -> Reading {
    Reading { cores, own }
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_f64().unwrap())
        .collect()
}

#[test]
fn tick_deltas_become_per_core_percentages() {
    let before = reading([[0, 1_000], [10, 1_000], [5, 1_000]].into(), 0);
    let after = reading([[500, 2_000], [10, 2_000], [1_005, 2_000]].into(), 0);
    assert_eq!(percentages(&before, &after), vec![50.0, 0.0, 100.0]);
}

#[test]
fn stalled_and_rewound_counters_never_report_invalid_load() {
    // No ticks elapsed at all, and a counter that went backwards.
    assert_eq!(
        percentages(
            &reading([[7, 100]].into(), 0),
            &reading([[7, 100]].into(), 0)
        ),
        vec![0.0]
    );
    assert_eq!(
        percentages(
            &reading([[90, 200]].into(), 0),
            &reading([[10, 100]].into(), 0)
        ),
        vec![0.0]
    );
    // Busy ticks cannot exceed the total the same reading reported.
    assert_eq!(
        percentages(&reading([[0, 0]].into(), 0), &reading([[60, 50]].into(), 0)),
        vec![100.0]
    );
}

#[test]
fn only_cores_present_in_both_readings_are_reported() {
    let before = reading([[0, 100], [0, 100], [0, 100], [0, 100]].into(), 0);
    let after = reading([[50, 200], [0, 200]].into(), 0);
    assert_eq!(percentages(&before, &after), vec![50.0, 0.0]);
    assert_eq!(
        percentages(&reading(Ticks::new(), 0), &after),
        Vec::<f64>::new()
    );
}

#[test]
fn the_samplers_own_cpu_is_not_reported_as_system_load() {
    // Two cores, one tick window each; the process itself burned one tick on
    // each core, so nothing else was busy.
    let before = reading([[0, 10], [0, 10]].into(), 8);
    let after = reading([[1, 11], [1, 11]].into(), 10);
    assert_eq!(percentages(&before, &after), vec![0.0, 0.0]);
    // Own ticks above the measured busy time clamp to zero instead of going
    // negative, and the mean stays inside 0..=100.
    let before = reading([[0, 10]].into(), 0);
    let after = reading([[1, 11]].into(), 500);
    assert_eq!(percentages(&before, &after), vec![0.0]);
}

#[test]
fn proc_stat_parser_keeps_core_order_and_skips_non_counter_lines() {
    let body = "\
cpu  100 0 100 800 0 0 0 0 0 0
cpu0 100 20 30 400 50 6 4 0 0 0
cpu1 0 0 0 500 0 0 0 0 0 0
intr 12345 0 0
ctxt 999
btime 1700000000
processes 42
";
    // busy = user + nice + system + irq + softirq + steal; idle includes iowait.
    assert_eq!(
        parse_proc_stat(body),
        vec![[160, 610], [0, 500]],
        "aggregate cpu line and non-counter lines are not cores"
    );
    assert_eq!(parse_proc_stat("cpu 1 2 3\n"), Ticks::new());
}

#[test]
fn own_ticks_survive_command_names_with_spaces_and_parentheses() {
    let stat = "4242 (cargo test (x)) S 4200 4242 4242 0 -1 4194560 100 0 0 0 7 11 2 3 20 0 8 0";
    assert_eq!(
        parse_own_ticks(stat),
        23,
        "utime 7 + stime 11 + waited children 2 + 3"
    );
    assert_eq!(parse_own_ticks(""), 0);
    assert_eq!(parse_own_ticks("1 (short) R"), 0);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn live_sampling_reports_a_bounded_load_per_core() {
    let Some(sampler) = Sampler::start() else {
        panic!("this platform reports no CPU ticks");
    };
    let mut busy = 0u64;
    for value in 0..2_000_000u64 {
        busy = busy.wrapping_add(value.wrapping_mul(2654435761));
    }
    std::hint::black_box(busy);
    let Some(usage) = sampler.usage() else {
        panic!("second CPU tick reading failed");
    };
    let loads = numbers(&usage);
    assert!(!loads.is_empty(), "{usage}");
    for load in loads {
        assert!((0.0..=100.0).contains(&load), "{load} outside 0..=100");
    }
}
