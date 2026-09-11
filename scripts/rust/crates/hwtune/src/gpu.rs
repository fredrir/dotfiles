use std::process::Command;
use std::time::Duration;

use hostkit::process::{self, CaptureLimits};

use crate::env::{self, Sysfs};

pub const QUERY: &str = "temperature.gpu,power.draw,power.limit,clocks.sm,clocks.mem,fan.speed";

#[derive(Debug, Clone, PartialEq)]
pub struct GpuStats {
    pub temp_c: f64,
    pub power_w: f64,
    pub power_cap_w: f64,
    pub sm_mhz: u32,
    pub mem_mhz: u32,
    pub fan_pct: u32,
}

pub fn parse_query(csv: &str) -> Result<GpuStats, String> {
    let line = csv
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or("nvidia-smi returned nothing")?;
    let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() < 6 {
        return Err(format!("nvidia-smi returned {line}"));
    }
    let number = |index: usize| -> Result<f64, String> {
        fields[index]
            .parse()
            .map_err(|_| format!("nvidia-smi field {index} is {}", fields[index]))
    };
    Ok(GpuStats {
        temp_c: number(0)?,
        power_w: number(1)?,
        power_cap_w: number(2)?,
        sm_mhz: number(3)? as u32,
        mem_mhz: number(4)? as u32,
        fan_pct: number(5)? as u32,
    })
}

pub fn query() -> Result<GpuStats, String> {
    let mut command = Command::new("nvidia-smi");
    command.args([
        &format!("--query-gpu={QUERY}"),
        "--format=csv,noheader,nounits",
    ]);
    let captured = process::output(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(10),
    )
    .map_err(|e| format!("nvidia-smi: {e}"))?;
    if !captured.status.success() {
        return Err(format!(
            "nvidia-smi: {}",
            String::from_utf8_lossy(&captured.stderr).trim()
        ));
    }
    parse_query(&String::from_utf8_lossy(&captured.stdout))
}

pub fn parse_resource(text: &str) -> Vec<u64> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let start = u64::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
            let end = u64::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
            (end > start).then(|| end - start + 1)
        })
        .collect()
}

pub fn bar_sizes(sys: &Sysfs, bdf: &str) -> Result<Vec<u64>, String> {
    let path = sys.sys.join("bus/pci/devices").join(bdf).join("resource");
    Ok(parse_resource(&env::read_text(&path)?))
}

#[cfg(test)]
#[path = "../tests/unit/gpu_tests.rs"]
mod tests;
