use super::{
    record::{ANY, CLEAN, Run},
    report,
    store::Store,
};
use std::collections::BTreeSet;
pub use ui_picker::{Column, Pick};

fn selected<'a>(picks: &'a [Pick], kind: &str) -> &'a str {
    picks
        .iter()
        .find(|pick| pick.kind == kind)
        .map(|pick| pick.option.as_str())
        .unwrap_or("")
}
fn note(message: &str) -> Column {
    Column::new("note", "", vec![(message.into(), String::new())])
}
fn simple(kind: &str, title: &str, options: Vec<String>) -> Column {
    Column::new(
        kind,
        title,
        options
            .into_iter()
            .map(|option| (option, String::new()))
            .collect(),
    )
}
pub fn choose(title: &str, options: &[String]) -> Result<Option<usize>, String> {
    super::require_terminal(title)?;
    ui_picker::choose(title, options)
}

pub fn cascade(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
) -> Result<Option<Vec<Pick>>, String> {
    super::require_terminal(title)?;
    let selected = ui_picker::cascade(title, expand)?;
    if let Some(picks) = &selected {
        println!(
            "  {title} — {}",
            picks
                .iter()
                .map(|pick| pick.option.as_str())
                .collect::<Vec<_>>()
                .join(" › ")
        );
    }
    Ok(selected)
}

const MENU: [(&str, &str); 8] = [
    ("run", "measure this machine now"),
    ("show", "inspect a stored run"),
    ("health", "warnings derived from benchmark history"),
    ("list", "stored runs"),
    ("compare", "two runs side by side"),
    ("trend", "one metric over time"),
    ("baseline", "set or clear the reference run"),
    ("prune", "thin old runs"),
];
const MODES: [(&str, &str); 4] = [
    (
        "machine vs machine",
        "the same metric on two different machines",
    ),
    (
        "before vs after upgrade",
        "two hardware configurations of one machine",
    ),
    ("distro vs distro", "two installations on one machine"),
    ("pick two runs", "choose both sides by hand"),
];
struct History {
    runs: Vec<Run>,
    hosts: Vec<String>,
}
impl History {
    fn host<'a>(&'a self, picks: &'a [Pick]) -> &'a str {
        let host = selected(picks, "host");
        if host.is_empty() {
            self.hosts.first().map(String::as_str).unwrap_or("")
        } else {
            host
        }
    }
    fn run_column(&self, host: &str, clean: bool, title: &str, kind: &str) -> Column {
        let found = self
            .runs
            .iter()
            .filter(|run| (host.is_empty() || run.host == host) && (!clean || run.grade == "clean"))
            .map(|run| {
                (
                    format!("{}:{}", run.host, run.run_id),
                    report::describe_run(run),
                )
            })
            .collect::<Vec<_>>();
        if found.is_empty() {
            note("no runs recorded")
        } else {
            Column::new(kind, title, found)
        }
    }
    fn grouping(picks: &[Pick]) -> (&'static str, &'static str) {
        if selected(picks, "compare") == "before vs after upgrade" {
            ("epoch", "hardware configuration")
        } else {
            ("install", "installation")
        }
    }
    fn group_keys(&self, host: &str, kind: &str) -> Vec<String> {
        let mut keys = Vec::new();
        for run in self.runs.iter().filter(|run| run.host == host) {
            let key = if kind == "epoch" {
                run.epoch()
            } else {
                run.os_id().into()
            };
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }
    fn pair(&self, picks: &[Pick], side: &str) -> Column {
        let host = self.host(picks);
        let (kind, noun) = Self::grouping(picks);
        let keys = self.group_keys(host, kind);
        if keys.len() < 2 {
            return note(&format!("{host} has only one {noun} on record"));
        }
        let selected_a = selected(picks, &format!("{kind}-a"));
        simple(
            &format!("{kind}-{side}"),
            match (kind, side) {
                ("epoch", "a") => "earlier configuration",
                ("epoch", _) => "later configuration",
                (_, "a") => "first installation",
                _ => "second installation",
            },
            keys.into_iter()
                .filter(|key| side == "a" || key != selected_a)
                .collect(),
        )
    }
    fn after_host(&self, flow: &str, picks: &[Pick]) -> Column {
        let host = self.host(picks);
        match flow {
            "trend" => {
                let keys = self
                    .runs
                    .iter()
                    .filter(|run| run.host == host && run.grade == "clean")
                    .flat_map(|run| run.metrics.iter().map(|metric| metric.key.clone()))
                    .collect::<BTreeSet<_>>();
                if keys.is_empty() {
                    note(&format!("{host} has no clean runs"))
                } else {
                    simple("metric", "which metric?", keys.into_iter().collect())
                }
            }
            "baseline" => self.run_column(host, true, "use which run as the baseline?", "run"),
            _ => self.pair(picks, "a"),
        }
    }
    fn opening(&self, flow: &str, picks: &[Pick]) -> Option<Column> {
        match flow {
            "show" => Some(self.run_column("", false, "show which run?", "run")),
            "compare" => Some(Column::new(
                "compare",
                "compare what?",
                MODES
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            )),
            "trend" | "baseline" => Some(if self.hosts.len() > 1 {
                simple("host", "which machine?", self.hosts.clone())
            } else {
                self.after_host(flow, picks)
            }),
            _ => None,
        }
    }
    fn expand(&self, flow: &str, picks: &[Pick]) -> Option<Column> {
        if picks.is_empty() {
            return if flow.is_empty() {
                Some(Column::new(
                    "menu",
                    "",
                    MENU.iter()
                        .map(|(a, b)| (a.to_string(), b.to_string()))
                        .collect(),
                ))
            } else {
                self.opening(flow, picks)
            };
        }
        let last = picks.last()?;
        match last.kind.as_str() {
            "menu" => self.opening(&last.option, picks),
            "compare" => Some(match last.option.as_str() {
                "machine vs machine" => {
                    if self.hosts.len() < 2 {
                        note("two machines are needed; only one has runs")
                    } else {
                        simple("host-a", "first machine", self.hosts.clone())
                    }
                }
                "pick two runs" => self.run_column("", false, "left run", "run-a"),
                _ => {
                    if self.hosts.len() > 1 {
                        simple("host", "which machine?", self.hosts.clone())
                    } else {
                        self.pair(picks, "a")
                    }
                }
            }),
            "host" => Some(self.after_host(
                if flow.is_empty() {
                    selected(picks, "menu")
                } else {
                    flow
                },
                picks,
            )),
            "host-a" => Some(simple(
                "host-b",
                "second machine",
                self.hosts
                    .iter()
                    .filter(|name| *name != &last.option)
                    .cloned()
                    .collect(),
            )),
            "run-a" => Some(self.run_column("", false, "right run", "run-b")),
            "epoch-a" | "install-a" => Some(self.pair(picks, "b")),
            _ => None,
        }
    }
    fn sides(&self, store: &Store, picks: &[Pick]) -> Result<(Run, Run), String> {
        match selected(picks, "compare") {
            "machine vs machine" => Ok((
                super::require_run(store, selected(picks, "host-a"))?,
                super::require_run(store, selected(picks, "host-b"))?,
            )),
            "pick two runs" => Ok((
                super::require_run(store, selected(picks, "run-a"))?,
                super::require_run(store, selected(picks, "run-b"))?,
            )),
            _ => {
                let (kind, _) = Self::grouping(picks);
                let host = self.host(picks);
                let find = |side: &str| {
                    let key = selected(picks, &format!("{kind}-{side}"));
                    self.runs
                        .iter()
                        .find(|run| {
                            run.host == host
                                && if kind == "epoch" {
                                    run.epoch() == key
                                } else {
                                    run.os_id() == key
                                }
                        })
                        .cloned()
                        .ok_or_else(|| "selected run is no longer available".to_string())
                };
                Ok((find("a")?, find("b")?))
            }
        }
    }
}
pub fn open(store: &Store, flow: &str, host: Option<&str>) -> Result<(), String> {
    super::require_terminal(if flow.is_empty() {
        "benchmark menu"
    } else {
        flow
    })?;
    let history = History {
        runs: store.list_runs(host, ANY)?,
        hosts: match host {
            Some(host) => vec![host.to_owned()],
            None => store.known_hosts()?,
        },
    };
    let Some(picks) = cascade("hwtune bench", |picks| history.expand(flow, picks))? else {
        return Ok(());
    };
    if picks.last().is_some_and(|p| p.kind == "note") {
        return Err(picks.last().unwrap().option.clone());
    }
    let name = if flow.is_empty() {
        selected(&picks, "menu")
    } else {
        flow
    };
    match name {
        "show" => {
            report::render_run(&super::require_run(store, selected(&picks, "run"))?);
            Ok(())
        }
        "compare" => {
            let (a, b) = history.sides(store, &picks)?;
            super::emit_comparison(&a, &b, false)
        }
        "trend" => {
            let runs = store.list_runs(Some(history.host(&picks)), CLEAN)?;
            report::render_trend(&runs, selected(&picks, "metric"));
            Ok(())
        }
        "baseline" => {
            super::set_baseline(store, &super::require_run(store, selected(&picks, "run"))?)
        }
        _ => {
            let mut arguments = vec!["bench", name];
            if let Some(host) = host {
                arguments.extend(["--host", host]);
            }
            let matches = super::command()
                .try_get_matches_from(arguments)
                .map_err(|e| e.to_string())?;
            super::run(&matches)
        }
    }
}
