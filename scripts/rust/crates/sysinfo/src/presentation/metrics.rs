use crate::formatting::{
    gpu_memory, is_actionable_filesystem, is_virtual_disk, list, nvidia_for, number, percentage,
    string,
};
use crate::model::{DiskGauge, Gauge, Snapshot};
use serde_json::Value;
use std::collections::HashSet;

/// Compact CPU/GPU/RAM metrics for the default `-p` dashboard.
pub fn gauges(snapshot: &Snapshot) -> Vec<Gauge> {
    [cpu_gauge(snapshot), gpu_gauge(snapshot), memory_gauge(snapshot)]
        .into_iter()
        .flatten()
        .collect()
}

fn cpu_gauge(snapshot: &Snapshot) -> Option<Gauge> {
    let cpu = snapshot.result("CPU");
    if cpu.is_null() {
        return None;
    }
    let loads = list(snapshot.result("CPUUsage"))
        .iter()
        .filter_map(Value::as_f64)
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    let load = (!loads.is_empty()).then(|| loads.iter().sum::<f64>() / loads.len() as f64);
    let temperature = cpu["temperature"].as_f64().filter(|value| value.is_finite());
    Some(Gauge {
        kind: "cpu".into(),
        label: "CPU".into(),
        load,
        temperature,
        ..Gauge::default()
    })
}

fn gpu_gauge(snapshot: &Snapshot) -> Option<Gauge> {
    let gpus = list(snapshot.result("GPU"));
    let gpu = gpus
        .iter()
        .find(|gpu| gpu["type"] == "Discrete")
        .or_else(|| gpus.first())?;
    let nvidia = nvidia_for(snapshot, gpu);
    let (used, total) = gpu_memory(gpu, nvidia);
    let load = nvidia["utilization"]
        .as_f64()
        .or_else(|| gpu["coreUsage"].as_f64())
        .filter(|value| value.is_finite());
    let temperature = nvidia["temperature"]
        .as_f64()
        .or_else(|| gpu["temperature"].as_f64())
        .filter(|value| value.is_finite());
    Some(Gauge {
        kind: "gpu".into(),
        label: "GPU".into(),
        load,
        temperature,
        used: (total > 0.0).then_some(used),
        total: (total > 0.0).then_some(total),
    })
}

fn memory_gauge(snapshot: &Snapshot) -> Option<Gauge> {
    let memory = snapshot.result("Memory");
    let total = number(&memory["total"]);
    if total <= 0.0 {
        return None;
    }
    let used = number(&memory["used"]);
    Some(Gauge {
        kind: "memory".into(),
        label: "RAM".into(),
        load: Some(percentage(used, total)),
        temperature: None,
        used: Some(used),
        total: Some(total),
    })
}

/// Aggregate filesystem usage onto physical disks so each row names a real device.
///
/// Linux exposes partition device paths (`/dev/nvme0n1p2`), so usage is summed
/// per matching physical disk. macOS reports volume names for APFS volumes and
/// shares container space, so a single physical disk inherits its largest volume.
/// When no physical disk is available the filesystem rows are used as-is.
pub fn disks(snapshot: &Snapshot) -> Vec<DiskGauge> {
    let volumes = list(snapshot.result("Disk"))
        .iter()
        .filter(|disk| is_actionable_filesystem(disk) && number(&disk["bytes"]["total"]) > 0.0)
        .collect::<Vec<_>>();
    let physical = list(snapshot.result("PhysicalDisk"))
        .iter()
        .filter(|disk| !is_virtual_disk(disk))
        .collect::<Vec<_>>();
    let mut gauges = Vec::new();
    for disk in &physical {
        let device = string(disk, "devPath");
        let mut seen = HashSet::new();
        let mut matched = volumes
            .iter()
            .filter(|volume| {
                !device.is_empty()
                    && string(volume, "name").starts_with(device)
                    && seen.insert(string(volume, "name"))
            })
            .collect::<Vec<_>>();
        if matched.is_empty()
            && physical.len() == 1
            && let Some(largest) = volumes
                .iter()
                .max_by(|a, b| number(&a["bytes"]["used"]).total_cmp(&number(&b["bytes"]["used"])))
        {
            matched.push(largest);
        }
        if matched.is_empty() {
            continue;
        }
        gauges.push(DiskGauge {
            label: device
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| string(disk, "name").into()),
            model: string(disk, "name").into(),
            used: matched.iter().map(|v| number(&v["bytes"]["used"])).sum(),
            total: matched.iter().map(|v| number(&v["bytes"]["total"])).sum(),
        });
    }
    if gauges.is_empty() {
        gauges.extend(volumes.iter().filter_map(|volume| {
            let label = string(volume, "mountpoint");
            (!label.is_empty()).then(|| DiskGauge {
                label: label.into(),
                model: string(volume, "name").into(),
                used: number(&volume["bytes"]["used"]),
                total: number(&volume["bytes"]["total"]),
            })
        }));
    }
    gauges
}

#[cfg(test)]
#[path = "../../tests/unit/presentation/metrics.rs"]
mod tests;
