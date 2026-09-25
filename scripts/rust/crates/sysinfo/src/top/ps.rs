//! macOS hides other users' processes from unprivileged readers; the setuid
//! `ps` still reports them, so two of its readings inside the window fill those rows.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const FIELDS: &str = "pid=,ppid=,uid=,time=,rss=,etime=,comm=";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub parent: u32,
    pub uid: u32,
    pub cpu_ms: f64,
    pub memory: u64,
    pub age: u64,
    pub exe: String,
}

#[derive(Clone, Debug)]
pub struct Reading {
    pub at: Instant,
    pub took: Duration,
    pub entries: HashMap<u32, Entry>,
}

pub fn parse(body: &str) -> HashMap<u32, Entry> {
    body.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<(u32, Entry)> {
    let mut rest = line.trim_start();
    let mut fields = [""; 6];
    for field in &mut fields {
        let end = rest.find(char::is_whitespace)?;
        *field = &rest[..end];
        rest = rest[end..].trim_start();
    }
    let [pid, parent, uid, time, rss, elapsed] = fields;
    Some((
        pid.parse().ok()?,
        Entry {
            parent: parent.parse().ok()?,
            uid: uid.parse().ok()?,
            cpu_ms: clock(time)? * 1000.0,
            memory: rss.parse::<u64>().ok()? * 1024,
            age: clock(elapsed)? as u64,
            exe: rest.trim_end().into(),
        },
    ))
}

/// Seconds in a `[dd-][hh:]mm:ss[.cc]` clock.
pub fn clock(text: &str) -> Option<f64> {
    let (days, time) = match text.split_once('-') {
        Some((days, time)) => (days.parse::<f64>().ok()?, time),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    let mut parts = time.split(':').peekable();
    while let Some(part) = parts.next() {
        let value = part.parse::<f64>().ok()?;
        seconds = if parts.peek().is_some() {
            (seconds + value) * 60.0
        } else {
            seconds + value
        };
    }
    Some(days * 86_400.0 + seconds)
}

/// Entries of `after`, with CPU time since `before` rescaled to `window`.
pub fn deltas(before: &Reading, after: &Reading, window: Duration) -> HashMap<u32, Entry> {
    let elapsed = after.at.saturating_duration_since(before.at).as_secs_f64();
    let scale = if elapsed > 0.0 {
        window.as_secs_f64() / elapsed
    } else {
        1.0
    };
    after
        .entries
        .iter()
        .map(|(pid, entry)| {
            let spent = before
                .entries
                .get(pid)
                .map_or(entry.cpu_ms, |earlier| entry.cpu_ms - earlier.cpu_ms);
            let entry = Entry {
                cpu_ms: spent.max(0.0) * scale,
                ..entry.clone()
            };
            (*pid, entry)
        })
        .collect()
}

/// One reading of `pids`; `None` off macOS or when none of them is alive.
#[cfg(target_os = "macos")]
pub fn read(pids: &[u32]) -> Option<Reading> {
    if pids.is_empty() {
        return None;
    }
    let pids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let began = Instant::now();
    let body = crate::collect::probe(
        std::process::Command::new("/bin/ps").args(["-ww", "-o", FIELDS, "-p", &pids]),
        Duration::from_secs(2),
    )
    .ok()?;
    let took = began.elapsed();
    Some(Reading {
        at: began + took / 2,
        took,
        entries: parse(&body),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn read(_pids: &[u32]) -> Option<Reading> {
    None
}

#[cfg(test)]
#[path = "../../tests/unit/top/ps.rs"]
mod tests;
