use super::*;

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("devices/system/cpu/cpufreq/policy0");
    fs::create_dir_all(&policy).unwrap();
    for (name, value) in [
        ("scaling_driver", "intel_pstate"),
        ("scaling_governor", "powersave"),
        ("scaling_available_governors", "performance powersave"),
        ("energy_performance_preference", "balance_performance"),
        (
            "energy_performance_available_preferences",
            "performance balance_performance balance_power power",
        ),
    ] {
        fs::write(policy.join(name), value).unwrap();
    }
    temp
}

#[test]
fn discovery_is_read_only_and_candidates_use_supported_control_combinations() {
    let temp = fixture();
    let sys = Sysfs {
        sys: temp.path().into(),
        dev: temp.path().join("dev"),
    };
    let plan = discover(&sys).unwrap();
    assert_eq!(plan.controls.len(), 2);
    assert!(plan.unavailable.is_empty());
    let performance = plan
        .candidates
        .iter()
        .find(|profile| profile.name == "performance")
        .unwrap();
    assert_eq!(
        performance.values[Path::new("devices/system/cpu/cpufreq/policy0/scaling_governor")],
        "performance"
    );
    assert_eq!(
        performance.values
            [Path::new("devices/system/cpu/cpufreq/policy0/energy_performance_preference")],
        "performance"
    );
    assert_eq!(
        read_text(
            &temp
                .path()
                .join("devices/system/cpu/cpufreq/policy0/scaling_governor")
        )
        .unwrap(),
        "powersave"
    );
}

#[test]
fn custom_platform_state_is_excluded_because_it_cannot_be_restored() {
    let temp = fixture();
    let acpi = temp.path().join("firmware/acpi");
    fs::create_dir_all(&acpi).unwrap();
    fs::write(acpi.join("platform_profile"), "custom").unwrap();
    fs::write(
        acpi.join("platform_profile_choices"),
        "balanced performance custom",
    )
    .unwrap();
    let plan = discover(&Sysfs {
        sys: temp.path().into(),
        dev: temp.path().into(),
    })
    .unwrap();
    assert_eq!(plan.controls.len(), 2);
    assert!(
        plan.unavailable
            .iter()
            .any(|reason| reason.contains("cannot be restored"))
    );
}

#[test]
fn unsupported_system_has_an_explicit_reason_and_no_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let plan = discover(&Sysfs {
        sys: temp.path().into(),
        dev: temp.path().into(),
    })
    .unwrap();
    assert!(plan.controls.is_empty());
    assert!(plan.candidates.is_empty());
    assert!(!plan.unavailable.is_empty());
}

#[test]
fn control_allowlist_rejects_unrelated_and_traversing_paths() {
    for path in [
        "/etc/passwd",
        "devices/system/cpu/cpufreq/policy0/../../../etc/passwd",
        "devices/system/cpu/cpufreq/policy0/scaling_max_freq",
        "devices/system/cpu/cpufreq/policyx/scaling_governor",
    ] {
        assert!(!allowed_path(Path::new(path)), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn discovery_rejects_controls_resolving_outside_captured_sysfs() {
    let temp = fixture();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "powersave").unwrap();
    let path = temp
        .path()
        .join("devices/system/cpu/cpufreq/policy0/scaling_governor");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(outside.path(), &path).unwrap();
    let plan = discover(&Sysfs {
        sys: temp.path().into(),
        dev: temp.path().into(),
    })
    .unwrap();
    assert!(
        plan.unavailable
            .iter()
            .any(|reason| reason.contains("escapes sysfs"))
    );
}
