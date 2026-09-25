use super::*;

const LACT: &str = "gpus:\n  GPU-A:\n    power_cap: 350.0\nprofiles:\n  comfort:\n    gpus:\n      GPU-A:\n        power_cap: 250.0\ncurrent_profile: null\n";

fn lact() -> Value {
    serde_yaml_ng::from_str(LACT).unwrap()
}

fn roots(base: &Path) -> Roots {
    Roots {
        etc: base.join("etc"),
        state: base.join("state/profile"),
        proc: base.join("proc"),
        lact: base.join("lact.yaml"),
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn names_are_lowercase_path_safe_words() {
    assert!(valid_name("comfort"));
    assert!(valid_name("low-noise-2"));
    for name in ["", "Comfort", "../etc", "a b", "x.yaml", &"a".repeat(33)] {
        assert!(!valid_name(name), "{name}");
    }
}

#[test]
fn assignments_read_environment_files() {
    let values = assignments("# note\nCPU_EPP=\"balance_power\"\n CPU_BOOST = 1\n\nnoise\n");
    assert_eq!(values["CPU_EPP"], "balance_power");
    assert_eq!(values["CPU_BOOST"], "1");
    assert_eq!(values.len(), 2);
}

#[test]
fn default_profile_falls_back_to_lact_default() {
    assert_eq!(
        lact_profile(&lact(), DEFAULT),
        Some(Gpu {
            lact: "Default".into(),
            power_cap: Some("350".into()),
        })
    );
    assert_eq!(
        lact_profile(&lact(), "comfort"),
        Some(Gpu {
            lact: "comfort".into(),
            power_cap: Some("250".into()),
        })
    );
    assert_eq!(lact_profile(&lact(), "performance"), None);
}

#[test]
fn fan2go_config_argument_names_the_profile() {
    let managed = b"/usr/bin/fan2go\0-c\0/etc/fan2go/profiles/comfort.yaml\0--no-style\0";
    assert_eq!(config_profile(managed), Some("comfort".into()));
    let joined = b"/usr/bin/fan2go\0--config=/etc/fan2go/profiles/performance.yaml\0";
    assert_eq!(config_profile(joined), Some("performance".into()));
    let unmanaged = b"/usr/bin/fan2go\0-c\0/etc/fan2go/fan2go.yaml\0";
    assert_eq!(
        config_profile(unmanaged),
        Some("/etc/fan2go/fan2go.yaml".into())
    );
    assert_eq!(config_profile(b"/usr/bin/fan2go\0"), None);
}

#[test]
fn installed_profiles_join_fan_cpu_and_gpu_parts() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = roots(temporary.path());
    for name in ["balanced", "comfort", "Bad Name"] {
        write(&roots.fans_dir().join(format!("{name}.yaml")), "fans: []\n");
    }
    write(&roots.fans_dir().join("notes.txt"), "");
    write(&roots.cpu_file("comfort"), "CPU_EPP=balance_power\n");
    write(&roots.lact, LACT);
    let profiles = installed(&roots).unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["balanced", "comfort"]
    );
    assert_eq!(profiles[0].missing(), ["cpu"]);
    assert!(profiles[1].missing().is_empty());
    let table = profile_table(&profiles, "balanced", &Style::plain());
    assert!(table.contains("comfort"), "{table}");
    assert!(table.contains("250 W"), "{table}");
    // The LACT profile name is gone from the GPU column; only the cap remains.
    assert!(!table.contains("comfort 250 W"), "{table}");
    assert!(!table.contains("Default 350 W"), "{table}");
}

#[test]
fn selection_defaults_until_a_state_file_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = roots(temporary.path());
    assert_eq!(selected(&roots).unwrap(), DEFAULT);
    write(&roots.state, "HWTUNE_PROFILE=comfort\n");
    assert_eq!(selected(&roots).unwrap(), "comfort");
    write(&roots.state, "\n");
    assert!(selected(&roots).is_err());
}

#[test]
fn live_cpu_joins_differing_policy_values() {
    let temporary = tempfile::tempdir().unwrap();
    let cpufreq = temporary.path().join("devices/system/cpu/cpufreq");
    write(&cpufreq.join("boost"), "1\n");
    for (policy, epp) in [("policy0", "balance_power"), ("policy1", "power")] {
        write(
            &cpufreq.join(policy).join("scaling_governor"),
            "powersave\n",
        );
        write(
            &cpufreq.join(policy).join("energy_performance_preference"),
            &format!("{epp}\n"),
        );
    }
    let sys = Sysfs {
        sys: temporary.path().into(),
        dev: temporary.path().into(),
    };
    assert_eq!(
        cpu_summary(&live_cpu(&sys).unwrap()),
        "powersave balance_power+power boost on"
    );
}

#[test]
fn status_table_headers_the_check_columns() {
    let rows = vec![Row::ok("fans", "performance"), Row::bad("cpu", "missing")];
    let text = status_table(&rows, &Style::plain());
    assert!(text.starts_with("PART"), "{text}");
    assert!(text.contains("STATUS"), "{text}");
    assert!(text.contains("DETAIL"), "{text}");
    assert!(text.contains("ok"), "{text}");
    assert!(text.contains("bad"), "{text}");
}
