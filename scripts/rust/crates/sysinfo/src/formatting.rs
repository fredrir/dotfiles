use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

pub const GIB: f64 = 1_073_741_824.0;
pub const MIB: f64 = 1_048_576.0;
pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(text)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => value.to_string(),
    }
}
pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub fn number(value: &Value) -> f64 {
    value.as_f64().filter(|v| v.is_finite()).unwrap_or(0.0)
}
pub fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn compact_number(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    let rounded = text.parse::<f64>().unwrap_or(value);
    if rounded == 0.0 {
        "0".into()
    } else if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        text
    }
}
pub fn capacity(value: f64) -> String {
    let (scale, unit) = if value >= 1e12 {
        (1e12, "TB")
    } else if value >= 1e9 {
        (1e9, "GB")
    } else if value > 0.0 {
        (1e6, "MB")
    } else {
        return "unknown".into();
    };
    format!(
        "{} {unit}",
        compact_number((value / scale * 10.0).round() / 10.0, 1)
    )
}
pub fn memory_capacity(value: f64) -> String {
    if value > 0.0 {
        format!("{:.0} GB", (value / GIB / 8.0).ceil() * 8.0)
    } else {
        "unknown".into()
    }
}
pub fn format_bytes(value: f64) -> String {
    for (divisor, suffix) in [
        (1024.0 * GIB, "TB"),
        (GIB, "GB"),
        (MIB, "MB"),
        (1024.0, "KB"),
    ] {
        if value >= divisor {
            return format!("{} {suffix}", compact_number(value / divisor, 1));
        }
    }
    format!("{:.0} B", value.trunc())
}
pub fn format_frequency(value: f64) -> String {
    if value == 0.0 {
        String::new()
    } else {
        format!("{} GHz", compact_number(value / 1000.0, 2))
    }
}
pub fn format_temperature(value: Option<f64>) -> String {
    value
        .filter(|v| v.is_finite())
        .map(|v| format!("{}°C", compact_number(v, 1)))
        .unwrap_or_default()
}
pub fn format_duration(milliseconds: f64) -> String {
    let seconds = (milliseconds / 1000.0) as u64;
    let (days, hours, minutes) = (seconds / 86400, seconds / 3600 % 24, seconds / 60 % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}
pub fn percentage(used: f64, total: f64) -> f64 {
    if total == 0.0 {
        0.0
    } else {
        (used / total * 100.0).clamp(0.0, 100.0)
    }
}
static MEMORY_AMOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([0-9]+(?:\.[0-9]+)?)\s*(TB|GB|MB)\b").unwrap());
pub fn configured_memory_bytes(description: &str) -> f64 {
    let Some(found) = MEMORY_AMOUNT.captures(description) else {
        return 0.0;
    };
    let scale = match found[2].to_uppercase().as_str() {
        "TB" => GIB * 1024.0,
        "GB" => GIB,
        _ => MIB,
    };
    found[1].parse::<f64>().unwrap_or(0.0) * scale
}
pub fn memory_summary(description: &str, detected: f64) -> String {
    static SPEED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(?:LP)?DDR\d(?:-\d+)?\b").unwrap());
    static TIMING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bCL\d+\b").unwrap());
    let parts = [&*MEMORY_AMOUNT, &*SPEED, &*TIMING]
        .into_iter()
        .filter_map(|re| re.find(description).map(|m| m.as_str().to_uppercase()))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        memory_capacity(detected)
    } else {
        parts.join("  ")
    }
}
pub fn is_virtual_disk(disk: &Value) -> bool {
    let identity = ["name", "kind", "interconnect", "volumeType", "mountFrom"]
        .map(|key| text(&disk[key]))
        .join(" ")
        .to_lowercase();
    [
        "disk image",
        "virtual interface",
        "virtual",
        "loop device",
        "sparse image",
    ]
    .iter()
    .any(|m| identity.contains(m))
}
pub fn is_actionable_filesystem(disk: &Value) -> bool {
    if is_virtual_disk(disk) || disk["readOnly"].as_bool() == Some(true) {
        return false;
    }
    if ["devfs", "iso9660", "squashfs", "tmpfs", "udf"]
        .contains(&string(disk, "filesystem").to_lowercase().as_str())
    {
        return false;
    }
    let flags = text(&disk["volumeType"]).to_lowercase();
    if flags.contains("read-only") || flags.contains("readonly") {
        return false;
    }
    let mount = string(disk, "mountpoint");
    !mount.starts_with("/System/Volumes/") || mount == "/System/Volumes/Data"
}
pub fn named_gpu(gpu: &Value) -> String {
    let (vendor, name) = (string(gpu, "vendor"), string(gpu, "name"));
    if !vendor.is_empty() && !name.to_lowercase().starts_with(&vendor.to_lowercase()) {
        format!("{vendor} {name}")
    } else if name.is_empty() {
        "unknown".into()
    } else {
        name.into()
    }
}
pub fn nvidia_for<'a>(snapshot: &'a crate::model::Snapshot, gpu: &Value) -> &'a Value {
    let name = named_gpu(gpu).to_lowercase();
    snapshot
        .nvidia
        .iter()
        .find(|device| {
            if !gpu["index"].is_null() && device["index"] == gpu["index"] {
                return true;
            }
            let device_name = string(device, "name").to_lowercase();
            !device_name.is_empty() && (device_name.contains(&name) || name.contains(&device_name))
        })
        .unwrap_or(&Value::Null)
}
pub fn gpu_memory(gpu: &Value, nvidia: &Value) -> (f64, f64) {
    if let Some(total) = nvidia["memory_total_mib"].as_f64() {
        (number(&nvidia["memory_used_mib"]) * MIB, total * MIB)
    } else {
        let dedicated = &gpu["memory"]["dedicated"];
        (number(&dedicated["used"]), number(&dedicated["total"]))
    }
}
