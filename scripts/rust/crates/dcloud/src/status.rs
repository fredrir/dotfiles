use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use hostkit::process::{CaptureLimits, CapturedOutput};
use serde_json::{Value, json};

use crate::config::{Config, HostConfig};
use crate::state::{ReplicaState, RunState, State};

pub fn collect(config: &Config, local_only: bool, overdue_only: bool) -> Result<Value> {
    let state = State::open_readonly(&config.state_dir)?;
    collect_with(config, state.as_ref(), local_only, overdue_only, query)
}

pub(crate) fn local(config: &Config, state: Option<&State>) -> Result<Value> {
    let runs = state.map(State::runs).transpose()?.unwrap_or_default();
    let mut items = Vec::new();
    for (name, job) in &config.jobs {
        if !job.sources.contains_key(&config.host) {
            continue;
        }
        for destination in &job.destinations {
            let receipts = runs
                .iter()
                .filter(|run| run.host == config.host && &run.job == name)
                .filter_map(|run| run.replicas.get(destination))
                .filter(|receipt| {
                    receipt.state == ReplicaState::Verified
                        && receipt.destination == *destination
                        && receipt
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| !snapshot.is_empty())
                        && receipt.verified_at.is_some()
                })
                .collect::<Vec<_>>();
            let receipt = receipts
                .iter()
                .filter(|receipt| receipt.verified_at.is_some())
                .max_by_key(|receipt| receipt.verified_at);
            let last_verified = receipt.and_then(|receipt| receipt.verified_at);
            let last_full_restore = receipts
                .iter()
                .filter_map(|receipt| receipt.full_verified_at)
                .max();
            let overdue = last_verified.is_none_or(|at| {
                Utc::now().signed_duration_since(at).num_hours() >= job.overdue_hours as i64
            });
            items.push(json!({
                "host": config.host, "job": name, "destination": destination,
                "last_verified": last_verified, "last_full_restore": last_full_restore,
                "snapshot": receipt.and_then(|receipt| receipt.snapshot.clone()),
                "overdue": overdue, "status": if overdue { "overdue" } else { "verified" },
                "journal_host": config.host,
            }));
        }
    }
    let pending = runs
        .iter()
        .filter(|run| run.host == config.host && !matches!(run.state, RunState::Committed))
        .collect::<Vec<_>>();
    let sync = config
        .sync
        .iter()
        .filter(|(_, pair)| pair.owner == config.host)
        .map(|(name, _)| crate::sync::status(config, name))
        .collect::<Result<Vec<_>>>()?;
    let pending_cleanup = state
        .map(|state| crate::maintenance::pending_cleanup(config, state))
        .transpose()?
        .unwrap_or_default();
    let maintenance = state
        .map(|state| state.load_value::<Value>("maintenance-result", &config.host))
        .transpose()?
        .flatten();
    let observed = Utc::now();
    Ok(json!({
        "host": config.host, "local_only": true, "observed_at": observed,
        "items": items, "pending": pending, "sync": sync, "pending_cleanup": pending_cleanup,
        "maintenance": maintenance, "errors": [],
        "hosts": [{"host": config.host, "status": "local", "observed_at": observed, "duration_ms": 0, "maintenance": maintenance}],
    }))
}

struct PeerStatus {
    value: Value,
    exit_code: i32,
}

fn collect_with<F>(
    config: &Config,
    state: Option<&State>,
    local_only: bool,
    overdue_only: bool,
    probe: F,
) -> Result<Value>
where
    F: Fn(&str, &HostConfig) -> Result<PeerStatus> + Sync,
{
    let started = Instant::now();
    let mut result = local(config, state)?;
    result["local_only"] = json!(local_only);
    result["hosts"][0]["duration_ms"] = json!(started.elapsed().as_millis());
    if !local_only {
        let mut owners = BTreeSet::new();
        for job in config.jobs.values() {
            owners.extend(job.sources.keys().cloned());
        }
        for pair in config.sync.values() {
            owners.insert(pair.owner.clone());
        }
        owners.remove(&config.host);
        let owners: Vec<_> = owners.into_iter().collect();
        for batch in owners.chunks(8) {
            let reports = std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|host| {
                        let probe = &probe;
                        scope.spawn(move || {
                            let started = Instant::now();
                            let outcome = config
                                .hosts
                                .get(host)
                                .context("source host has no SSH configuration")
                                .and_then(|settings| probe(host, settings))
                                .and_then(|report| validate_peer(config, host, report));
                            (host, outcome, started.elapsed().as_millis())
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| anyhow::anyhow!("status worker failed"))
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
            for (host, outcome, duration) in reports {
                match outcome {
                    Ok(report) => {
                        for key in ["items", "pending", "sync", "pending_cleanup"] {
                            result[key]
                                .as_array_mut()
                                .context("invalid local status shape")?
                                .extend(
                                    report.value[key]
                                        .as_array()
                                        .context("invalid peer status shape")?
                                        .iter()
                                        .cloned(),
                                );
                        }
                        let observed = report.value["observed_at"].clone();
                        result["hosts"].as_array_mut().unwrap().push(json!({
                            "host": host, "status": "reachable", "observed_at": observed, "duration_ms": duration,
                            "exit_code": report.exit_code, "maintenance": report.value["maintenance"],
                        }));
                        if report.exit_code == 2 {
                            result["errors"].as_array_mut().unwrap().push(json!(format!(
                                "{host}: source journal reports pending work or failures"
                            )));
                        }
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        result["hosts"].as_array_mut().unwrap().push(json!({
                            "host": host, "status": "unreachable", "observed_at": Utc::now(),
                            "duration_ms": duration, "message": message, "maintenance": null,
                        }));
                        result["errors"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(format!("{host}: {message}")));
                        for (job, destination) in expected(config, host) {
                            result["items"].as_array_mut().unwrap().push(json!({
                                "host": host, "job": job, "destination": destination, "journal_host": host,
                                "last_verified": null, "last_full_restore": null, "snapshot": null,
                                "overdue": null, "status": "unknown; source unreachable",
                            }));
                        }
                    }
                }
            }
        }
    }
    if overdue_only {
        result["items"]
            .as_array_mut()
            .unwrap()
            .retain(|item| item["overdue"] == true || item["overdue"].is_null());
    }
    Ok(result)
}

fn expected(config: &Config, host: &str) -> BTreeSet<(String, String)> {
    config
        .jobs
        .iter()
        .filter(|(_, job)| job.sources.contains_key(host))
        .flat_map(|(name, job)| {
            job.destinations
                .iter()
                .map(move |destination| (name.clone(), destination.clone()))
        })
        .collect()
}

fn validate_peer(config: &Config, host: &str, mut report: PeerStatus) -> Result<PeerStatus> {
    ensure!(
        report.value["host"].as_str() == Some(host),
        "source status host identity mismatch"
    );
    ensure!(
        report.value["local_only"] == true,
        "source did not return local-only status; update its dcloud binary"
    );
    DateTime::parse_from_rfc3339(
        report.value["observed_at"]
            .as_str()
            .context("source observation time missing")?,
    )?;
    let expected = expected(config, host);
    let mut seen = BTreeSet::new();
    let items = report.value["items"]
        .as_array_mut()
        .context("source items missing")?;
    for row in items.iter() {
        ensure!(
            row["host"].as_str() == Some(host) && row["journal_host"].as_str() == Some(host),
            "source item belongs to another journal host"
        );
        let tuple = (
            row["job"]
                .as_str()
                .context("source item job missing")?
                .to_string(),
            row["destination"]
                .as_str()
                .context("source item destination missing")?
                .to_string(),
        );
        if !expected.contains(&tuple) {
            continue;
        }
        ensure!(
            seen.insert(tuple),
            "source returned duplicate replica status"
        );
        for key in ["last_verified", "last_full_restore"] {
            if !row[key].is_null() {
                DateTime::parse_from_rfc3339(
                    row[key]
                        .as_str()
                        .context("invalid source verification time")?,
                )?;
            }
        }
        ensure!(
            row["overdue"].is_boolean(),
            "source overdue status is invalid"
        );
        ensure!(row["status"].as_str().is_some(), "source status is missing");
        ensure!(
            row["status"]
                == if row["overdue"] == true {
                    "overdue"
                } else {
                    "verified"
                },
            "source verification status contradicts its overdue flag"
        );
        ensure!(
            row["last_verified"].is_null() == row["snapshot"].is_null(),
            "source snapshot and verification timestamp disagree"
        );
        ensure!(
            row["last_verified"].is_null()
                || row["snapshot"].as_str().is_some_and(|id| !id.is_empty()),
            "source verification has no snapshot"
        );
        ensure!(
            row["last_full_restore"].is_null() || !row["last_verified"].is_null(),
            "source full restore has no verified snapshot"
        );
        ensure!(
            row["status"] != "verified" || !row["last_verified"].is_null(),
            "source claimed verification without a timestamp"
        );
        ensure!(
            row["status"] != "verified"
                || row["snapshot"].as_str().is_some_and(|id| !id.is_empty()),
            "source claimed verification without a snapshot"
        );
    }
    ensure!(
        seen == expected,
        "source did not report all configured backup replicas; check its configuration"
    );
    items.retain(|row| {
        expected.contains(&(
            row["job"].as_str().unwrap_or_default().to_string(),
            row["destination"].as_str().unwrap_or_default().to_string(),
        ))
    });
    for key in ["pending", "pending_cleanup"] {
        let rows = report.value[key]
            .as_array_mut()
            .with_context(|| format!("source {key} missing"))?;
        ensure!(
            rows.iter().all(|row| row["host"].as_str() == Some(host)),
            "source {key} belongs to another host"
        );
        rows.retain(|row| {
            row["job"] == "_uploads"
                || config
                    .jobs
                    .get(row["job"].as_str().unwrap_or_default())
                    .is_some_and(|job| job.sources.contains_key(host))
        });
    }
    let sync = report.value["sync"]
        .as_array_mut()
        .context("source sync status missing")?;
    ensure!(
        sync.iter().all(|row| row["owner"].as_str() == Some(host)),
        "source sync belongs to another owner"
    );
    sync.retain(|row| {
        config
            .sync
            .get(row["pair"].as_str().unwrap_or_default())
            .is_some_and(|pair| pair.owner == host)
    });
    Ok(report)
}

fn remote_script(settings: &HostConfig) -> String {
    let mut words = vec![
        settings.binary.clone().unwrap_or_else(|| "dcloud".into()),
        "--json".into(),
    ];
    if let Some(path) = &settings.config {
        words.extend(["--config".into(), path.clone()]);
    }
    words.extend(["status".into(), "--local".into()]);
    words
        .iter()
        .map(|word| hostkit::shell::quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn query(_host: &str, settings: &HostConfig) -> Result<PeerStatus> {
    let alias = settings
        .ssh
        .as_deref()
        .context("source host has no SSH alias")?;
    let output = hostkit::ssh::Session::new(alias)
        .batch()
        .option("ConnectTimeout=4")
        .option("StrictHostKeyChecking=yes")
        .option("UpdateHostKeys=no")
        .script(&remote_script(settings))
        .output_bounded(
            CaptureLimits {
                stdout: 8 * 1024 * 1024,
                stderr: 16 * 1024,
            },
            Duration::from_secs(5),
        )
        .context("source status SSH query failed")?;
    decode(output)
}

fn decode(output: CapturedOutput) -> Result<PeerStatus> {
    ensure!(
        matches!(output.status.code(), Some(0 | 2)),
        "source status command failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    ensure!(
        !output.stdout_truncated,
        "source status response exceeds 8 MiB"
    );
    let value =
        serde_json::from_slice(&output.stdout).context("source returned invalid status JSON")?;
    Ok(PeerStatus {
        value,
        exit_code: output.status.code().unwrap_or(2),
    })
}

#[cfg(test)]
#[path = "../tests/unit/status_tests.rs"]
mod tests;
