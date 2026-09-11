use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Vec<Control>, Profile) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sys");
    let policy = root.join("devices/system/cpu/cpufreq/policy0");
    fs::create_dir_all(&policy).unwrap();
    for (name, value) in [
        ("scaling_driver", "intel_pstate"),
        ("scaling_governor", "powersave"),
        ("scaling_available_governors", "performance powersave"),
        ("energy_performance_preference", "balance_performance"),
        (
            "energy_performance_available_preferences",
            "performance balance_performance power",
        ),
    ] {
        fs::write(policy.join(name), value).unwrap();
    }
    let plan = controls::discover(&Sysfs {
        sys: root,
        dev: temp.path().into(),
    })
    .unwrap();
    let candidate = plan
        .candidates
        .into_iter()
        .find(|profile| profile.name == "performance")
        .unwrap();
    (temp, plan.controls, candidate)
}

#[test]
fn dropped_trial_restores_original_settings_without_persisted_undo() {
    let (temp, controls, candidate) = fixture();
    let root = temp.path().join("sys");
    {
        let mut guard = Guard::begin(&root, controls.clone()).unwrap();
        guard.apply(&candidate).unwrap();
        assert_eq!(
            controls::read(&root, &controls[0].path).unwrap(),
            "performance"
        );
    }
    verify(&root, &controls::original_profile(&controls)).unwrap();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn validated_winner_can_be_retained() {
    let (temp, controls, candidate) = fixture();
    let root = temp.path().join("sys");
    let mut guard = Guard::begin(&root, controls).unwrap();
    guard.apply(&candidate).unwrap();
    guard.complete(true).unwrap();
    verify(&root, &candidate).unwrap();
}

#[test]
fn unavailable_value_is_rejected_before_any_setting_is_written() {
    let (temp, controls, mut candidate) = fixture();
    let root = temp.path().join("sys");
    let mut guard = Guard::begin(&root, controls.clone()).unwrap();
    candidate
        .values
        .insert(controls[0].path.clone(), "overclock".into());
    assert!(guard.apply(&candidate).is_err());
    verify(&root, &controls::original_profile(&controls)).unwrap();
}

#[test]
fn incompatible_governor_epp_pair_is_rejected_before_governor_changes() {
    let (temp, controls, mut candidate) = fixture();
    let root = temp.path().join("sys");
    let mut guard = Guard::begin(&root, controls.clone()).unwrap();
    let epp = controls
        .iter()
        .find(|control| control.path.ends_with("energy_performance_preference"))
        .unwrap();
    candidate.values.insert(epp.path.clone(), "power".into());
    assert!(
        guard
            .apply(&candidate)
            .unwrap_err()
            .contains("requires the performance EPP")
    );
    verify(&root, &controls::original_profile(&controls)).unwrap();
}

#[test]
fn outside_change_in_one_policy_does_not_prevent_other_policies_restoring() {
    let (temp, _, _) = fixture();
    let root = temp.path().join("sys");
    let first = root.join("devices/system/cpu/cpufreq/policy0");
    let second = first.with_file_name("policy1");
    fs::create_dir_all(&second).unwrap();
    for entry in fs::read_dir(&first).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), second.join(entry.file_name())).unwrap();
    }
    let plan = controls::discover(&Sysfs {
        sys: root.clone(),
        dev: root.clone(),
    })
    .unwrap();
    let candidate = plan
        .candidates
        .iter()
        .find(|profile| profile.name == "performance")
        .unwrap();
    let mut guard = Guard::begin(&root, plan.controls).unwrap();
    guard.apply(candidate).unwrap();
    fs::write(first.join("energy_performance_preference"), "power").unwrap();
    assert!(guard.reset().is_err());
    assert_eq!(
        fs::read_to_string(first.join("energy_performance_preference")).unwrap(),
        "power"
    );
    assert_eq!(
        fs::read_to_string(second.join("scaling_governor"))
            .unwrap()
            .trim(),
        "powersave"
    );
    assert_eq!(
        fs::read_to_string(second.join("energy_performance_preference"))
            .unwrap()
            .trim(),
        "balance_performance"
    );
}

#[test]
fn outside_policy_changes_are_preserved_even_when_guard_is_dropped() {
    let (temp, controls, candidate) = fixture();
    let root = temp.path().join("sys");
    let epp = controls
        .iter()
        .find(|control| control.path.ends_with("energy_performance_preference"))
        .unwrap();
    {
        let mut guard = Guard::begin(&root, controls.clone()).unwrap();
        fs::write(root.join(&epp.path), "power").unwrap();
        assert!(guard.apply(&candidate).is_err());
    }
    assert_eq!(controls::read(&root, &epp.path).unwrap(), "power");
}

#[test]
fn failed_restoration_does_not_recreate_missing_sysfs_attributes() {
    let (temp, controls, candidate) = fixture();
    let root = temp.path().join("sys");
    {
        let mut guard = Guard::begin(&root, controls.clone()).unwrap();
        guard.apply(&candidate).unwrap();
        fs::remove_file(root.join(&controls[0].path)).unwrap();
        assert!(guard.reset().is_err());
    }
    assert!(!root.join(&controls[0].path).exists());
    assert_eq!(
        controls::read(&root, &controls[1].path).unwrap(),
        controls[1].original
    );
}

#[test]
fn cpuidle_candidate_applies_and_restores_only_the_governor() {
    let (temp, _, _) = fixture();
    let root = temp.path().join("sys");
    let cpuidle = root.join("devices/system/cpu/cpuidle");
    fs::create_dir_all(&cpuidle).unwrap();
    fs::write(cpuidle.join("current_governor"), "menu").unwrap();
    fs::write(cpuidle.join("available_governors"), "ladder menu teo").unwrap();
    let plan = controls::discover(&Sysfs {
        sys: root.clone(),
        dev: temp.path().into(),
    })
    .unwrap();
    let candidate = plan
        .candidates
        .iter()
        .find(|profile| profile.name == "cpuidle-teo")
        .unwrap();
    let mut guard = Guard::begin(&root, plan.controls.clone()).unwrap();
    guard.apply(candidate).unwrap();
    assert_eq!(
        controls::read(&root, Path::new(controls::CPUIDLE_GOVERNOR)).unwrap(),
        "teo"
    );
    assert_eq!(
        controls::read(&root, &plan.controls[0].path).unwrap(),
        plan.controls[0].original
    );
    guard.complete(false).unwrap();
    assert_eq!(
        controls::read(&root, Path::new(controls::CPUIDLE_GOVERNOR)).unwrap(),
        "menu"
    );
}
