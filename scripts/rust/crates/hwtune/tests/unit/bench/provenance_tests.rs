use super::*;

#[test]
fn bios_settings_ignore_export_date_formatting_and_unrelated_order() {
    let a = bios_source(
        Path::new("a.txt"),
        b"[2026/09/11 12:00]\nBoost [Enabled]\nPower [Auto]\n",
    )
    .unwrap();
    let b = bios_source(
        Path::new("b.txt"),
        b"[2026/09/12 14:00]\r\nPower [Auto]  \r\nBoost [Enabled]\r\n",
    )
    .unwrap();
    assert_eq!(a.settings_sha256, b.settings_sha256);
    assert_ne!(a.content_sha256, b.content_sha256);
    let changed = bios_source(Path::new("c.txt"), b"Boost [Disabled]\nPower [Auto]\n").unwrap();
    assert_ne!(a.settings_sha256, changed.settings_sha256);
}

#[test]
fn repeated_bios_setting_occurrences_retain_their_order() {
    let a = bios_source(Path::new("a"), b"Core [1]\nCore [2]\n").unwrap();
    let b = bios_source(Path::new("b"), b"Core [2]\nCore [1]\n").unwrap();
    assert_ne!(a.settings_sha256, b.settings_sha256);
    assert!(bios_source(Path::new("invalid"), b"no settings").is_err());
}

#[test]
fn lact_settings_ignore_comments_key_order_and_yaml_style() {
    let a = lact_source(Path::new("a"), b"# config\ngpu:\n  fan: 40\n  cap: 220\n").unwrap();
    let b = lact_source(Path::new("b"), b"gpu: {cap: 220, fan: 40}\n").unwrap();
    assert_eq!(a.settings_sha256, b.settings_sha256);
    assert_ne!(a.content_sha256, b.content_sha256);
    let changed = lact_source(Path::new("c"), b"gpu: {cap: 221, fan: 40}\n").unwrap();
    assert_ne!(a.settings_sha256, changed.settings_sha256);
    assert!(lact_source(Path::new("invalid"), b"[a, b]").is_err());
}

#[test]
fn observations_enumerate_available_policies_without_inventing_missing_controls() {
    let dir = tempfile::tempdir().unwrap();
    for (policy, governor) in [("policy0", "powersave"), ("policy8", "performance")] {
        let path = dir.path().join("devices/system/cpu/cpufreq").join(policy);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("scaling_governor"), governor).unwrap();
    }
    let sys = Sysfs {
        sys: dir.path().into(),
        dev: dir.path().join("dev"),
    };
    let settings = observed_settings(&sys);
    assert_eq!(settings.len(), 2);
    assert_eq!(settings["cpu.policy8.scaling_governor"], "performance");
    assert!(!settings.contains_key("cpu.boost"));
}

fn receipt() -> StabilitySession {
    StabilitySession {
        schema: 1,
        host: "desktop".into(),
        session: "cpu-session".into(),
        completed: "2026-09-11T00:00:00Z".into(),
        profile: "all-core".into(),
        result: "pass".into(),
        context: RunContext {
            observed: BTreeMap::from([(
                "cpu.policy0.scaling_governor".into(),
                "performance".into(),
            )]),
            ..RunContext::default()
        },
        context_unchanged: true,
        evidence_known: true,
        details: BTreeMap::new(),
        samples_path: "cpu-session.csv".into(),
    }
}

#[test]
fn stability_links_require_matching_host_and_configuration_and_retain_failed_verdicts() {
    let mut receipt = receipt();
    let mut context = receipt.context.clone();
    assert!(link_session(&mut context, "laptop", &receipt).is_err());
    receipt.evidence_known = false;
    assert!(link_session(&mut context, "desktop", &receipt).is_err());
    receipt.evidence_known = true;
    receipt.context_unchanged = false;
    assert!(link_session(&mut context, "desktop", &receipt).is_err());
    receipt.context_unchanged = true;
    receipt.result = "fail".into();
    link_session(&mut context, "desktop", &receipt).unwrap();
    link_session(&mut context, "desktop", &receipt).unwrap();
    assert_eq!(context.stability_sessions.len(), 1);
    assert_eq!(context.stability_sessions[0].result, "fail");
    context
        .observed
        .insert("cpu.policy0.scaling_governor".into(), "powersave".into());
    assert!(link_session(&mut context, "desktop", &receipt).is_err());
}

#[test]
fn absent_observations_never_establish_a_matching_active_configuration() {
    assert!(!same_settings(
        &RunContext::default(),
        &RunContext::default()
    ));
}

#[test]
fn stability_receipts_create_versioned_store_and_round_trip_context() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("benchmarks"));
    let expected = receipt();
    let path = save_stability(&store, &expected).unwrap();
    let actual: StabilitySession = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(actual.context, expected.context);
    assert_eq!(actual.session, expected.session);
    assert!(store.root.join("store.json").is_file());
    assert!(save_stability(&store, &expected).is_err());
}

#[test]
fn optional_context_does_not_change_existing_hardware_epoch_or_metric_json() {
    let mut run = super::super::record::Run::default();
    let epoch = run.epoch();
    assert!(run.to_json().get("context").is_none());
    run.context = Some(receipt().context);
    let loaded: super::super::record::Run = serde_json::from_value(run.to_json()).unwrap();
    assert_eq!(loaded.context, run.context);
    assert_eq!(loaded.epoch(), epoch);
}
