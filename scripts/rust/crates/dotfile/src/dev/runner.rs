use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use super::Options;
use super::plan::Task;
use super::report::{Failure, Outcome, Report};

pub(super) struct Budget {
    slots: usize,
    pub workers: usize,
}

impl Budget {
    pub fn new(options: &Options, tasks: &[Task]) -> Self {
        let jobs = options
            .jobs
            .map(usize::from)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from));
        let independent = tasks.iter().filter(|task| !task.cargo).count();
        let lanes = independent + usize::from(tasks.iter().any(|task| task.cargo));
        let slots = usize::from(options.concurrency).min(jobs).min(lanes.max(1));
        Self {
            slots,
            workers: (jobs / slots).max(1),
        }
    }
}

#[cfg(unix)]
struct Running {
    task: Task,
    child: hostkit::process::ChildGroup,
    log: tempfile::NamedTempFile,
    position: u64,
    started: Instant,
    interrupted: Option<Instant>,
}

#[cfg(unix)]
impl Running {
    fn start(task: Task) -> Result<Self, (Task, io::Error)> {
        let spawn = || -> io::Result<_> {
            let log = tempfile::Builder::new()
                .prefix("dotfile-dev-")
                .suffix(".log")
                .tempfile()?;
            let child = hostkit::process::ChildGroup::spawn_detached(
                task.command()
                    .stdin(Stdio::null())
                    .stdout(log.as_file().try_clone()?)
                    .stderr(log.as_file().try_clone()?),
            )?;
            Ok((child, log))
        };
        match spawn() {
            Ok((child, log)) => Ok(Self {
                task,
                child,
                log,
                position: 0,
                started: Instant::now(),
                interrupted: None,
            }),
            Err(error) => Err((task, error)),
        }
    }

    fn drain(&mut self, owner: &mut Option<String>) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;

        let mut buffer = [0_u8; 8192];
        let mut total = 0;
        let mut stdout = io::stdout().lock();
        for _ in 0..8 {
            let count = match self.log.as_file().read_at(&mut buffer, self.position) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if owner.as_deref() != Some(&self.task.name) {
                writeln!(stdout, "\n[{}]", self.task.name)?;
                *owner = Some(self.task.name.clone());
            }
            stdout.write_all(&buffer[..count])?;
            self.position += count as u64;
            total += count;
        }
        if total > 0 {
            stdout.flush()?;
        }
        Ok(total)
    }
}

#[cfg(unix)]
pub(super) fn run(
    tasks: Vec<Task>,
    budget: Budget,
    action: &str,
    verbose: bool,
) -> Result<ExitCode, String> {
    use std::os::unix::process::ExitStatusExt;

    let total = tasks.len();
    let mut report = Report::new(action, &tasks, verbose);
    let mut pending = VecDeque::from(tasks);
    let mut running: Vec<Running> = Vec::with_capacity(budget.slots.min(total));
    let mut failures = Vec::new();
    let mut passed = 0;
    let mut cancelled = 0;
    let mut output_owner = None;
    loop {
        let signal = workstation::screen::termination_signal();
        if signal == 0 {
            while running.len() < budget.slots {
                let cargo_busy = running.iter().any(|task| task.task.cargo);
                let Some(index) = pending.iter().position(|task| !task.cargo || !cargo_busy) else {
                    break;
                };
                let task = pending.remove(index).unwrap();
                report.start(&task);
                output_owner = None;
                match Running::start(task) {
                    Ok(task) => running.push(task),
                    Err((task, error)) => {
                        report.complete(&task, Outcome::Failed, Duration::ZERO);
                        failures.push(Failure {
                            name: task.name,
                            code: 127,
                            detail: format!("{}: {error}", task.program),
                            log: None,
                        });
                    }
                }
                if workstation::screen::termination_requested() {
                    break;
                }
            }
        } else {
            cancelled += pending.len();
            pending.clear();
            for task in &mut running {
                match task.interrupted {
                    None => {
                        let _ = task.child.signal(signal);
                        task.interrupted = Some(Instant::now());
                    }
                    Some(at) if at.elapsed() >= Duration::from_millis(750) => {
                        task.child.terminate()
                    }
                    Some(_) => {}
                }
            }
        }
        let mut index = 0;
        while index < running.len() {
            if verbose {
                running[index]
                    .drain(&mut output_owner)
                    .map_err(|error| error.to_string())?;
            }
            let status = running[index]
                .child
                .try_wait()
                .map_err(|error| error.to_string())?;
            let Some(status) = status else {
                index += 1;
                continue;
            };
            let mut finished = running.remove(index);
            finished.child.terminate();
            if verbose {
                while finished
                    .drain(&mut output_owner)
                    .map_err(|error| error.to_string())?
                    > 0
                {}
            }
            let code = status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
            let outcome = if finished.interrupted.is_some() {
                cancelled += 1;
                Outcome::Cancelled
            } else if status.success() {
                passed += 1;
                Outcome::Passed
            } else {
                Outcome::Failed
            };
            report.complete(&finished.task, outcome, finished.started.elapsed());
            if outcome == Outcome::Failed {
                failures.push(Failure::capture(finished.task.name, code, finished.log));
            }
            output_owner = None;
        }
        if running.is_empty() && pending.is_empty() {
            break;
        }
        if !running.is_empty() {
            report.progress(running.iter().map(|task| &task.task), signal != 0);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    report.finish(passed, cancelled, &failures);
    let signal = workstation::screen::termination_signal();
    Ok(workstation::exit_code(if signal != 0 {
        128 + signal
    } else {
        failures.first().map_or(0, |failure| failure.code)
    }))
}

#[cfg(not(unix))]
pub(super) fn run(
    _tasks: Vec<Task>,
    _budget: Budget,
    _action: &str,
    _verbose: bool,
) -> Result<ExitCode, String> {
    Err("development tasks require Unix".into())
}
