use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use super::Options;
use super::plan::Task;

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
    log: std::fs::File,
    position: u64,
    started: Instant,
    last_activity: Instant,
    interrupted: Option<Instant>,
}

#[cfg(unix)]
impl Running {
    fn start(task: Task) -> io::Result<Self> {
        let log = tempfile::tempfile()?;
        let child = hostkit::process::ChildGroup::spawn_detached(
            task.command()
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log.try_clone()?),
        )?;
        Ok(Self {
            task,
            child,
            log,
            position: 0,
            started: Instant::now(),
            last_activity: Instant::now(),
            interrupted: None,
        })
    }

    fn drain(&mut self, owner: &mut Option<String>) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;

        let mut buffer = [0_u8; 8192];
        let mut total = 0;
        let mut stdout = io::stdout().lock();
        for _ in 0..8 {
            let count = match self.log.read_at(&mut buffer, self.position) {
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
            self.last_activity = Instant::now();
        }
        Ok(total)
    }
}

#[cfg(unix)]
pub(super) fn run(tasks: Vec<Task>, budget: Budget) -> Result<ExitCode, String> {
    use std::os::unix::process::ExitStatusExt;

    let started = Instant::now();
    let total = tasks.len();
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
                let name = task.name.clone();
                eprintln!("[run] {name}");
                output_owner = None;
                match Running::start(task) {
                    Ok(task) => running.push(task),
                    Err(error) => {
                        eprintln!("[fail] {name} (0.00s): {error}");
                        failures.push((name, 127));
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
            running[index]
                .drain(&mut output_owner)
                .map_err(|error| error.to_string())?;
            let status = running[index]
                .child
                .try_wait()
                .map_err(|error| error.to_string())?;
            let Some(status) = status else {
                if running[index].last_activity.elapsed() >= Duration::from_secs(10) {
                    eprintln!(
                        "[running] {} ({:.1}s)",
                        running[index].task.name,
                        running[index].started.elapsed().as_secs_f64()
                    );
                    running[index].last_activity = Instant::now();
                    output_owner = None;
                }
                index += 1;
                continue;
            };
            let mut finished = running.remove(index);
            finished.child.terminate();
            while finished
                .drain(&mut output_owner)
                .map_err(|error| error.to_string())?
                > 0
            {}
            let code = status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
            let outcome = if finished.interrupted.is_some() {
                cancelled += 1;
                "cancel"
            } else if status.success() {
                passed += 1;
                "pass"
            } else {
                failures.push((finished.task.name.clone(), code));
                "fail"
            };
            eprintln!(
                "[{outcome}] {} ({:.2}s, exit {code})",
                finished.task.name,
                finished.started.elapsed().as_secs_f64()
            );
            output_owner = None;
        }
        if running.is_empty() && pending.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!(
        "{passed} passed, {} failed, {cancelled} cancelled / {total} tasks ({:.2}s)",
        failures.len(),
        started.elapsed().as_secs_f64()
    );
    for (name, code) in &failures {
        eprintln!("  {name}: exit {code}");
    }
    let signal = workstation::screen::termination_signal();
    Ok(workstation::exit_code(if signal != 0 {
        128 + signal
    } else {
        failures.first().map_or(0, |(_, code)| *code)
    }))
}

#[cfg(not(unix))]
pub(super) fn run(_tasks: Vec<Task>, _budget: Budget) -> Result<ExitCode, String> {
    Err("development tasks require Unix".into())
}
