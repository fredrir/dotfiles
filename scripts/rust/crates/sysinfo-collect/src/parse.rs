//! Pure parsing used by the Linux collector, kept platform-neutral so the
//! logic is unit-tested on every platform — the Linux boxes this must be
//! right on are exactly the machines this crate is not developed on.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::collections::HashSet;

/// fastfetch prunes marketing noise from /proc/cpuinfo model names; the bench
/// epoch stores the pruned form ("AMD Ryzen 7 9800X3D", not
/// "AMD Ryzen 7 9800X3D 8-Core Processor"), so the same pruning must happen
/// here.
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

/// psABI micro-architecture level from /proc/cpuinfo flags.
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

/// Marketing name from libdrm's amdgpu.ids ("did, rid, name" rows).
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

/// Device name from pci.ids; the bracketed marketing name wins when present,
/// which is how "GB203 [GeForce RTX 5070 Ti]" becomes "GeForce RTX 5070 Ti".
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
mod tests {
    use super::*;

    #[test]
    fn prunes_the_names_the_committed_baselines_depend_on() {
        assert_eq!(
            pruned_cpu_name("AMD Ryzen 7 9800X3D 8-Core Processor"),
            "AMD Ryzen 7 9800X3D"
        );
        assert_eq!(
            pruned_cpu_name("Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz"),
            "Intel Core i7-9700K"
        );
        assert_eq!(
            pruned_cpu_name("AMD Ryzen 7 5700G with Radeon Graphics"),
            "AMD Ryzen 7 5700G"
        );
    }

    #[test]
    fn ranks_march_levels() {
        assert_eq!(
            x86_march("avx512f avx512bw avx512cd avx512dq avx512vl avx2 bmi2 fma movbe xsave"),
            "x86_64-v4"
        );
        assert_eq!(x86_march("avx2 bmi2 fma movbe xsave sse4_2"), "x86_64-v3");
        assert_eq!(x86_march("sse4_2 popcnt ssse3 cx16"), "x86_64-v2");
        assert_eq!(x86_march("sse2"), "x86_64");
    }

    #[test]
    fn reads_os_release_with_and_without_quotes() {
        let body = "NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";
        assert_eq!(
            os_release_field(body, "NAME").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(os_release_field(body, "ID").as_deref(), Some("arch"));
        assert_eq!(os_release_field(body, "VERSION_ID"), None);
    }

    #[test]
    fn reads_cpuinfo_fields() {
        let body = "processor\t: 0\nmodel name\t: AMD Ryzen 7 9800X3D 8-Core Processor\n";
        assert_eq!(
            cpuinfo_value(body, "model name").as_deref(),
            Some("AMD Ryzen 7 9800X3D 8-Core Processor")
        );
    }

    #[test]
    fn resolves_gpu_names_from_the_id_databases() {
        let pci = "10de  NVIDIA Corporation\n\t2c05  GB203 [GeForce RTX 5070 Ti]\n\
                   \t\t1043 8a2c  Subsystem\n1002  Advanced Micro Devices\n";
        assert_eq!(
            pci_device_name_in(pci, "0x10de", "0x2c05").as_deref(),
            Some("GeForce RTX 5070 Ti")
        );
        assert_eq!(pci_device_name_in(pci, "0x10de", "9999"), None);
        let amd = "13C0,\t00,\tAMD Radeon Graphics\n164E,\tC1,\tAMD Radeon 610M\n";
        assert_eq!(
            amdgpu_name_in(amd, "0x13c0", "0x00").as_deref(),
            Some("AMD Radeon Graphics")
        );
    }
}
