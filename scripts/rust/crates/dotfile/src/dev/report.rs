use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::{cursor::MoveToColumn, terminal::Clear, terminal::ClearType};
use workstation::Style;
use workstation::text::{counted, truncate_back};

use super::plan::Task;
use crate::ui::{UiPolicy, sanitize_text};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Outcome {
    Passed,
    Failed,
    Cancelled,
    Skipped,
}

pub(super) struct Failure {
    pub name: String,
    pub code: i32,
    pub detail: String,
    pub log: Option<PathBuf>,
}

impl Failure {
    pub fn capture(name: String, code: i32, mut log: tempfile::NamedTempFile) -> Self {
        let mut detail = match excerpt(log.as_file_mut()) {
            Ok(value) => value,
            Err(error) => format!("log unreadable: {error}"),
        };
        let path = match log.keep() {
            Ok((_, path)) => Some(path),
            Err(error) => {
                detail.push_str(&format!("\nlog could not be saved: {}", error.error));
                None
            }
        };
        Self {
            name,
            code,
            detail,
            log: path,
        }
    }
}

fn excerpt(file: &mut std::fs::File) -> io::Result<String> {
    let length = file.metadata()?.len();
    let start = length.saturating_sub(32 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(32 * 1024);
    file.take(32 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    let mut tail: Vec<_> = lines
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(6)
        .map(|line| truncate_back(&sanitize_text(line), 180))
        .collect();
    tail.reverse();
    Ok(tail.join("\n"))
}

#[derive(Default)]
struct Suite {
    total: usize,
    done: usize,
    failed: usize,
    cancelled: usize,
    started: Option<Instant>,
}

pub(super) struct Report {
    style: Style,
    policy: UiPolicy,
    verbose: bool,
    suites: BTreeMap<String, Suite>,
    started: Instant,
    refreshed: Option<Instant>,
    visible: bool,
    done: usize,
    total: usize,
}

impl Report {
    pub fn new(action: &str, tasks: &[Task], verbose: bool) -> Self {
        let policy = UiPolicy::detect(
            io::stdin().is_terminal(),
            io::stderr().is_terminal(),
            "DOTFILE_REDUCED_MOTION",
        );
        let style = Style::for_stdout_with_color(policy.color);
        let mut suites = BTreeMap::<String, Suite>::new();
        for task in tasks {
            suites.entry(task.suite().into()).or_default().total += 1;
        }
        eprintln!(
            "{}  {}",
            style.bold(&style.teal(&format!("dev {action}"))),
            style.dim(&counted(tasks.len(), "task", "tasks"))
        );
        Self {
            style,
            policy,
            verbose,
            suites,
            started: Instant::now(),
            refreshed: None,
            visible: false,
            done: 0,
            total: tasks.len(),
        }
    }

    pub fn start(&mut self, task: &Task) {
        self.suites
            .get_mut(task.suite())
            .unwrap()
            .started
            .get_or_insert_with(Instant::now);
        if self.verbose {
            eprintln!("{} {}", self.style.teal("›"), sanitize_text(&task.name));
            eprintln!("  {}", self.style.dim(&sanitize_text(&task.display())));
        }
    }

    pub fn complete(&mut self, task: &Task, outcome: Outcome, elapsed: Duration) {
        self.done += 1;
        let suite = self.suites.get_mut(task.suite()).unwrap();
        suite.done += 1;
        suite.failed += usize::from(outcome == Outcome::Failed);
        suite.cancelled += usize::from(matches!(outcome, Outcome::Cancelled | Outcome::Skipped));
        if self.verbose {
            self.row(&task.name, outcome, elapsed);
        } else if suite.done == suite.total {
            let outcome = if suite.failed > 0 {
                Outcome::Failed
            } else if suite.cancelled > 0 {
                Outcome::Cancelled
            } else {
                Outcome::Passed
            };
            let elapsed = suite.started.map_or(Duration::ZERO, |at| at.elapsed());
            let label = if suite.total > 1 {
                format!(
                    "{} ({})",
                    task.suite(),
                    counted(suite.total, "task", "tasks")
                )
            } else {
                task.suite().into()
            };
            self.row(&label, outcome, elapsed);
        }
    }

    fn row(&mut self, name: &str, outcome: Outcome, elapsed: Duration) {
        self.clear();
        let mark = match outcome {
            Outcome::Passed => self.style.green("✓"),
            Outcome::Failed => self.style.red("✗"),
            Outcome::Cancelled | Outcome::Skipped => self.style.dim("○"),
        };
        eprintln!(
            "  {mark} {}  {}",
            sanitize_text(name),
            self.style.dim(&format!("{:.2}s", elapsed.as_secs_f64()))
        );
    }

    pub fn progress<'a>(&mut self, active: impl Iterator<Item = &'a Task>, stopping: bool) {
        let dynamic = self.policy.interactive && !self.verbose;
        let interval = if dynamic {
            Duration::from_millis(if self.policy.motion { 100 } else { 1000 })
        } else {
            Duration::from_secs(10)
        };
        if self
            .refreshed
            .map_or(self.started.elapsed(), |at| at.elapsed())
            < interval
        {
            return;
        }
        self.refreshed = Some(Instant::now());
        let mut names = Vec::new();
        for task in active {
            if !names.contains(&task.suite()) {
                names.push(task.suite());
            }
        }
        let status = if stopping { "stopping" } else { "running" };
        let line = format!(
            "{}/{} · {status} {} · {:.1}s",
            self.done,
            self.total,
            names.join(" + "),
            self.started.elapsed().as_secs_f64()
        );
        let mark = if dynamic && self.policy.motion {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[(self.started.elapsed().as_millis() / 100 % 10) as usize]
        } else {
            "…"
        };
        self.clear();
        if dynamic {
            let room = workstation::terminal_width()
                .unwrap_or(80)
                .saturating_sub(5);
            let mut stderr = io::stderr().lock();
            let _ = write!(
                stderr,
                "  {} {}",
                self.style.teal(mark),
                self.style.dim(&truncate_back(&line, room))
            );
            let _ = stderr.flush();
            self.visible = true;
        } else {
            eprintln!("  {} {}", self.style.teal(mark), self.style.dim(&line));
        }
    }

    pub fn finish(
        &mut self,
        passed: usize,
        cancelled: usize,
        skipped: usize,
        failures: &[Failure],
    ) {
        self.clear();
        let mut counts = Vec::new();
        if passed > 0 {
            counts.push(self.style.green(&format!("{passed} passed")));
        }
        if !failures.is_empty() {
            counts.push(self.style.red(&format!("{} failed", failures.len())));
        }
        if cancelled > 0 {
            counts.push(self.style.dim(&format!("{cancelled} cancelled")));
        }
        if skipped > 0 {
            counts.push(self.style.dim(&format!("{skipped} skipped")));
        }
        eprintln!(
            "\n{}  {}",
            counts.join(", "),
            self.style
                .dim(&format!("{:.2}s", self.started.elapsed().as_secs_f64()))
        );
        for failure in failures {
            eprintln!(
                "\n{}",
                self.style.red(&format!(
                    "{}: exit {}",
                    sanitize_text(&failure.name),
                    failure.code
                ))
            );
            if !self.verbose || failure.log.is_none() {
                for line in failure.detail.lines() {
                    eprintln!("  {}", sanitize_text(line));
                }
            }
            if let Some(path) = &failure.log {
                eprintln!("  log: {}", sanitize_text(&path.display().to_string()));
            }
        }
    }

    fn clear(&mut self) {
        if self.visible {
            let mut stderr = io::stderr().lock();
            let _ = crossterm::execute!(stderr, MoveToColumn(0), Clear(ClearType::CurrentLine));
            self.visible = false;
        }
    }
}

impl Drop for Report {
    fn drop(&mut self) {
        self.clear();
    }
}
