use super::*;
use crate::config::Job;

fn configuration() -> Config {
    Config {
        host: "macie".into(),
        hosts: [("archie".into(), HostConfig::default())].into(),
        jobs: [(
            "Documents".into(),
            Job {
                sources: [
                    ("macie".into(), vec!["/Documents".into()]),
                    ("archie".into(), vec!["/Documents".into()]),
                ]
                .into(),
                destinations: vec!["drive".into()],
                ..Job::default()
            },
        )]
        .into(),
        ..Config::default()
    }
}

fn peer(config: &Config) -> PeerStatus {
    let mut remote = config.clone();
    remote.host = "archie".into();
    let mut value = local(&remote, None).unwrap();
    value["items"][0]["last_verified"] = json!("2026-09-10T12:45:00Z");
    value["items"][0]["last_full_restore"] = json!("2026-09-10T13:00:00Z");
    value["items"][0]["overdue"] = json!(false);
    value["items"][0]["status"] = json!("verified");
    value["items"][0]["snapshot"] = json!("verified-snapshot");
    value["maintenance"] =
        json!({"at":"2026-09-10T13:30:00Z","warning":"fixture historical maintenance failure"});
    PeerStatus {
        value,
        exit_code: 0,
    }
}

#[test]
fn failed_newer_receipts_do_not_replace_an_older_verified_copy() {
    use crate::state::{ReplicaReceipt, RunRecord};
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::open(temp.path()).unwrap();
    let config = configuration();
    let mut good = RunRecord::new("macie", "Documents", "fingerprint");
    good.state = RunState::Committed;
    good.snapshot = Some("good-capture".into());
    let verified = Utc::now() - chrono::Duration::hours(2);
    good.replicas.insert(
        "drive".into(),
        ReplicaReceipt {
            destination: "drive".into(),
            snapshot: Some("good-snapshot".into()),
            state: ReplicaState::Verified,
            verified_at: Some(verified),
            full_verified_at: Some(verified),
            offsite: true,
            error: None,
        },
    );
    state.save_run(&good).unwrap();
    let mut failed = RunRecord::new("macie", "Documents", "fingerprint");
    failed.state = RunState::Failed;
    failed.replicas.insert(
        "drive".into(),
        ReplicaReceipt {
            destination: "drive".into(),
            snapshot: Some("failed-snapshot".into()),
            state: ReplicaState::Failed,
            verified_at: Some(Utc::now()),
            full_verified_at: Some(Utc::now()),
            offsite: true,
            error: Some("restore verification failed".into()),
        },
    );
    state.save_run(&failed).unwrap();
    let result = local(&config, Some(&state)).unwrap();
    assert_eq!(result["items"][0]["snapshot"], "good-snapshot");
    assert_eq!(result["items"][0]["last_verified"], json!(verified));
    assert_eq!(result["items"][0]["last_full_restore"], json!(verified));
    assert_eq!(result["pending"][0]["id"], failed.id);
}

#[test]
fn merged_status_preserves_owner_verification_and_maintenance_without_cloud_reads() {
    let config = configuration();
    let result = collect_with(&config, None, false, false, |host, _| {
        assert_eq!(host, "archie");
        Ok(peer(&config))
    })
    .unwrap();
    let row = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["host"] == "archie")
        .unwrap();
    assert_eq!(row["status"], "verified");
    assert_eq!(row["last_verified"], "2026-09-10T12:45:00Z");
    assert_eq!(row["last_full_restore"], "2026-09-10T13:00:00Z");
    assert_eq!(row["journal_host"], "archie");
    assert_eq!(result["hosts"][1]["status"], "reachable");
    assert_eq!(
        result["hosts"][1]["maintenance"]["warning"],
        "fixture historical maintenance failure"
    );
    assert_eq!(result["errors"], json!([]));
}

#[test]
fn local_only_never_queries_foreign_hosts_or_includes_their_rows() {
    let config = configuration();
    let result = collect_with(&config, None, true, false, |_, _| {
        panic!("local-only must not query SSH")
    })
    .unwrap();
    assert_eq!(result["local_only"], true);
    assert_eq!(result["hosts"].as_array().unwrap().len(), 1);
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["host"] == "macie")
    );
}

#[test]
fn wrong_owner_missing_tuple_and_unreachable_peer_never_claim_verified() {
    let config = configuration();
    for corruption in [
        "host",
        "journal_host",
        "tuple",
        "duplicate",
        "contradiction",
        "missing_snapshot",
        "unreachable",
    ] {
        let result = collect_with(&config, None, false, false, |_, _| {
            let mut report = peer(&config);
            match corruption {
                "host" => report.value["host"] = json!("another"),
                "journal_host" => report.value["items"][0]["journal_host"] = json!("another"),
                "tuple" => report.value["items"][0]["destination"] = json!("unexpected"),
                "contradiction" => report.value["items"][0]["overdue"] = json!(true),
                "missing_snapshot" => report.value["items"][0]["snapshot"] = Value::Null,
                "duplicate" => {
                    let duplicate = report.value["items"][0].clone();
                    report.value["items"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                _ => anyhow::bail!("SSH deadline exceeded"),
            }
            Ok(report)
        })
        .unwrap();
        let row = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["host"] == "archie")
            .unwrap();
        assert!(row["last_verified"].is_null());
        assert_eq!(row["status"], "unknown; source unreachable");
        assert_eq!(result["hosts"][1]["status"], "unreachable");
        assert!(!result["errors"].as_array().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn a_valid_exit_two_status_retains_verified_rows_and_reports_its_failure() {
    use std::os::unix::process::ExitStatusExt;
    let config = configuration();
    let report = peer(&config);
    let decoded = decode(CapturedOutput {
        status: std::process::ExitStatus::from_raw(2 << 8),
        stdout: serde_json::to_vec(&report.value).unwrap(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    })
    .unwrap();
    let result = collect_with(&config, None, false, false, |_, _| {
        Ok(PeerStatus {
            value: decoded.value.clone(),
            exit_code: decoded.exit_code,
        })
    })
    .unwrap();
    assert_eq!(result["hosts"][1]["status"], "reachable");
    assert_eq!(result["hosts"][1]["exit_code"], 2);
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["host"] == "archie" && row["status"] == "verified")
    );
    assert!(!result["errors"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn remote_command_quotes_paths_and_only_invokes_local_status() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("dcloud binary'quoted");
    std::fs::write(&binary, "#!/bin/sh\nprintf '%s\\000' \"$@\"\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = temp.path().join("config'quoted.toml");
    let script = remote_script(&HostConfig {
        binary: Some(binary.display().to_string()),
        config: Some(config_path.display().to_string()),
        ..HostConfig::default()
    });
    let output = hostkit::process::output(
        std::process::Command::new("sh").args(["-c", &script]),
        CaptureLimits::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.split_terminator('\0').collect::<Vec<_>>(),
        [
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "status",
            "--local"
        ]
    );
}
