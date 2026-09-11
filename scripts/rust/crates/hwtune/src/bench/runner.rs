use super::{
    capture, conditions,
    record::{Metric, Run, epoch_of, relative_deviation},
    suites::{self, Job, Samples, WRITTEN},
};
use chrono::{SecondsFormat, Utc};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
pub fn cancelled() -> bool {
    ui_terminal::termination_requested() || crate::tune::monitor::cancelled()
}

pub const FAMILIES: [&str; 7] = ["cpu", "mem", "cache", "disk", "gpu", "thermal", "workload"];
#[derive(Clone, Debug)]
pub struct Setting {
    pub tier: String,
    pub workdir: PathBuf,
    pub families: Vec<String>,
    pub memory_bytes: u64,
}
impl Setting {
    pub fn accepts(&self, family: &str) -> bool {
        self.families.is_empty() || self.families.iter().any(|selected| selected == family)
    }
}
#[derive(Default)]
pub struct Options {
    pub context: Option<super::provenance::RunContext>,
    pub host: String,
    pub tier: String,
    pub families: Vec<String>,
    pub note: String,
    pub tags: Vec<String>,
    pub force: bool,
    pub workdir: Option<PathBuf>,
}
pub fn write_budget(tier: &str) -> u64 {
    match tier {
        "standard" => 30 * 1024_u64.pow(3),
        "heavy" => 70 * 1024_u64.pow(3),
        _ => 0,
    }
}
pub fn default_workdir() -> PathBuf {
    super::suites::gpu::cache_dir().join("work")
}
pub fn converged(collected: &Samples) -> bool {
    !collected.is_empty()
        && collected
            .values()
            .all(|values| relative_deviation(values) <= 2.5)
}
pub fn measure_job(job: &mut Job) -> Result<Samples, String> {
    let mut collected = Samples::new();
    let minimum = if job.repeat { 3 } else { 1 };
    let limit = if job.repeat { 6 } else { 1 };
    for attempt in 1..=limit {
        if cancelled() {
            return Err("benchmark interrupted".into());
        }
        let measured = (job.measure)()?;
        for (key, values) in measured.values {
            if values.iter().any(|value| !value.is_finite()) {
                return Err(format!("{} reported non-finite measurements", job.name));
            }
            collected.entry(key).or_default().extend(values);
        }
        if let Some(detail) = measured.detail.as_object() {
            for (key, value) in detail {
                job.detail[key] = value.clone();
            }
        }
        if attempt >= minimum && converged(&collected) {
            break;
        }
    }
    Ok(collected)
}
pub fn metrics_for(job: &Job, collected: &Samples) -> Vec<Metric> {
    job.outputs
        .iter()
        .filter_map(|output| {
            let samples = collected
                .get(&output.key)
                .filter(|values| !values.is_empty())?;
            let mut metric = output.clone();
            metric.method = job.method.clone();
            metric.tool = job.tool.clone();
            metric.tool_version = job.version.clone();
            metric.samples = samples.clone();
            metric.detail = job.detail.clone();
            Some(metric)
        })
        .collect()
}
pub fn execute(options: &Options, report: &mut dyn FnMut(&str, &str, &str)) -> Result<Run, String> {
    let workdir = options.workdir.clone().unwrap_or_else(default_workdir);
    fs::create_dir_all(&workdir).map_err(|error| format!("{}: {error}", workdir.display()))?;
    let mut snapshot = sysinfo::collect::collect_snapshot_for_host(
        true,
        &options.host,
        &super::hosts::inventory_context(),
    );
    let mut conditions = conditions::capture_conditions(&snapshot, &workdir);
    if !options.force && conditions["throttled_at_start"] == true {
        let deadline = Instant::now() + Duration::from_secs(240);
        while Instant::now() < deadline {
            report(
                "cool",
                "cooling",
                &format!(
                    "{}s left",
                    deadline.saturating_duration_since(Instant::now()).as_secs()
                ),
            );
            for _ in 0..150 {
                if cancelled() {
                    return Err("benchmark interrupted".into());
                }
                thread::sleep(Duration::from_millis(100));
            }
            let cooled = sysinfo::collect::collect_snapshot_for_host(
                true,
                &options.host,
                &super::hosts::inventory_context(),
            );
            let values = conditions::capture_conditions(&cooled, &workdir);
            if values["throttled_at_start"] != true {
                snapshot = cooled;
                conditions = values;
                break;
            }
        }
    }
    conditions["filesystem"] = capture::filesystem_of(&workdir);
    let budget = write_budget(&options.tier);
    let writes_disk = budget > 0
        && (options.families.is_empty() || options.families.iter().any(|family| family == "disk"));
    let reasons = conditions::gate_reasons(&conditions, writes_disk);
    if !reasons.is_empty() && !options.force {
        return Err(format!(
            "{}\nconditions are not suitable; pass --force to measure anyway",
            reasons.join("\n")
        ));
    }
    let described = capture::describe_snapshot(&snapshot);
    let started = Utc::now();
    let setting = Setting {
        tier: options.tier.clone(),
        workdir,
        families: options.families.clone(),
        memory_bytes: snapshot.result("Memory")["total"].as_u64().unwrap_or(0),
    };
    let (mut jobs, mut failures) = suites::collect_jobs(&setting);
    let count = jobs.len();
    let mut metrics = Vec::new();
    let mut written = 0_u64;
    for (position, job) in jobs.iter_mut().enumerate() {
        if cancelled() {
            break;
        }
        if job.writes > 0 && written.saturating_add(job.writes) > budget {
            let refused = format!(
                "would write {:.1} GiB, past the {} budget",
                written.saturating_add(job.writes) as f64 / 1073741824.0,
                options.tier
            );
            failures.push(format!("{}: {refused}", job.name));
            report("skip", &job.name, &refused);
            continue;
        }
        report("start", &job.name, &format!("{} of {count}", position + 1));
        // Reserve before invoking a tool: failed/aborted jobs may have written
        // their files already and must still consume this run's budget.
        written = written.saturating_add(job.writes);
        let began = Instant::now();
        let mut collected = match measure_job(job) {
            Ok(values) => values,
            Err(error) => {
                failures.push(format!("{}: {error}", job.name));
                report("skip", &job.name, &error);
                continue;
            }
        };
        let measured = collected
            .remove(WRITTEN)
            .unwrap_or_default()
            .into_iter()
            .fold(0.0, f64::max) as u64;
        written = written.saturating_add(measured.saturating_sub(job.writes));
        let produced = metrics_for(job, &collected);
        let samples = produced.iter().map(|m| m.samples.len()).max().unwrap_or(0);
        metrics.extend(produced);
        report(
            "done",
            &job.name,
            &format!("{}s, n={samples}", began.elapsed().as_secs()),
        );
    }
    let grade = if cancelled() {
        "aborted"
    } else {
        conditions::grade_for(&reasons, metrics.len(), &failures)
    };
    let gate_reasons = reasons.into_iter().chain(failures).collect();
    Ok(Run {
        run_id: format!(
            "{}-{}-{:x}",
            started.format("%Y-%m-%dT%H-%M-%S%.9fZ"),
            epoch_of(&described),
            std::process::id()
        ),
        context: options.context.clone(),
        host: options.host.clone(),
        started: started.to_rfc3339_opts(SecondsFormat::Secs, true),
        tier: options.tier.clone(),
        grade: grade.into(),
        snapshot: described,
        install: sysinfo::report::describe_install(&snapshot),
        conditions,
        metrics,
        note: options.note.clone(),
        tags: options.tags.clone(),
        dotfiles_sha: capture::dotfiles_sha(),
        gate_reasons,
        bytes_written: written,
        schema: 1,
    })
}
