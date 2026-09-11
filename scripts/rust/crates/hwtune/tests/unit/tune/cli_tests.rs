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
fn invalid_limits_are_rejected_without_collecting_or_writing() {
    let options = ValidationOptions {
        metric: "cpu.multi".into(),
        guard: vec!["idle".into()],
        max_temp: Some(f64::NAN),
        stress_seconds: 30,
        json: false,
    };
    assert!(validate_options(&options).is_err());
    let options = ValidationOptions {
        metric: "nonsense.read".into(),
        guard: Vec::new(),
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

fn options(metric: &str, guard: &[&str]) -> ValidationOptions {
    ValidationOptions {
        metric: metric.into(),
        guard: guard.iter().map(|g| g.to_string()).collect(),
        max_temp: None,
        stress_seconds: 30,
        json: false,
    }
}

#[test]
fn objective_may_be_any_family_and_guards_join_the_trial_families() {
    assert!(validate_options(&options("compile.dev", &["idle"])).is_ok());
    assert!(validate_options(&options("ai.generate_tps", &["idle", "sched"])).is_ok());
    assert!(validate_options(&options("cpu", &[])).is_err());
    assert!(validate_options(&options("cpu.", &[])).is_err());
    assert!(validate_options(&options("compile.dev", &["bogus"])).is_err());
    assert_eq!(
        trial_families(&options("compile.dev", &["idle", "compile", "idle"])),
        ["compile", "idle"]
    );
    let cli = crate::cli::Cli::try_parse_from(["hwtune", "tune", "auto"]).unwrap();
    let Some(crate::cli::Command::Tune {
        command: Command::Auto(auto),
    }) = cli.command
    else {
        panic!("tune auto expected")
    };
    assert_eq!(auto.validation.guard, ["idle"]);
    let cli = crate::cli::Cli::try_parse_from([
        "hwtune",
        "tune",
        "auto",
        "--metric",
        "compile.dev",
        "--guard",
        "idle,sched",
    ])
    .unwrap();
    let Some(crate::cli::Command::Tune {
        command: Command::Auto(auto),
    }) = cli.command
    else {
        panic!("tune auto expected")
    };
    assert_eq!(auto.validation.guard, ["idle", "sched"]);
}

#[test]
fn scoped_run_parses_a_profile_and_a_trailing_command() {
    let cli = crate::cli::Cli::try_parse_from([
        "hwtune",
        "run",
        "--profile",
        "cpuidle-teo",
        "--",
        "cargo",
        "build",
        "--release",
    ])
    .unwrap();
    let Some(crate::cli::Command::Run(options)) = cli.command else {
        panic!("run expected")
    };
    assert_eq!(options.profile, "cpuidle-teo");
    assert_eq!(options.command, ["cargo", "build", "--release"]);
    let cli = crate::cli::Cli::try_parse_from(["hwtune", "run", "--", "make"]).unwrap();
    let Some(crate::cli::Command::Run(options)) = cli.command else {
        panic!("run expected")
    };
    assert_eq!(options.profile, "performance");
    assert!(
        crate::cli::Cli::try_parse_from(["hwtune", "run", "--profile", "performance"]).is_err()
    );
}

#[test]
fn scoped_profiles_resolve_original_or_a_named_candidate() {
    let control = Control {
        path: "devices/system/cpu/cpufreq/policy0/scaling_governor".into(),
        original: "powersave".into(),
        choices: vec!["powersave".into(), "performance".into()],
        driver: Some("amd-pstate-epp".into()),
    };
    let mut performance = controls::original_profile(std::slice::from_ref(&control));
    performance.name = "performance".into();
    performance
        .values
        .insert(control.path.clone(), "performance".into());
    let plan = controls::Plan {
        controls: vec![control.clone()],
        candidates: vec![performance],
        unavailable: Vec::new(),
    };
    assert_eq!(
        select_profile(&plan, "original").unwrap().values[&control.path],
        "powersave"
    );
    assert_eq!(
        select_profile(&plan, "performance").unwrap().values[&control.path],
        "performance"
    );
    let error = select_profile(&plan, "quiet").unwrap_err();
    assert!(error.contains("original, performance"), "{error}");
    assert_eq!(
        passwd_home(
            "root:x:0:0::/root:/bin/bash\nfredrir:x:1000:1000::/home/fredrir:/bin/zsh\n",
            "fredrir"
        ),
        Some("/home/fredrir".into())
    );
    assert_eq!(passwd_home("root:x:0:0::/root:/bin/bash\n", "nobody"), None);
}
