use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{Cleanup, Job, Retention};
use crate::state::{ReplicaReceipt, ReplicaState, RunRecord, RunState, State};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplicaAssessment {
    pub satisfied: bool,
    pub degraded: bool,
    pub verified_copies: usize,
    pub missing_required: Vec<String>,
    pub pending: Vec<String>,
    pub offsite_verified: bool,
}

pub fn evaluate(
    job: &Job,
    replicas: &BTreeMap<String, ReplicaReceipt>,
) -> Result<ReplicaAssessment> {
    ensure!(
        job.min_copies > 0 && job.min_copies <= job.destinations.len(),
        "invalid replica quorum"
    );
    let selected: BTreeSet<_> = job.destinations.iter().cloned().collect();
    ensure!(
        selected.len() == job.destinations.len(),
        "duplicate destinations cannot count toward quorum"
    );
    ensure!(
        job.required.iter().all(|name| selected.contains(name)),
        "required destinations must be selected"
    );
    let valid = |name: &str| {
        replicas.get(name).filter(|receipt| {
            receipt.destination == name
                && receipt.state == ReplicaState::Verified
                && receipt.verified_at.is_some()
                && receipt.snapshot.is_some()
        })
    };
    let verified: Vec<_> = selected.iter().filter_map(|name| valid(name)).collect();
    let pending: Vec<_> = selected
        .iter()
        .filter(|name| valid(name).is_none())
        .cloned()
        .collect();
    let missing_required: Vec<_> = job
        .required
        .iter()
        .filter(|name| valid(name).is_none())
        .cloned()
        .collect();
    let offsite_verified = verified.iter().any(|receipt| receipt.offsite);
    let satisfied = missing_required.is_empty()
        && verified.len() >= job.min_copies
        && (!job.require_offsite || offsite_verified);
    Ok(ReplicaAssessment {
        satisfied,
        degraded: !pending.is_empty(),
        verified_copies: verified.len(),
        missing_required,
        pending,
        offsite_verified,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetentionEntry {
    pub id: String,
    pub host: String,
    pub job: String,
    pub time: DateTime<Utc>,
    pub pinned: bool,
    pub complete: bool,
    pub verified: bool,
    pub protected: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RetentionPlan {
    pub retain: Vec<String>,
    pub delete: Vec<String>,
}

pub fn retention_plan(
    entries: &[RetentionEntry],
    rules: &Retention,
    now: DateTime<Utc>,
) -> Result<RetentionPlan> {
    ensure!(
        rules.last > 0,
        "retention must preserve at least one recent snapshot"
    );
    let unique: BTreeSet<_> = entries.iter().map(|entry| &entry.id).collect();
    ensure!(
        unique.len() == entries.len(),
        "retention input contains duplicate snapshot IDs"
    );
    let mut groups: BTreeMap<(&str, &str), Vec<&RetentionEntry>> = BTreeMap::new();
    for entry in entries {
        groups
            .entry((&entry.host, &entry.job))
            .or_default()
            .push(entry);
    }
    let mut keep = BTreeSet::new();
    for group in groups.values_mut() {
        group.sort_by(|left, right| {
            right
                .time
                .cmp(&left.time)
                .then_with(|| left.id.cmp(&right.id))
        });
        let eligible: Vec<_> = group
            .iter()
            .copied()
            .filter(|entry| entry.complete && entry.verified && entry.time <= now)
            .collect();
        for entry in group.iter().filter(|entry| {
            entry.pinned
                || entry.protected
                || !entry.complete
                || !entry.verified
                || entry.time > now
        }) {
            keep.insert(entry.id.clone());
        }
        if let Some(latest) = eligible.first() {
            keep.insert(latest.id.clone());
        }
        for entry in eligible.iter().take(rules.last) {
            keep.insert(entry.id.clone());
        }
        let mut weeks = BTreeSet::new();
        let mut months = BTreeSet::new();
        let mut years = BTreeSet::new();
        for entry in eligible {
            let week = entry.time.iso_week();
            if weeks.len() < rules.weekly && weeks.insert((week.year(), week.week())) {
                keep.insert(entry.id.clone());
            }
            if months.len() < rules.monthly
                && months.insert((entry.time.year(), entry.time.month()))
            {
                keep.insert(entry.id.clone());
            }
            if years.len() < rules.yearly && years.insert(entry.time.year()) {
                keep.insert(entry.id.clone());
            }
        }
    }
    let mut plan = RetentionPlan::default();
    for id in unique {
        if keep.contains(id) {
            plan.retain.push(id.clone());
        } else {
            plan.delete.push(id.clone());
        }
    }
    Ok(plan)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupEvidence {
    pub run_id: String,
    pub snapshot: String,
    pub source_fingerprint: String,
    pub replicas: BTreeMap<String, ReplicaReceipt>,
}

impl CleanupEvidence {
    pub fn from_run(run: &RunRecord, source_fingerprint: String) -> Result<Self> {
        ensure!(
            matches!(run.state, RunState::Committed | RunState::Degraded),
            "cleanup requires a committed backup run"
        );
        ensure!(
            !source_fingerprint.is_empty(),
            "cleanup requires a source fingerprint"
        );
        Ok(Self {
            run_id: run.id.clone(),
            snapshot: run
                .snapshot
                .clone()
                .context("cleanup run has no snapshot")?,
            source_fingerprint,
            replicas: run.replicas.clone(),
        })
    }

    fn validate(&self, policy: &Cleanup) -> Result<()> {
        ensure!(
            policy.min_age_days > 0 && policy.quarantine_days > 0 && policy.min_verified_copies > 0,
            "cleanup needs positive age, grace and verified-copy limits"
        );
        ensure!(
            !self.source_fingerprint.is_empty(),
            "cleanup fingerprint is empty"
        );
        let count = self.verified_receipts().count();
        ensure!(
            count >= policy.min_verified_copies,
            "cleanup requires {} verified copies; found {count}",
            policy.min_verified_copies
        );
        Ok(())
    }

    fn verified_receipts(&self) -> impl Iterator<Item = &ReplicaReceipt> {
        self.replicas.iter().filter_map(|(name, receipt)| {
            (receipt.destination == *name
                && receipt.snapshot.is_some()
                && receipt.state == ReplicaState::Verified
                && receipt.verified_at.is_some()
                && receipt.full_verified_at.is_some())
            .then_some(receipt)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineState {
    Prepared,
    Held,
    Changed,
    Deleted,
    Restored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuarantineTicket {
    pub id: String,
    pub original: PathBuf,
    pub held: PathBuf,
    pub created: DateTime<Utc>,
    pub delete_after: DateTime<Utc>,
    pub state: QuarantineState,
    pub evidence: CleanupEvidence,
}

pub fn quarantine(
    state: &State,
    source: &Path,
    policy: &Cleanup,
    evidence: CleanupEvidence,
    now: DateTime<Utc>,
    fingerprint: impl Fn(&Path) -> Result<String>,
) -> Result<QuarantineTicket> {
    let _lock = state.lock(&format!("cleanup:{}", source.display()))?;
    evidence.validate(policy)?;
    ensure!(
        source.is_absolute() && source.parent().is_some() && source.file_name().is_some(),
        "cleanup requires a selected absolute path"
    );
    let existing = state
        .values::<QuarantineTicket>("quarantine")?
        .into_iter()
        .map(|(_, ticket)| ticket)
        .find(|ticket| ticket.original == source && ticket.evidence.run_id == evidence.run_id);
    if let Some(mut ticket) = existing.clone() {
        ensure!(
            ticket.evidence.snapshot == evidence.snapshot
                && ticket.evidence.source_fingerprint == evidence.source_fingerprint,
            "existing quarantine ticket has different backup evidence"
        );
        if ticket.state != QuarantineState::Prepared {
            return Ok(ticket);
        }
        validate_ticket_path(&ticket)?;
        match std::fs::symlink_metadata(&ticket.held) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "quarantined source became a symbolic link"
                );
                ticket.state = if fingerprint(&ticket.held)? == ticket.evidence.source_fingerprint {
                    QuarantineState::Held
                } else {
                    QuarantineState::Changed
                };
                state.save_value("quarantine", &ticket.id, &ticket)?;
                return Ok(ticket);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = std::fs::symlink_metadata(source).context("inspect cleanup source")?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "cleanup source must not be a symbolic link"
    );
    let cutoff = now - Duration::days(i64::from(policy.min_age_days));
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.context("scan cleanup source")?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let modified: DateTime<Utc> = metadata.modified()?.into();
        ensure!(
            modified <= cutoff,
            "cleanup source contains recently modified material: {}",
            entry.path().display()
        );
    }
    ensure!(
        fingerprint(source)? == evidence.source_fingerprint,
        "source changed since its verified backup"
    );
    let parent = source.parent().context("cleanup source has no parent")?;
    let directory = parent.join(".dcloud-quarantine");
    if let Ok(metadata) = std::fs::symlink_metadata(&directory) {
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "quarantine must be a real directory"
        );
    } else {
        std::fs::create_dir(&directory)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let id = existing
        .map(|ticket| ticket.id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut ticket = QuarantineTicket {
        held: directory.join(&id),
        id,
        original: source.to_owned(),
        created: now,
        delete_after: now + Duration::days(i64::from(policy.quarantine_days)),
        state: QuarantineState::Prepared,
        evidence,
    };
    state.save_value("quarantine", &ticket.id, &ticket)?;
    std::fs::rename(source, &ticket.held).context("move source into same-filesystem quarantine")?;
    sync_directory(parent)?;
    sync_directory(&directory)?;
    match fingerprint(&ticket.held) {
        Ok(actual) if actual == ticket.evidence.source_fingerprint => {
            ticket.state = QuarantineState::Held
        }
        _ => ticket.state = QuarantineState::Changed,
    }
    state.save_value("quarantine", &ticket.id, &ticket)?;
    Ok(ticket)
}

pub fn reap_quarantine(
    state: &State,
    ticket: &mut QuarantineTicket,
    policy: &Cleanup,
    evidence: CleanupEvidence,
    now: DateTime<Utc>,
    fingerprint: impl Fn(&Path) -> Result<String>,
) -> Result<bool> {
    let _lock = state.lock(&format!("cleanup:{}", ticket.original.display()))?;
    evidence.validate(policy)?;
    ensure!(
        ticket.evidence.run_id == evidence.run_id
            && ticket.evidence.snapshot == evidence.snapshot
            && ticket.evidence.source_fingerprint == evidence.source_fingerprint,
        "cleanup recheck does not cover the quarantined source"
    );
    ensure!(
        matches!(
            ticket.state,
            QuarantineState::Held | QuarantineState::Prepared
        ),
        "quarantine is not eligible for deletion"
    );
    if now < ticket.delete_after {
        return Ok(false);
    }
    let fresh = evidence
        .verified_receipts()
        .filter(|receipt| {
            receipt
                .full_verified_at
                .is_some_and(|verified| verified >= now - Duration::minutes(15) && verified <= now)
        })
        .count();
    ensure!(
        fresh >= policy.min_verified_copies,
        "quarantine deletion needs fresh full verification of the destination copies"
    );
    let expected_parent = validate_ticket_path(ticket)?;
    let parent_metadata = std::fs::symlink_metadata(&expected_parent)?;
    ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "quarantine directory was replaced"
    );
    let directory =
        cap_std::fs::Dir::open_ambient_dir(&expected_parent, cap_std::ambient_authority())?;
    ensure!(
        metadata_unchanged(
            &cap_std::fs::Metadata::from_just_metadata(parent_metadata),
            &directory.dir_metadata()?
        )?,
        "quarantine directory changed during access"
    );
    let metadata = match directory.symlink_metadata(&ticket.id) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            directory.open(".")?.sync_all()?;
            ticket.evidence = evidence;
            ticket.state = QuarantineState::Deleted;
            state.save_value("quarantine", &ticket.id, ticket)?;
            return Ok(true);
        }
        Err(error) => return Err(error).context("inspect quarantined source"),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "quarantined source became a symbolic link"
    );
    ticket.evidence = evidence;
    state.save_value("quarantine", &ticket.id, ticket)?;
    if fingerprint(&ticket.held)? != ticket.evidence.source_fingerprint
        || !metadata_unchanged(&metadata, &directory.symlink_metadata(&ticket.id)?)?
    {
        ticket.state = QuarantineState::Changed;
        state.save_value("quarantine", &ticket.id, ticket)?;
        return Ok(false);
    }
    if metadata.is_dir() {
        directory.remove_dir_all(&ticket.id)?;
    } else {
        directory.remove_file(&ticket.id)?;
    }
    directory.open(".")?.sync_all()?;
    ticket.state = QuarantineState::Deleted;
    state.save_value("quarantine", &ticket.id, ticket)?;
    Ok(true)
}

fn validate_ticket_path(ticket: &QuarantineTicket) -> Result<PathBuf> {
    uuid::Uuid::parse_str(&ticket.id).context("invalid quarantine ticket ID")?;
    let expected_parent = ticket
        .original
        .parent()
        .context("invalid original cleanup path")?
        .join(".dcloud-quarantine");
    ensure!(
        ticket.held.parent() == Some(expected_parent.as_path())
            && ticket.held.file_name().and_then(|name| name.to_str()) == Some(ticket.id.as_str()),
        "invalid quarantine ticket path"
    );
    Ok(expected_parent)
}

fn metadata_unchanged(
    before: &cap_std::fs::Metadata,
    after: &cap_std::fs::Metadata,
) -> Result<bool> {
    let mut matches = before.file_type() == after.file_type()
        && before.len() == after.len()
        && before.modified()? == after.modified()?;
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        matches &= before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec();
    }
    Ok(matches)
}

fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?
        .sync_all()
        .context("persist directory changes")
}

#[cfg(test)]
#[path = "../tests/unit/policy_tests.rs"]
mod tests;
