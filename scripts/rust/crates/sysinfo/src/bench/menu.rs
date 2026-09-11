use super::{
    record::{ANY, CLEAN, Run},
    report,
    store::Store,
};
use std::collections::BTreeSet;
use workstation::{Key, Screen};

#[derive(Clone, Debug)]
pub struct Pick {
    pub kind: String,
    pub option: String,
}
#[derive(Clone, Debug)]
pub struct Column {
    pub kind: String,
    pub title: String,
    pub options: Vec<(String, String)>,
    pub index: usize,
}
impl Column {
    fn new(kind: &str, title: &str, options: Vec<(String, String)>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            options,
            index: 0,
        }
    }
    fn picked(&self) -> Pick {
        Pick {
            kind: self.kind.clone(),
            option: self.options[self.index].0.clone(),
        }
    }
}
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
    let found = cascade(title, |picks| {
        if picks.is_empty() {
            Some(simple("choice", title, options.to_vec()))
        } else {
            None
        }
    })?;
    Ok(found.and_then(|picks| {
        picks
            .last()
            .and_then(|pick| options.iter().position(|option| option == &pick.option))
    }))
}
pub fn cascade(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
) -> Result<Option<Vec<Pick>>, String> {
    super::require_terminal(title)?;
    let Some(root) = expand(&[]).filter(|column| !column.options.is_empty()) else {
        return Ok(None);
    };
    let Some(mut screen) = Screen::open().map_err(|e| e.to_string())? else {
        return Err("terminal unavailable".into());
    };
    let mut columns = vec![root];
    loop {
        let size = screen.size().unwrap_or((80, 24));
        screen
            .draw(&frame(title, &columns, size.0, size.1))
            .map_err(|e| e.to_string())?;
        let key = screen.key().map_err(|e| e.to_string())?;
        match key {
            Key::Escape | Key::Interrupt | Key::Char('q') => {
                screen.clear().map_err(|e| e.to_string())?;
                return Ok(None);
            }
            Key::Left | Key::Backspace | Key::Char('h') => {
                if columns.len() > 1 {
                    columns.pop();
                }
            }
            Key::Up | Key::Char('k') => {
                let column = columns.last_mut().unwrap();
                column.index = (column.index + column.options.len() - 1) % column.options.len();
            }
            Key::Down | Key::Char('j') => {
                let column = columns.last_mut().unwrap();
                column.index = (column.index + 1) % column.options.len();
            }
            Key::Home => columns.last_mut().unwrap().index = 0,
            Key::End => {
                let column = columns.last_mut().unwrap();
                column.index = column.options.len() - 1;
            }
            Key::PageUp | Key::PageDown => {
                let column = columns.last_mut().unwrap();
                let step = size.1.saturating_sub(6).max(1);
                column.index = if key == Key::PageUp {
                    column.index.saturating_sub(step)
                } else {
                    (column.index + step).min(column.options.len() - 1)
                };
            }
            Key::Enter | Key::Right | Key::Char('l') => {
                let picks = columns.iter().map(Column::picked).collect::<Vec<_>>();
                if let Some(child) = expand(&picks).filter(|column| !column.options.is_empty()) {
                    columns.push(child);
                } else if key == Key::Enter {
                    screen.clear().map_err(|e| e.to_string())?;
                    drop(screen);
                    println!(
                        "  {title} — {}",
                        picks
                            .iter()
                            .map(|pick| pick.option.as_str())
                            .collect::<Vec<_>>()
                            .join(" › ")
                    );
                    return Ok(Some(picks));
                }
            }
            Key::Char(digit) if digit.is_ascii_digit() && digit != '0' => {
                let index = (digit as u8 - b'1') as usize;
                let column = columns.last_mut().unwrap();
                if index < column.options.len() {
                    column.index = index;
                }
            }
            _ => {}
        }
    }
}
pub fn frame(title: &str, columns: &[Column], width: usize, height: usize) -> Vec<String> {
    let width = width.saturating_sub(1).max(1);
    let room = height.saturating_sub(6).max(1);
    let active = columns.len() - 1;
    let widths = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            column
                .options
                .iter()
                .map(|(name, detail)| {
                    unicode_width::UnicodeWidthStr::width(name.as_str())
                        + 2
                        + if index == active && !detail.is_empty() {
                            2 + unicode_width::UnicodeWidthStr::width(detail.as_str())
                        } else {
                            0
                        }
                })
                .max()
                .unwrap_or(0)
                .max(unicode_width::UnicodeWidthStr::width(column.title.as_str()))
        })
        .collect::<Vec<_>>();
    let mut first = 0;
    while first < active && 2 + widths[first..].iter().sum::<usize>() + (active - first) * 3 > width
    {
        first += 1;
    }
    let heading = if first > 0 {
        format!(
            "  {title}  ‹ {}",
            columns[..first]
                .iter()
                .map(|column| column.options[column.index].0.as_str())
                .collect::<Vec<_>>()
                .join(" › ")
        )
    } else {
        format!("  {title}")
    };
    let mut lines = vec![
        String::new(),
        fit(&heading, width),
        fit("  ↑/↓ move | ←/→ level | ↩ select | q quit", width),
        String::new(),
    ];
    let mut headers = String::from("  ");
    for (index, column) in columns.iter().enumerate().skip(first) {
        if index > first {
            headers.push_str("   ");
        }
        headers.push_str(&column.title);
        if index < active {
            headers.push_str(
                &" ".repeat(
                    widths[index].saturating_sub(unicode_width::UnicodeWidthStr::width(
                        column.title.as_str(),
                    )),
                ),
            );
        }
    }
    lines.push(fit(&headers, width));
    let shown = columns[first..]
        .iter()
        .map(|column| column.options.len().min(room))
        .max()
        .unwrap_or(0);
    for row in 0..shown {
        let mut line = String::from("  ");
        for (index, column) in columns.iter().enumerate().skip(first) {
            if index > first {
                line.push_str("   ");
            }
            let start = column.index.saturating_sub(room.saturating_sub(1));
            let item = start + row;
            let cell = if let Some((option, detail)) = column.options.get(item) {
                format!(
                    "{}{}{}",
                    if item == column.index { "❯ " } else { "  " },
                    option,
                    if index == active && !detail.is_empty() {
                        format!("  {detail}")
                    } else {
                        String::new()
                    }
                )
            } else {
                String::new()
            };
            line.push_str(&cell);
            if index < active {
                line.push_str(
                    &" ".repeat(
                        widths[index]
                            .saturating_sub(unicode_width::UnicodeWidthStr::width(cell.as_str())),
                    ),
                );
            }
        }
        lines.push(fit(&line, width));
    }
    lines
}
fn fit(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + size > width {
            break;
        }
        result.push(ch);
        used += size;
    }
    result
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
pub fn open(store: &Store, flow: &str) -> Result<(), String> {
    super::require_terminal(if flow.is_empty() {
        "benchmark menu"
    } else {
        flow
    })?;
    let history = History {
        runs: store.list_runs(None, ANY)?,
        hosts: store.known_hosts()?,
    };
    let Some(picks) = cascade("sysinfo bench", |picks| history.expand(flow, picks))? else {
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
            let matches = super::command()
                .try_get_matches_from(["bench", name])
                .map_err(|e| e.to_string())?;
            super::run(&matches)
        }
    }
}
