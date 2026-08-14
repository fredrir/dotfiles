//! Modules the sysinfo crate covers on every platform.

use serde_json::json;
use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};

use crate::Module;

pub fn collect(out: &mut Vec<Module>) {
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );

    out.push((
        "Memory",
        json!({"total": system.total_memory(), "used": system.used_memory()}),
    ));
    out.push(("Swap", swap(&system)));

    let disks = Disks::new_with_refreshed_list();
    let filesystems: Vec<_> = disks
        .list()
        .iter()
        .map(|disk| {
            let total = disk.total_space();
            let available = disk.available_space();
            json!({
                "mountpoint": disk.mount_point().to_string_lossy(),
                "name": disk.name().to_string_lossy(),
                "filesystem": disk.file_system().to_string_lossy(),
                "bytes": {
                    "total": total,
                    "used": total.saturating_sub(available),
                    "free": available,
                    "available": available,
                },
                "readOnly": disk.is_read_only(),
                "removable": disk.is_removable(),
            })
        })
        .collect();
    out.push(("Disk", json!(filesystems)));

    out.push(("Uptime", json!({"uptime": System::uptime() * 1000})));
}

// fastfetch reports macOS swap as one flat entry and Linux swap as entries
// with a nested bytes object; the Python readers expect each platform's shape.
fn swap(system: &System) -> serde_json::Value {
    let total = system.total_swap();
    let used = system.used_swap();
    if cfg!(target_os = "macos") {
        json!([{"total": total, "used": used}])
    } else if total > 0 {
        json!([{"name": "swap", "bytes": {"total": total, "used": used}}])
    } else {
        json!([])
    }
}
