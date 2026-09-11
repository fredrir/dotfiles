use crate::formatting::{gpu_memory, is_virtual_disk, named_gpu, nvidia_for, string};
use crate::model::Snapshot;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command, time::Duration};

pub fn probe(command: &mut Command, seconds: u64) -> Option<hostkit::process::CapturedOutput> {
    hostkit::process::output(
        command,
        hostkit::process::CaptureLimits {
            stdout: 4 * 1024 * 1024,
            stderr: 64 * 1024,
        },
        Duration::from_secs(seconds),
    )
    .ok()
}
pub fn detect_virtualized() -> bool {
    if let Some(output) = probe(&mut Command::new("systemd-detect-virt"), 10) {
        let text = String::from_utf8_lossy(&output.stdout);
        if !text.trim().is_empty() && text.trim() != "none" {
            return true;
        }
    }
    fs::read_to_string("/proc/cpuinfo").is_ok_and(|text| text.contains(" hypervisor"))
}
pub fn describe_snapshot(snapshot: &Snapshot) -> Value {
    let mut value = describe_hardware(snapshot);
    value["virtualized"] = json!(detect_virtualized());
    value
}
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
pub fn dotfiles_sha() -> String {
    let root = crate::inventory::repo_root();
    let Some(output) = probe(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--short", "HEAD"]),
        15,
    )
    .filter(|o| o.status.success()) else {
        return String::new();
    };
    let mut sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if probe(
        Command::new("git").arg("-C").arg(root).args([
            "status",
            "--porcelain",
            "--untracked-files=no",
        ]),
        15,
    )
    .is_some_and(|o| o.status.success() && !o.stdout.is_empty())
    {
        sha.push_str("-dirty");
    }
    sha
}
pub fn filesystem_of(path: &Path) -> Value {
    if cfg!(target_os = "macos") {
        let Some(result) =
            probe(&mut Command::new("/sbin/mount"), 10).filter(|r| r.status.success())
        else {
            return json!({});
        };
        let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut found = json!({});
        let mut longest = 0;
        for line in String::from_utf8_lossy(&result.stdout).lines() {
            if let Some((source, rest)) = line.split_once(" on ")
                && let Some((mount, options)) = rest.split_once(" (")
                && target.starts_with(mount)
                && mount.len() > longest
            {
                longest = mount.len();
                found = json!({"fstype":options.split(',').next().unwrap_or("").trim_matches([' ',')']),"source":source,"target":mount});
            }
        }
        found
    } else {
        let Some(result) = probe(
            Command::new("findmnt")
                .args(["-no", "FSTYPE,SOURCE,TARGET", "--target"])
                .arg(path),
            10,
        )
        .filter(|r| r.status.success()) else {
            return json!({});
        };
        let text = String::from_utf8_lossy(&result.stdout);
        let fields = text.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 {
            json!({})
        } else {
            json!({"fstype":fields[0],"source":fields[1],"target":fields[2]})
        }
    }
}
