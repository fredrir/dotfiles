use super::{
    compare::{Delta, compare_runs, regressions},
    record::{CLEAN, Run},
    store::Store,
};
use chrono::{DateTime, Utc};
use sysinfo::model::{HealthIssue, Severity};
pub fn regression_issue(delta: &Delta, baseline: &Run, latest: &Run) -> HealthIssue {
    let direction = if delta.change_pct > 0.0 {
        "above"
    } else {
        "below"
    };
    HealthIssue {
        severity: Severity::Warning,
        title: format!(
            "{} is {:.0}% {direction} its baseline",
            delta.key,
            delta.change_pct.abs()
        ),
        detail: format!(
            "{} at the baseline of {}, {} on {}",
            super::report::format_value(Some(delta.left), &delta.scale),
            baseline.started.get(..10).unwrap_or(&baseline.started),
            super::report::format_value(Some(delta.right), &delta.scale),
            latest.started.get(..10).unwrap_or(&latest.started)
        ),
        action: format!(
            "Re-run hwtune bench run --only {} to confirm, then look for thermal or configuration causes",
            delta.key.split('.').next().unwrap_or("")
        ),
    }
}
pub fn issues(store: &Store, host: &str) -> Result<Vec<HealthIssue>, String> {
    if host.is_empty() {
        return Ok(Vec::new());
    }
    issues_for_runs(store, host, &store.list_runs(Some(host), CLEAN)?)
}
pub fn issues_for_runs(
    store: &Store,
    host: &str,
    runs: &[Run],
) -> Result<Vec<HealthIssue>, String> {
    if host.is_empty() {
        return Ok(Vec::new());
    }
    let Some(latest) = runs
        .iter()
        .filter(|run| run.host == host && run.grade == "clean")
        .max_by(|a, b| a.started.cmp(&b.started))
    else {
        return Ok(Vec::new());
    };
    let mut issues = Vec::new();
    if let Ok(started) = DateTime::parse_from_rfc3339(&latest.started) {
        let age = (Utc::now() - started.with_timezone(&Utc)).num_days();
        if age >= 120 {
            issues.push(HealthIssue {
                severity: Severity::Warning,
                title: "The benchmark history for this machine is stale".into(),
                detail: format!("The last clean run was {age} days ago"),
                action: "Run hwtune bench run to refresh the series".into(),
            });
        }
    }
    if let Some(baseline) = store.baseline_run(host, &latest.epoch())?
        && baseline.run_id != latest.run_id
    {
        issues.extend(
            regressions(&compare_runs(&baseline, latest), 10.0)
                .into_iter()
                .map(|delta| regression_issue(delta, &baseline, latest)),
        );
    }
    Ok(issues)
}
