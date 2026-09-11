use super::{
    branding::{resolve_brand, strip_brand},
    facts, joined,
};
use crate::formatting::*;
use crate::model::{Component, Snapshot};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

fn cpu_component(snapshot: &Snapshot) -> Component {
    let cpu = snapshot.result("CPU");
    let name = cpu["cpu"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("Unknown CPU");
    let vendor = string(cpu, "vendor");
    let brand = resolve_brand("cpu", &[vendor, name]);
    let profile = name.to_lowercase().contains("ryzen 7 9800x3d");
    let family = if profile {
        "Granite Ridge X3D"
    } else {
        string(cpu, "codeName")
    };
    let cores = &cpu["cores"];
    let physical = number(&cores["physical"]);
    let logical = number(&cores["logical"]);
    let threads = if physical > 0.0 && logical > 0.0 {
        format!("{physical:.0} cores / {logical:.0} threads")
    } else {
        String::new()
    };
    let cache = list(&snapshot.result("CPUCache")["l3"])
        .iter()
        .map(|entry| {
            number(&entry["size"]) * entry["num"].as_f64().filter(|v| *v != 0.0).unwrap_or(1.0)
        })
        .sum::<f64>();
    let loads = list(snapshot.result("CPUUsage"))
        .iter()
        .filter_map(Value::as_f64)
        .collect::<Vec<_>>();
    let usage = if loads.is_empty() {
        String::new()
    } else {
        format!("{:.0}%", loads.iter().sum::<f64>() / loads.len() as f64)
    };
    Component {
        kind: "cpu".into(),
        label: "CPU".into(),
        vendor: if vendor.is_empty() {
            brand.name.clone()
        } else {
            vendor.into()
        },
        model: strip_brand(name, brand),
        identifiers: vec![name.into(), family.into()],
        compact: true,
        facts: facts([
            ("Cores", threads),
            (
                "Cache",
                if cache > 0.0 {
                    format!("{} L3", format_bytes(cache))
                } else {
                    String::new()
                },
            ),
            ("Family", family.into()),
            ("Clock", format_frequency(number(&cpu["frequency"]["max"]))),
            (
                "Power",
                if profile {
                    "120 W TDP".into()
                } else {
                    String::new()
                },
            ),
            (
                "Temperature",
                format_temperature(cpu["temperature"].as_f64()),
            ),
            ("Load", usage),
            ("Process ISA", text(&cpu["march"])),
            ("Process", text(&cpu["technology"])),
        ]),
        ..Component::default()
    }
}
fn gpu_components(snapshot: &Snapshot) -> Vec<Component> {
    let gpus = list(snapshot.result("GPU"));
    let discrete = gpus.iter().any(|g| g["type"] == "Discrete");
    gpus.iter()
        .map(|gpu| {
            let name = named_gpu(gpu);
            let vendor = string(gpu, "vendor");
            let brand = resolve_brand("gpu", &[vendor, &name]);
            let profile = name.to_lowercase().contains("geforce rtx 5070 ti");
            let nvidia = nvidia_for(snapshot, gpu);
            let (used, total) = gpu_memory(gpu, nvidia);
            let number_from = |native: &str, fallback: &str| {
                nvidia[native].as_f64().or_else(|| gpu[fallback].as_f64())
            };
            let power = nvidia["power_draw"]
                .as_f64()
                .map(
                    |draw| match nvidia["power_limit"].as_f64().filter(|v| *v != 0.0) {
                        Some(limit) => format!("{draw:.0} W / {limit:.0} W"),
                        None => format!("{draw:.0} W"),
                    },
                )
                .unwrap_or_default();
            let pcie = &gpu["pcieSpeed"]["max"];
            let link = if number(&pcie["gen"]) > 0.0 && number(&pcie["lanes"]) > 0.0 {
                format!("PCIe {}.0 ×{}", text(&pcie["gen"]), text(&pcie["lanes"]))
            } else {
                String::new()
            };
            let integrated = gpu["type"] == "Integrated";
            Component {
                kind: "gpu".into(),
                label: if integrated { "INTEGRATED GPU" } else { "GPU" }.into(),
                vendor: if vendor.is_empty() {
                    brand.name.clone()
                } else {
                    vendor.into()
                },
                model: strip_brand(&name, brand),
                identifiers: vec![name, text(&gpu["driver"])],
                compact: !integrated || !discrete,
                facts: facts([
                    (
                        "Architecture",
                        if profile {
                            "Blackwell".into()
                        } else {
                            String::new()
                        },
                    ),
                    (
                        "Memory",
                        if profile {
                            "16 GB GDDR7".into()
                        } else if total > 0.0 {
                            format!("{} VRAM", format_bytes(total))
                        } else {
                            String::new()
                        },
                    ),
                    (
                        "Cores",
                        if profile {
                            "8,960 CUDA cores".into()
                        } else {
                            String::new()
                        },
                    ),
                    (
                        "VRAM use",
                        if total > 0.0 {
                            format!("{} / {}", format_bytes(used), format_bytes(total))
                        } else {
                            String::new()
                        },
                    ),
                    (
                        "Load",
                        number_from("utilization", "coreUsage")
                            .map(|v| format!("{v:.0}%"))
                            .unwrap_or_default(),
                    ),
                    (
                        "Clock",
                        format_frequency(number_from("clock_mhz", "frequency").unwrap_or(0.0)),
                    ),
                    (
                        "Temperature",
                        format_temperature(number_from("temperature", "temperature")),
                    ),
                    ("Power", power),
                    ("Link", link),
                    (
                        "Driver",
                        nvidia["driver"]
                            .as_str()
                            .filter(|v| !v.is_empty())
                            .unwrap_or(string(gpu, "driver"))
                            .into(),
                    ),
                ]),
                ..Component::default()
            }
        })
        .collect()
}
fn memory_component(snapshot: &Snapshot) -> Component {
    static KIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z0-9]{8,}\b").unwrap());
    let memory = snapshot.result("Memory");
    let description = snapshot.configured("memory");
    let detected = number(&memory["total"]);
    let used = number(&memory["used"]);
    let modules = list(snapshot.result("PhysicalMemory"));
    let mut hints = vec![description.to_string()];
    for module in modules {
        for key in ["vendor", "manufacturer", "partNumber", "type"] {
            let value = text(&module[key]);
            if !value.is_empty() {
                hints.push(value);
            }
        }
    }
    let mut identities = hints.iter().map(String::as_str).collect::<Vec<_>>();
    if snapshot.is_macos() {
        identities.push("Apple");
    }
    let brand = resolve_brand("memory", &identities);
    let (swap_used, swap_total) =
        list(snapshot.result("Swap"))
            .iter()
            .fold((0.0, 0.0), |(used, total), swap| {
                let bytes = &swap["bytes"];
                (
                    used + number(&bytes["used"]),
                    total + number(&bytes["total"]),
                )
            });
    let swap = if swap_total > 0.0 {
        format!("{} / {}", format_bytes(swap_used), format_bytes(swap_total))
    } else if !snapshot.is_macos() {
        "Disabled".into()
    } else {
        String::new()
    };
    Component {
        kind: "memory".into(),
        label: "MEMORY".into(),
        vendor: brand.name.clone(),
        model: memory_summary(description, detected),
        identifiers: hints,
        compact: true,
        facts: facts([
            (
                "Kit",
                KIT.find(description)
                    .map(|m| m.as_str())
                    .unwrap_or("")
                    .into(),
            ),
            (
                "Modules",
                if !snapshot.is_macos() && !modules.is_empty() {
                    modules.len().to_string()
                } else {
                    String::new()
                },
            ),
            ("Detected", memory_capacity(detected)),
            (
                "Usage",
                if detected > 0.0 {
                    format!("{} / {}", format_bytes(used), format_bytes(detected))
                } else {
                    String::new()
                },
            ),
            (
                "Load",
                if detected > 0.0 {
                    format!("{:.0}%", percentage(used, detected))
                } else {
                    String::new()
                },
            ),
            ("Swap", swap),
        ]),
        ..Component::default()
    }
}
fn motherboard_component(snapshot: &Snapshot) -> Component {
    let board = snapshot.result("Board");
    let configured = snapshot.configured("motherboard");
    let vendor = string(board, "vendor");
    let name = if !configured.is_empty() {
        configured
    } else {
        board["name"]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or("Unknown motherboard")
    };
    let brand = resolve_brand("motherboard", &[vendor, name]);
    Component {
        kind: "motherboard".into(),
        label: "MOTHERBOARD".into(),
        vendor: if vendor.is_empty() {
            brand.name.clone()
        } else {
            vendor.into()
        },
        model: strip_brand(name, brand),
        identifiers: vec![vendor.into(), name.into()],
        facts: facts([("Revision", text(&board["version"]))]),
        compact: true,
        ..Component::default()
    }
}
fn disk_components(snapshot: &Snapshot) -> Vec<Component> {
    let mut components = Vec::new();
    for disk in list(snapshot.result("PhysicalDisk"))
        .iter()
        .filter(|d| !is_virtual_disk(d))
    {
        let name = disk["name"]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or("Unknown disk");
        let lower = name.to_lowercase();
        let family = if lower.contains("kingston snvs2000g") {
            "Kingston NV1"
        } else if lower.contains("wdc wd20ezrz") {
            "WD Blue"
        } else {
            ""
        };
        let brand = resolve_brand("storage", &[name]);
        let raw = name.strip_prefix("ATA ").unwrap_or(name);
        let model = strip_brand(if family.is_empty() { raw } else { family }, brand);
        let capacity = capacity(number(&disk["size"]));
        components.push(Component {
            kind: "storage".into(),
            label: "STORAGE".into(),
            vendor: brand.name.clone(),
            model: joined(&[&model, &capacity, string(disk, "kind")]),
            identifiers: vec![name.into(), family.into()],
            compact: true,
            facts: facts([
                ("Device", raw.into()),
                ("Capacity", capacity),
                ("Interface", text(&disk["interconnect"])),
                ("Type", text(&disk["kind"])),
                (
                    "Temperature",
                    format_temperature(disk["temperature"].as_f64()),
                ),
            ]),
            ..Component::default()
        });
    }
    if components.is_empty() {
        let total = list(snapshot.result("Disk"))
            .iter()
            .map(|d| number(&d["bytes"]["total"]))
            .sum::<f64>();
        if total > 0.0 {
            components.push(Component {
                kind: "storage".into(),
                label: "STORAGE".into(),
                model: capacity(total),
                compact: true,
                ..Component::default()
            });
        }
    }
    components
}
fn useful_name(value: &Value, fallback: &str, rejected: &[&str]) -> String {
    let name = text(value).trim().to_owned();
    let lower = name.to_lowercase();
    if name.is_empty()
        || name.chars().all(|c| c.is_ascii_digit())
        || ["unknown", "n/a", "none", "null"].contains(&lower.as_str())
        || rejected.iter().any(|p| lower.starts_with(p))
    {
        fallback.into()
    } else {
        name
    }
}
fn portable_power_components(snapshot: &Snapshot) -> Vec<Component> {
    let mut components = Vec::new();
    for battery in list(snapshot.result("Battery")) {
        let raw = if !text(&battery["modelName"]).is_empty() {
            &battery["modelName"]
        } else {
            &battery["name"]
        };
        let name = useful_name(raw, "Internal battery", &["bq", "smc"]);
        let vendor = battery["manufacturer"]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or(if snapshot.is_macos() { "Apple" } else { "" });
        components.push(Component {
            kind: "power".into(),
            label: "BATTERY".into(),
            vendor: vendor.into(),
            model: name.clone(),
            art_kind: "battery".into(),
            identifiers: vec![vendor.into(), name],
            compact: true,
            facts: facts([
                (
                    "Charge",
                    battery["capacity"]
                        .as_f64()
                        .map(|v| format!("{v:.0}%"))
                        .unwrap_or_default(),
                ),
                ("Status", text(&battery["status"])),
                (
                    "Temperature",
                    format_temperature(battery["temperature"].as_f64()),
                ),
                ("Cycles", text(&battery["cycleCount"])),
            ]),
        });
    }
    for adapter in list(snapshot.result("PowerAdapter")) {
        let watts = number(&adapter["watts"]);
        let fallback = if watts != 0.0 {
            format!("{watts} W")
        } else {
            "Connected".into()
        };
        let raw = if !text(&adapter["modelName"]).is_empty() {
            &adapter["modelName"]
        } else {
            &adapter["name"]
        };
        let name = useful_name(raw, &fallback, &[]);
        let vendor = adapter["manufacturer"]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or(if snapshot.is_macos() { "Apple" } else { "" });
        components.push(Component {
            kind: "power".into(),
            label: "POWER ADAPTER".into(),
            vendor: vendor.into(),
            model: name.clone(),
            art_kind: "adapter".into(),
            identifiers: vec![vendor.into(), name.clone()],
            compact: false,
            facts: facts([(
                "Output",
                if watts != 0.0 && name != fallback {
                    fallback
                } else {
                    String::new()
                },
            )]),
        });
    }
    components
}
pub fn hardware_components(snapshot: &Snapshot) -> Vec<Component> {
    let mut components = vec![cpu_component(snapshot)];
    components.extend(gpu_components(snapshot));
    components.push(memory_component(snapshot));
    components.push(motherboard_component(snapshot));
    components.extend(disk_components(snapshot));
    for (kind, label, key) in [
        ("cooling", "CPU COOLING", "cpu_cooler"),
        ("case", "CHASSIS", "case"),
        ("power", "POWER SUPPLY", "power_supply"),
    ] {
        let description = snapshot.configured(key);
        if matches!(description, "" | "not set") {
            continue;
        }
        let brand = resolve_brand(kind, &[description]);
        components.push(Component {
            kind: kind.into(),
            label: label.into(),
            vendor: brand.name.clone(),
            model: strip_brand(description, brand),
            identifiers: vec![description.into()],
            compact: false,
            ..Component::default()
        });
    }
    components.extend(portable_power_components(snapshot));
    components
}
