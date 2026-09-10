use super::*;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-10T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn replica(name: &str, snapshot: &str, offsite: bool) -> ReplicaReceipt {
    ReplicaReceipt {
        destination: name.into(),
        snapshot: Some(snapshot.into()),
        offsite,
        state: ReplicaState::Verified,
        verified_at: Some(now()),
        full_verified_at: Some(now()),
        error: None,
    }
}

#[test]
fn quorum_counts_unique_selected_verified_copies_and_required_offsite() -> Result<()> {
    let job = Job {
        destinations: vec!["local".into(), "drive".into(), "vps".into()],
        required: vec!["drive".into()],
        min_copies: 2,
        require_offsite: true,
        ..Job::default()
    };
    let mut receipts = BTreeMap::from([
        ("local".into(), replica("local", "one", false)),
        ("vps".into(), replica("vps", "two", true)),
    ]);
    let assessment = evaluate(&job, &receipts)?;
    assert!(!assessment.satisfied);
    assert_eq!(assessment.missing_required, vec!["drive"]);
    receipts.insert("drive".into(), replica("drive", "three", true));
    assert!(evaluate(&job, &receipts)?.satisfied);
    receipts.get_mut("drive").unwrap().verified_at = None;
    assert!(!evaluate(&job, &receipts)?.satisfied);
    Ok(())
}

#[test]
fn copies_outside_selection_and_misnamed_receipts_do_not_count() -> Result<()> {
    let job = Job {
        destinations: vec!["drive".into()],
        min_copies: 1,
        ..Job::default()
    };
    let receipts = BTreeMap::from([
        ("drive".into(), replica("another", "one", true)),
        ("outside".into(), replica("outside", "two", true)),
    ]);
    assert_eq!(evaluate(&job, &receipts)?.verified_copies, 0);
    Ok(())
}

fn entry(id: &str, host: &str, days_ago: i64) -> RetentionEntry {
    RetentionEntry {
        id: id.into(),
        host: host.into(),
        job: "documents".into(),
        time: now() - Duration::days(days_ago),
        pinned: false,
        complete: true,
        verified: true,
        protected: false,
    }
}

#[test]
fn retention_groups_hosts_and_never_counts_incomplete_or_future_snapshots() -> Result<()> {
    let mut incomplete = entry("incomplete", "archie", 0);
    incomplete.complete = false;
    let mut pinned = entry("pinned", "archie", 100);
    pinned.pinned = true;
    let entries = vec![
        entry("recent", "archie", 2),
        entry("old", "archie", 3),
        entry("other-host", "macie", 100),
        entry("future", "archie", -50),
        incomplete,
        pinned,
    ];
    let rules = Retention {
        last: 1,
        weekly: 0,
        monthly: 0,
        yearly: 0,
        auto: true,
    };
    let plan = retention_plan(&entries, &rules, now())?;
    assert_eq!(plan.delete, vec!["old"]);
    assert!(plan.retain.contains(&"other-host".into()));
    assert!(plan.retain.contains(&"recent".into()));
    Ok(())
}

#[test]
fn retention_is_deterministic_across_input_order_and_calendar_buckets() -> Result<()> {
    let entries: Vec<_> = (0..100)
        .map(|day| entry(&format!("snapshot-{day:03}"), "archie", day))
        .collect();
    let rules = Retention {
        last: 3,
        weekly: 3,
        monthly: 3,
        yearly: 1,
        auto: true,
    };
    let first = retention_plan(&entries, &rules, now())?;
    let mut reversed = entries.clone();
    reversed.reverse();
    let second = retention_plan(&reversed, &rules, now())?;
    assert_eq!(first.retain, second.retain);
    assert_eq!(first.delete, second.delete);
    assert!(first.retain.contains(&"snapshot-000".into()));
    assert!(first.retain.contains(&"snapshot-001".into()));
    assert!(first.retain.contains(&"snapshot-002".into()));
    assert!(first.retain.len() >= 6);
    Ok(())
}

fn evidence(source: &Path) -> Result<CleanupEvidence> {
    let mut run = RunRecord::new("archie", "documents", "hash");
    run.state = RunState::Committed;
    run.snapshot = Some("canonical".into());
    run.replicas
        .insert("drive".into(), replica("drive", "destination-copy", true));
    CleanupEvidence::from_run(&run, std::fs::read_to_string(source)?)
}

#[test]
fn source_deletion_requires_full_verified_copies() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    let mut evidence = evidence(&source)?;
    evidence.replicas.get_mut("drive").unwrap().full_verified_at = None;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    assert!(
        quarantine(&state, &source, &policy, evidence, now(), |path| Ok(
            std::fs::read_to_string(path)?
        ))
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(source)?, "important");
    Ok(())
}

#[test]
fn quarantine_keeps_changed_source_and_observes_grace_period() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time((now() - Duration::days(10)).timestamp(), 0),
    )?;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    let evidence = evidence(&source)?;
    let fingerprint = |path: &Path| Ok(std::fs::read_to_string(path)?);
    let mut ticket = quarantine(
        &state,
        &source,
        &policy,
        evidence.clone(),
        now(),
        fingerprint,
    )?;
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&ticket.held)?, "important");
    assert!(!reap_quarantine(
        &state,
        &mut ticket,
        &policy,
        evidence.clone(),
        now(),
        fingerprint
    )?);
    std::fs::write(&ticket.held, "changed through an existing writer")?;
    let future = now() + Duration::days(2);
    let mut fresh = evidence;
    fresh.replicas.get_mut("drive").unwrap().full_verified_at = Some(future);
    assert!(!reap_quarantine(
        &state,
        &mut ticket,
        &policy,
        fresh,
        future,
        fingerprint
    )?);
    assert_eq!(ticket.state, QuarantineState::Changed);
    assert!(ticket.held.exists());
    Ok(())
}

#[test]
fn interrupted_reaper_reconciles_already_removed_material() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time((now() - Duration::days(10)).timestamp(), 0),
    )?;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    let mut proof = evidence(&source)?;
    let mut ticket = quarantine(&state, &source, &policy, proof.clone(), now(), |path| {
        Ok(std::fs::read_to_string(path)?)
    })?;
    std::fs::remove_file(&ticket.held)?;
    let future = now() + Duration::days(2);
    proof.replicas.get_mut("drive").unwrap().full_verified_at = Some(future);
    assert!(reap_quarantine(
        &state,
        &mut ticket,
        &policy,
        proof,
        future,
        |_| panic!("missing material must not be read")
    )?);
    assert_eq!(ticket.state, QuarantineState::Deleted);
    assert_eq!(
        state
            .load_value::<QuarantineTicket>("quarantine", &ticket.id)?
            .unwrap()
            .state,
        QuarantineState::Deleted
    );
    Ok(())
}

#[test]
fn source_changed_between_verification_and_rename_remains_quarantined() -> Result<()> {
    use std::cell::Cell;
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time((now() - Duration::days(10)).timestamp(), 0),
    )?;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    let evidence = evidence(&source)?;
    let calls = Cell::new(0);
    let ticket = quarantine(&state, &source, &policy, evidence, now(), |path| {
        calls.set(calls.get() + 1);
        let value = std::fs::read_to_string(path)?;
        if calls.get() == 1 {
            std::fs::write(path, "concurrent change")?;
        }
        Ok(value)
    })?;
    assert_eq!(ticket.state, QuarantineState::Changed);
    assert_eq!(std::fs::read_to_string(ticket.held)?, "concurrent change");
    Ok(())
}

#[test]
fn quarantine_retries_reuse_the_existing_ticket_after_source_has_moved() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time((now() - Duration::days(10)).timestamp(), 0),
    )?;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    let evidence = evidence(&source)?;
    let first = quarantine(&state, &source, &policy, evidence.clone(), now(), |path| {
        Ok(std::fs::read_to_string(path)?)
    })?;
    let mut interrupted = first.clone();
    interrupted.state = QuarantineState::Prepared;
    state.save_value("quarantine", &interrupted.id, &interrupted)?;
    let retry = quarantine(&state, &source, &policy, evidence, now(), |path| {
        Ok(std::fs::read_to_string(path)?)
    })?;
    assert_eq!(retry.id, first.id);
    assert_eq!(retry.state, QuarantineState::Held);
    assert_eq!(state.values::<QuarantineTicket>("quarantine")?.len(), 1);
    assert_eq!(std::fs::read_to_string(&retry.held)?, "important");
    Ok(())
}

#[test]
fn fresh_but_misidentified_replica_cannot_authorize_quarantine_deletion() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::write(&source, "important")?;
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time((now() - Duration::days(10)).timestamp(), 0),
    )?;
    let policy = Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    };
    let state = State::open(&directory.path().join("state"))?;
    let mut evidence = evidence(&source)?;
    let mut ticket = quarantine(&state, &source, &policy, evidence.clone(), now(), |path| {
        Ok(std::fs::read_to_string(path)?)
    })?;
    let future = now() + Duration::days(2);
    let mut impersonated = replica("incorrect", "copy", true);
    impersonated.full_verified_at = Some(future);
    evidence.replicas.insert("another".into(), impersonated);
    assert!(
        reap_quarantine(&state, &mut ticket, &policy, evidence, future, |path| Ok(
            std::fs::read_to_string(path)?
        ))
        .is_err()
    );
    assert!(ticket.held.exists());
    Ok(())
}
