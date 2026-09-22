//! Per-core CPU load sampled across the collection window.
//!
//! Two tick readings bracket the collectors, so the sample needs no subprocess
//! and no sleep: the window is work the snapshot already does. Counters advance
//! at 100 Hz, so a short window reads coarsely, and the snapshot's own CPU is
//! subtracted to keep the collection out of the load it reports.

use serde_json::{Value, json};

/// Busy and total ticks per logical core at one point in time.
pub type Ticks = Vec<[u64; 2]>;

/// One tick reading of the host and of this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub cores: Ticks,
    /// Ticks this process consumed, at the same rate as `cores`.
    pub own: u64,
}

pub struct Sampler {
    first: Reading,
}

impl Sampler {
    /// First reading; `None` when this platform cannot report CPU ticks.
    pub fn start() -> Option<Self> {
        Some(Self { first: read()? })
    }
    /// Percent busy per logical core over the window since [`Sampler::start`].
    pub fn usage(self) -> Option<Value> {
        Some(json!(percentages(&self.first, &read()?)))
    }
}

/// Turn two readings into percent busy per core, matched by core order.
pub fn percentages(before: &Reading, after: &Reading) -> Vec<f64> {
    let cores = before.cores.len().max(1) as f64;
    // Collecting is work too; spread the sampler's own ticks over the cores so
    // a short window never reports the snapshot as system load.
    let own = after.own.saturating_sub(before.own) as f64 / cores;
    before
        .cores
        .iter()
        .zip(&after.cores)
        .map(|(before, after)| {
            let total = after[1].saturating_sub(before[1]);
            // Counters only advance; saturate so a wrap or a core that went
            // offline cannot report negative or over-committed load.
            let busy = after[0].saturating_sub(before[0]).min(total);
            if total == 0 {
                0.0
            } else {
                (busy as f64 - own).max(0.0) * 100.0 / total as f64
            }
        })
        .collect()
}

/// Parse the per-core `cpu<N>` tick counters of `/proc/stat`.
#[cfg(any(target_os = "linux", test))]
pub fn parse_proc_stat(body: &str) -> Ticks {
    body.lines()
        .filter_map(|line| {
            let (core, counters) = line.split_once(char::is_whitespace)?;
            if core.strip_prefix("cpu")?.parse::<usize>().is_err() {
                return None;
            }
            let mut ticks = counters
                .split_whitespace()
                .map(|value| value.parse().unwrap_or(0));
            // guest and guest_nice are already counted in user and nice.
            let (user, nice, system, idle, wait, irq, softirq, steal) = (
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
                ticks.next()?,
            );
            let idle = idle + wait;
            let busy = user + nice + system + irq + softirq + steal;
            Some([busy, busy + idle])
        })
        .collect()
}

/// Parse this snapshot's own CPU ticks out of `/proc/self/stat`.
///
/// Covers the process and the probes it waited for, so the load report keeps
/// its own work out of the window it measures.
#[cfg(any(target_os = "linux", test))]
pub fn parse_own_ticks(body: &str) -> u64 {
    // The command name may hold spaces and parentheses, so count from its end.
    let rest = body.rsplit_once(')').map(|(_, rest)| rest).unwrap_or(body);
    let fields: Vec<u64> = rest
        .split_whitespace()
        .map(|value| value.parse().unwrap_or(0))
        .collect();
    // Field 3 (state) is first here, so utime and stime are indexes 11 and 12
    // and the waited-for children are 13 and 14.
    [11, 12, 13, 14]
        .into_iter()
        .map(|index| fields.get(index).copied().unwrap_or(0))
        .sum()
}

fn read() -> Option<Reading> {
    platform().filter(|reading| !reading.cores.is_empty())
}

#[cfg(target_os = "macos")]
fn platform() -> Option<Reading> {
    Some(Reading {
        cores: super::macos::cpu_ticks()?,
        own: super::macos::own_cpu_ticks().unwrap_or(0),
    })
}

#[cfg(target_os = "linux")]
fn platform() -> Option<Reading> {
    Some(Reading {
        cores: parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?),
        own: parse_own_ticks(&std::fs::read_to_string("/proc/self/stat").ok()?),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform() -> Option<Reading> {
    None
}

#[cfg(test)]
#[path = "../../tests/unit/collect/cpu.rs"]
mod tests;
