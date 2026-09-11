use crate::formatting::*;
use crate::model::{HealthIssue, Severity, Snapshot};
pub fn cpu_temperature_limit(name: &str) -> Option<f64> {
    name.to_lowercase()
        .contains("ryzen 7 9800x3d")
        .then_some(95.0)
}
pub fn temperature_issue(
    name: &str,
    temperature: Option<f64>,
    maximum: Option<f64>,
) -> Option<HealthIssue> {
    let (temperature, maximum) = (temperature.filter(|v| v.is_finite())?, maximum?);
    let (severity, title, action) = if temperature >= maximum {
        (
            Severity::Error,
            format!("{name} temperature is above its limit"),
            "Reduce load and verify cooling before continuing sustained work",
        )
    } else if temperature >= maximum * 0.85 {
        (
            Severity::Warning,
            format!("{name} is running warm"),
            "Check airflow and background load if the temperature continues rising",
        )
    } else {
        return None;
    };
    Some(HealthIssue {
        severity,
        title,
        detail: format!(
            "{} measured, {maximum}°C maximum",
            format_temperature(Some(temperature))
        ),
        action: action.into(),
    })
}
fn issue(
    severity: Severity,
    title: impl Into<String>,
    detail: impl Into<String>,
    action: &str,
) -> HealthIssue {
    HealthIssue {
        severity,
        title: title.into(),
        detail: detail.into(),
        action: action.into(),
    }
}
pub fn health_issues(snapshot: &Snapshot) -> Vec<HealthIssue> {
    use Severity::{Error, Warning};
    let mut issues = snapshot
        .probe_errors
        .iter()
        .map(|message| {
            if message.to_lowercase().contains("kernel driver") {
                issue(
                    Error,
                    message.clone(),
                    "NVIDIA telemetry cannot start while the versions differ",
                    "Reboot to load the updated NVIDIA kernel module",
                )
            } else {
                issue(Warning, message.clone(), "", "")
            }
        })
        .collect::<Vec<_>>();
    let memory = snapshot.result("Memory");
    let detected = number(&memory["total"]);
    let configured = configured_memory_bytes(snapshot.configured("memory"));
    if configured > 0.0 && detected > 0.0 && detected < configured * 0.8 {
        issues.push(issue(
            Warning,
            "Installed memory is not fully visible",
            format!(
                "Configured as {}, detected as {}",
                format_bytes(configured),
                memory_capacity(detected)
            ),
            "Check firmware memory training and reseat the DIMMs if this persists",
        ));
    }
    let memory_use = percentage(number(&memory["used"]), detected);
    if memory_use >= 90.0 && list(snapshot.result("Swap")).is_empty() && !snapshot.is_macos() {
        issues.push(issue(
            Error,
            "Memory pressure has no swap fallback",
            format!("Memory is {memory_use:.0}% used and swap is disabled"),
            "Reduce memory use before starting another heavy workload",
        ));
    }
    let cpu = snapshot.result("CPU");
    let cpu_name = if string(cpu, "cpu").is_empty() {
        "CPU"
    } else {
        string(cpu, "cpu")
    };
    issues.extend(temperature_issue(
        cpu_name,
        cpu["temperature"].as_f64(),
        cpu_temperature_limit(cpu_name),
    ));
    for gpu in list(snapshot.result("GPU")) {
        let name = named_gpu(gpu);
        let nvidia = nvidia_for(snapshot, gpu);
        let limit = name
            .to_lowercase()
            .contains("geforce rtx 5070 ti")
            .then_some(88.0);
        issues.extend(temperature_issue(
            &name,
            nvidia["temperature"]
                .as_f64()
                .or_else(|| gpu["temperature"].as_f64()),
            limit,
        ));
        let (used, total) = gpu_memory(gpu, nvidia);
        let usage = percentage(used, total);
        if total > 0.0 && usage >= 90.0 {
            issues.push(issue(
                Warning,
                format!("{name} VRAM is nearly full"),
                format!("{usage:.0}% of VRAM is in use"),
                "Close GPU workloads or reduce their memory allocation",
            ));
        }
    }
    for disk in list(snapshot.result("Disk"))
        .iter()
        .filter(|d| is_actionable_filesystem(d))
    {
        let usage = percentage(
            number(&disk["bytes"]["used"]),
            number(&disk["bytes"]["total"]),
        );
        if usage >= 90.0 {
            let name = disk["mountpoint"]
                .as_str()
                .or_else(|| disk["name"].as_str())
                .unwrap_or("Disk");
            issues.push(issue(
                Warning,
                format!("{name} is nearly full"),
                format!("{usage:.0}% of the filesystem is used"),
                "Remove or relocate data before free space becomes critical",
            ));
        }
    }
    for disk in list(snapshot.result("PhysicalDisk"))
        .iter()
        .filter(|d| !is_virtual_disk(d))
    {
        let name = string(disk, "name");
        let lower = name.to_lowercase();
        let (family, limit) = if lower.contains("kingston snvs2000g") {
            ("Kingston NV1", Some(70.0))
        } else if lower.contains("wdc wd20ezrz") {
            ("WD Blue", Some(60.0))
        } else {
            (name, None)
        };
        issues.extend(temperature_issue(
            family,
            disk["temperature"].as_f64(),
            limit,
        ));
    }
    for battery in list(snapshot.result("Battery")) {
        let Some(capacity) = battery["capacity"].as_f64() else {
            continue;
        };
        let status = text(&battery["status"]).to_lowercase();
        let charging = status.starts_with("charging")
            || ["ac connected", "connected", "full", "fully charged"].contains(&status.as_str());
        if capacity <= 15.0 && !charging {
            issues.push(issue(
                if capacity <= 5.0 { Error } else { Warning },
                "Battery charge is low",
                format!("{capacity:.0}% remaining"),
                "Connect external power",
            ));
        }
    }
    issues
}
pub fn health_summary(issues: &[HealthIssue]) -> String {
    [Severity::Error, Severity::Warning]
        .into_iter()
        .filter_map(|severity| {
            let count = issues.iter().filter(|i| i.severity == severity).count();
            (count > 0).then(|| {
                format!(
                    "{count} {}{}",
                    severity.as_str(),
                    if count == 1 { "" } else { "s" }
                )
            })
        })
        .collect::<Vec<_>>()
        .join("  ")
}
