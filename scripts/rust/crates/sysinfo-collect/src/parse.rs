#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::collections::HashSet;

pub fn pruned_cpu_name(raw: &str) -> String {
    let mut name = raw.to_string();
    for junk in ["(R)", "(TM)", "(tm)", "®", "™"] {
        name = name.replace(junk, "");
    }
    if let Some(position) = name.find('@') {
        name.truncate(position);
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        let next = words.get(index + 1).copied().unwrap_or_default();
        if word.ends_with("-Core") && next == "Processor" {
            index += 2;
            continue;
        }
        if word == "CPU" || word == "Processor" {
            index += 1;
            continue;
        }
        if word == "with" && next == "Radeon" {
            break;
        }
        kept.push(word);
        index += 1;
    }
    kept.join(" ")
}

pub fn x86_march(flags: &str) -> &'static str {
    let held: HashSet<&str> = flags.split_whitespace().collect();
    let has = |names: &[&str]| names.iter().all(|name| held.contains(name));
    if has(&["avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"]) {
        "x86_64-v4"
    } else if has(&["avx2", "bmi2", "fma", "movbe", "xsave"]) {
        "x86_64-v3"
    } else if has(&["sse4_2", "popcnt", "ssse3", "cx16"]) {
        "x86_64-v2"
    } else {
        "x86_64"
    }
}

pub fn os_release_field(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        if let Some(value) = line.strip_prefix(key)
            && let Some(value) = value.strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub fn cpuinfo_value(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        if let Some((name, value)) = line.split_once(':')
            && name.trim() == key
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

pub fn amdgpu_name_in(body: &str, device_id: &str, revision: &str) -> Option<String> {
    let device = device_id.trim_start_matches("0x").to_uppercase();
    let revision = revision.trim_start_matches("0x").to_uppercase();
    for line in body.lines() {
        let fields: Vec<&str> = line.split(",\t").map(str::trim).collect();
        if fields.len() >= 3 && fields[0] == device && fields[1] == revision {
            return Some(fields[2].to_string());
        }
    }
    None
}

pub fn pci_device_name_in(body: &str, vendor_id: &str, device_id: &str) -> Option<String> {
    let vendor = vendor_id.trim_start_matches("0x").to_lowercase();
    let device = device_id.trim_start_matches("0x").to_lowercase();
    let mut in_vendor = false;
    for line in body.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            in_vendor = line.starts_with(&vendor);
            continue;
        }
        if in_vendor && !line.starts_with("\t\t") {
            let entry = line.trim_start();
            if let Some(rest) = entry.strip_prefix(&device) {
                let name = rest.trim();
                if let (Some(open), Some(close)) = (name.find('['), name.rfind(']')) {
                    return Some(name[open + 1..close].to_string());
                }
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "../tests/unit/parse_tests.rs"]
mod tests;
