use super::*;
use serde_json::json;

fn run(id: &str, started: &str, tags: &[&str], value: f64) -> RunSummary {
    summarize(&json!({
        "run_id": id, "started": started, "grade": "clean", "tags": tags,
        "metrics": [{"key": "cpu.multi", "samples": [value, value + 2.0, value - 1.0]}]
    }))
    .unwrap()
}

#[test]
fn median_handles_even_and_odd_counts() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    assert_eq!(median(&[]), None);
}

#[test]
fn summaries_keep_the_median_per_metric() {
    let run = run("r1", "2026-09-11T17:06:48Z", &["bios:aaaa"], 10.0);
    assert_eq!(run.metrics["cpu.multi"], 10.0);
    assert_eq!(bios_tag(&run), "bios:aaaa");
}

#[test]
fn latest_run_per_tag_is_ordered_by_time() {
    let runs = vec![
        run("r1", "2026-09-01T00:00:00Z", &["bios:aaaa"], 10.0),
        run("r2", "2026-09-02T00:00:00Z", &["bios:aaaa"], 11.0),
        run("r3", "2026-09-03T00:00:00Z", &[], 12.0),
    ];
    let groups = latest_per_tag(&runs);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, "bios:aaaa");
    assert_eq!(groups[0].1.run_id, "r2");
    assert_eq!(groups[1].0, "untagged");
    let text = render(&groups, Some("cpu"));
    assert!(text.starts_with("metric     bios:aaaa"));
    assert!(text.contains("cpu.multi  11.0"));
}
