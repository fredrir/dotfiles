use std::collections::HashSet;
use std::env;
use std::ffi::CStr;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::Module;
use crate::parse::{cpuinfo_value, os_release_field, pruned_cpu_name, x86_march};

pub fn collect(out: &mut Vec<Module>) {
    out.push(("OS", os_module()));
    out.push(("Kernel", kernel_module()));
    out.push(("CPU", cpu_module()));
    out.push(("GPU", gpu_module()));
    out.push(("PhysicalMemory", json!([])));
    out.push(("PhysicalDisk", physical_disks()));
    out.push(("Board", board_module()));
    let (batteries, adapters) = power_modules();
    out.push(("Battery", batteries));
    out.push(("PowerAdapter", adapters));
    if let Some(de) = de_module() {
        out.push(("DE", de));
    }
    if let Some(wm) = wm_module() {
        out.push(("WM", wm));
    }
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn os_module() -> Value {
    let body = read("/etc/os-release")
        .or_else(|| read("/usr/lib/os-release"))
        .unwrap_or_default();
    let field = |key: &str| os_release_field(&body, key).unwrap_or_default();
    json!({
        "id": field("ID"),
        "idLike": field("ID_LIKE"),
        "name": field("NAME"),
        "prettyName": field("PRETTY_NAME"),
        "version": os_release_field(&body, "VERSION")
            .or_else(|| os_release_field(&body, "BUILD_ID"))
            .unwrap_or_default(),
        "versionID": field("VERSION_ID"),
    })
}

fn uname() -> Option<libc::utsname> {
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    (unsafe { libc::uname(&mut names) } == 0).then_some(names)
}

fn c_field(bytes: &[libc::c_char]) -> String {
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_string_lossy()
        .to_string()
}

fn kernel_module() -> Value {
    let Some(names) = uname() else {
        return json!({});
    };
    json!({
        "name": c_field(&names.sysname),
        "release": c_field(&names.release),
        "version": c_field(&names.version),
        "architecture": c_field(&names.machine),
    })
}

fn max_frequency_mhz() -> Option<u64> {
    let entries = fs::read_dir("/sys/devices/system/cpu").ok()?;
    let mut max_khz = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let path = entry.path().join("cpufreq/cpuinfo_max_freq");
        if let Some(khz) = read(path).and_then(|value| value.parse::<u64>().ok()) {
            max_khz = max_khz.max(khz);
        }
    }
    (max_khz > 0).then_some(max_khz / 1000)
}

const CPU_HWMON_NAMES: &[&str] = &[
    "k10temp",
    "zenpower",
    "coretemp",
    "cpu_thermal",
    "cpu-thermal",
];

fn cpu_temperature() -> Option<f64> {
    let entries = fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let name = read(entry.path().join("name")).unwrap_or_default();
        if !CPU_HWMON_NAMES.contains(&name.as_str()) {
            continue;
        }
        if let Some(millidegrees) =
            read(entry.path().join("temp1_input")).and_then(|value| value.parse::<f64>().ok())
        {
            return Some(millidegrees / 1000.0);
        }
    }
    None
}

fn cpu_module() -> Value {
    let body = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let raw_name = cpuinfo_value(&body, "model name").unwrap_or_default();
    let vendor = cpuinfo_value(&body, "vendor_id").unwrap_or_default();
    let logical = body
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count();
    let mut cores: HashSet<(String, String)> = HashSet::new();
    let mut physical_id = String::new();
    for line in body.lines() {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "physical id" => physical_id = value.trim().to_string(),
                "core id" => {
                    cores.insert((physical_id.clone(), value.trim().to_string()));
                }
                _ => {}
            }
        }
    }
    let physical = if cores.is_empty() {
        logical
    } else {
        cores.len()
    };
    let march = if cfg!(target_arch = "x86_64") {
        x86_march(&cpuinfo_value(&body, "flags").unwrap_or_default())
    } else {
        ""
    };
    json!({
        "cpu": pruned_cpu_name(&raw_name),
        "vendor": vendor,
        "cores": {"physical": physical, "logical": logical, "online": logical},
        "frequency": {"base": 0, "max": max_frequency_mhz().unwrap_or(0)},
        "temperature": cpu_temperature(),
        "march": march,
    })
}

fn pci_vendor_name(vendor_id: &str) -> &'static str {
    match vendor_id {
        "10de" => "NVIDIA",
        "1002" => "AMD",
        "8086" => "Intel",
        _ => "",
    }
}

fn amdgpu_name(device_id: &str, revision: &str) -> Option<String> {
    let body = fs::read_to_string("/usr/share/libdrm/amdgpu.ids").ok()?;
    crate::parse::amdgpu_name_in(&body, device_id, revision)
}

fn pci_device_name(vendor_id: &str, device_id: &str) -> Option<String> {
    let body = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|path| fs::read_to_string(path).ok())?;
    crate::parse::pci_device_name_in(&body, vendor_id, device_id)
}

fn nvidia_driver() -> String {
    let version = read("/sys/module/nvidia/version").unwrap_or_default();
    let open = fs::read_to_string("/proc/driver/nvidia/version")
        .map(|body| body.contains("Open"))
        .unwrap_or(false);
    match (open, version.is_empty()) {
        (_, true) => "nvidia".to_string(),
        (true, false) => format!("nvidia (open source) {version}"),
        (false, false) => format!("nvidia {version}"),
    }
}

const INTEGRATED_VRAM_LIMIT: u64 = 3 << 29; // 1.5 GiB

fn gpu_module() -> Value {
    let mut gpus: Vec<Value> = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return json!([]);
    };
    let mut cards: Vec<_> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy().to_string();
            name.strip_prefix("card")
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    cards.sort_by_key(|entry| entry.file_name());
    for card in cards.iter() {
        let device = card.path().join("device");
        let vendor_id = read(device.join("vendor")).unwrap_or_default();
        let vendor_id = vendor_id.trim_start_matches("0x").to_lowercase();
        let device_id = read(device.join("device")).unwrap_or_default();
        let revision = read(device.join("revision")).unwrap_or_default();
        let driver = fs::read_link(device.join("driver"))
            .ok()
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let vendor = pci_vendor_name(&vendor_id).to_string();
        let name = match driver.as_str() {
            "amdgpu" => amdgpu_name(&device_id, &revision),
            _ => None,
        }
        .or_else(|| pci_device_name(&vendor_id, &device_id))
        .unwrap_or_else(|| format!("Unknown device {device_id}"));
        let vram_total =
            read(device.join("mem_info_vram_total")).and_then(|value| value.parse::<u64>().ok());
        let vram_used =
            read(device.join("mem_info_vram_used")).and_then(|value| value.parse::<u64>().ok());
        let integrated = match driver.as_str() {
            "nvidia" => false,
            "i915" | "xe" => true,
            _ => vram_total.is_none_or(|total| total < INTEGRATED_VRAM_LIMIT),
        };
        gpus.push(json!({
            "index": Value::Null,
            "name": name,
            "vendor": vendor,
            "type": if integrated { "Integrated" } else { "Discrete" },
            "driver": if driver == "nvidia" { nvidia_driver() } else { driver },
            "memory": {"dedicated": {"total": vram_total, "used": vram_used}},
            "temperature": Value::Null,
        }));
    }
    json!(gpus)
}

const SKIPPED_BLOCK_PREFIXES: &[&str] = &["loop", "ram", "zram", "dm-", "md", "sr", "fd"];

fn disk_interconnect(device_path: &Path, name: &str) -> &'static str {
    if name.starts_with("nvme") {
        return "NVMe";
    }
    if name.starts_with("mmcblk") {
        return "MMC";
    }
    let resolved = fs::canonicalize(device_path).unwrap_or_default();
    let resolved = resolved.to_string_lossy();
    if resolved.contains("/usb") {
        "USB"
    } else if resolved.contains("/ata") {
        "ATA"
    } else if resolved.contains("/virtio") {
        "Virtio"
    } else {
        ""
    }
}

fn physical_disks() -> Value {
    let mut disks: Vec<Value> = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return json!([]);
    };
    let mut blocks: Vec<_> = entries.flatten().collect();
    blocks.sort_by_key(|entry| entry.file_name());
    for block in blocks {
        let name = block.file_name();
        let name = name.to_string_lossy().to_string();
        if SKIPPED_BLOCK_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let path = block.path();
        let model = read(path.join("device/model")).unwrap_or_default();
        let vendor = read(path.join("device/vendor")).unwrap_or_default();
        let mut label = model.clone();
        if label.is_empty() {
            label = name.clone();
        } else if !vendor.is_empty() && vendor != "ATA" && !label.starts_with(&vendor) {
            label = format!("{vendor} {label}");
        }
        let sectors = read(path.join("size"))
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let rotational = read(path.join("queue/rotational")).as_deref() == Some("1");
        let removable = read(path.join("removable")).as_deref() == Some("1");
        disks.push(json!({
            "name": label,
            "devPath": format!("/dev/{name}"),
            "size": sectors * 512,
            "kind": if rotational { "HDD" } else { "SSD" },
            "interconnect": disk_interconnect(&path.join("device"), &name),
            "removable": removable,
            "readOnly": read(path.join("ro")).as_deref() == Some("1"),
            "temperature": Value::Null,
        }));
    }
    json!(disks)
}

fn board_module() -> Value {
    json!({
        "name": read("/sys/class/dmi/id/board_name").unwrap_or_default(),
        "vendor": read("/sys/class/dmi/id/board_vendor").unwrap_or_default(),
        "version": read("/sys/class/dmi/id/board_version").unwrap_or_default(),
    })
}

fn power_modules() -> (Value, Value) {
    let mut batteries: Vec<Value> = Vec::new();
    let mut adapters: Vec<Value> = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return (json!([]), json!([]));
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match read(path.join("type")).as_deref() {
            Some("Battery") => {
                let capacity = read(path.join("capacity")).and_then(|v| v.parse::<f64>().ok());
                let status = read(path.join("status")).unwrap_or_default();
                batteries.push(json!({
                    "modelName": read(path.join("model_name")).unwrap_or_default(),
                    "manufacturer": read(path.join("manufacturer")).unwrap_or_default(),
                    "capacity": capacity,
                    "status": if status.is_empty() { json!([]) } else { json!([status]) },
                    "cycleCount": read(path.join("cycle_count"))
                        .and_then(|v| v.parse::<i64>().ok()),
                    "temperature": read(path.join("temp"))
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|tenths| tenths / 10.0),
                }));
            }
            Some("Mains") if read(path.join("online")).as_deref() == Some("1") => {
                adapters.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "modelName": "",
                    "manufacturer": "",
                    "watts": Value::Null,
                }));
            }
            _ => {}
        }
    }
    (json!(batteries), json!(adapters))
}

fn running_process(names: &[&str]) -> Option<(String, u32)> {
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid = entry.file_name();
        let Ok(pid) = pid.to_string_lossy().parse::<u32>() else {
            continue;
        };
        let comm = read(entry.path().join("comm")).unwrap_or_default();
        if names.contains(&comm.as_str()) {
            return Some((comm, pid));
        }
    }
    None
}

fn process_environ(pid: u32, key: &str) -> Option<String> {
    let body = fs::read(format!("/proc/{pid}/environ")).ok()?;
    for entry in body.split(|byte| *byte == 0) {
        let text = String::from_utf8_lossy(entry);
        if let Some(value) = text.strip_prefix(key)
            && let Some(value) = value.strip_prefix('=')
        {
            return Some(value.to_string());
        }
    }
    None
}

const DESKTOP_PROCESSES: &[&str] = &["plasmashell", "gnome-shell", "Hyprland", "sway"];

fn detected_desktop() -> String {
    let current = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !current.is_empty() {
        return current;
    }
    match running_process(DESKTOP_PROCESSES) {
        Some((name, _)) => name,
        None => String::new(),
    }
}

fn de_module() -> Option<Value> {
    let current = detected_desktop();
    let lowered = current.to_lowercase();
    let lowered = if lowered == "plasmashell" {
        "kde".to_string()
    } else {
        lowered
    };
    let pretty = if lowered.contains("kde") {
        "KDE Plasma"
    } else if lowered.contains("gnome") {
        "GNOME"
    } else if lowered.contains("hyprland") {
        "Hyprland"
    } else if lowered.contains("xfce") {
        "Xfce"
    } else if current.is_empty() {
        return None;
    } else {
        return Some(json!({"prettyName": current, "processName": current, "version": ""}));
    };
    json!({"prettyName": pretty, "processName": current, "version": de_version(pretty)}).into()
}

fn de_version(pretty: &str) -> String {
    let package = match pretty {
        "KDE Plasma" => "plasma-workspace",
        "Hyprland" => "hyprland",
        "GNOME" => "gnome-shell",
        _ => return String::new(),
    };
    let Ok(entries) = fs::read_dir("/var/lib/pacman/local") else {
        return String::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix(&format!("{package}-"))
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            return rest
                .rsplit_once('-')
                .map(|(v, _)| v.to_string())
                .unwrap_or_default();
        }
    }
    String::new()
}

fn session_protocol() -> &'static str {
    match env::var("XDG_SESSION_TYPE").unwrap_or_default().as_str() {
        "wayland" => return "Wayland",
        "x11" => return "X11",
        _ => {}
    }
    // The compositor creates the display, so WAYLAND_DISPLAY is only in its
    // clients' environments; ask a session process instead.
    if let Some((_, pid)) = running_process(DESKTOP_PROCESSES) {
        if process_environ(pid, "WAYLAND_DISPLAY").is_some() {
            return "Wayland";
        }
        if process_environ(pid, "DISPLAY").is_some() {
            return "X11";
        }
    }
    ""
}

fn wm_module() -> Option<Value> {
    let protocol = session_protocol();
    let desktop = detected_desktop().to_lowercase();
    let (pretty, process) = if desktop.contains("kde") || desktop == "plasmashell" {
        ("KWin", "kwin_wayland")
    } else if desktop.contains("hyprland") {
        ("Hyprland", "Hyprland")
    } else if desktop.contains("gnome") {
        ("Mutter", "gnome-shell")
    } else if desktop.contains("sway") {
        ("Sway", "sway")
    } else {
        return None;
    };
    Some(json!({"prettyName": pretty, "processName": process, "protocolName": protocol}))
}
