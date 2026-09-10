use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::archive;
use crate::config::{Config, DestinationKind, Job, expand, identifier};
use crate::engine::{Restic, Snapshot};
use crate::policy::{self, CleanupEvidence, QuarantineState, QuarantineTicket, RetentionEntry};
use crate::state::{ReplicaReceipt, ReplicaState, RunRecord, RunState, State};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupDetails {
    tree: String,
    time: DateTime<Utc>,
    sources: Vec<PathBuf>,
    fingerprints: BTreeMap<PathBuf, String>,
    total_bytes: u64,
    #[serde(default)]
    cleanup_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupCleanup {
    pub run_id: String,
    pub host: String,
    pub job: String,
    pub tickets: Vec<QuarantineTicket>,
    pub pending: BTreeMap<PathBuf, String>,
    #[serde(default)]
    pub directory_modes: BTreeMap<PathBuf, u32>,
}

pub fn repository(config: &Config, host: &str, job: &str, from: &str) -> Result<Restic> {
    identifier(host)?;
    identifier(job)?;
    let policy = config
        .jobs
        .get(job)
        .with_context(|| format!("unknown backup job: {job}"))?;
    let location = if from == "spool" {
        config
            .state_dir
            .join("spool")
            .join(host)
            .join(job)
            .to_string_lossy()
            .into_owned()
    } else {
        config
            .destinations
            .get(from)
            .with_context(|| format!("unknown destination: {from}"))?
            .repository(host, job)?
    };
    Ok(Restic {
        binary: config.tools.restic.clone(),
        repository: location,
        password_file: config.password_file.clone(),
        cache_dir: config
            .state_dir
            .join("cache")
            .join(from)
            .join(host)
            .join(job),
        bandwidth_kib: policy.bandwidth_kib,
        read_concurrency: policy.read_concurrency,
        timeout: Duration::from_secs(config.tools.timeout_seconds),
        rclone: config.tools.rclone.clone(),
        rclone_config: config.rclone_config_file.clone(),
    })
}

pub fn backup(config: &Config, state: &mut State, job_name: &str, dry_run: bool) -> Result<Value> {
    let _lock = state.lock(&job_lock(&config.host, job_name))?;
    backup_locked(config, state, job_name, dry_run, None)
}

fn backup_locked(
    config: &Config,
    state: &mut State,
    job_name: &str,
    dry_run: bool,
    occurrence: Option<DateTime<Utc>>,
) -> Result<Value> {
    let job = config
        .jobs
        .get(job_name)
        .with_context(|| format!("unknown backup job: {job_name}"))?;
    let paths = local_sources(config, job)?;
    let bytes = source_bytes(&paths)?;
    if dry_run {
        return Ok(
            json!({"dry_run": true, "host": config.host, "job": job_name, "sources": paths,
            "source_bytes": bytes, "destinations": job.destinations, "required": job.required,
            "min_copies": job.min_copies, "require_offsite": job.require_offsite, "cleanup": job.cleanup}),
        );
    }
    prerequisites(config, job, &paths, bytes)?;
    let mut run = RunRecord::new(&config.host, job_name, &config.backup_digest(job_name)?);
    for name in &job.destinations {
        run.replicas.insert(
            name.clone(),
            ReplicaReceipt {
                destination: name.clone(),
                snapshot: None,
                offsite: config.destinations[name].offsite,
                state: ReplicaState::Pending,
                verified_at: None,
                full_verified_at: None,
                error: None,
            },
        );
    }
    state.save_run(&run)?;
    if let Some(occurrence) = occurrence {
        state.save_value(
            "scheduled_run",
            &job_lock(&config.host, job_name),
            &ScheduledRun {
                occurrence,
                run_id: run.id.clone(),
            },
        )?;
    }
    let capture = capture(config, state, job_name, job, &paths, &mut run);
    if let Err(error) = capture {
        run.state = RunState::Failed;
        run.error = Some(format!("{error:#}"));
        state.save_run(&run)?;
        alert(config, job, &run);
        return Err(error.context(format!(
            "backup run {} failed; source material was preserved",
            run.id
        )));
    }
    let result = replicate(config, state, job_name, job, &mut run);
    if let Err(error) = result {
        if !matches!(run.state, RunState::Committed | RunState::Degraded) {
            run.state = RunState::Failed;
        }
        run.error = Some(format!("{error:#}"));
        state.save_run(&run)?;
        alert(config, job, &run);
        return Err(error.context(format!("run {} remains available for retry", run.id)));
    }
    let mut result = serde_json::to_value(&run)?;
    attach_cleanup(config, state, job, &run, &mut result)?;
    result["maintenance"] = automatic_retention(config, state, job_name, job);
    Ok(result)
}

fn capture(
    config: &Config,
    state: &mut State,
    job_name: &str,
    job: &Job,
    paths: &[PathBuf],
    run: &mut RunRecord,
) -> Result<()> {
    run.state = RunState::BackingUp;
    state.save_run(run)?;
    let capture = (|| -> Result<()> {
        hooks(config, &job.before, "before-backup")?;
        let _spool_lock = state.lock("backup-spool")?;
        let bytes = source_bytes(paths)?;
        prerequisites(config, job, paths, bytes)?;
        let fingerprints = if job.cleanup.is_some() {
            paths
                .iter()
                .map(|path| Ok((path.clone(), fingerprint(path, &[])?)))
                .collect::<Result<BTreeMap<_, _>>>()?
        } else {
            BTreeMap::new()
        };
        let mut details = BackupDetails {
            tree: String::new(),
            time: run.started,
            sources: paths.to_vec(),
            fingerprints,
            total_bytes: bytes,
            cleanup_error: None,
        };
        state.save_value("backup_details", &run.id, &details)?;
        let spool = repository(config, &config.host, job_name, "spool")?;
        spool.init()?;
        let category = if job.category.is_empty() {
            "uncategorized"
        } else {
            &job.category
        };
        let receipt = spool
            .backup_run(
                paths,
                &config.host,
                job_name,
                category,
                &job.labels,
                &job.exclude,
                &run.id,
            )?
            .context("backup did not produce a snapshot")?;
        let snapshot = spool.snapshot(&receipt.snapshot_id)?;
        ensure!(
            snapshot.hostname == config.host && snapshot.job() == Some(job_name),
            "created snapshot belongs to another host or job"
        );
        ensure!(
            capture_complete(&snapshot),
            "capture was not marked complete after successful backup"
        );
        if job.cleanup.is_some() {
            for (path, before) in &details.fingerprints {
                match fingerprint(path, &[]) {
                    Ok(after) if after == *before => {}
                    Ok(_) => {
                        details.cleanup_error = Some(format!(
                            "source changed during capture; deletion is blocked: {}",
                            path.display()
                        ));
                        break;
                    }
                    Err(error) => {
                        details.cleanup_error = Some(format!(
                            "source recheck failed; deletion is blocked: {error:#}"
                        ));
                        break;
                    }
                }
            }
        }
        details.tree = snapshot.tree.clone();
        details.time = snapshot.time;
        state.save_value("backup_details", &run.id, &details)?;
        run.snapshot = Some(snapshot.id.clone());
        run.state = RunState::Replicating;
        state.cache_manifest("spool", &snapshot.id, &snapshot)?;
        state.save_run(run)?;
        Ok(())
    })();
    let release = hooks(config, &job.after, "after-backup");
    match (capture, release) {
        (Err(capture), Err(release)) => {
            bail!("capture failed: {capture:#}; release hook also failed: {release:#}")
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn replicate(
    config: &Config,
    state: &mut State,
    job_name: &str,
    job: &Job,
    run: &mut RunRecord,
) -> Result<()> {
    ensure!(
        run.config_hash == config.backup_digest(job_name)?,
        "configuration changed since capture; pending replication requires the original configuration"
    );
    let source_id = run
        .snapshot
        .clone()
        .context("run has no immutable snapshot to replicate")?;
    let spool = repository(config, &run.host, job_name, "spool")?;
    let source = spool.snapshot(&source_id)?;
    ensure!(
        capture_complete(&source),
        "incomplete captures cannot be replica donors"
    );
    let mut details: BackupDetails = state
        .load_value("backup_details", &run.id)?
        .context("run capture details are missing")?;
    ensure!(
        source.tree == details.tree && source.time == details.time,
        "staged snapshot identity changed"
    );
    for name in &job.destinations {
        let destination = &config.destinations[name];
        let mut receipt = run.replicas.get(name).cloned().unwrap_or(ReplicaReceipt {
            destination: name.clone(),
            snapshot: None,
            offsite: destination.offsite,
            state: ReplicaState::Pending,
            verified_at: None,
            full_verified_at: None,
            error: None,
        });
        let target = repository(config, &run.host, job_name, name)?;
        let mut success = false;
        for attempt in 0..=job.retries {
            let result = (|| -> Result<Snapshot> {
                if destination.kind == DestinationKind::Local {
                    validate_local_repository(Path::new(&target.repository))?;
                }
                target.init()?;
                if let Some(quota) = destination.quota_bytes {
                    let used = target
                        .stats(None)?
                        .get("total_size")
                        .and_then(Value::as_u64)
                        .context("repository size is unavailable for quota check")?;
                    ensure!(used < quota, "destination quota reached for {name}");
                }
                let id = target.copy_from(&spool, &source_id)?;
                receipt.snapshot = Some(id.clone());
                receipt.state = ReplicaState::Uploaded;
                receipt.error = None;
                run.replicas.insert(name.clone(), receipt.clone());
                state.save_run(run)?;
                let snapshot = target.snapshot(&id)?;
                ensure!(
                    snapshot.tree == source.tree
                        && snapshot.time == source.time
                        && snapshot.paths == source.paths,
                    "destination snapshot differs from staged source"
                );
                target.check(false, None)?;
                receipt.verified_at = Some(Utc::now());
                if job.cleanup.is_some() {
                    let expected = details.cleanup_error.is_none().then_some(&details);
                    if let Some(error) = full_restore(config, &target, &snapshot, expected)? {
                        details.cleanup_error = Some(error);
                        state.save_value("backup_details", &run.id, &details)?;
                    }
                    receipt.full_verified_at = Some(Utc::now());
                }
                Ok(snapshot)
            })();
            match result {
                Ok(snapshot) => {
                    receipt.state = ReplicaState::Verified;
                    receipt.error = None;
                    state.cache_manifest(name, &snapshot.id, &snapshot)?;
                    run.replicas.insert(name.clone(), receipt.clone());
                    state.save_run(run)?;
                    success = true;
                    break;
                }
                Err(error) => {
                    receipt.state = ReplicaState::Failed;
                    receipt.error = Some(format!("{error:#}"));
                    receipt.verified_at = None;
                    receipt.full_verified_at = None;
                    run.replicas.insert(name.clone(), receipt.clone());
                    state.save_run(run)?;
                    if attempt < job.retries {
                        std::thread::sleep(Duration::from_secs(
                            job.retry_delay_seconds
                                .saturating_mul(u64::from(attempt + 1))
                                .min(300),
                        ));
                    }
                }
            }
        }
        if !success && job.required.contains(name) {
            run.error = Some(format!("required destination {name} is unavailable"));
        }
    }
    let assessment = policy::evaluate(job, &run.replicas)?;
    if !assessment.satisfied {
        run.state = if matches!(run.state, RunState::Committed | RunState::Degraded) {
            RunState::Degraded
        } else {
            RunState::Failed
        };
        run.error = Some(format!(
            "replica policy incomplete: {} verified copies, missing required {:?}, offsite verified {}",
            assessment.verified_copies, assessment.missing_required, assessment.offsite_verified
        ));
        state.save_run(run)?;
        bail!(
            "{}",
            run.error.as_deref().unwrap_or("replica policy incomplete")
        );
    }
    run.state = if assessment.degraded {
        RunState::Degraded
    } else {
        RunState::Committed
    };
    run.error = assessment.degraded.then(|| {
        format!(
            "optional destinations pending: {}",
            assessment.pending.join(", ")
        )
    });
    if job.cleanup.is_some() {
        run.last_restore = Some(Utc::now());
    }
    state.save_run(run)?;
    if assessment.degraded {
        alert(config, job, run);
    }
    Ok(())
}

pub fn retry(config: &Config, state: &mut State, run_id: Option<&str>) -> Result<Value> {
    let runs = state.runs()?;
    if let Some(id) = run_id {
        ensure!(runs.iter().any(|run| run.id == id), "unknown run: {id}");
    }
    let mut output = Vec::new();
    let mut failures = Vec::new();
    for mut run in runs.into_iter().filter(|run| {
        run.host == config.host
            && config.jobs.contains_key(&run.job)
            && run_id.is_none_or(|id| run.id == id)
            && (run_id.is_some() || !matches!(run.state, RunState::Committed))
    }) {
        let _lock = state.lock(&job_lock(&run.host, &run.job))?;
        let job_name = run.job.clone();
        let job = config
            .jobs
            .get(&job_name)
            .with_context(|| format!("pending run {} refers to missing job {job_name}", run.id))?;
        let result = resume(config, state, &job_name, job, &mut run);
        match result {
            Ok(()) => {
                let mut value = serde_json::to_value(&run)?;
                attach_cleanup(config, state, job, &run, &mut value)?;
                value["maintenance"] = automatic_retention(config, state, &job_name, job);
                output.push(value);
            }
            Err(error) => failures.push(json!({"run": run.id, "error": format!("{error:#}")})),
        }
    }
    ensure!(
        failures.is_empty(),
        "some pending replicas remain incomplete: {}",
        serde_json::to_string(&failures)?
    );
    Ok(json!({"runs": output}))
}

fn resume(
    config: &Config,
    state: &mut State,
    job_name: &str,
    job: &Job,
    run: &mut RunRecord,
) -> Result<()> {
    ensure!(
        run.config_hash == config.backup_digest(job_name)?,
        "configuration changed since this run; restore its original configuration before retry"
    );
    if run.snapshot.is_none() && !recover_capture(config, state, run)? {
        let paths = local_sources(config, job)?;
        let bytes = source_bytes(&paths)?;
        prerequisites(config, job, &paths, bytes)?;
        capture(config, state, job_name, job, &paths, run)?;
    }
    replicate(config, state, job_name, job, run)
}

fn recover_capture(config: &Config, state: &mut State, run: &mut RunRecord) -> Result<bool> {
    let spool = repository(config, &run.host, &run.job, "spool")?;
    if !Path::new(&spool.repository).join("config").is_file() {
        return Ok(false);
    }
    let tag = format!("dcloud.run:{}", run.id);
    let snapshots: Vec<_> = spool
        .snapshots(Some(&run.host), Some(&run.job))?
        .into_iter()
        .filter(|snapshot| snapshot.tags.contains(&tag) && capture_complete(snapshot))
        .collect();
    if snapshots.is_empty() {
        return Ok(false);
    }
    ensure!(
        snapshots.len() == 1,
        "multiple snapshots share the interrupted run ID; refusing ambiguous recovery"
    );
    let snapshot = &snapshots[0];
    let mut details: BackupDetails = state
        .load_value("backup_details", &run.id)?
        .context("interrupted snapshot has no capture journal")?;
    let expected: BTreeSet<_> = details
        .sources
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    ensure!(
        snapshot.paths.iter().cloned().collect::<BTreeSet<_>>() == expected,
        "interrupted snapshot has unexpected source paths"
    );
    details.tree = snapshot.tree.clone();
    details.time = snapshot.time;
    state.save_value("backup_details", &run.id, &details)?;
    run.snapshot = Some(snapshot.id.clone());
    run.state = RunState::Replicating;
    state.save_run(run)?;
    hooks(
        config,
        &config.jobs[&run.job].after,
        "release interrupted backup",
    )?;
    Ok(true)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScheduledRun {
    occurrence: DateTime<Utc>,
    run_id: String,
}

pub fn run_due(config: &Config, state: &mut State) -> Result<Value> {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    for (name, job) in &config.jobs {
        if !job.sources.contains_key(&config.host) || !job.schedule.enabled {
            continue;
        }
        let result = (|| -> Result<Option<Value>> {
            let _lock = state.lock(&job_lock(&config.host, name))?;
            if let Some(pending) =
                state.load_value::<ScheduledRun>("scheduled_run", &job_lock(&config.host, name))?
                && state
                    .occurrence(&config.host, name)?
                    .is_none_or(|completed| pending.occurrence > completed)
                && let Some(mut run) = state.load_run(&pending.run_id)?
            {
                if matches!(run.state, RunState::Committed | RunState::Degraded) {
                    state.set_occurrence(&config.host, name, pending.occurrence, &run.id)?;
                } else {
                    resume(config, state, name, job, &mut run)?;
                    let mut value = serde_json::to_value(&run)?;
                    attach_cleanup(config, state, job, &run, &mut value)?;
                    state.set_occurrence(&config.host, name, pending.occurrence, &run.id)?;
                    return Ok(Some(value));
                }
            }
            let Some(occurrence) = crate::schedule::due(
                &job.schedule,
                state.occurrence(&config.host, name)?,
                Utc::now(),
            )?
            else {
                return Ok(None);
            };
            let value = backup_locked(config, state, name, false, Some(occurrence))?;
            let id = value["id"]
                .as_str()
                .context("completed backup did not return its run ID")?;
            state.set_occurrence(&config.host, name, occurrence, id)?;
            Ok(Some(value))
        })();
        match result {
            Ok(Some(value)) => completed.push(value),
            Ok(None) => {}
            Err(error) => failed.push(json!({"job": name, "error": format!("{error:#}")})),
        }
    }
    let cleanup = retry_cleanup(config, state)?;
    let maintenance = crate::maintenance::run_due(config, state)?;
    let synchronized = crate::sync::run_due(config, state)?;
    if let Some(errors) = synchronized.get("errors").and_then(Value::as_array) {
        failed.extend(errors.iter().cloned());
    }
    notify_overdue(config, state)?;
    ensure!(
        failed.is_empty(),
        "scheduled jobs remain due: {}",
        serde_json::to_string(&failed)?
    );
    Ok(
        json!({"host": config.host, "runs": completed, "sync": synchronized["pairs"], "cleanup": cleanup, "cleanup_pending": cleanup["cleanup_pending"], "maintenance": maintenance}),
    )
}

fn notify_overdue(config: &Config, state: &State) -> Result<()> {
    let runs = state.runs()?;
    let now = Utc::now();
    for (name, job) in &config.jobs {
        if !job.sources.contains_key(&config.host) || !job.schedule.enabled || job.alert.is_empty()
        {
            continue;
        }
        let latest = runs.iter().find(|run| {
            run.host == config.host
                && run.job == *name
                && matches!(run.state, RunState::Committed | RunState::Degraded)
        });
        let overdue = latest.is_none_or(|run| {
            now.signed_duration_since(run.started).num_hours()
                >= i64::try_from(job.overdue_hours).unwrap_or(i64::MAX)
        });
        let stale = runs
            .iter()
            .filter(|run| {
                run.host == config.host && run.job == *name && run.state != RunState::Committed
            })
            .any(|run| {
                now.signed_duration_since(run.started).num_days()
                    >= i64::from(job.pending_max_age_days)
            });
        if !overdue && !stale {
            continue;
        }
        let key = job_lock(&config.host, name);
        if state
            .load_value::<DateTime<Utc>>("overdue_alert", &key)?
            .is_some_and(|last| now - last < chrono::Duration::hours(24))
        {
            continue;
        }
        let mut notification = latest
            .cloned()
            .unwrap_or_else(|| RunRecord::new(&config.host, name, "notification"));
        notification.error = Some(if stale {
            "pending replicas exceeded their age threshold; their source snapshot is protected"
                .into()
        } else {
            "backup is overdue".into()
        });
        alert(config, job, &notification);
        state.save_value("overdue_alert", &key, &now)?;
    }
    Ok(())
}

fn automatic_retention(config: &Config, state: &mut State, job_name: &str, job: &Job) -> Value {
    if !job.retention.auto {
        return json!([]);
    }
    let mut output = Vec::new();
    for name in job
        .destinations
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("spool"))
    {
        if name != "spool" {
            let destination = &config.destinations[name];
            if destination
                .maintenance_owner
                .as_deref()
                .unwrap_or(&config.host)
                != config.host
            {
                continue;
            }
            if destination.append_only && destination.maintenance_location.is_none() {
                output.push(json!({"destination": name, "deferred": "append-only maintenance requires separate credentials"}));
                continue;
            }
        }
        match retention_locked(config, state, &config.host, job_name, name, true, true) {
            Ok(value) => output.push(value),
            Err(error) => output.push(json!({"destination": name, "error": format!("{error:#}")})),
        }
    }
    json!(output)
}

pub fn retention(
    config: &Config,
    state: &mut State,
    host: &str,
    job_name: &str,
    from: &str,
    apply: bool,
    prune: bool,
) -> Result<Value> {
    let _lock = state.lock(&job_lock(host, job_name))?;
    retention_locked(config, state, host, job_name, from, apply, prune)
}

fn retention_locked(
    config: &Config,
    state: &mut State,
    host: &str,
    job_name: &str,
    from: &str,
    apply: bool,
    prune: bool,
) -> Result<Value> {
    let job = config.jobs.get(job_name).context("unknown backup job")?;
    ensure!(
        !apply || job.cleanup.is_none() || config.host == host,
        "retention with source cleanup must run on source owner {host}; dispatch maintenance there to preserve quarantine donors"
    );
    ensure!(
        !prune || apply,
        "prune requires applying the retention plan"
    );
    let mut target = repository(config, host, job_name, from)?;
    if from == "spool" {
        ensure!(
            !apply || host == config.host,
            "only the source owner can prune its spool"
        );
    } else {
        let destination = config
            .destinations
            .get(from)
            .context("unknown destination")?;
        let owner = destination.maintenance_owner.as_deref().unwrap_or(host);
        ensure!(
            !apply || owner == config.host,
            "maintenance for {from} is owned by {owner}"
        );
        if apply && destination.append_only {
            let location = destination
                .maintenance_location
                .as_ref()
                .context("append-only retention needs a separate maintenance location")?;
            let mut maintenance = destination.clone();
            maintenance.location = location.clone();
            maintenance.append_only = false;
            target.repository = maintenance.repository(host, job_name)?;
        }
    }
    target.check(false, None)?;
    let snapshots = target.snapshots(Some(host), Some(job_name))?;
    let mut runs: Vec<_> = state
        .runs()?
        .into_iter()
        .filter(|run| run.host == host && run.job == job_name)
        .collect();
    if job.cleanup.is_some() {
        for run in &mut runs {
            if state
                .load_value::<BackupCleanup>("backup-cleanup", &run.id)?
                .is_none_or(|progress| !progress.pending.is_empty())
                && run.state == RunState::Committed
            {
                run.state = RunState::Degraded;
            }
        }
    }
    let tickets: Vec<QuarantineTicket> = state
        .values::<QuarantineTicket>("quarantine")?
        .into_iter()
        .map(|(_, ticket)| ticket)
        .filter(|ticket| {
            !matches!(
                ticket.state,
                QuarantineState::Deleted | QuarantineState::Restored
            )
        })
        .collect();
    let entries = retention_entries(&snapshots, &runs, &tickets, from);
    let mut plan = policy::retention_plan(&entries, &job.retention, Utc::now())?;
    let mut retirement = BTreeMap::new();
    if from != "spool" && !plan.delete.is_empty() {
        let mut observed: BTreeMap<String, Vec<Snapshot>> = BTreeMap::new();
        let mut other_plans = BTreeMap::new();
        for name in &job.destinations {
            let result = (|| -> Result<Vec<Snapshot>> {
                if name == from {
                    return Ok(snapshots.clone());
                }
                let repository = repository(config, host, job_name, name)?;
                repository.check(false, None)?;
                repository.snapshots(Some(host), Some(job_name))
            })();
            match result {
                Ok(snapshots) => {
                    let entries = retention_entries(&snapshots, &runs, &tickets, name);
                    other_plans.insert(
                        name.clone(),
                        policy::retention_plan(&entries, &job.retention, Utc::now())?,
                    );
                    observed.insert(name.clone(), snapshots);
                }
                Err(error) => ensure!(
                    !job.required.contains(name),
                    "required destination {name} is unavailable; retention stopped: {error:#}"
                ),
            }
        }
        let mut permitted = Vec::new();
        for id in &plan.delete {
            let snapshot = snapshots
                .iter()
                .find(|snapshot| &snapshot.id == id)
                .context("retention snapshot disappeared")?;
            let key = retirement_key(host, job_name, snapshot);
            let already_retired = state
                .load_value::<Retirement>("retired_snapshots", &key)?
                .is_some();
            let mut copies = BTreeMap::new();
            let mut cohort = BTreeMap::new();
            let mut all_expire = true;
            for name in &job.destinations {
                let copy = observed.get(name).and_then(|snapshots| {
                    snapshots.iter().find(|copy| same_snapshot(snapshot, copy))
                });
                if let Some(copy) = copy {
                    cohort.insert(name.clone(), copy.id.clone());
                    all_expire &= other_plans[name].delete.contains(&copy.id);
                    if name != from {
                        copies.insert(
                            name.clone(),
                            ReplicaReceipt {
                                destination: name.clone(),
                                snapshot: Some(copy.id.clone()),
                                offsite: config.destinations[name].offsite,
                                state: ReplicaState::Verified,
                                verified_at: Some(Utc::now()),
                                full_verified_at: None,
                                error: None,
                            },
                        );
                    }
                } else {
                    all_expire = false;
                }
            }
            let preserves_quorum = policy::evaluate(job, &copies)?.satisfied;
            if already_retired || all_expire || preserves_quorum {
                permitted.push(id.clone());
                if all_expire && !already_retired {
                    retirement.insert(
                        key,
                        Retirement {
                            host: host.into(),
                            job: job_name.into(),
                            tree: snapshot.tree.clone(),
                            time: snapshot.time,
                            destinations: cohort,
                            retired_at: Utc::now(),
                        },
                    );
                }
            } else {
                plan.retain.push(id.clone());
            }
        }
        plan.delete = permitted;
        plan.retain.sort();
    }
    if apply && !plan.delete.is_empty() {
        for (key, retirement) in &retirement {
            state.save_value("retired_snapshots", key, retirement)?;
        }
        target.forget(&plan.delete, prune)?;
    }
    Ok(
        json!({"host": host, "job": job_name, "destination": from, "applied": apply, "pruned": apply && prune && !plan.delete.is_empty(), "plan": plan}),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Retirement {
    host: String,
    job: String,
    tree: String,
    time: DateTime<Utc>,
    destinations: BTreeMap<String, String>,
    retired_at: DateTime<Utc>,
}

fn retention_key_matches(run: &RunRecord, snapshot: &Snapshot, destination: &str) -> bool {
    if destination == "spool" {
        return run
            .snapshot
            .as_deref()
            .is_some_and(|id| snapshot.matches_id(id));
    }
    run.replicas
        .get(destination)
        .and_then(|receipt| receipt.snapshot.as_deref())
        .is_some_and(|id| snapshot.matches_id(id))
        || run
            .snapshot
            .as_deref()
            .is_some_and(|id| snapshot.matches_id(id))
}

fn retention_entries(
    snapshots: &[Snapshot],
    runs: &[RunRecord],
    tickets: &[QuarantineTicket],
    destination: &str,
) -> Vec<RetentionEntry> {
    snapshots
        .iter()
        .map(|snapshot| {
            let run = runs
                .iter()
                .find(|run| retention_key_matches(run, snapshot, destination));
            let pending = run.is_some_and(|run| run.state != RunState::Committed);
            let quarantine = tickets.iter().any(|ticket| {
                if destination == "spool" {
                    snapshot.matches_id(&ticket.evidence.snapshot)
                } else {
                    ticket
                        .evidence
                        .replicas
                        .get(destination)
                        .and_then(|receipt| receipt.snapshot.as_deref())
                        .is_some_and(|id| snapshot.matches_id(id))
                        || snapshot.matches_id(&ticket.evidence.snapshot)
                }
            });
            let complete = capture_complete(snapshot);
            RetentionEntry {
                id: snapshot.id.clone(),
                host: snapshot.hostname.clone(),
                job: snapshot.job().unwrap_or_default().into(),
                time: snapshot.time,
                pinned: snapshot.pinned(),
                complete,
                verified: complete,
                protected: pending || quarantine || (destination == "spool" && run.is_none()),
            }
        })
        .collect()
}

fn retirement_key(host: &str, job: &str, snapshot: &Snapshot) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{host}\0{job}\0{}\0{}",
                snapshot.tree,
                snapshot.time.to_rfc3339()
            )
            .as_bytes()
        )
    )
}

fn same_snapshot(left: &Snapshot, right: &Snapshot) -> bool {
    left.tree == right.tree
        && left.time == right.time
        && left.paths == right.paths
        && left.hostname == right.hostname
        && left.job() == right.job()
}

pub fn restore_test(
    config: &Config,
    state: &mut State,
    host: &str,
    job: &str,
    from: &str,
    snapshot: Option<&str>,
) -> Result<Value> {
    let _lock = state.lock(&job_lock(host, job))?;
    let target = repository(config, host, job, from)?;
    let snapshot = match snapshot {
        Some(id) => target.snapshot(id)?,
        None => target
            .snapshots(Some(host), Some(job))?
            .into_iter()
            .rev()
            .find(capture_complete)
            .context("repository contains no complete matching snapshot")?,
    };
    ensure!(
        snapshot.hostname == host && snapshot.job() == Some(job),
        "snapshot does not belong to requested host and job"
    );
    ensure!(
        capture_complete(&snapshot),
        "incomplete capture cannot count as a verified recovery point"
    );
    full_restore(config, &target, &snapshot, None)?;
    let verified = Utc::now();
    for mut run in state
        .runs()?
        .into_iter()
        .filter(|run| run.host == host && run.job == job)
    {
        if let Some(receipt) = run.replicas.get_mut(from) {
            if !receipt
                .snapshot
                .as_deref()
                .is_some_and(|id| snapshot.matches_id(id))
            {
                continue;
            }
            receipt.state = ReplicaState::Verified;
            receipt.verified_at = Some(verified);
            receipt.full_verified_at = Some(verified);
            run.last_restore = Some(verified);
            state.save_run(&run)?;
        }
    }
    state.cache_manifest(from, &snapshot.id, &snapshot)?;
    Ok(
        json!({"snapshot": snapshot.id, "host": host, "job": job, "destination": from, "verified_at": verified, "full_restore": true}),
    )
}

fn full_restore(
    config: &Config,
    repository: &Restic,
    snapshot: &Snapshot,
    expected: Option<&BackupDetails>,
) -> Result<Option<String>> {
    let temporary = tempfile::Builder::new()
        .prefix("restore-drill-")
        .tempdir_in(&config.state_dir)?;
    let destination = temporary.path().join("restored");
    repository.restore(&snapshot.id, &destination, &[])?;
    if let Some(expected) = expected {
        for (source, source_fingerprint) in &expected.fingerprints {
            let relative = source
                .strip_prefix("/")
                .context("recorded source path is not absolute")?;
            let restored = destination.join(relative);
            match fingerprint(&restored, &[]) {
                Ok(actual) if actual == *source_fingerprint => {}
                Ok(_) => {
                    return Ok(Some(format!(
                        "restored source differs from its pre-capture fingerprint; deletion is blocked: {}",
                        source.display()
                    )));
                }
                Err(error) => {
                    return Ok(Some(format!(
                        "restored source fingerprint is unavailable; deletion is blocked: {error:#}"
                    )));
                }
            }
        }
    }
    Ok(None)
}

fn attach_cleanup(
    config: &Config,
    state: &mut State,
    job: &Job,
    run: &RunRecord,
    result: &mut Value,
) -> Result<()> {
    if job.cleanup.is_none() {
        return Ok(());
    }
    let progress = match cleanup_run(config, state, job, run) {
        Ok(progress) => progress,
        Err(error) => {
            let mut progress = state
                .load_value::<BackupCleanup>("backup-cleanup", &run.id)?
                .unwrap_or_else(|| new_cleanup(run));
            progress
                .pending
                .insert(PathBuf::from("."), format!("{error:#}"));
            state.save_value("backup-cleanup", &run.id, &progress)?;
            progress
        }
    };
    result["cleanup"] = serde_json::to_value(&progress.tickets)?;
    result["cleanup_pending"] = json!(!progress.pending.is_empty());
    result["cleanup_warnings"] = serde_json::to_value(&progress.pending)?;
    Ok(())
}

fn new_cleanup(run: &RunRecord) -> BackupCleanup {
    BackupCleanup {
        run_id: run.id.clone(),
        host: run.host.clone(),
        job: run.job.clone(),
        tickets: Vec::new(),
        pending: BTreeMap::new(),
        directory_modes: BTreeMap::new(),
    }
}

pub fn retry_cleanup(config: &Config, state: &mut State) -> Result<Value> {
    let mut output = Vec::new();
    let mut pending = false;
    for run in state.runs()? {
        if run.host != config.host
            || !matches!(run.state, RunState::Committed | RunState::Degraded)
            || !config
                .jobs
                .get(&run.job)
                .is_some_and(|job| job.cleanup.is_some())
        {
            continue;
        }
        if state
            .load_value::<BackupCleanup>("backup-cleanup", &run.id)?
            .is_some_and(|progress| progress.pending.is_empty())
        {
            continue;
        }
        let lock = state.lock(&job_lock(&run.host, &run.job));
        let _lock = match lock {
            Ok(lock) => lock,
            Err(error) => {
                pending = true;
                output.push(json!({"run_id": run.id, "warning": format!("{error:#}")}));
                continue;
            }
        };
        let mut value = json!({"run_id": run.id});
        attach_cleanup(config, state, &config.jobs[&run.job], &run, &mut value)?;
        pending |= value["cleanup_pending"].as_bool().unwrap_or(true);
        output.push(value);
    }
    Ok(json!({"runs": output, "cleanup_pending": pending}))
}

fn cleanup_run(
    config: &Config,
    state: &mut State,
    job: &Job,
    run: &RunRecord,
) -> Result<BackupCleanup> {
    let cleanup = job.cleanup.as_ref().context("no cleanup policy")?;
    ensure!(
        run.config_hash == config.backup_digest(&run.job)?,
        "cleanup configuration changed since capture"
    );
    ensure!(
        job.exclude.is_empty(),
        "automatic source deletion requires a complete unfiltered backup"
    );
    let details: BackupDetails = state
        .load_value("backup_details", &run.id)?
        .context("cleanup capture evidence is unavailable")?;
    if let Some(error) = &details.cleanup_error {
        bail!("{error}");
    }
    let mut progress = state
        .load_value::<BackupCleanup>("backup-cleanup", &run.id)?
        .unwrap_or_else(|| new_cleanup(run));
    progress.pending.remove(Path::new("."));
    for path in &details.sources {
        if !progress
            .tickets
            .iter()
            .any(|ticket| ticket.original == *path)
            && !progress.pending.contains_key(path)
        {
            progress
                .pending
                .insert(path.clone(), "source cleanup has not completed".into());
        }
    }
    state.save_value("backup-cleanup", &run.id, &progress)?;
    let mut fresh = run.clone();
    let mut updated = run.clone();
    for (name, receipt) in &mut fresh.replicas {
        if receipt.state != ReplicaState::Verified
            || receipt
                .full_verified_at
                .is_some_and(|time| Utc::now() - time < chrono::Duration::minutes(15))
        {
            continue;
        }
        let recheck = (|| -> Result<()> {
            let target = repository(config, &run.host, &run.job, name)?;
            let snapshot = target.snapshot(
                receipt
                    .snapshot
                    .as_deref()
                    .context("replica snapshot missing")?,
            )?;
            ensure!(
                capture_complete(&snapshot)
                    && snapshot.tree == details.tree
                    && snapshot.time == details.time,
                "replica no longer matches the captured source"
            );
            if let Some(error) = full_restore(config, &target, &snapshot, Some(&details))? {
                bail!("{error}");
            }
            Ok(())
        })();
        match recheck {
            Ok(()) => {
                receipt.full_verified_at = Some(Utc::now());
                receipt.verified_at = receipt.full_verified_at;
                updated.replicas.insert(name.clone(), receipt.clone());
                updated.last_restore = receipt.full_verified_at;
            }
            Err(error) => {
                receipt.state = ReplicaState::Failed;
                receipt.full_verified_at = None;
                receipt.error = Some(format!("cleanup readback failed: {error:#}"));
            }
        }
    }
    state.save_run(&updated)?;
    ensure!(
        policy::evaluate(job, &fresh.replicas)?.satisfied,
        "cleanup needs the configured verified replica policy"
    );
    for path in &details.sources {
        let attempt = (|| -> Result<QuarantineTicket> {
            if let Ok(metadata) = fs::symlink_metadata(path)
                && metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && !progress.directory_modes.contains_key(path)
            {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    progress
                        .directory_modes
                        .insert(path.clone(), metadata.permissions().mode());
                }
                #[cfg(not(unix))]
                progress
                    .directory_modes
                    .insert(path.clone(), u32::from(metadata.permissions().readonly()));
                state.save_value("backup-cleanup", &run.id, &progress)?;
            }
            let digest = details
                .fingerprints
                .get(path)
                .context("source fingerprint missing")?
                .clone();
            let evidence = CleanupEvidence::from_run(&fresh, digest)?;
            let ticket = policy::quarantine(state, path, cleanup, evidence, Utc::now(), |path| {
                fingerprint(path, &[])
            })?;
            state.save_value("cleanup-policy", &ticket.id, cleanup)?;
            if let Some(mode) = progress.directory_modes.get(path) {
                preserve_directory_root(path, *mode)?;
            }
            ensure!(
                ticket.state != QuarantineState::Changed,
                "quarantined source changed; retained for manual recovery"
            );
            Ok(ticket)
        })();
        match attempt {
            Ok(ticket) => {
                progress.pending.remove(path);
                progress
                    .tickets
                    .retain(|existing| existing.original != *path);
                progress.tickets.push(ticket);
            }
            Err(error) => {
                progress.pending.insert(path.clone(), format!("{error:#}"));
            }
        }
        state.save_value("backup-cleanup", &run.id, &progress)?;
    }
    Ok(progress)
}

fn preserve_directory_root(path: &Path, mode: u32) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "source root was replaced with a non-directory"
            );
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = cap_std::fs::Dir::open_ambient_dir(
        path.parent().context("source has no parent")?,
        cap_std::ambient_authority(),
    )?;
    let name = path.file_name().context("source has no filename")?;
    parent
        .create_dir(name)
        .context("recreate scheduled source directory")?;
    let directory = parent.open_dir(name)?.open(".")?.into_std();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = directory.metadata()?.permissions();
        permissions.set_readonly(mode != 0);
        directory.set_permissions(permissions)?;
    }
    directory.sync_all()?;
    parent.open(".")?.sync_all()?;
    Ok(())
}

fn local_sources(config: &Config, job: &Job) -> Result<Vec<PathBuf>> {
    let paths = job
        .sources
        .get(&config.host)
        .with_context(|| format!("job has no sources for host {}", config.host))?;
    paths
        .iter()
        .map(|path| {
            let path = expand(path)?;
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("source unavailable: {}", path.display()))?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "source root must not be a symlink: {}",
                path.display()
            );
            ensure!(
                metadata.is_dir() || metadata.is_file(),
                "unsupported source: {}",
                path.display()
            );
            path.canonicalize()
                .with_context(|| format!("cannot resolve source: {}", path.display()))
        })
        .collect()
}

fn source_bytes(paths: &[PathBuf]) -> Result<u64> {
    let mut bytes = 0u64;
    for path in paths {
        for entry in walkdir::WalkDir::new(path).follow_links(false) {
            let entry = entry.context("scan source size")?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .context("source size overflow")?;
            }
        }
    }
    Ok(bytes)
}

fn fingerprint(path: &Path, excludes: &[String]) -> Result<String> {
    if excludes.is_empty() {
        return archive::tree_digest(path);
    }
    let mut entries = archive::hash_tree(path, excludes)?;
    if !path.is_dir() {
        for entry in &mut entries {
            entry.path = ".".into();
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&entries)?)
    ))
}

fn prerequisites(config: &Config, job: &Job, _paths: &[PathBuf], bytes: u64) -> Result<()> {
    ensure!(
        job.cleanup.is_none() || job.exclude.is_empty(),
        "automatic source deletion requires a complete unfiltered backup"
    );
    fs::create_dir_all(&config.state_dir)?;
    let available = fs2::available_space(&config.state_dir)?;
    let upper_bound = bytes
        .saturating_add(bytes / 10)
        .saturating_add(64 * 1024 * 1024);
    ensure!(
        available >= upper_bound.saturating_add(job.min_free_bytes),
        "not enough free staging space for a safe backup"
    );
    let spool = config.state_dir.join("spool");
    let existing = if spool.exists() {
        source_bytes(&[spool])?
    } else {
        0
    };
    ensure!(
        existing.saturating_add(upper_bound) <= job.spool_limit_bytes,
        "pending spool would exceed {} bytes; retry pending copies or increase the limit",
        job.spool_limit_bytes
    );
    if job.ac_only {
        ensure!(on_ac_power()?, "backup is waiting for AC power");
    }
    if let Some(probe) = &job.network_probe {
        let addresses: Vec<_> = if probe.contains(':') {
            probe.to_socket_addrs()?.collect()
        } else {
            (probe.as_str(), 443).to_socket_addrs()?.collect()
        };
        ensure!(
            addresses
                .into_iter()
                .any(
                    |address| TcpStream::connect_timeout(&address, Duration::from_secs(5)).is_ok()
                ),
            "configured network probe is unavailable"
        );
    }
    Ok(())
}

fn on_ac_power() -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let output = crate::transport::run_command(
            Command::new("pmset").args(["-g", "batt"]),
            Duration::from_secs(10),
            "power status",
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).contains("AC Power"))
    }
    #[cfg(target_os = "linux")]
    {
        let root = Path::new("/sys/class/power_supply");
        let mut battery = false;
        if !root.exists() {
            return Ok(true);
        }
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let kind = fs::read_to_string(path.join("type"))?;
            if kind.trim() == "Battery" {
                battery = true;
            }
            if kind.trim() != "Battery"
                && fs::read_to_string(path.join("online")).is_ok_and(|online| online.trim() == "1")
            {
                return Ok(true);
            }
        }
        Ok(!battery)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("AC power detection is unavailable on this operating system")
}

fn hooks(config: &Config, hooks: &[Vec<String>], label: &str) -> Result<()> {
    for arguments in hooks {
        let program = arguments.first().context("empty hook")?;
        crate::transport::run_command(
            Command::new(program).args(&arguments[1..]),
            Duration::from_secs(config.tools.timeout_seconds),
            label,
        )?;
    }
    Ok(())
}

fn alert(config: &Config, job: &Job, run: &RunRecord) {
    if let Some(program) = job.alert.first() {
        let mut command = Command::new(program);
        command
            .args(&job.alert[1..])
            .env("DCLOUD_HOST", &run.host)
            .env("DCLOUD_JOB", &run.job)
            .env("DCLOUD_RUN", &run.id)
            .env(
                "DCLOUD_ERROR",
                run.error.as_deref().unwrap_or("backup needs attention"),
            );
        let _ = crate::transport::run_command(
            &mut command,
            Duration::from_secs(config.tools.timeout_seconds.min(60)),
            "backup alert",
        );
    }
}

pub fn job_lock(host: &str, job: &str) -> String {
    format!("backup:{host}:{job}")
}

fn validate_local_repository(path: &Path) -> Result<()> {
    let mut ancestor = Some(path);
    while let Some(path) = ancestor {
        match fs::metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir(),
                    "local repository is obstructed by a non-directory: {}",
                    path.display()
                );
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspect local repository parent {}", path.display())
                });
            }
        }
        ancestor = path.parent();
    }
    bail!("local repository has no accessible parent directory")
}

fn capture_complete(snapshot: &Snapshot) -> bool {
    snapshot
        .tags
        .iter()
        .any(|tag| tag == "dcloud.capture:complete")
        && !snapshot
            .tags
            .iter()
            .any(|tag| tag == "dcloud.capture:pending")
}

#[cfg(test)]
#[path = "../tests/unit/backups_tests.rs"]
mod tests;
