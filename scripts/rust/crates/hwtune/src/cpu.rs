use std::collections::BTreeSet;
use std::fs;

use crate::env::{self, Sysfs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpufreq {
    pub boost: Option<bool>,
    pub governor: String,
    pub epp: String,
    pub max_khz: u64,
}

pub fn cpufreq(sys: &Sysfs) -> Result<Cpufreq, String> {
    let cpu0 = sys.sys.join("devices/system/cpu/cpu0/cpufreq");
    let boost = env::read_text(&sys.sys.join("devices/system/cpu/cpufreq/boost"))
        .ok()
        .map(|value| value == "1");
    Ok(Cpufreq {
        boost,
        governor: env::read_text(&cpu0.join("scaling_governor")).unwrap_or_else(|_| "?".into()),
        epp: env::read_text(&cpu0.join("energy_performance_preference"))
            .unwrap_or_else(|_| "?".into()),
        max_khz: env::read_number(&cpu0.join("cpuinfo_max_freq"))?,
    })
}

fn cpu_dirs(sys: &Sysfs) -> Result<Vec<(u32, std::path::PathBuf)>, String> {
    let root = sys.sys.join("devices/system/cpu");
    let entries = fs::read_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut cpus = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = name.to_str()?.strip_prefix("cpu")?.parse::<u32>().ok()?;
            Some((id, entry.path()))
        })
        .collect::<Vec<_>>();
    cpus.sort();
    Ok(cpus)
}

pub fn logical_count(sys: &Sysfs) -> Result<usize, String> {
    Ok(cpu_dirs(sys)?.len())
}

pub fn first_sibling(list: &str) -> Option<u32> {
    list.split([',', '-'])
        .next()
        .and_then(|first| first.trim().parse().ok())
}

pub fn physical_cores(sys: &Sysfs) -> Result<Vec<u32>, String> {
    let mut cores = BTreeSet::new();
    for (id, dir) in cpu_dirs(sys)? {
        let siblings = env::read_text(&dir.join("topology/thread_siblings_list"))
            .ok()
            .and_then(|list| first_sibling(&list))
            .unwrap_or(id);
        cores.insert(siblings);
    }
    Ok(cores.into_iter().collect())
}

pub fn prefcore_ranking(sys: &Sysfs, cores: &[u32]) -> Vec<(u32, Option<u64>)> {
    cores
        .iter()
        .map(|core| {
            let path = sys.sys.join(format!(
                "devices/system/cpu/cpu{core}/cpufreq/amd_pstate_prefcore_ranking"
            ));
            (*core, env::read_number(&path).ok())
        })
        .collect()
}

pub fn parse_cores(text: &str) -> Result<Vec<u32>, String> {
    let mut cores = BTreeSet::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.split_once('-') {
            Some((start, end)) => {
                let start: u32 = start.trim().parse().map_err(|_| bad_cores(text))?;
                let end: u32 = end.trim().parse().map_err(|_| bad_cores(text))?;
                if end < start {
                    return Err(bad_cores(text));
                }
                cores.extend(start..=end);
            }
            None => {
                cores.insert(part.parse().map_err(|_| bad_cores(text))?);
            }
        }
    }
    if cores.is_empty() {
        return Err(bad_cores(text));
    }
    Ok(cores.into_iter().collect())
}

fn bad_cores(text: &str) -> String {
    format!("invalid core list: {text}")
}

#[cfg(test)]
#[path = "../tests/unit/cpu_tests.rs"]
mod tests;
