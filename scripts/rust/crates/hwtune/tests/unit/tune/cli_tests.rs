use super::*;
use clap::Parser;

#[test]
fn auto_restores_by_default_and_only_explicit_apply_retains_the_winner() {
    let cli = crate::cli::Cli::try_parse_from(["hwtune", "tune", "auto"]).unwrap();
    let Some(crate::cli::Command::Tune {
        command: Command::Auto(options),
    }) = cli.command
    else {
        panic!("tune auto expected")
    };
    assert!(!options.apply);
    assert_eq!(options.validation.metric, "cpu.multi");
    let cli = crate::cli::Cli::try_parse_from(["hwtune", "tune", "auto", "--apply"]).unwrap();
    let Some(crate::cli::Command::Tune {
        command: Command::Auto(options),
    }) = cli.command
    else {
        panic!("tune auto expected")
    };
    assert!(options.apply);
}

#[test]
fn no_persistent_undo_interface_is_exposed() {
    assert!(crate::cli::Cli::try_parse_from(["hwtune", "tune", "restore"]).is_err());
    assert!(crate::cli::Cli::try_parse_from(["hwtune", "tune", "apply"]).is_ok());
}

#[test]
fn invalid_limits_are_rejected_without_collecting_or_writing() {
    let options = ValidationOptions {
        metric: "cpu.multi".into(),
        max_temp: Some(f64::NAN),
        stress_seconds: 30,
        json: false,
    };
    assert!(validate_options(&options).is_err());
    let options = ValidationOptions {
        metric: "disk.read".into(),
        max_temp: None,
        stress_seconds: 30,
        json: false,
    };
    assert!(validate_options(&options).is_err());
    assert!(
        crate::cli::Cli::try_parse_from(["hwtune", "tune", "auto", "--stress-seconds", "0"])
            .is_err()
    );
}

#[test]
fn checked_out_profile_is_bound_to_host_hardware_and_supported_values() {
    let control = Control {
        path: "devices/system/cpu/cpufreq/policy0/scaling_governor".into(),
        original: "powersave".into(),
        choices: vec!["powersave".into(), "performance".into()],
        driver: Some("intel_pstate".into()),
    };
    let controls = vec![control];
    let baseline = record::Run {
        host: "host".into(),
        ..record::Run::default()
    };
    let mut desired = desired_from(
        "host",
        controls::original_profile(&controls),
        &controls,
        "session",
        &baseline,
    );
    assert!(validate_desired(&desired, "host", &controls, &baseline).is_ok());
    assert!(validate_desired(&desired, "other", &controls, &baseline).is_err());
    desired.hardware_epoch = "other".into();
    assert!(validate_desired(&desired, "host", &controls, &baseline).is_err());
    desired.hardware_epoch = baseline.epoch();
    desired
        .profile
        .values
        .insert(controls[0].path.clone(), "unsupported".into());
    assert!(validate_desired(&desired, "host", &controls, &baseline).is_err());
}

#[test]
fn profile_commit_failure_preserves_previous_git_contents_and_external_edits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("host.json");
    let previous = b"checked out desired settings\n";
    let next = b"validated replacement\n";
    fs::write(&path, previous).unwrap();
    commit_profile(&path, Some(previous), next).unwrap();
    undo_profile(&path, Some(previous), next).unwrap();
    assert_eq!(fs::read(&path).unwrap(), previous);
    fs::write(&path, "concurrent git checkout").unwrap();
    assert!(commit_profile(&path, Some(previous), next).is_err());
    assert!(undo_profile(&path, Some(previous), next).is_err());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "concurrent git checkout"
    );
}

#[test]
fn failed_first_profile_commit_removes_only_its_own_new_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("host.json");
    commit_profile(&path, None, b"candidate").unwrap();
    undo_profile(&path, None, b"candidate").unwrap();
    assert!(!path.exists());
}
