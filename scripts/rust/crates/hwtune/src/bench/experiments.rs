use std::collections::{BTreeMap, BTreeSet};

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde::Serialize;

use super::compare::{self, Comparison};
use super::provenance::{RunContext, Source};
use super::record::{CLEAN, Run, method_series};
use super::{report, select::Selector, store::Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    Bios,
    BiosLact,
}

pub fn command() -> Command {
    Command::new("report")
        .about("Review tuning configurations or compare a before/after experiment")
        .arg(
            Arg::new("group-by")
                .long("group-by")
                .value_parser(["bios", "bios-lact"])
                .default_value("bios-lact"),
        )
        .arg(
            Arg::new("metric")
                .long("metric")
                .help("Only metrics whose key contains this text"),
        )
        .arg(
            Arg::new("before")
                .long("before")
                .requires("after")
                .help("Run selector before the tuning change"),
        )
        .arg(
            Arg::new("after")
                .long("after")
                .requires("before")
                .help("Run selector after the tuning change"),
        )
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
}

fn source_hash(source: Option<&Source>) -> &str {
    source.map_or("unknown", |source| source.settings_sha256.as_str())
}

pub fn configuration(run: &Run, group: GroupBy) -> String {
    let context = run.context.as_ref();
    let bios = source_hash(context.and_then(|context| context.bios.as_ref()));
    match group {
        GroupBy::Bios => format!("bios:{bios}"),
        GroupBy::BiosLact => format!(
            "bios:{bios}/lact:{}",
            source_hash(context.and_then(|context| context.lact.as_ref()))
        ),
    }
}

fn protocol(run: &Run) -> String {
    let metrics = run
        .metrics
        .iter()
        .map(|metric| {
            (
                metric.key.as_str(),
                method_series(&metric.method),
                metric.scale.as_str(),
                metric.proportion.as_str(),
                metric.comparable.as_str(),
                metric.tool.as_str(),
                if metric.comparable == "world" {
                    metric.tool_version.as_str()
                } else {
                    ""
                },
            )
        })
        .collect::<BTreeSet<_>>();
    // Tuples of strings always serialize; using debug formatting also keeps this
    // internal partition deterministic without introducing a fallible API.
    let revision = if run
        .metrics
        .iter()
        .any(|metric| metric.family() == "workload")
    {
        run.dotfiles_sha.as_str()
    } else {
        ""
    };
    format!("{metrics:?}/{revision}")
}

#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub configuration: String,
    pub run: Run,
}

pub fn latest_by_configuration(runs: &[Run], group: GroupBy) -> Vec<Group> {
    let mut groups = BTreeMap::new();
    for run in runs.iter().filter(|run| run.grade == "clean") {
        let configuration = configuration(run, group);
        let key = (
            run.host.clone(),
            run.os_id().to_string(),
            run.epoch(),
            run.tier.clone(),
            protocol(run),
            run.context.as_ref().map(|context| context.observed.clone()),
            configuration.clone(),
        );
        let current = groups.entry(key).or_insert_with(|| Group {
            configuration,
            run: run.clone(),
        });
        if (&run.started, &run.run_id) > (&current.run.started, &current.run.run_id) {
            current.run = run.clone();
        }
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by(|a, b| (&a.run.started, &a.run.run_id).cmp(&(&b.run.started, &b.run.run_id)));
    groups
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextChange {
    pub setting: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

fn context_values(context: Option<&RunContext>) -> BTreeMap<String, String> {
    let Some(context) = context else {
        return BTreeMap::new();
    };
    let mut values = context.observed.clone();
    for (name, source) in [
        ("bios.settings", &context.bios),
        ("lact.settings", &context.lact),
    ] {
        if let Some(source) = source {
            values.insert(name.into(), source.settings_sha256.clone());
        }
    }
    values
}

pub fn context_changes(before: &Run, after: &Run) -> Vec<ContextChange> {
    let left = context_values(before.context.as_ref());
    let right = context_values(after.context.as_ref());
    left.keys()
        .chain(right.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| left.get(*key) != right.get(*key))
        .map(|key| ContextChange {
            setting: key.clone(),
            before: left.get(key).cloned(),
            after: right.get(key).cloned(),
        })
        .collect()
}

pub fn before_after(before: &Run, after: &Run) -> Result<Comparison, String> {
    if before.grade != "clean" || after.grade != "clean" {
        return Err("before/after experiments require two clean runs".into());
    }
    if before.host != after.host || before.epoch() != after.epoch() {
        return Err(
            "before/after experiments require the same host and hardware configuration".into(),
        );
    }
    if before.os_id() != after.os_id() || before.tier != after.tier {
        return Err("before/after experiments require the same platform and benchmark tier".into());
    }
    let result = compare::compare_runs(before, after);
    if result.deltas.is_empty() {
        return Err("before/after runs have no shared measured metrics".into());
    }
    Ok(result)
}

fn filter_comparison(result: &mut Comparison, wanted: Option<&str>) {
    if let Some(wanted) = wanted {
        result.deltas.retain(|metric| metric.key.contains(wanted));
        result.only_left.retain(|key| key.contains(wanted));
        result.only_right.retain(|key| key.contains(wanted));
    }
}

fn require_run(store: &Store, selector: &str) -> Result<Run, String> {
    Selector::parse(selector)
        .resolve(store)?
        .ok_or_else(|| format!("no benchmark run matches {selector}"))
}

fn render_groups(groups: &[Group]) {
    let mut cohorts = BTreeMap::<_, Vec<&Group>>::new();
    for group in groups {
        cohorts
            .entry((
                group.run.host.clone(),
                group.run.os_id().to_string(),
                group.run.epoch(),
                group.run.tier.clone(),
                protocol(&group.run),
            ))
            .or_default()
            .push(group);
    }
    for ((host, os, epoch, tier, _), groups) in cohorts {
        println!("\n  {host}  {os}  {tier}  epoch {epoch}");
        let mut headers = vec!["metric".to_string(), "unit".to_string()];
        for (index, group) in groups.iter().enumerate() {
            let column = format!("run {}", index + 1);
            headers.push(column.clone());
            println!(
                "  {column}: {host}:{}  {}",
                group.run.run_id, group.run.started
            );
            report::render_context(&group.run);
        }
        let keys = groups
            .iter()
            .flat_map(|group| group.run.metrics.iter().map(|m| &m.key))
            .collect::<BTreeSet<_>>();
        let rows = keys
            .into_iter()
            .map(|key| {
                let scale = groups
                    .iter()
                    .find_map(|group| group.run.metric(key))
                    .map_or("", |metric| metric.scale.as_str());
                let mut row = vec![key.clone(), scale.into()];
                row.extend(groups.iter().map(|group| {
                    report::format_value(group.run.metric(key).and_then(|m| m.median()), "")
                }));
                row
            })
            .collect::<Vec<_>>();
        let headers = headers.iter().map(String::as_str).collect::<Vec<_>>();
        print!("{}", crate::table::render(&headers, &rows));
    }
}

pub fn run(store: &Store, args: &ArgMatches) -> Result<(), String> {
    let metric = args.get_one::<String>("metric").map(String::as_str);
    if let (Some(before), Some(after)) = (
        args.get_one::<String>("before"),
        args.get_one::<String>("after"),
    ) {
        let before = require_run(store, before)?;
        let after = require_run(store, after)?;
        let mut result = before_after(&before, &after)?;
        filter_comparison(&mut result, metric);
        let changes = context_changes(&before, &after);
        if args.get_flag("json") {
            return super::json_output(&serde_json::json!({
                "before": before, "after": after, "comparison": result, "context_changes": changes,
            }));
        }
        println!("  before → after");
        for change in changes {
            println!(
                "  {}: {} → {}",
                change.setting,
                change.before.as_deref().unwrap_or("unknown"),
                change.after.as_deref().unwrap_or("unknown")
            );
        }
        report::render_context(&before);
        report::render_context(&after);
        report::render_comparison(&before, &after, &result);
        return Ok(());
    }
    let host = args
        .get_one::<String>("host")
        .map(String::as_str)
        .filter(|s| !s.is_empty());
    let by = if args
        .get_one::<String>("group-by")
        .is_some_and(|value| value == "bios")
    {
        GroupBy::Bios
    } else {
        GroupBy::BiosLact
    };
    let mut groups = latest_by_configuration(&store.list_runs(host, CLEAN)?, by);
    if let Some(metric) = metric {
        for group in &mut groups {
            group.run.metrics.retain(|m| m.key.contains(metric));
        }
        groups.retain(|group| !group.run.metrics.is_empty());
    }
    if args.get_flag("json") {
        return super::json_output(&groups);
    }
    if groups.is_empty() {
        println!("  no clean benchmark runs match this report");
    }
    render_groups(&groups);
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/bench/experiments_tests.rs"]
mod tests;
