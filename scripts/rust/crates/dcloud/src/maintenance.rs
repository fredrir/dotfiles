use crate::config::{self, Config};
use crate::state::State;
use crate::{backups, objects};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

pub fn cleanup(config: &Config, state: &mut State, apply: bool) -> Result<Value> {
    let mut result = if apply {
        let _lock = state.lock(&format!("maintenance:{}", config.host))?;
        attempt(config, state, Utc::now(), true)?.outcome?
    } else {
        cleanup_inner(config, state, false, true)?
    };
    if let Some(object) = result.as_object_mut()
        && let Some(warnings) = object.remove("warnings")
    {
        object.insert("errors".into(), warnings);
    }
    Ok(result)
}

pub fn run_due(config: &Config, state: &mut State) -> Result<Value> {
    run_due_at(config, state, Utc::now())
}

fn run_due_at(config: &Config, state: &mut State, now: DateTime<Utc>) -> Result<Value> {
    let _lock = state.lock(&format!("maintenance:{}", config.host))?;
    let previous: Option<DateTime<Utc>> = state.load_value("maintenance-attempt", &config.host)?;
    if let Some(previous) = previous {
        let record: Option<Value> = state.load_value("maintenance-result", &config.host)?;
        let succeeded = record.as_ref().is_some_and(|record| {
            record
                .get("at")
                .and_then(Value::as_str)
                .and_then(|time| time.parse::<DateTime<Utc>>().ok())
                == Some(previous)
                && record.get("result").is_some_and(Value::is_object)
                && !has_failure(record)
        });
        let interval = if succeeded {
            chrono::Duration::days(1)
        } else {
            chrono::Duration::minutes(15)
        };
        if now.signed_duration_since(previous) < interval {
            return Ok(
                json!({"attempted":false,"last_attempt":previous,"next_attempt":previous + interval}),
            );
        }
    }
    Ok(attempt(config, state, now, false)?.record)
}

struct Attempt {
    record: Value,
    outcome: Result<Value>,
}

fn attempt(
    config: &Config,
    state: &mut State,
    now: DateTime<Utc>,
    manual: bool,
) -> Result<Attempt> {
    state.save_value("maintenance-attempt", &config.host, &now)?;
    let outcome = cleanup_inner(config, state, true, manual);
    let record = match &outcome {
        Ok(result) => {
            json!({"attempted":true,"at":now,"manual":manual,"succeeded":!has_failure(result),"result":result})
        }
        Err(error) => {
            json!({"attempted":true,"at":now,"manual":manual,"succeeded":false,"warning":format!("cleanup deferred; verified backups retained: {error:#}")})
        }
    };
    state.save_value("maintenance-result", &config.host, &record)?;
    Ok(Attempt { record, outcome })
}

fn has_failure(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (matches!(
                key.as_str(),
                "warning" | "warnings" | "error" | "errors" | "pending"
            ) && nonempty(value))
                || (key == "cleanup_pending" && value.as_bool() == Some(true))
                || (key == "succeeded" && value.as_bool() == Some(false))
                || has_failure(value)
        }),
        Value::Array(items) => items.iter().any(has_failure),
        _ => false,
    }
}

fn nonempty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Number(value) => value.as_u64() != Some(0),
    }
}

pub fn pending_cleanup(config: &Config, state: &State) -> Result<Vec<Value>> {
    Ok(state
        .list_values::<backups::BackupCleanup>("backup-cleanup")?
        .into_iter()
        .filter(|(_, progress)| progress.host == config.host && !progress.pending.is_empty())
        .map(|(_, progress)| {
            json!({
                "run_id": progress.run_id, "host": progress.host, "job": progress.job,
                "backup": "verified", "cleanup_pending": true, "warnings": progress.pending,
                "quarantined_sources": progress.tickets.len(),
            })
        })
        .collect())
}

fn cleanup_inner(
    config: &Config,
    state: &mut State,
    apply: bool,
    retry_sources: bool,
) -> Result<Value> {
    let _lock = state.lock("quarantine-cleanup")?;
    let source_cleanup = if apply && retry_sources {
        let mut result = backups::retry_cleanup(config, state)?;
        result["pending"] = json!(pending_cleanup(config, state)?);
        result
    } else {
        json!({"applied":false,"pending":pending_cleanup(config, state)?})
    };
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    for (id, mut ticket) in state.list_values::<crate::policy::QuarantineTicket>("quarantine")? {
        if !matches!(
            ticket.state,
            crate::policy::QuarantineState::Prepared | crate::policy::QuarantineState::Held
        ) {
            continue;
        }
        let mut deleted = false;
        if apply && ticket.delete_after <= Utc::now() {
            let outcome = (|| -> Result<bool> {
                let policy: config::Cleanup = state
                    .load_value("cleanup-policy", &id)?
                    .context("quarantine cleanup policy missing")?;
                let run = state
                    .load_run(&ticket.evidence.run_id)?
                    .context("quarantine run missing")?;
                ensure!(
                    run.host == config.host,
                    "quarantine belongs to another source host"
                );
                if run.job != "_uploads" {
                    for from in run.replicas.keys() {
                        backups::restore_test(
                            config,
                            state,
                            &run.host,
                            &run.job,
                            from,
                            run.replicas[from].snapshot.as_deref(),
                        )?;
                    }
                } else {
                    for from in run.replicas.keys() {
                        let temp = tempfile::tempdir_in(&config.state_dir)?;
                        objects::download(
                            config,
                            &run.id,
                            &run.host,
                            from,
                            &temp.path().join("restore"),
                            &[],
                        )?;
                    }
                    let mut refreshed = run.clone();
                    for receipt in refreshed.replicas.values_mut() {
                        receipt.full_verified_at = Some(Utc::now());
                    }
                    state.save_run(&refreshed)?;
                }
                let refreshed = state.load_run(&run.id)?.context("quarantine run missing")?;
                let evidence = crate::policy::CleanupEvidence::from_run(
                    &refreshed,
                    ticket.evidence.source_fingerprint.clone(),
                )?;
                crate::policy::reap_quarantine(
                    state,
                    &mut ticket,
                    &policy,
                    evidence,
                    Utc::now(),
                    objects::fingerprint,
                )
            })();
            match outcome {
                Ok(result) => deleted = result,
                Err(error) => warnings.push(json!({"quarantine":id,"warning":format!("source deletion deferred: {error:#}")})),
            }
        }
        results.push(json!({"id":id,"source":ticket.original,"held":ticket.held,"delete_after":ticket.delete_after,"deleted":deleted}));
    }
    let expired = match objects::expire(config, state, apply) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(json!({"warning":format!("archive expiration deferred: {error:#}")}));
            Value::Null
        }
    };
    Ok(
        json!({"source_cleanup":source_cleanup,"quarantine":results,"expired_archives":expired,"warnings":warnings}),
    )
}

#[cfg(test)]
#[path = "../tests/unit/maintenance_tests.rs"]
mod tests;
