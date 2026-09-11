use super::super::provenance::{RunContext, bios_source, lact_source};
use super::super::record::Metric;
use super::*;
use std::path::Path;

fn sample(id: &str, bios: &str, lact: &str, score: f64) -> Run {
    Run {
        run_id: id.into(),
        started: id.into(),
        host: "desktop".into(),
        tier: "quick".into(),
        grade: "clean".into(),
        install: serde_json::json!({"os":"linux"}),
        snapshot: serde_json::json!({"cpu":{"model":"cpu-a","cores_physical":8,"cores_logical":16}}),
        context: Some(RunContext {
            bios: Some(
                bios_source(Path::new("bios"), format!("Boost [{bios}]\n").as_bytes()).unwrap(),
            ),
            lact: Some(
                lact_source(Path::new("lact"), format!("cap: {lact}\n").as_bytes()).unwrap(),
            ),
            ..RunContext::default()
        }),
        metrics: vec![Metric {
            key: "cpu.multi".into(),
            method: "cpu/1.0".into(),
            scale: "ops/s".into(),
            samples: vec![score; 3],
            ..Metric::default()
        }],
        ..Run::default()
    }
}

#[test]
fn grouping_distinguishes_lact_changes_and_selects_latest_clean_run_without_input_order_assumptions()
 {
    let first = sample("1", "on", "200", 100.0);
    let newer = sample("2", "on", "200", 105.0);
    let lact = sample("3", "on", "220", 110.0);
    let mut noisy = sample("4", "on", "220", 120.0);
    noisy.grade = "noisy".into();
    let runs = [noisy, first, lact, newer];
    let groups = latest_by_configuration(&runs, GroupBy::BiosLact);
    assert_eq!(
        groups
            .iter()
            .map(|g| g.run.run_id.as_str())
            .collect::<Vec<_>>(),
        ["2", "3"]
    );
    let groups = latest_by_configuration(&runs, GroupBy::Bios);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].run.run_id, "3");
}

#[test]
fn grouping_preserves_hardware_and_protocol_boundaries() {
    let first = sample("1", "on", "200", 100.0);
    let mut hardware = first.clone();
    hardware.run_id = "2".into();
    hardware.snapshot["cpu"]["model"] = "other".into();
    let mut method = first.clone();
    method.run_id = "3".into();
    method.metrics[0].method = "cpu/2.0".into();
    let mut tier = first.clone();
    tier.run_id = "4".into();
    tier.tier = "heavy".into();
    assert_eq!(
        latest_by_configuration(&[first, hardware, method, tier], GroupBy::BiosLact).len(),
        4
    );
}

#[test]
fn before_after_reports_real_change_and_rejects_uncontrolled_experiments() {
    let first = sample("1", "on", "200", 100.0);
    let mut after = sample("2", "on", "220", 110.0);
    let comparison = before_after(&first, &after).unwrap();
    assert_eq!(comparison.deltas[0].verdict, "better");
    assert_eq!(context_changes(&first, &after)[0].setting, "lact.settings");
    after.snapshot["cpu"]["model"] = "different".into();
    assert!(
        before_after(&first, &after)
            .unwrap_err()
            .contains("hardware")
    );
    after.snapshot = first.snapshot.clone();
    after.grade = "noisy".into();
    assert!(before_after(&first, &after).unwrap_err().contains("clean"));
}

#[test]
fn changed_metric_units_direction_or_scope_cannot_look_like_a_gain() {
    let first = sample("1", "on", "200", 100.0);
    for field in ["unit", "direction", "scope"] {
        let mut after = sample("2", "on", "220", 1000.0);
        match field {
            "unit" => after.metrics[0].scale = "ms".into(),
            "direction" => after.metrics[0].proportion = "LIB".into(),
            _ => after.metrics[0].comparable = "world".into(),
        }
        assert_eq!(
            before_after(&first, &after).unwrap().deltas[0].verdict,
            "blocked"
        );
    }
}

#[test]
fn reporting_retains_distinct_workload_revisions_and_observed_tuning_candidates() {
    let mut first = sample("1", "on", "200", 100.0);
    first.metrics[0].key = "workload.build".into();
    first.dotfiles_sha = "abc123".into();
    let mut revision = first.clone();
    revision.run_id = "2".into();
    revision.dotfiles_sha = "def456".into();
    let mut tuned = first.clone();
    tuned.run_id = "3".into();
    tuned
        .context
        .as_mut()
        .unwrap()
        .observed
        .insert("cpu.policy0.scaling_governor".into(), "performance".into());
    assert_eq!(
        latest_by_configuration(&[first, revision, tuned], GroupBy::BiosLact).len(),
        3
    );
}
