use super::*;
use crate::bench::provenance::RunContext;

fn export_of(lines: &[&str]) -> Export {
    export::parse(&lines.join("\n"))
}

fn all_core_export() -> Export {
    export_of(&[
        "[2026/09/12 10:00:00]",
        "Curve Optimizer [All Cores]",
        "All Core Curve Optimizer Sign [Negative]",
        "All Core Curve Optimizer Magnitude [25]",
        "Curve Optimizer [Disable]",
    ])
}

fn session(
    bios: &str,
    minutes: u64,
    verdicts: &[(u32, &str)],
    offset: Option<i32>,
) -> StabilitySession {
    let mut details = BTreeMap::new();
    details.insert("profile".to_string(), "per-core".to_string());
    details.insert("minutes".to_string(), minutes.to_string());
    details.insert("bios".to_string(), bios.to_string());
    details.insert("result".to_string(), "pass".to_string());
    if let Some(offset) = offset {
        details.insert("offset".to_string(), offset.to_string());
    }
    for (core, verdict) in verdicts {
        details.insert(format!("core{core}"), (*verdict).to_string());
    }
    StabilitySession {
        schema: 1,
        host: "fixture".into(),
        session: format!("session-{bios}-{minutes}"),
        completed: "2026-09-12T10:00:00".into(),
        profile: "per-core".into(),
        result: "pass".into(),
        context: RunContext::default(),
        context_unchanged: true,
        evidence_known: true,
        details,
        samples_path: String::new(),
    }
}

#[test]
fn all_core_exports_apply_one_offset_to_every_core() {
    let offsets = parse_offsets(&all_core_export(), &[0, 1, 2]);
    assert_eq!(offsets, Offsets::from([(0, -25), (1, -25), (2, -25)]));
    assert_eq!(mode(&all_core_export()).as_deref(), Some("All Cores"));
}

#[test]
fn per_core_exports_take_the_first_occurrence_per_core() {
    let export = export_of(&[
        "Curve Optimizer [Per Core]",
        "Core 0 Curve Optimizer Sign [Negative]",
        "Core 0 Curve Optimizer Magnitude [30]",
        "Core 1 Curve Optimizer Sign [Positive]",
        "Core 1 Curve Optimizer Magnitude [5]",
        "Core 0 Curve Optimizer Magnitude [99]",
        "Core 3 Curve Optimizer Sign [Negative]",
    ]);
    let offsets = parse_offsets(&export, &[0, 1, 2, 3]);
    assert_eq!(offsets, Offsets::from([(0, -30), (1, 5)]));
}

#[test]
fn disabled_or_absent_curve_optimizer_is_zero_or_unknown() {
    let disabled = export_of(&["Curve Optimizer [Disable]"]);
    assert_eq!(
        parse_offsets(&disabled, &[0, 1]),
        Offsets::from([(0, 0), (1, 0)])
    );
    let absent = export_of(&["Precision Boost Overdrive [Enabled]"]);
    assert!(parse_offsets(&absent, &[0, 1]).is_empty());
    let auto = export_of(&[
        "Curve Optimizer [All Cores]",
        "All Core Curve Optimizer Sign [Negative]",
        "All Core Curve Optimizer Magnitude [Auto]",
    ]);
    assert!(parse_offsets(&auto, &[0]).is_empty());
}

#[test]
fn evidence_comes_from_long_per_core_sessions_matched_to_exports() {
    let cores = [0, 1];
    let exports = BTreeMap::from([
        ("aaaa".to_string(), Offsets::from([(0, -25), (1, -25)])),
        ("bbbb".to_string(), Offsets::from([(0, -30), (1, -30)])),
    ]);
    let mut sessions = vec![
        session("aaaa", 10, &[(0, "pass"), (1, "pass")], None),
        session("bbbb", 10, &[(0, "pass"), (1, "fail (exit 1)")], None),
        session("cccc", 5, &[(0, "pass"), (1, "pass")], Some(-35)),
        session(
            "cccc",
            10,
            &[(0, "fail (rebooted)"), (1, "unknown")],
            Some(-35),
        ),
    ];
    let mut ignored = session("aaaa", 10, &[(0, "fail")], None);
    ignored.details.insert("profile".into(), "all-core".into());
    sessions.push(ignored);
    let found = evidence(&sessions, &exports, &cores);
    assert_eq!(found[&0].passed, BTreeSet::from([-25, -30]));
    assert_eq!(found[&0].failed, BTreeSet::from([-35]));
    assert_eq!(found[&1].passed, BTreeSet::from([-25]));
    assert_eq!(found[&1].failed, BTreeSet::from([-30]));
    assert_eq!(found[&0].best_passed(), Some(-30));
    assert_eq!(found[&1].shallowest_failed(), Some(-30));
}

#[test]
fn suggestions_follow_the_ladder_rules() {
    let proof = |passed: &[i32], failed: &[i32]| CoreEvidence {
        passed: passed.iter().copied().collect(),
        failed: failed.iter().copied().collect(),
    };
    assert_eq!(suggest(None, &proof(&[], &[])).action, Action::NoExport);
    let stress = suggest(Some(-25), &proof(&[], &[]));
    assert_eq!((stress.action, stress.offset), (Action::Stress, Some(-25)));
    let next = suggest(Some(-25), &proof(&[-25], &[]));
    assert_eq!((next.action, next.offset), (Action::Try, Some(-30)));
    let hold = suggest(Some(-25), &proof(&[-25], &[-30]));
    assert_eq!((hold.action, hold.offset), (Action::Hold, Some(-25)));
    let back = suggest(Some(-35), &proof(&[-25, -30], &[-35]));
    assert_eq!((back.action, back.offset), (Action::BackOff, Some(-30)));
    let untested = suggest(Some(-35), &proof(&[], &[-35]));
    assert_eq!(
        (untested.action, untested.offset),
        (Action::BackOff, Some(-30))
    );
    let zero = suggest(Some(0), &proof(&[], &[0]));
    assert_eq!(zero.offset, Some(0));
    let floor = suggest(Some(-50), &proof(&[-50], &[]));
    assert_eq!(floor.action, Action::Hold);
    assert_eq!(next.text(), "try -30");
    assert_eq!(stress.text(), "stress -25 first");
    assert_eq!(back.text(), "back off to -30");
    assert_eq!(hold.text(), "hold");
}

#[test]
fn drift_and_stretch_thresholds() {
    assert!((drift_pct(100.0, 97.0).unwrap() + 3.0).abs() < 1e-9);
    assert!(drift_pct(0.0, 97.0).is_none());
    assert!(!stretching(drift_pct(100.0, 97.0)));
    assert!(stretching(drift_pct(100.0, 96.9)));
    assert!(!stretching(None));
}

#[test]
fn status_groups_commands_by_target_offset_and_flags_stretching() {
    let cores = [(0, Some(196)), (1, Some(176)), (2, Some(166))];
    let exports = BTreeMap::from([(
        "aaaa".to_string(),
        Offsets::from([(0, -25), (1, -25), (2, -25)]),
    )]);
    let sessions = vec![session(
        "aaaa",
        10,
        &[(0, "pass"), (1, "pass"), (2, "fail")],
        None,
    )];
    let samples = vec![
        Sample {
            schema: 1,
            host: "fixture".into(),
            taken: "2026-09-11T10:00:00".into(),
            bios: "aaaa".into(),
            iterations: 1,
            worker: "w1".into(),
            worker_path: String::new(),
            cores: BTreeMap::from([
                (
                    0,
                    CoreSample {
                        mops: 900.0,
                        prefcore: Some(196),
                    },
                ),
                (
                    1,
                    CoreSample {
                        mops: 900.0,
                        prefcore: Some(176),
                    },
                ),
            ]),
        },
        Sample {
            schema: 1,
            host: "fixture".into(),
            taken: "2026-09-12T10:00:00".into(),
            bios: "aaaa".into(),
            iterations: 1,
            worker: "w1".into(),
            worker_path: String::new(),
            cores: BTreeMap::from([
                (
                    0,
                    CoreSample {
                        mops: 905.0,
                        prefcore: Some(196),
                    },
                ),
                (
                    1,
                    CoreSample {
                        mops: 850.0,
                        prefcore: Some(176),
                    },
                ),
            ]),
        },
    ];
    let status = build_status(
        "fixture",
        &cores,
        Some(("fixture-1681-20260912.txt".into(), all_core_export())),
        &exports,
        &sessions,
        &samples,
        None,
    );
    assert_eq!(status.mode.as_deref(), Some("All Cores"));
    assert_eq!(status.cores[0].next.text(), "try -30");
    assert_eq!(status.cores[1].next.text(), "try -30");
    assert_eq!(status.cores[2].next.text(), "back off to -20");
    assert!(!status.cores[0].stretching);
    assert!(status.cores[1].stretching);
    assert_eq!(
        status.bios_changes,
        vec!["core 0 -30", "core 1 -30", "core 2 -20"]
    );
    assert_eq!(
        status.commands,
        vec![
            "hwtune stress cpu --profile per-core --cores 2 --offset -20 --minutes 10",
            "hwtune stress cpu --profile per-core --cores 0,1 --offset -30 --minutes 10",
        ]
    );
    let text = render(&status);
    assert!(text.contains("curve optimizer  All Cores  (fixture-1681-20260912.txt)"));
    assert!(text.contains("stretching?"));
    assert!(text.contains("ryzen_smu-dkms-git"));
    assert!(text.contains("stress  hwtune stress cpu --profile per-core --cores 0,1 --offset -30"));
}

#[test]
fn hold_and_missing_export_produce_no_commands() {
    let cores = [(0, None)];
    let status = build_status("fixture", &cores, None, &BTreeMap::new(), &[], &[], None);
    assert_eq!(status.cores[0].next.action, Action::NoExport);
    assert!(status.commands.is_empty());
    assert!(render(&status).contains("no export"));
}

#[test]
fn bench_output_and_samples_round_trip() {
    assert!((parse_mops("{\"value\":895.25}").unwrap() - 895.25).abs() < 1e-9);
    assert!(parse_mops("{\"value\":0}").is_err());
    assert!(parse_mops("nope").is_err());
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path().join("history"));
    assert!(load_samples(&store, "fixture").unwrap().is_empty());
    let dir = temp.path().join("history/hosts/fixture/curve");
    fs::create_dir_all(&dir).unwrap();
    for (name, taken, host) in [
        ("b.json", "2026-09-12T10:00:00", "fixture"),
        ("a.json", "2026-09-13T10:00:00", "fixture"),
        ("c.json", "2026-09-14T10:00:00", "other"),
    ] {
        let sample = Sample {
            schema: 1,
            host: host.into(),
            taken: taken.into(),
            bios: "aaaa".into(),
            iterations: 1,
            worker: "w1".into(),
            worker_path: String::new(),
            cores: BTreeMap::new(),
        };
        fs::write(dir.join(name), serde_json::to_vec(&sample).unwrap()).unwrap();
    }
    let samples = load_samples(&store, "fixture").unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[1].taken, "2026-09-13T10:00:00");
    assert!(load_samples(&store, "../x").is_err());
}

#[test]
fn ryzen_smu_presence_is_read_from_sysfs() {
    let temp = tempfile::tempdir().unwrap();
    let sys = Sysfs {
        sys: temp.path().into(),
        dev: temp.path().join("dev"),
    };
    assert!(ryzen_smu(&sys).is_none());
    let dir = temp.path().join("kernel/ryzen_smu_drv");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("version"), "0.1.7\n").unwrap();
    fs::write(dir.join("codename"), "GraniteRidge\n").unwrap();
    let smu = ryzen_smu(&sys).unwrap();
    assert_eq!(
        (smu.version.as_str(), smu.codename.as_str()),
        ("0.1.7", "GraniteRidge")
    );
}

#[test]
fn drift_only_compares_samples_from_the_same_worker_and_iterations() {
    let sample = |taken: &str, worker: &str, iterations: u64, mops: f64| Sample {
        schema: 1,
        host: "fixture".into(),
        taken: taken.into(),
        bios: "aaaa".into(),
        iterations,
        worker: worker.into(),
        worker_path: String::new(),
        cores: BTreeMap::from([(
            0,
            CoreSample {
                mops,
                prefcore: None,
            },
        )]),
    };
    let release = sample("2026-09-10T10:00:00", "rel", 10, 900.0);
    let debug = sample("2026-09-11T10:00:00", "dbg", 10, 300.0);
    let latest = sample("2026-09-12T10:00:00", "rel", 10, 880.0);
    assert!(comparable(&release, &latest));
    assert!(!comparable(&debug, &latest));
    assert!(!comparable(&sample("x", "rel", 5, 1.0), &latest));
    let status = build_status(
        "fixture",
        &[(0, None)],
        None,
        &BTreeMap::new(),
        &[],
        &[release, debug, latest.clone()],
        None,
    );
    assert_eq!(status.cores[0].previous_mops, Some(900.0));
    assert!(status.cores[0].drift_pct.is_some_and(|d| d < -2.0));
    let unknown = build_status(
        "fixture",
        &[(0, None)],
        None,
        &BTreeMap::new(),
        &[],
        &[latest],
        None,
    );
    assert_eq!(unknown.cores[0].previous_mops, None);
    let legacy: Sample = serde_json::from_str(
        "{\"schema\":1,\"host\":\"fixture\",\"taken\":\"t\",\"bios\":\"b\",\"iterations\":1,\"cores\":{}}",
    )
    .unwrap();
    assert!(legacy.worker.is_empty());
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("worker");
    fs::write(&binary, b"bytes").unwrap();
    assert_eq!(worker_identity(&binary).unwrap().len(), 8);
    assert!(worker_identity(&temp.path().join("missing")).is_err());
}
