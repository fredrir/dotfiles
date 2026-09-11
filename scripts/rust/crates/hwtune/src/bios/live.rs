use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use hostkit::process::{self, CaptureLimits};

use crate::bios::spec::Spec;
use crate::cpu;
use crate::env::{self, Sysfs};
use crate::gpu;
use crate::hwmon::{self, Hwmon};
use crate::rows::Row;

pub const QFAN_HEADERS: [(&str, u8); 4] = [
    ("CPU Fan", 2),
    ("Chassis Fan 1", 1),
    ("Chassis Fan 2", 3),
    ("AIO Pump", 7),
];

const BOOST_TOLERANCE_MHZ: u64 = 50;
const REBAR_MINIMUM: u64 = 8 << 30;
const SPD_PART: std::ops::Range<usize> = 521..551;
const SPD_MODULE_VENDOR: usize = 512;
const SPD_DRAM_VENDOR: usize = 552;

pub fn expected_boost_offset(spec: &Spec) -> Option<u32> {
    let value = spec.value("Max CPU Boost Clock Override(+)")?;
    if let Ok(offset) = value.parse::<u32>() {
        return Some(offset);
    }
    let enabled = spec
        .value("CPU Boost Clock Override")
        .is_some_and(|mode| mode.starts_with("Enabled"));
    Some(if value == "Auto" && enabled { 200 } else { 0 })
}

pub fn boost(spec: &Spec, sys: &Sysfs) -> Option<Row> {
    let base = spec.live.base_boost_mhz?;
    let offset = expected_boost_offset(spec)?;
    let expected = u64::from(base + offset);
    let freq = match cpu::cpufreq(sys) {
        Ok(freq) => freq,
        Err(e) => return Some(Row::warn("boost", e)),
    };
    let ceiling = freq.max_khz / 1000;
    let summary = format!("ceiling {ceiling} MHz, expected {expected} (+{offset})");
    Some(if freq.boost == Some(false) {
        Row::warn("boost", format!("cpufreq boost is off; {summary}"))
    } else if ceiling + BOOST_TOLERANCE_MHZ >= expected {
        Row::ok("boost", summary)
    } else {
        Row::bad("boost", summary)
    })
}

pub fn kvm(spec: &Spec, sys: &Sysfs) -> Option<Row> {
    let svm = spec.value("SVM Mode")?;
    let present = sys.dev.join("kvm").exists();
    Some(match (svm == "Enabled", present) {
        (true, true) => Row::ok("svm", "/dev/kvm present"),
        (true, false) => Row::bad("svm", "/dev/kvm missing"),
        (false, false) => Row::ok("svm", "disabled, /dev/kvm absent"),
        (false, true) => Row::bad("svm", "disabled in spec but /dev/kvm present"),
    })
}

pub fn parse_type17(raw: &[u8]) -> Option<(u16, u32)> {
    if raw.len() < 0x22 {
        return None;
    }
    let size = u16::from_le_bytes([raw[0x0C], raw[0x0D]]);
    if size == 0 {
        return None;
    }
    let speed = u16::from_le_bytes([raw[0x15], raw[0x16]]);
    let configured = u16::from_le_bytes([raw[0x20], raw[0x21]]);
    let configured = if configured == 0xFFFF && raw.len() >= 0x58 {
        u32::from_le_bytes([raw[0x54], raw[0x55], raw[0x56], raw[0x57]])
    } else {
        u32::from(configured)
    };
    Some((speed, configured))
}

pub fn parse_dmidecode(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("Configured Memory Speed:"))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|number| number.parse().ok())
        .collect()
}

fn dmi_raw_speeds(sys: &Sysfs) -> Option<Vec<u32>> {
    let entries = fs::read_dir(sys.sys.join("firmware/dmi/entries")).ok()?;
    let mut speeds = Vec::new();
    let mut readable = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("17-") {
            continue;
        }
        let raw = fs::read(entry.path().join("raw")).ok()?;
        readable = true;
        if let Some((_, configured)) = parse_type17(&raw) {
            speeds.push(configured);
        }
    }
    readable.then_some(speeds)
}

fn dmidecode_speeds() -> Option<Vec<u32>> {
    let mut command = Command::new("sudo");
    command.args(["-n", "dmidecode", "-t", "17"]);
    let captured = process::output(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(10),
    )
    .ok()?;
    captured
        .status
        .success()
        .then(|| parse_dmidecode(&String::from_utf8_lossy(&captured.stdout)))
}

pub fn memory_speed(spec: &Spec, sys: &Sysfs) -> Option<Row> {
    let expected = spec.live.memory_mts?;
    let speeds = match dmi_raw_speeds(sys).or_else(dmidecode_speeds) {
        Some(speeds) => speeds,
        None => return Some(Row::note("memory", "needs root: sudo -n dmidecode -t 17")),
    };
    let populated = speeds
        .iter()
        .filter(|speed| **speed > 0)
        .collect::<Vec<_>>();
    if populated.is_empty() {
        return Some(Row::warn("memory", "no populated DIMM reported"));
    }
    let listed = populated
        .iter()
        .map(|speed| speed.to_string())
        .collect::<Vec<_>>()
        .join("/");
    Some(if populated.iter().all(|speed| **speed == expected) {
        Row::ok("memory", format!("{listed} MT/s configured"))
    } else {
        Row::bad(
            "memory",
            format!("{listed} MT/s configured, expected {expected}"),
        )
    })
}

pub fn jep106(continuation: u8, code: u8) -> Option<&'static str> {
    match (continuation & 0x7F, code) {
        (0, 0x2C) => Some("Micron"),
        (0, 0xAD) => Some("SK hynix"),
        (0, 0xCE) => Some("Samsung"),
        (1, 0x98) => Some("Kingston"),
        (2, 0x9E) => Some("Corsair"),
        _ => None,
    }
}

pub fn decode_spd(spd: &[u8]) -> Option<String> {
    if spd.len() < SPD_DRAM_VENDOR + 2 {
        return None;
    }
    let part = String::from_utf8_lossy(&spd[SPD_PART])
        .trim_end_matches(['\0', ' '])
        .to_string();
    let vendor = |offset: usize| {
        jep106(spd[offset], spd[offset + 1])
            .map(str::to_string)
            .unwrap_or_else(|| format!("{:02x}{:02x}", spd[offset], spd[offset + 1]))
    };
    Some(format!(
        "{part} ({} module, {} DRAM)",
        vendor(SPD_MODULE_VENDOR),
        vendor(SPD_DRAM_VENDOR)
    ))
}

pub fn dimms(sys: &Sysfs) -> Vec<String> {
    let Ok(devices) = fs::read_dir(sys.sys.join("bus/i2c/devices")) else {
        return Vec::new();
    };
    let mut found = devices
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| env::read_text(&dir.join("name")).is_ok_and(|name| name == "spd5118"))
        .filter_map(|dir| fs::read(dir.join("eeprom")).ok())
        .filter_map(|spd| decode_spd(&spd))
        .collect::<Vec<_>>();
    found.sort();
    found
}

pub fn dimm_row(sys: &Sysfs) -> Row {
    let dimms = dimms(sys);
    if dimms.is_empty() {
        return Row::note("dimms", "no SPD readable");
    }
    Row::note("dimms", format!("{} × {}", dimms.len(), dimms[0]))
        .with_details(dimms.iter().skip(1).cloned().collect())
}

fn spec_points(spec: &Spec, header: &str) -> Vec<(u32, u8)> {
    (1..=4)
        .filter_map(|point| {
            let temperature = spec
                .value(&format!("{header} Point{point} Temperature"))?
                .parse()
                .ok()?;
            let duty = spec
                .value(&format!("{header} Point{point} Duty Cycle (%)"))?
                .parse()
                .ok()?;
            Some((temperature, duty))
        })
        .collect()
}

pub fn qfan(spec: &Spec, chip: &Hwmon) -> Vec<Row> {
    QFAN_HEADERS
        .iter()
        .filter_map(|(header, channel)| {
            let expected = spec_points(spec, header);
            if expected.len() < 4 {
                return None;
            }
            let label = format!("q-fan {}", header.to_lowercase());
            let actual = match chip.auto_points(*channel) {
                Ok(points) => points,
                Err(e) => return Some(Row::warn(label, e)),
            };
            let details = expected
                .iter()
                .zip(actual.iter())
                .enumerate()
                .filter(|(_, ((temp, duty), (chip_temp, chip_pwm)))| {
                    temp != chip_temp || !hwmon::duty_matches(*chip_pwm, *duty)
                })
                .map(|(index, ((temp, duty), (chip_temp, chip_pwm)))| {
                    format!(
                        "point {}: chip {}°C {}%, spec {temp}°C {duty}%",
                        index + 1,
                        chip_temp,
                        hwmon::duty_pct(*chip_pwm)
                    )
                })
                .collect::<Vec<_>>();
            let duties = expected
                .iter()
                .map(|(_, duty)| duty.to_string())
                .collect::<Vec<_>>()
                .join("/");
            Some(if actual.len() < 4 {
                Row::warn(label, format!("chip exposes {} points", actual.len()))
            } else if details.is_empty() {
                Row::ok(label, format!("points 1-4 programmed ({duties} %)"))
            } else {
                Row::warn(label, "chip points differ from spec").with_details(details)
            })
        })
        .collect()
}

pub fn rebar(spec: &Spec, sys: &Sysfs) -> Option<Row> {
    let enabled = spec.value("Resize BAR Support")? == "Enabled";
    let bdf = spec.live.gpu.as_deref()?;
    let sizes = match gpu::bar_sizes(sys, bdf) {
        Ok(sizes) => sizes,
        Err(e) => return Some(Row::warn("rebar", e)),
    };
    let largest = sizes.iter().copied().max().unwrap_or(0);
    let gib = largest >> 30;
    Some(match (enabled, largest >= REBAR_MINIMUM) {
        (true, true) => Row::ok("rebar", format!("largest BAR {gib} GiB")),
        (true, false) => Row::bad("rebar", format!("largest BAR {gib} GiB, expected ≥ 8")),
        (false, _) => Row::note("rebar", format!("disabled in spec, largest BAR {gib} GiB")),
    })
}

pub fn all(spec: &Spec, sys: &Sysfs, chip: Option<&Hwmon>) -> Vec<Row> {
    let mut rows = Vec::new();
    rows.extend(boost(spec, sys));
    rows.extend(kvm(spec, sys));
    rows.extend(memory_speed(spec, sys));
    rows.extend(rebar(spec, sys));
    match chip {
        Some(chip) => rows.extend(qfan(spec, chip)),
        None => rows.push(Row::note("q-fan", "nct6799 hwmon not found")),
    }
    rows.push(dimm_row(sys));
    rows
}

pub fn spd_path_is_dimm(path: &Path) -> bool {
    env::read_text(&path.join("name")).is_ok_and(|name| name == "spd5118")
}

#[cfg(test)]
#[path = "../../tests/unit/bios/live_tests.rs"]
mod tests;
