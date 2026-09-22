use super::*;
use crate::model::Snapshot;
use serde_json::{Value, json};

fn snapshot(modules: impl IntoIterator<Item = (&'static str, Value)>) -> Snapshot {
    Snapshot {
        modules: modules.into_iter().map(|(kind, value)| (kind.into(), value)).collect(),
        ..Snapshot::default()
    }
}

#[test]
fn gauges_report_cpu_gpu_and_memory_loads() {
    let snapshot = snapshot([
        ("CPU", json!({"temperature": 47.5})),
        ("CPUUsage", json!([10.0, 30.0])),
        (
            "GPU",
            json!([{
                "name": "NVIDIA Test",
                "vendor": "NVIDIA",
                "type": "Discrete",
                "temperature": 61.0,
                "coreUsage": 42.0,
                "memory": {"dedicated": {"used": 5_368_709_120_u64, "total": 17_179_869_184_u64}}
            }]),
        ),
        (
            "Memory",
            json!({"total": 34_359_738_368_u64, "used": 8_589_934_592_u64}),
        ),
    ]);
    let rows = gauges(&snapshot);
    assert_eq!(
        rows.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
        ["CPU", "GPU", "RAM"]
    );
    assert_eq!(rows[0].load, Some(20.0));
    assert_eq!(rows[0].temperature, Some(47.5));
    assert_eq!(rows[1].load, Some(42.0));
    assert_eq!(rows[1].temperature, Some(61.0));
    assert_eq!(rows[1].used, Some(5_368_709_120.0));
    assert_eq!(rows[2].load, Some(25.0));
    assert_eq!(rows[2].total, Some(34_359_738_368.0));
}

#[test]
fn gauges_omit_gpu_memory_when_only_integrated_memory_is_shared() {
    let snapshot = snapshot([(
        "GPU",
        json!([{
            "name": "Apple Test GPU",
            "vendor": "Apple",
            "type": "Integrated",
            "memory": {"dedicated": {"used": Value::Null, "total": Value::Null}}
        }]),
    )]);
    let rows = gauges(&snapshot);
    let gpu = rows.iter().find(|row| row.kind == "gpu").unwrap();
    assert_eq!(gpu.used, None);
    assert_eq!(gpu.total, None);
}

#[test]
fn disks_aggregate_partition_usage_onto_their_physical_device() {
    let snapshot = snapshot([
        (
            "PhysicalDisk",
            json!([{"name": "Samsung SSD 990", "devPath": "/dev/nvme0n1", "size": 1_000_000_000_000_u64, "kind": "SSD"}]),
        ),
        (
            "Disk",
            json!([
                {"name": "/dev/nvme0n1p1", "mountpoint": "/boot", "filesystem": "vfat", "bytes": {"used": 100, "total": 1000}},
                {"name": "/dev/nvme0n1p2", "mountpoint": "/", "filesystem": "ext4", "bytes": {"used": 400, "total": 9000}},
                {"name": "/dev/sdb1", "mountpoint": "/mnt/usb", "filesystem": "ext4", "bytes": {"used": 50, "total": 500}}
            ]),
        ),
    ]);
    let rows = disks(&snapshot);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "nvme0n1");
    assert_eq!(rows[0].used, 500.0);
    assert_eq!(rows[0].total, 10_000.0);
}

#[test]
fn disks_use_the_largest_volume_when_a_single_disk_reports_volume_names() {
    let snapshot = snapshot([
        (
            "PhysicalDisk",
            json!([{"name": "APPLE SSD", "devPath": "/dev/disk0", "size": 1_000_000_000_000_u64, "kind": "SSD"}]),
        ),
        (
            "Disk",
            json!([
                {"name": "Macintosh HD", "mountpoint": "/System/Volumes/Data", "filesystem": "apfs", "bytes": {"used": 650, "total": 926}},
                {"name": "Preboot", "mountpoint": "/System/Volumes/Preboot", "filesystem": "apfs", "bytes": {"used": 11, "total": 926}}
            ]),
        ),
    ]);
    let rows = disks(&snapshot);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "disk0");
    assert_eq!(rows[0].used, 650.0);
    assert_eq!(rows[0].total, 926.0);
}

#[test]
fn disks_fall_back_to_mountpoints_without_a_physical_disk_report() {
    let snapshot = snapshot([(
        "Disk",
        json!([{"name": "/dev/sda1", "mountpoint": "/", "filesystem": "ext4", "bytes": {"used": 100, "total": 1000}}]),
    )]);
    let rows = disks(&snapshot);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "/");
    assert_eq!(rows[0].used, 100.0);
}
