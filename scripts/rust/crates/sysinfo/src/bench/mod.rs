pub mod capture;
pub mod compare;
pub mod conditions;
pub mod health;
mod menu;
mod plan;
pub mod record;
pub mod report;
pub mod runner;
pub mod select;
pub mod store;
pub mod suites;
#[cfg(test)]
#[path = "../../tests/bench_tests.rs"]
mod tests;

use clap::{Arg, ArgAction, ArgMatches};
pub use health::benchmark_issues;
use record::{ANY, CLEAN, Run};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
};
use store::Store;

fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help)
}
fn option(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help)
}
fn target(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).help(help)
}
fn measurements(name: &'static str, about: &'static str) -> clap::Command {
    clap::Command::new(name)
        .about(about)
        .arg(
            option("tier", "quick, standard or heavy")
                .value_parser(record::TIERS)
                .default_value("quick"),
        )
        .arg(option("only", "Comma separated families to measure"))
        .arg(option("workdir", "Directory the disk tier writes in"))
        .arg(flag("json", "Emit JSON"))
}
pub fn command() -> clap::Command {
    clap::Command::new("bench")
        .about("Measure this machine and compare runs over time")
        .subcommand(
            measurements("run", "Measure this machine and store the result")
                .arg(option("note", "Why this run was taken"))
                .arg(option("tag", "Label for this run").action(ArgAction::Append))
                .arg(option("host", "Record against this host"))
                .arg(flag("force", "Measure despite poor conditions"))
                .arg(flag("no-save", "Print without storing"))
                .arg(flag("baseline", "Pin this run as the baseline")),
        )
        .subcommand(measurements(
            "plan",
            "Show available suites and expected writes without measuring",
        ))
        .subcommand(
            clap::Command::new("show")
                .about("Show a stored run")
                .arg(target(
                    "target",
                    "Selector such as archie or archie@a3f19c2e",
                ))
                .arg(flag("json", "Emit JSON")),
        )
        .subcommand(
            clap::Command::new("list")
                .about("List stored runs")
                .arg(option("host", "Only this machine"))
                .arg(
                    option("limit", "Rows to print; 0 for all")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("20"),
                )
                .arg(flag("all", "Include noisy and aborted runs")),
        )
        .subcommand(
            clap::Command::new("health")
                .about("Warnings derived from benchmark history")
                .arg(option("host", "Machine to judge"))
                .arg(flag("json", "Emit JSON")),
        )
        .subcommand(
            clap::Command::new("compare")
                .about("Compare two runs")
                .arg(target("left", "Left selector"))
                .arg(target("right", "Right selector"))
                .arg(flag("json", "Emit JSON")),
        )
        .subcommand(
            clap::Command::new("trend")
                .about("Show one metric over time")
                .arg(target("target", "Selector such as archie"))
                .arg(target("metric", "Metric key such as cpu.multi")),
        )
        .subcommand(
            clap::Command::new("baseline")
                .about("Set or clear the reference run")
                .arg(
                    target("action", "set, clear or show")
                        .value_parser(["set", "clear", "show"])
                        .default_value("show"),
                )
                .arg(target("target", "Selector such as archie@a3f19c2e")),
        )
        .subcommand(
            clap::Command::new("prune")
                .about("Thin old runs, retaining baselines and oldest runs")
                .arg(option("host", "Only this machine"))
                .arg(
                    option("keep", "Runs to keep per hardware configuration")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("12"),
                )
                .arg(flag("dry-run", "Report without deleting"))
                .arg(flag("yes", "Delete without confirming")),
        )
}
fn text<'a>(args: &'a ArgMatches, key: &str) -> &'a str {
    args.get_one::<String>(key)
        .map(String::as_str)
        .unwrap_or("")
}
fn nonempty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}
fn json_output(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}
pub fn complete(source: &str) -> Result<Vec<String>, String> {
    let store = Store::discover();
    if source == "bench-hosts" {
        return store.known_hosts();
    }
    let runs = store.list_runs(None, ANY)?;
    let mut rows = BTreeSet::new();
    let mut hosts = BTreeMap::<&str, usize>::new();
    let mut epochs = BTreeMap::<String, usize>::new();
    for run in &runs {
        match source {
            "runs" => {
                *hosts.entry(&run.host).or_default() += 1;
                *epochs
                    .entry(format!("{}@{}", run.host, run.epoch()))
                    .or_default() += 1;
                // `_describe` uses the first unescaped colon to split value and label.
                rows.insert(format!(
                    "{}\\:{}:{} {} {}",
                    run.host, run.run_id, run.started, run.tier, run.grade
                ));
            }
            "metrics" if run.grade == "clean" => {
                rows.extend(run.metrics.iter().map(|m| m.key.clone()))
            }
            _ => {}
        }
    }
    rows.extend(
        hosts
            .into_iter()
            .map(|(host, count)| format!("{host}:{count} stored runs")),
    );
    rows.extend(
        epochs
            .into_iter()
            .map(|(epoch, count)| format!("{epoch}:{count} runs on this hardware")),
    );
    Ok(rows.into_iter().collect())
}
pub fn run(args: &ArgMatches) -> Result<(), String> {
    let store = Store::discover();
    let Some((name, args)) = args.subcommand() else {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            workstation::cli::decorate(command())
                .print_help()
                .map_err(|e| e.to_string())?;
            println!();
            return Ok(());
        }
        return menu::open(&store, "");
    };
    match name {
        "run" => measure(&store, args),
        "plan" => plan::run(args),
        "show" => {
            if text(args, "target").is_empty() {
                return menu::open(&store, "show");
            }
            let found = require_run(&store, text(args, "target"))?;
            if args.get_flag("json") {
                json_output(&found)
            } else {
                report::render_run(&found);
                Ok(())
            }
        }
        "list" => {
            let runs = store.list_runs(
                nonempty(text(args, "host")),
                if args.get_flag("all") { ANY } else { CLEAN },
            )?;
            let limit = *args.get_one::<usize>("limit").unwrap_or(&20);
            let shown = if limit == 0 {
                runs.len()
            } else {
                runs.len().min(limit)
            };
            report::render_list(&runs[..shown]);
            if shown < runs.len() {
                println!("  {} more; pass --limit 0 for all", runs.len() - shown);
            }
            Ok(())
        }
        "health" => {
            let host = crate::inventory::resolve(text(args, "host"))?;
            let issues = benchmark_issues(&host)?;
            if args.get_flag("json") {
                json_output(&issues)
            } else {
                if issues.is_empty() {
                    println!(
                        "  no benchmark findings for {}",
                        if host.is_empty() {
                            "this machine"
                        } else {
                            &host
                        }
                    );
                }
                for issue in issues {
                    println!("  {}: {}", issue.severity, issue.title);
                    if !issue.detail.is_empty() {
                        println!("    {}", issue.detail);
                    }
                    if !issue.action.is_empty() {
                        println!("    {}", issue.action);
                    }
                }
                Ok(())
            }
        }
        "compare" => {
            let left = text(args, "left");
            let right = text(args, "right");
            if left.is_empty() || right.is_empty() {
                return menu::open(&store, "compare");
            }
            emit_comparison(
                &require_run(&store, left)?,
                &require_run(&store, right)?,
                args.get_flag("json"),
            )
        }
        "trend" => {
            if text(args, "target").is_empty() || text(args, "metric").is_empty() {
                return menu::open(&store, "trend");
            }
            let selector = select::Selector::parse(text(args, "target"));
            report::render_trend(&selector.candidates(&store, CLEAN)?, text(args, "metric"));
            Ok(())
        }
        "baseline" => {
            match text(args, "action") {
                "show" => {
                    let pins = store.load_baselines()?;
                    if pins.is_empty() {
                        println!("  no baselines pinned");
                    }
                    for (host, pins) in pins {
                        for (epoch, id) in pins {
                            println!("  {host}@{epoch}  {id}");
                        }
                    }
                }
                "set" => {
                    if text(args, "target").is_empty() {
                        return menu::open(&store, "baseline");
                    }
                    set_baseline(&store, &require_run(&store, text(args, "target"))?)?;
                }
                "clear" => {
                    let selector = select::Selector::parse(text(args, "target"));
                    if selector.host.is_empty() || selector.epoch.is_empty() {
                        return Err("clear needs a host and epoch, such as archie@a3f19c2e".into());
                    }
                    let _lock = store.exclusive()?;
                    let removed = store.clear_baseline(&selector.host, &selector.epoch)?;
                    println!(
                        "sysinfo bench: {} for {}@{}",
                        if removed {
                            "cleared the baseline"
                        } else {
                            "no baseline was pinned"
                        },
                        selector.host,
                        selector.epoch
                    );
                }
                _ => unreachable!(),
            }
            Ok(())
        }
        "prune" => prune(
            &store,
            nonempty(text(args, "host")),
            *args.get_one::<usize>("keep").unwrap_or(&12),
            args.get_flag("dry-run"),
            args.get_flag("yes"),
        ),
        _ => Err(format!("unknown benchmark command: {name}")),
    }
}
pub(crate) fn require_terminal(what: &str) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        Err(format!(
            "{what} needs a terminal; pass the arguments instead"
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn require_run(store: &Store, target: &str) -> Result<Run, String> {
    select::Selector::parse(target)
        .resolve(store)?
        .ok_or_else(|| format!("no run matches {target}"))
}
pub(crate) fn set_baseline(store: &Store, run: &Run) -> Result<(), String> {
    let _lock = store.exclusive()?;
    store.set_baseline(&run.host, &run.epoch(), &run.run_id)?;
    println!(
        "sysinfo bench: baseline for {}@{} is {}",
        run.host,
        run.epoch(),
        run.run_id
    );
    Ok(())
}
pub(crate) fn emit_comparison(left: &Run, right: &Run, as_json: bool) -> Result<(), String> {
    let comparison = compare::compare_runs(left, right);
    if as_json {
        json_output(
            &json!({"left":left.run_id,"right":right.run_id,"changes":comparison.changes,"deltas":comparison.deltas}),
        )
    } else {
        report::render_comparison(left, right, &comparison);
        Ok(())
    }
}
fn families(args: &ArgMatches) -> Result<Vec<String>, String> {
    text(args, "only")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|family| {
            if runner::FAMILIES.contains(&family) {
                Ok(family.into())
            } else {
                Err(format!(
                    "unknown family '{family}'; expected one of {}",
                    runner::FAMILIES.join(", ")
                ))
            }
        })
        .collect()
}
fn resolve_host(explicit: &str) -> Result<(String, bool), String> {
    let known = crate::inventory::load_hosts()?;
    let name = crate::inventory::resolve(explicit)?;
    if known.iter().any(|host| host.name == name) {
        return Ok((name, true));
    }
    if !explicit.is_empty() {
        return Err(format!(
            "unknown host '{explicit}'; known hosts: {}",
            known
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let detected = crate::inventory::local_hostnames();
    let primary = detected.first().map(String::as_str).unwrap_or("unknown");
    eprintln!(
        "sysinfo bench: this machine is not described in config/hosts.dotfile (hostname {primary})"
    );
    require_terminal("adopting a host")?;
    let choice = menu::choose(
        "unknown machine",
        &[
            "adopt as a new host".into(),
            "run without saving".into(),
            "quit".into(),
        ],
    )?;
    match choice {
        Some(0) => {
            let default = primary.split('.').next().unwrap_or(primary);
            let name = prompt("host name", default)?;
            if !crate::inventory::valid_name(&name) {
                return Err("host must be a valid single path component".into());
            }
            if known.iter().any(|h| h.name == name) {
                return Err(format!(
                    "{name} is already described in config/hosts.dotfile"
                ));
            }
            let role = prompt("role", "hyprland")?;
            let host = crate::inventory::Host {
                name: name.clone(),
                hostnames: detected,
                role,
                ..Default::default()
            };
            let path = crate::inventory::append_host(&host)?;
            crate::inventory::save_host(&name)?;
            println!(
                "sysinfo bench: wrote {name} to {} and pinned this machine to it",
                path.display()
            );
            Ok((name, true))
        }
        Some(1) => Ok(("unsaved".into(), false)),
        _ => Err("benchmark cancelled".into()),
    }
}
fn prompt(title: &str, default: &str) -> Result<String, String> {
    print!("{title} [{default}]: ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(if answer.trim().is_empty() {
        default.to_string()
    } else {
        answer.trim().into()
    })
}
fn measure(store: &Store, args: &ArgMatches) -> Result<(), String> {
    let families = families(args)?;
    let (host, persist) = resolve_host(text(args, "host"))?;
    let persist = persist && !args.get_flag("no-save");
    let as_json = args.get_flag("json");
    let _signals = ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
        reraise_on_drop: false,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let options = runner::Options {
        host,
        tier: text(args, "tier").into(),
        families,
        note: text(args, "note").into(),
        tags: args
            .get_many::<String>("tag")
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
        force: args.get_flag("force"),
        workdir: nonempty(text(args, "workdir")).map(PathBuf::from),
    };
    let measured;
    {
        let _lock = store.exclusive()?;
        measured = runner::execute(&options, &mut |kind, job, detail| {
            if !as_json {
                match kind {
                    "cool" => eprintln!("  waiting for the machine to cool ({detail})"),
                    "start" => eprintln!("  {job} … ({detail})"),
                    "done" => eprintln!("  {job} done in {detail}"),
                    "skip" => eprintln!("  {job} skipped: {detail}"),
                    _ => {}
                }
            }
        })?;
        if measured.metrics.is_empty() {
            return Err("no benchmark produced a result".into());
        }
        if persist {
            store.save_run(&measured)?;
            if args.get_flag("baseline") {
                store.set_baseline(&measured.host, &measured.epoch(), &measured.run_id)?;
            }
        }
    }
    if as_json {
        json_output(&measured)?;
    } else {
        report::render_run(&measured);
        if !persist {
            println!("  not stored");
        }
        if args.get_flag("baseline") && persist {
            println!(
                "  baseline for {}@{} is now this run",
                measured.host,
                measured.epoch()
            );
        }
    }
    if let Some(reference) = store.baseline_run(&measured.host, &measured.epoch())?
        && reference.run_id != measured.run_id
    {
        let result = compare::compare_runs(&reference, &measured);
        let failed = compare::regressions(&result, 10.0);
        if !failed.is_empty() {
            if !as_json {
                for delta in failed {
                    println!(
                        "  regression: {} {:+.1}% against the baseline",
                        delta.key, delta.change_pct
                    );
                }
            }
            return Err("benchmark regression detected".into());
        }
    }
    if measured.grade == "aborted" {
        return Err("benchmark interrupted".into());
    }
    Ok(())
}
pub(crate) fn prune(
    store: &Store,
    host: Option<&str>,
    keep: usize,
    dry_run: bool,
    yes: bool,
) -> Result<(), String> {
    let dropped = store.prunable(host, keep)?;
    if dropped.is_empty() {
        println!("  nothing to prune");
        return Ok(());
    }
    for run in &dropped {
        println!(
            "  {} {}/{}",
            if dry_run { "would remove" } else { "remove" },
            run.host,
            run.run_id
        );
    }
    if dry_run {
        println!(
            "  {} runs would be removed; re-run without --dry-run",
            dropped.len()
        );
        return Ok(());
    }
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(format!(
                "refusing to remove {} runs unattended; pass --yes",
                dropped.len()
            ));
        }
        let answer = prompt(&format!("Remove {} runs? y/N", dropped.len()), "n")?;
        if !matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("  nothing removed");
            return Ok(());
        }
    }
    let _lock = store.exclusive()?;
    let eligible = store
        .prunable(host, keep)?
        .into_iter()
        .map(|run| (run.host, run.run_id))
        .collect::<BTreeSet<_>>();
    let mut removed = 0;
    for run in dropped {
        if !eligible.contains(&(run.host.clone(), run.run_id.clone())) {
            continue;
        }
        match fs::remove_file(store.run_path(&run.host, &run.run_id)?) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    println!("  removed {removed} runs");
    Ok(())
}
