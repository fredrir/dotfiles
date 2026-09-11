use super::capture;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use sysinfo::model::Snapshot;
pub fn on_battery(snapshot: &Snapshot) -> bool {
    let Some(batteries) = snapshot
        .result("Battery")
        .as_array()
        .filter(|b| !b.is_empty())
    else {
        return false;
    };
    if snapshot
        .result("PowerAdapter")
        .as_array()
        .is_some_and(|p| !p.is_empty())
    {
        return false;
    }
    !batteries.iter().any(|battery| {
        let status = sysinfo::formatting::text(&battery["status"]).to_lowercase();
        status.starts_with("charging")
            || ["ac connected", "connected", "full", "fully charged"].contains(&status.as_str())
    })
}
pub fn throttled(snapshot: &Snapshot) -> bool {
    let cpu = snapshot.result("CPU");
    cpu["temperature"]
        .as_f64()
        .zip(sysinfo::health::cpu_temperature_limit(
            cpu["cpu"].as_str().unwrap_or(""),
        ))
        .is_some_and(|(temperature, limit)| temperature >= limit - 5.0)
}
pub fn nvidia_throttled() -> bool {
    capture::probe(
        Command::new("nvidia-smi").args([
            "--query-gpu=clocks_throttle_reasons.hw_thermal_slowdown,clocks_throttle_reasons.sw_thermal_slowdown",
            "--format=csv,noheader",
        ]),
        3,
    ).is_some_and(|result| {
        result.status.success() && String::from_utf8_lossy(&result.stdout)
            .split([',', '\n']).any(|field| field.trim().eq_ignore_ascii_case("active"))
    })
}
pub fn capture_conditions(snapshot: &Snapshot, workdir: &Path) -> Value {
    let memory = snapshot.result("Memory");
    let total = memory["total"].as_u64().unwrap_or(0);
    let used = memory["used"].as_u64().unwrap_or(0);
    let free = fs2::available_space(workdir).ok();
    let disk_total = fs2::total_space(workdir).ok();
    json!({
        "on_battery": on_battery(snapshot),
        "governor": fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").unwrap_or_default().trim(),
        "loadavg_1": sysinfo_backend::System::load_average().one,
        "cpu_count": std::thread::available_parallelism().map_or(1, usize::from),
        "idle_temp_c": snapshot.result("CPU")["temperature"],
        "free_ram_bytes": total.saturating_sub(used),
        "free_disk_bytes": free,
        "free_disk_ratio": free.zip(disk_total).filter(|(_, total)| *total > 0).map(|(free, total)| free as f64 / total as f64),
        "throttled_at_start": throttled(snapshot) || nvidia_throttled(),
        "virtualized": capture::detect_virtualized(),
        "platform": capture::platform(),
    })
}
pub fn gate_reasons(conditions: &Value, writes_disk: bool) -> Vec<String> {
    let mut reasons = Vec::new();
    if conditions["on_battery"] == true {
        reasons.push("running on battery".into());
    }
    let count = conditions["cpu_count"]
        .as_f64()
        .filter(|n| *n > 0.0)
        .unwrap_or(1.0);
    if let Some(load) = conditions["loadavg_1"].as_f64()
        && load / count > 0.30
    {
        reasons.push(format!(
            "load average is {load:.2} across {count:.0} logical cores"
        ));
    }
    if conditions["throttled_at_start"] == true {
        reasons.push("the machine is already thermally throttled".into());
    }
    if writes_disk
        && let Some(ratio) = conditions["free_disk_ratio"].as_f64()
        && ratio < 0.20
    {
        reasons.push(format!(
            "only {:.0}% of the filesystem is free",
            ratio * 100.0
        ));
    }
    let fstype = conditions["filesystem"]["fstype"].as_str().unwrap_or("");
    if writes_disk && ["tmpfs", "ramfs", "devtmpfs", "hugetlbfs"].contains(&fstype) {
        reasons.push(format!(
            "the work directory is on {fstype}, which measures memory and not disk"
        ));
    }
    reasons
}
pub fn grade_for(reasons: &[String], metrics: usize, failures: &[String]) -> &'static str {
    if metrics == 0 {
        "aborted"
    } else if !reasons.is_empty() || !failures.is_empty() {
        "noisy"
    } else {
        "clean"
    }
}
