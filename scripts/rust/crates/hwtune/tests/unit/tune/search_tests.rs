use super::*;
use crate::bench::record::Metric;

fn run(value: f64) -> Run {
    Run {
        host: "fixture".into(),
        run_id: format!("run-{value}"),
        tier: "quick".into(),
        grade: "clean".into(),
        metrics: vec![Metric {
            key: "cpu.multi".into(),
            method: "cpu.multi/1.0.0".into(),
            tool: "fixture".into(),
            samples: vec![value, value, value],
            ..Metric::default()
        }],
        ..Run::default()
    }
}

#[test]
fn winner_requires_improvement_above_threshold_and_noise() {
    assert!(evaluate(&run(100.0), &run(110.0), "cpu.multi", 3.0).accepted);
    assert!(!evaluate(&run(100.0), &run(101.0), "cpu.multi", 0.0).accepted);
    assert!(!evaluate(&run(100.0), &run(104.0), "cpu.multi", 5.0).accepted);
}

#[test]
fn objective_improvement_cannot_hide_another_metric_regression() {
    let mut before = run(100.0);
    let mut after = run(110.0);
    let mut other = before.metrics[0].clone();
    other.key = "cpu.single".into();
    before.metrics.push(other.clone());
    other.samples = vec![70.0; 3];
    after.metrics.push(other);
    let verdict = evaluate(&before, &after, "cpu.multi", 3.0);
    assert!(!verdict.accepted);
    assert!(verdict.reason.contains("cpu.single regressed"));
}

#[test]
fn missing_noisy_aborted_and_incompatible_trials_are_rejected() {
    for grade in ["noisy", "aborted"] {
        let mut after = run(150.0);
        after.grade = grade.into();
        assert!(!evaluate(&run(100.0), &after, "cpu.multi", 3.0).accepted);
    }
    let mut after = run(150.0);
    after.metrics.clear();
    assert!(!evaluate(&run(100.0), &after, "cpu.multi", 3.0).accepted);
    after = run(150.0);
    after.metrics[0].method = "cpu.multi/2.0.0".into();
    assert!(!evaluate(&run(100.0), &after, "cpu.multi", 3.0).accepted);
}

#[test]
fn repeated_original_baseline_must_remain_within_noise() {
    assert!(baseline_stable(&run(100.0), &run(101.0), "cpu.multi").is_ok());
    assert!(baseline_stable(&run(100.0), &run(110.0), "cpu.multi").is_err());
}

#[test]
fn hardware_changes_and_insufficient_samples_cannot_select_a_winner() {
    let mut after = run(150.0);
    after.snapshot = serde_json::json!({"cpu":{"model":"different CPU"}});
    assert!(!evaluate(&run(100.0), &after, "cpu.multi", 3.0).accepted);
    after = run(150.0);
    after.metrics[0].samples = vec![150.0];
    assert!(!evaluate(&run(100.0), &after, "cpu.multi", 3.0).accepted);
}

#[test]
fn lower_is_better_metrics_are_scored_in_the_correct_direction() {
    let mut before = run(100.0);
    let mut after = run(80.0);
    before.metrics[0].proportion = "LIB".into();
    after.metrics[0].proportion = "LIB".into();
    let verdict = evaluate(&before, &after, "cpu.multi", 3.0);
    assert!(verdict.accepted);
    assert_eq!(verdict.improvement_pct, 20.0);
}

struct MeasuredFixture {
    governor: std::path::PathBuf,
    candidate_value: f64,
    validation_value: f64,
    interrupted: bool,
    interrupt_stress: bool,
    phases: Vec<String>,
    records: Vec<serde_json::Value>,
}

impl Measurements for MeasuredFixture {
    fn benchmark(&mut self, phase: &str) -> Result<Run, String> {
        self.phases.push(phase.into());
        let governor = std::fs::read_to_string(&self.governor).unwrap();
        let value = match phase {
            "baseline" | "validation-baseline" => {
                assert_eq!(
                    governor.trim(),
                    "powersave",
                    "baseline measured under wrong settings"
                );
                100.0
            }
            "winner-validation" => {
                assert_eq!(governor.trim(), "performance");
                self.validation_value
            }
            "performance" => {
                assert_eq!(governor.trim(), "performance");
                self.candidate_value
            }
            other => panic!("unexpected phase {other}"),
        };
        Ok(run(value))
    }

    fn stress(&mut self, phase: &str) -> Result<(), String> {
        self.phases.push(phase.into());
        if self.interrupt_stress {
            self.interrupted = true;
            Err("fixture interruption during stress".into())
        } else {
            Ok(())
        }
    }

    fn record(&mut self, evidence: serde_json::Value) -> Result<(), String> {
        self.records.push(evidence);
        Ok(())
    }

    fn interrupted(&self) -> bool {
        self.interrupted
    }
}

fn measured_fixture(
    candidate_value: f64,
    validation_value: f64,
) -> (tempfile::TempDir, Guard, Vec<Profile>, MeasuredFixture) {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("devices/system/cpu/cpufreq/policy0");
    std::fs::create_dir_all(&policy).unwrap();
    for (name, value) in [
        ("scaling_driver", "intel_pstate"),
        ("scaling_governor", "powersave"),
        ("scaling_available_governors", "powersave performance"),
    ] {
        std::fs::write(policy.join(name), value).unwrap();
    }
    let plan = crate::tune::controls::discover(&crate::env::Sysfs {
        sys: temp.path().into(),
        dev: temp.path().into(),
    })
    .unwrap();
    let guard = Guard::begin(temp.path(), plan.controls).unwrap();
    let measurements = MeasuredFixture {
        governor: policy.join("scaling_governor"),
        candidate_value,
        validation_value,
        interrupted: false,
        interrupt_stress: false,
        phases: Vec::new(),
        records: Vec::new(),
    };
    (temp, guard, plan.candidates, measurements)
}

#[test]
fn automatic_trials_measure_the_right_settings_and_validate_winner_independently() {
    let (_temp, mut guard, profiles, mut measurements) = measured_fixture(110.0, 111.0);
    let (winner, _) = optimize(&mut measurements, &mut guard, &profiles, "cpu.multi", 3.0).unwrap();
    assert_eq!(winner.name, "performance");
    assert_eq!(
        measurements.phases,
        [
            "baseline",
            "performance-stability",
            "performance",
            "validation-baseline",
            "winner-validation-stability",
            "winner-validation"
        ]
    );
    assert_eq!(measurements.records.len(), 2);
    guard.complete(false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&measurements.governor)
            .unwrap()
            .trim(),
        "powersave"
    );
}

#[test]
fn automatic_noise_rejection_restores_starting_settings() {
    let (_temp, mut guard, profiles, mut measurements) = measured_fixture(101.0, 101.0);
    let error = optimize(&mut measurements, &mut guard, &profiles, "cpu.multi", 3.0).unwrap_err();
    assert!(error.contains("no candidate"));
    guard.complete(false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&measurements.governor)
            .unwrap()
            .trim(),
        "powersave"
    );
    assert!(
        !measurements.records[0]["verdict"]["accepted"]
            .as_bool()
            .unwrap()
    );
}

#[test]
fn failed_independent_validation_cannot_leave_the_provisional_winner_applied() {
    let (_temp, mut guard, profiles, mut measurements) = measured_fixture(110.0, 101.0);
    let error = optimize(&mut measurements, &mut guard, &profiles, "cpu.multi", 3.0).unwrap_err();
    assert!(error.contains("independent validation"));
    drop(guard);
    assert_eq!(
        std::fs::read_to_string(&measurements.governor)
            .unwrap()
            .trim(),
        "powersave"
    );
}

#[test]
fn interruption_during_candidate_validation_restores_before_returning() {
    let (_temp, mut guard, profiles, mut measurements) = measured_fixture(110.0, 111.0);
    measurements.interrupt_stress = true;
    assert!(optimize(&mut measurements, &mut guard, &profiles, "cpu.multi", 3.0).is_err());
    assert_eq!(
        std::fs::read_to_string(&measurements.governor)
            .unwrap()
            .trim(),
        "powersave"
    );
    assert_eq!(measurements.records[0]["status"], "rejected");
}
