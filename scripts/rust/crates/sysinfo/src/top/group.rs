//! Rows: one per app by default, one per process when split.
//!
//! A process joins its parent when both are the same app, and siblings of the
//! same app and user share a row, so helpers and workers fold into their app.

use super::{Process, Row, Sample, Sort};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Identity {
    App(String),
    Exe(String),
    Name(String),
}

impl Identity {
    pub fn of(process: &Process) -> Self {
        match process.exe.as_deref().filter(|exe| !exe.is_empty()) {
            Some(exe) => {
                bundle(exe).map_or_else(|| Self::Exe(exe.into()), |app| Self::App(app.into()))
            }
            None => Self::Name(process.name.clone()),
        }
    }
}

/// Outermost `.app` bundle holding `exe`, so nested helper apps count as their host app.
pub fn bundle(exe: &str) -> Option<&str> {
    let end = exe.find(".app/")? + ".app".len();
    Some(&exe[..end])
}

pub fn rows(sample: &Sample, split: bool) -> Vec<Row> {
    let members = if split {
        sample
            .processes
            .iter()
            .map(|process| vec![process])
            .collect()
    } else {
        groups(&sample.processes)
    };
    members.iter().map(|members| row(sample, members)).collect()
}

/// Parent of the chain's root, the app, and the owner.
type Key<'a> = (Option<u32>, &'a Identity, Option<u32>);

pub fn groups(processes: &[Process]) -> Vec<Vec<&Process>> {
    let by_pid: HashMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    let identities: HashMap<u32, Identity> =
        processes.iter().map(|p| (p.pid, Identity::of(p))).collect();
    let mut grouped: HashMap<Key, Vec<&Process>> = HashMap::new();
    for process in processes {
        let root = root(process, &by_pid, &identities);
        grouped
            .entry((root.parent, &identities[&root.pid], root.uid))
            .or_default()
            .push(process);
    }
    grouped.into_values().collect()
}

fn root<'a>(
    process: &'a Process,
    by_pid: &HashMap<u32, &'a Process>,
    identities: &HashMap<u32, Identity>,
) -> &'a Process {
    // Bounded so reused pids that form a parent cycle still terminate.
    const DEPTH: usize = 64;
    let identity = &identities[&process.pid];
    let mut current = process;
    for _ in 0..DEPTH {
        match current.parent.and_then(|pid| by_pid.get(&pid)) {
            Some(parent) if parent.pid != current.pid && &identities[&parent.pid] == identity => {
                current = parent;
            }
            _ => break,
        }
    }
    current
}

fn row(sample: &Sample, members: &[&Process]) -> Row {
    let Some(oldest) = members
        .iter()
        .copied()
        .max_by(|a, b| a.age.cmp(&b.age).then(b.pid.cmp(&a.pid)))
    else {
        return Row::default();
    };
    let window_ms = (sample.window.as_secs_f64() * 1000.0).max(1.0);
    let cores = members.iter().map(|p| p.cpu_ms).sum::<f64>() / window_ms;
    let memory: u64 = members.iter().map(|p| p.memory).sum();
    let gpu = members
        .iter()
        .filter_map(|p| p.gpu)
        .reduce(|a, b| a + b)
        .map(|share| share.min(100.0));
    Row {
        pid: oldest.pid,
        uid: oldest.uid,
        user: String::new(),
        cpu: (cores * 100.0 / sample.cores.max(1) as f64).min(100.0),
        cores,
        memory,
        memory_share: memory as f64 * 100.0 / sample.memory.max(1) as f64,
        gpu,
        age: oldest.age,
        command: if members.len() == 1 {
            command_line(oldest)
        } else {
            label(oldest)
        },
        count: members.len(),
    }
}

/// Group name: the app bundle, else the program.
pub fn label(process: &Process) -> String {
    match Identity::of(process) {
        Identity::App(app) => Path::new(&app)
            .file_stem()
            .map_or(app.clone(), |stem| stem.to_string_lossy().into_owned()),
        _ => program(process).0,
    }
}

/// Program name and any arguments folded into `argv[0]`.
fn program(process: &Process) -> (String, String) {
    let Some(first) = process.command.first() else {
        return (bare_name(process), String::new());
    };
    if let Some(exe) = process.exe.as_deref()
        && let Some(tail) = first.strip_prefix(exe)
        && (tail.is_empty() || tail.starts_with(' '))
    {
        return (file_name(exe), tail.trim_start().into());
    }
    if first.contains('/') && !first.contains(' ') {
        return (file_name(first), String::new());
    }
    (first.clone(), String::new())
}

pub fn command_line(process: &Process) -> String {
    let (program, tail) = program(process);
    [program, tail]
        .into_iter()
        .chain(process.command.iter().skip(1).cloned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn bare_name(process: &Process) -> String {
    if process.kernel {
        format!("[{}]", process.name)
    } else {
        process.name.clone()
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.into(), |name| name.to_string_lossy().into_owned())
}

pub fn rank(rows: &mut [Row], sort: Sort) {
    rows.sort_by(|a, b| {
        b.share(sort)
            .total_cmp(&a.share(sort))
            .then(b.share(Sort::Total).total_cmp(&a.share(Sort::Total)))
            .then(b.cpu.total_cmp(&a.cpu))
            .then(b.memory.cmp(&a.memory))
            .then(a.pid.cmp(&b.pid))
    });
}

#[cfg(test)]
#[path = "../../tests/unit/top/group.rs"]
mod tests;
