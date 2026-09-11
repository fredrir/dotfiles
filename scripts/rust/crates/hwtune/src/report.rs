use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::paths::Paths;
use crate::table;

#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub run_id: String,
    pub started: String,
    pub grade: String,
    pub tags: Vec<String>,
    pub metrics: BTreeMap<String, f64>,
}

pub fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    })
}

pub fn summarize(run: &Value) -> Option<RunSummary> {
    let text = |key: &str| run.get(key)?.as_str().map(str::to_string);
    let metrics = run
        .get("metrics")?
        .as_array()?
        .iter()
        .filter_map(|metric| {
            let key = metric.get("key")?.as_str()?.to_string();
            let samples = metric
                .get("samples")?
                .as_array()?
                .iter()
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>();
            Some((key, median(&samples)?))
        })
        .collect();
    Some(RunSummary {
        run_id: text("run_id")?,
        started: text("started").unwrap_or_default(),
        grade: text("grade").unwrap_or_default(),
        tags: run
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        metrics,
    })
}

pub fn load_runs(dir: &Path) -> Result<Vec<RunSummary>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut runs = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let text = fs::read_to_string(&path).ok()?;
            summarize(&serde_json::from_str::<Value>(&text).ok()?)
        })
        .collect::<Vec<_>>();
    runs.sort_by(|a, b| a.started.cmp(&b.started));
    Ok(runs)
}

pub fn bios_tag(run: &RunSummary) -> String {
    run.tags
        .iter()
        .find(|tag| tag.starts_with("bios:"))
        .cloned()
        .unwrap_or_else(|| "untagged".into())
}

pub fn latest_per_tag(runs: &[RunSummary]) -> Vec<(String, RunSummary)> {
    let mut latest: BTreeMap<String, RunSummary> = BTreeMap::new();
    for run in runs
        .iter()
        .filter(|run| run.grade == "clean" || run.grade.is_empty())
    {
        latest.insert(bios_tag(run), run.clone());
    }
    let mut ordered = latest.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.1.started.cmp(&b.1.started));
    ordered
}

pub fn render(groups: &[(String, RunSummary)], metric: Option<&str>) -> String {
    let keys = groups
        .iter()
        .flat_map(|(_, run)| run.metrics.keys().cloned())
        .filter(|key| metric.is_none_or(|wanted| key.contains(wanted)))
        .collect::<std::collections::BTreeSet<_>>();
    let mut headers = vec!["metric".to_string()];
    headers.extend(groups.iter().map(|(tag, _)| tag.clone()));
    let header_refs = headers.iter().map(String::as_str).collect::<Vec<_>>();
    let mut rows = vec![{
        let mut row = vec!["run".to_string()];
        row.extend(groups.iter().map(|(_, run)| run.started.clone()));
        row
    }];
    for key in keys {
        let mut row = vec![key.clone()];
        row.extend(groups.iter().map(|(_, run)| {
            run.metrics
                .get(&key)
                .map_or("-".into(), |value| format!("{value:.1}"))
        }));
        rows.push(row);
    }
    table::render(&header_refs, &rows)
}

pub fn run(paths: &Paths, metric: Option<&str>) -> Result<ExitCode, String> {
    let runs = load_runs(&paths.benchmarks_dir().join(&paths.host))?;
    if runs.is_empty() {
        return Err(format!("no benchmark runs stored for {}", paths.host));
    }
    print!("{}", render(&latest_per_tag(&runs), metric));
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "../tests/unit/report_tests.rs"]
mod tests;
