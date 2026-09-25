//! Per-process GPU share: IOKit client GPU time on macOS, NVML samples on Linux.

use std::collections::HashMap;
use std::time::Instant;

/// Percent of all GPUs per pid; `None` when this host reports no per-process GPU use.
pub type Shares = Option<HashMap<u32, f64>>;

#[cfg(any(target_os = "macos", test))]
/// GPU time turned into a share of the elapsed window, capped at one full GPU.
pub fn shares(
    before: &HashMap<u32, u64>,
    after: &HashMap<u32, u64>,
    elapsed_ns: u128,
) -> HashMap<u32, f64> {
    let elapsed = elapsed_ns.max(1) as f64;
    after
        .iter()
        .map(|(pid, time)| {
            let used = time.saturating_sub(before.get(pid).copied().unwrap_or(0));
            (*pid, (used as f64 * 100.0 / elapsed).min(100.0))
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
/// Average utilization per pid over its samples, spread over `devices` GPUs.
pub fn average(samples: impl IntoIterator<Item = (u32, u32)>, devices: usize) -> HashMap<u32, f64> {
    let mut totals: HashMap<u32, (f64, usize)> = HashMap::new();
    for (pid, utilization) in samples {
        let entry = totals.entry(pid).or_default();
        entry.0 += f64::from(utilization);
        entry.1 += 1;
    }
    let devices = devices.max(1) as f64;
    totals
        .into_iter()
        .map(|(pid, (sum, count))| (pid, sum / count as f64 / devices))
        .collect()
}

#[cfg(target_os = "macos")]
pub struct Sampler {
    first: Option<(Instant, HashMap<u32, u64>)>,
}

#[cfg(target_os = "macos")]
impl Sampler {
    pub fn start(_deadline: Instant) -> Self {
        let first = crate::collect::macos::gpu_time_by_pid().map(|times| (Instant::now(), times));
        Self { first }
    }

    pub fn finish(self) -> Shares {
        let (started, before) = self.first?;
        let after = crate::collect::macos::gpu_time_by_pid()?;
        Some(shares(&before, &after, started.elapsed().as_nanos()))
    }
}

#[cfg(target_os = "linux")]
pub struct Sampler {
    worker: std::thread::JoinHandle<Shares>,
}

#[cfg(target_os = "linux")]
impl Sampler {
    /// NVML loads in the background; samples are read once the window closes.
    pub fn start(deadline: Instant) -> Self {
        Self {
            worker: std::thread::spawn(move || nvidia(deadline)),
        }
    }

    pub fn finish(self) -> Shares {
        self.worker.join().ok().flatten()
    }
}

#[cfg(target_os = "linux")]
fn nvidia(deadline: Instant) -> Shares {
    use nvml_wrapper::Nvml;
    use nvml_wrapper::error::NvmlError;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // The driver buffers samples; the last second covers several of its periods.
    const HISTORY: Duration = Duration::from_secs(1);

    let nvml = Nvml::init().ok()?;
    let devices = nvml.device_count().ok()?;
    if devices == 0 {
        return None;
    }
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .saturating_sub(HISTORY)
        .as_micros() as u64;
    let mut samples = Vec::new();
    for index in 0..devices {
        let Ok(device) = nvml.device_by_index(index) else {
            continue;
        };
        match device.process_utilization_stats(since) {
            Ok(found) => samples.extend(found.into_iter().map(|s| (s.pid, s.sm_util))),
            Err(NvmlError::NotFound) => {}
            Err(_) => return None,
        }
    }
    Some(average(samples, devices as usize))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub struct Sampler;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Sampler {
    pub fn start(_deadline: Instant) -> Self {
        Self
    }

    pub fn finish(self) -> Shares {
        None
    }
}

#[cfg(test)]
#[path = "../../tests/unit/top/gpu.rs"]
mod tests;
