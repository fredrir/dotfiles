use super::describe_hardware;
use crate::model::Snapshot;
use serde_json::json;

#[test]
fn hardware_report_omits_serials_virtual_disks_and_removable_media() {
    let mut snapshot = Snapshot::default();
    snapshot
        .modules
        .insert("CPU".into(), json!({"cpu": "CPU", "serial": "PRIVATE"}));
    snapshot.modules.insert(
        "PhysicalDisk".into(),
        json!([
            {"name": "ATA Real SSD", "size": 2000000000000_u64, "serial": "PRIVATE"},
            {"name": "Disk Image", "size": 500000},
            {"name": "USB", "removable": true}
        ]),
    );

    let result = describe_hardware(&snapshot);

    assert!(!result.to_string().contains("PRIVATE"));
    assert_eq!(result["disks"].as_array().unwrap().len(), 1);
    assert_eq!(result["disks"][0]["name"], "Real SSD");
    assert!(result.get("virtualized").is_none());
}
