use super::*;
use crate::bios::spec;
use crate::rows::Kind;
use std::fs;

fn spec_of(text: &str) -> Spec {
    spec::parse(text).unwrap()
}

fn fake() -> (tempfile::TempDir, Sysfs) {
    let root = tempfile::tempdir().unwrap();
    let sys = Sysfs {
        sys: root.path().join("sys"),
        dev: root.path().join("dev"),
    };
    fs::create_dir_all(sys.sys.join("devices/system/cpu/cpu0/cpufreq")).unwrap();
    fs::create_dir_all(sys.sys.join("devices/system/cpu/cpufreq")).unwrap();
    fs::write(
        sys.sys
            .join("devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"),
        "5455945\n",
    )
    .unwrap();
    fs::write(sys.sys.join("devices/system/cpu/cpufreq/boost"), "1\n").unwrap();
    fs::create_dir_all(&sys.dev).unwrap();
    (root, sys)
}

#[test]
fn boost_offset_reads_auto_as_the_maximum() {
    let auto = spec_of(
        "p {\n  CPU Boost Clock Override = Enabled (Positive)\n  Max CPU Boost Clock Override(+) = Auto\n}",
    );
    assert_eq!(expected_boost_offset(&auto), Some(200));
    let fixed = spec_of("p {\n  Max CPU Boost Clock Override(+) = 100\n}");
    assert_eq!(expected_boost_offset(&fixed), Some(100));
    let off = spec_of(
        "p {\n  CPU Boost Clock Override = Disabled\n  Max CPU Boost Clock Override(+) = Auto\n}",
    );
    assert_eq!(expected_boost_offset(&off), Some(0));
    assert_eq!(expected_boost_offset(&spec_of("p {\n  X = 1\n}")), None);
}

#[test]
fn boost_row_tolerates_amd_pstate_rounding() {
    let (_root, sys) = fake();
    let spec = spec_of(
        "live {\n  base_boost_mhz = 5200\n}\np {\n  CPU Boost Clock Override = Enabled (Positive)\n  Max CPU Boost Clock Override(+) = Auto\n}",
    );
    let row = boost(&spec, &sys).unwrap();
    assert_eq!(row.kind, Kind::Ok, "{}", row.summary);
    let higher = spec_of(
        "live {\n  base_boost_mhz = 5400\n}\np {\n  CPU Boost Clock Override = Enabled (Positive)\n  Max CPU Boost Clock Override(+) = Auto\n}",
    );
    assert_eq!(boost(&higher, &sys).unwrap().kind, Kind::Bad);
    fs::write(sys.sys.join("devices/system/cpu/cpufreq/boost"), "0\n").unwrap();
    assert_eq!(boost(&spec, &sys).unwrap().kind, Kind::Warn);
}

#[test]
fn kvm_row_follows_the_device_node() {
    let (_root, sys) = fake();
    let spec = spec_of("p {\n  SVM Mode = Enabled\n}");
    assert_eq!(kvm(&spec, &sys).unwrap().kind, Kind::Bad);
    fs::write(sys.dev.join("kvm"), "").unwrap();
    assert_eq!(kvm(&spec, &sys).unwrap().kind, Kind::Ok);
    assert!(kvm(&spec_of("p {\n  X = 1\n}"), &sys).is_none());
}

#[test]
fn dmi_type17_reads_configured_speed() {
    let mut raw = vec![0u8; 0x60];
    raw[0x0C] = 0x00;
    raw[0x0D] = 0x40;
    raw[0x15..0x17].copy_from_slice(&6000u16.to_le_bytes());
    raw[0x20..0x22].copy_from_slice(&0xFFFFu16.to_le_bytes());
    raw[0x54..0x58].copy_from_slice(&6000u32.to_le_bytes());
    assert_eq!(parse_type17(&raw), Some((6000, 6000)));
    raw[0x20..0x22].copy_from_slice(&5600u16.to_le_bytes());
    assert_eq!(parse_type17(&raw), Some((6000, 5600)));
    let empty = vec![0u8; 0x60];
    assert_eq!(parse_type17(&empty), None);
    assert_eq!(parse_type17(&[0u8; 4]), None);
}

#[test]
fn dmidecode_text_yields_one_speed_per_dimm() {
    let text = "Memory Device\n\tConfigured Memory Speed: 6000 MT/s\nMemory Device\n\tConfigured Memory Speed: Unknown\n\tConfigured Memory Speed: 6000 MT/s\n";
    assert_eq!(parse_dmidecode(text), vec![6000, 6000]);
}

#[test]
fn spd_decodes_part_and_vendors() {
    let mut spd = vec![0u8; 1024];
    spd[512] = 0x02;
    spd[513] = 0x9E;
    spd[521..539].copy_from_slice(b"CMK32GX5M2B6000Z30");
    spd[552] = 0x80;
    spd[553] = 0xAD;
    assert_eq!(
        decode_spd(&spd).unwrap(),
        "CMK32GX5M2B6000Z30 (Corsair module, SK hynix DRAM)"
    );
    assert_eq!(jep106(0x80, 0xCE), Some("Samsung"));
    assert_eq!(jep106(0x80, 0x01), None);
    assert!(decode_spd(&[0u8; 10]).is_none());
}

#[test]
fn qfan_compares_the_first_four_points() {
    let (_root, sys) = fake();
    let chip_dir = sys.sys.join("class/hwmon/hwmon0");
    fs::create_dir_all(&chip_dir).unwrap();
    fs::write(chip_dir.join("name"), "nct6799\n").unwrap();
    for (index, (temp, pwm)) in [(30, 38), (40, 38), (50, 38), (60, 51), (125, 255)]
        .iter()
        .enumerate()
    {
        let point = index + 1;
        fs::write(
            chip_dir.join(format!("pwm2_auto_point{point}_temp")),
            format!("{}\n", temp * 1000),
        )
        .unwrap();
        fs::write(
            chip_dir.join(format!("pwm2_auto_point{point}_pwm")),
            format!("{pwm}\n"),
        )
        .unwrap();
    }
    let chip = Hwmon::find(&sys, "nct6799").unwrap();
    let mut text = String::from("q {\n");
    for (point, (temp, duty)) in [(30, 15), (40, 15), (50, 15), (60, 20)].iter().enumerate() {
        text.push_str(&format!(
            "  CPU Fan Point{} Temperature = {temp}\n  CPU Fan Point{} Duty Cycle (%) = {duty}\n",
            point + 1,
            point + 1
        ));
    }
    text.push('}');
    let rows = qfan(&spec_of(&text), &chip);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, Kind::Ok, "{}", rows[0].summary);
    let drifted = text.replace("Point4 Duty Cycle (%) = 20", "Point4 Duty Cycle (%) = 30");
    let rows = qfan(&spec_of(&drifted), &chip);
    assert_eq!(rows[0].kind, Kind::Warn);
    assert!(rows[0].details[0].contains("point 4"));
}

#[test]
fn rebar_needs_a_large_bar() {
    let (_root, sys) = fake();
    let device = sys.sys.join("bus/pci/devices/0000:01:00.0");
    fs::create_dir_all(&device).unwrap();
    fs::write(
        device.join("resource"),
        "0x0000006000000000 0x00000063ffffffff 0x000000000014220c\n",
    )
    .unwrap();
    let spec = spec_of("live {\n  gpu = 0000:01:00.0\n}\np {\n  Resize BAR Support = Enabled\n}");
    assert_eq!(rebar(&spec, &sys).unwrap().kind, Kind::Ok);
    fs::write(
        device.join("resource"),
        "0x0000000000000000 0x000000000fffffff 0x0\n",
    )
    .unwrap();
    assert_eq!(rebar(&spec, &sys).unwrap().kind, Kind::Bad);
}
