use crate::formatting::{gpu_memory, is_virtual_disk, named_gpu, nvidia_for, string};
use crate::model::Snapshot;
use serde_json::{Value, json};

pub fn describe_hardware(snapshot: &Snapshot) -> Value {
    let cpu = snapshot.result("CPU");
    let memory = snapshot.result("Memory");
    let board = snapshot.result("Board");
    let gpus = snapshot
        .result("GPU")
        .as_array()
        .into_iter()
        .flatten()
        .map(|gpu| {
            let total = gpu_memory(gpu, nvidia_for(snapshot, gpu)).1;
            json!({
                "name": named_gpu(gpu),
                "vendor": string(gpu, "vendor"),
                "type": string(gpu, "type"),
                "memory_total": if total == 0.0 { Value::Null } else { json!(total) },
                "driver": string(gpu, "driver"),
            })
        })
        .collect::<Vec<_>>();
    let disks = snapshot
        .result("PhysicalDisk")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|disk| disk["removable"] != true && !is_virtual_disk(disk))
        .map(|disk| {
            json!({
                "name": string(disk, "name").strip_prefix("ATA ").unwrap_or(string(disk, "name")),
                "size": disk["size"],
                "kind": string(disk, "kind"),
                "interconnect": string(disk, "interconnect"),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "cpu": {
            "model": string(cpu, "cpu"),
            "vendor": string(cpu, "vendor"),
            "cores_physical": cpu["cores"]["physical"],
            "cores_logical": cpu["cores"]["logical"],
            "frequency_max": cpu["frequency"]["max"],
            "march": string(cpu, "march"),
        },
        "gpu": gpus,
        "memory": {
            "total": memory["total"],
            "modules": snapshot.result("PhysicalMemory").as_array().map_or(0, Vec::len),
        },
        "board": { "vendor": string(board, "vendor"), "name": string(board, "name") },
        "disks": disks,
        "configured": snapshot.hardware,
    })
}
pub fn describe_install(snapshot: &Snapshot) -> Value {
    let os = snapshot.result("OS");
    let kernel = snapshot.result("Kernel");
    let first = |a: &str, b: &str| {
        if a.is_empty() {
            b.to_string()
        } else {
            a.to_string()
        }
    };
    let driver = snapshot
        .nvidia
        .iter()
        .chain(snapshot.result("GPU").as_array().into_iter().flatten())
        .map(|gpu| string(gpu, "driver"))
        .find(|driver| !driver.is_empty())
        .unwrap_or("");
    json!({
        "os": first(&first(string(os, "id"), string(os, "name")), platform()),
        "name": first(string(os, "prettyName"), string(os, "name")),
        "version": first(string(os, "versionID"), string(os, "version")),
        "kernel": string(kernel, "release"),
        "arch": first(string(kernel, "architecture"), std::env::consts::ARCH),
        "driver": driver,
    })
}
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}
