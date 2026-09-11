use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use workstation_sysinfo::{
    collect, formatting, health, inventory,
    model::*,
    presentation::{self, branding},
};

fn snapshot(macos: bool) -> Snapshot {
    serde_json::from_str(if macos {
        include_str!("fixtures/macos_snapshot.json")
    } else {
        include_str!("fixtures/workstation_snapshot.json")
    })
    .unwrap()
}
fn colors() -> presentation::Colors {
    presentation::Colors::from_palette(&json!({"version":1,"colors":{"fg":"#cdd6f4","muted":"#a6adc8","separator":"#45475a","green":"#a6e3a1","yellow":"#f9e2af","red":"#f38ba8"},"roles":{"section_system":"#89b4fa","section_hardware":"#fab387","section_desktop":"#cba6f7"}})).unwrap()
}

#[test]
fn normalized_platform_reports_preserve_the_schema() {
    for (macos, expected) in [
        (false, include_str!("fixtures/workstation_expected.json")),
        (true, include_str!("fixtures/macos_expected.json")),
    ] {
        let snapshot = snapshot(macos);
        let expected: Value = serde_json::from_str(expected).unwrap();
        assert_eq!(
            serde_json::to_value(presentation::build_view(&snapshot)).unwrap(),
            expected["view"]
        );
        assert_eq!(
            serde_json::to_value(health::health_issues(&snapshot)).unwrap(),
            expected["health"]
        );
    }
}
fn pretty(
    snapshot: &Snapshot,
    width: usize,
    full: bool,
    explain: bool,
    issues: &[HealthIssue],
) -> String {
    let view = presentation::build_view(snapshot);
    presentation::render_with(
        &view,
        issues,
        RenderOptions {
            full,
            health: explain,
        },
        presentation::PrettyContext {
            colors: &colors(),
            width,
            username: "fredrir",
            hostname: if snapshot.is_macos() {
                "macie"
            } else {
                "archie"
            },
            colored: false,
        },
    )
}

#[test]
fn workstation_components_retain_brand_models_facts_and_privacy() {
    let view = presentation::build_view(&snapshot(false));
    for (label, model) in [
        ("CPU", "Ryzen 7 9800X3D"),
        ("GPU", "GeForce RTX 5070 Ti"),
        ("MEMORY", "32 GB  DDR5-6000  CL30"),
        ("MOTHERBOARD", "B850-PLUS WIFI"),
        ("STORAGE", "NV1  2 TB  SSD"),
        ("STORAGE", "WD Blue  2 TB  HDD"),
    ] {
        assert!(
            view.components
                .iter()
                .any(|c| c.label == label && c.model == model),
            "{label} {model}: {:?}",
            view.components
        );
    }
    assert!(
        !view
            .components
            .iter()
            .find(|c| c.label == "INTEGRATED GPU")
            .unwrap()
            .compact
    );
    assert_eq!(view.machine_type, "WORKSTATION");
    assert_eq!(view.summary, ["Arch Linux", "KDE Plasma", "Wayland"]);
    assert_eq!(
        view.software
            .iter()
            .map(|b| b.kind.as_str())
            .collect::<Vec<_>>(),
        ["hyprland", "wm", "session", "terminal", "shell"]
    );
    assert!(!serde_json::to_string(&view).unwrap().contains("PRIVATE"));
}
#[test]
fn macos_normalizes_unified_memory_virtual_disks_and_portable_power() {
    let snapshot = snapshot(true);
    let view = presentation::build_view(&snapshot);
    assert_eq!(view.platform.label, "macOS 26.0");
    assert_eq!(view.machine_type, "WORKSTATION");
    let component = |label: &str| view.components.iter().find(|c| c.label == label).unwrap();
    assert_eq!(component("MEMORY").vendor, "APPLE");
    assert_eq!(component("MEMORY").model, "24 GB");
    assert_eq!(
        view.components
            .iter()
            .filter(|c| c.label == "STORAGE")
            .count(),
        1
    );
    assert_eq!(component("STORAGE").model, "SSD AP1024Z Media  1 TB  SSD");
    assert_eq!(component("BATTERY").model, "Internal battery");
    assert_eq!(component("BATTERY").facts[1].value, "AC Connected");
    assert_eq!(component("POWER ADAPTER").model, "70 W");
    assert!(component("POWER ADAPTER").facts.is_empty());
    assert!(
        !view
            .components
            .iter()
            .any(|c| ["CPU COOLING", "CHASSIS", "POWER SUPPLY"].contains(&c.label.as_str()))
    );
    assert!(health::health_issues(&snapshot).is_empty());
}
#[test]
fn health_reports_memory_gpu_disk_limits_and_directions() {
    let mut snapshot = snapshot(false);
    snapshot.modules.insert(
        "Memory".into(),
        json!({"used":31u64*1024u64.pow(3),"total":32u64*1024u64.pow(3)}),
    );
    snapshot.modules.insert(
        "Disk".into(),
        json!([{"mountpoint":"/","bytes":{"used":95,"total":100}}]),
    );
    snapshot
        .hardware
        .insert("memory".into(), "Corsair 64 GB DDR5".into());
    snapshot.nvidia[0]["temperature"] = json!(90);
    let issues = health::health_issues(&snapshot);
    assert_eq!(issues.len(), 4);
    let text = serde_json::to_string(&issues).unwrap();
    for expected in [
        "Configured as 64 GB, detected as 32 GB",
        "Memory is 97% used and swap is disabled",
        "90°C measured, 88°C maximum",
        "95% of the filesystem is used",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert_eq!(health::health_summary(&issues), "2 errors  2 warnings");
}
#[test]
fn inactive_health_never_advises_swap_and_driver_mismatch_has_reboot_action() {
    let mut snapshot = snapshot(false);
    assert!(health::health_issues(&snapshot).is_empty());
    snapshot
        .probe_errors
        .push("NVIDIA kernel driver does not match the installed userspace library".into());
    let issues = health::health_issues(&snapshot);
    assert_eq!(issues[0].severity, Severity::Error);
    assert_eq!(
        issues[0].action,
        "Reboot to load the updated NVIDIA kernel module"
    );
}
#[test]
fn battery_charging_and_missing_temperature_are_not_false_alarms() {
    let mut snapshot = snapshot(false);
    for (status, capacity, count, severity) in [
        (json!("Discharging"), 12, 1, Severity::Warning),
        (json!("Discharging"), 4, 1, Severity::Error),
        (json!("Charging"), 4, 0, Severity::Error),
        (json!(["AC Connected"]), 4, 0, Severity::Error),
        (json!("Full"), 12, 0, Severity::Error),
    ] {
        snapshot.modules.insert(
            "Battery".into(),
            json!([{"capacity":capacity,"status":status}]),
        );
        let issues = health::health_issues(&snapshot);
        assert_eq!(issues.len(), count);
        if count > 0 {
            assert_eq!(issues[0].severity, severity);
        }
    }
    assert!(health::temperature_issue("unknown", None, Some(90.0)).is_none());
    assert!(health::temperature_issue("unknown", Some(f64::NAN), Some(90.0)).is_none());
    assert!(health::temperature_issue("unknown", Some(90.0), None).is_none());
}
#[test]
fn filesystem_health_excludes_readonly_virtual_and_system_volumes() {
    for disk in [
        json!({"readOnly":true}),
        json!({"filesystem":"tmpfs"}),
        json!({"filesystem":"squashfs"}),
        json!({"name":"Disk Image"}),
        json!({"volumeType":["Read-Only"]}),
        json!({"mountpoint":"/System/Volumes/Preboot"}),
    ] {
        assert!(!formatting::is_actionable_filesystem(&disk), "{disk}");
    }
    assert!(formatting::is_actionable_filesystem(
        &json!({"mountpoint":"/System/Volumes/Data","filesystem":"apfs"})
    ));
}
#[test]
fn plain_default_and_full_modes_preserve_compact_selection() {
    let view = presentation::build_view(&snapshot(false));
    let compact = presentation::render_plain(&view, &[], RenderOptions::default());
    assert!(compact.starts_with("System: Arch Linux  KDE Plasma  Wayland\n"));
    assert!(compact.contains("CPU: AMD Ryzen 7 9800X3D"));
    assert!(!compact.contains("INTEGRATED GPU"));
    assert!(!compact.contains("CPU COOLING"));
    let full = presentation::render_plain(
        &view,
        &[],
        RenderOptions {
            full: true,
            health: false,
        },
    );
    for expected in [
        "System\n",
        "Hardware\n",
        "Software\n",
        "CPU COOLING",
        "8 cores / 16 threads",
        "96 MB L3",
        "PCIe 5.0 ×16",
    ] {
        assert!(full.contains(expected), "{full}");
    }
    assert!(!full.contains("PRIVATE"));
}
#[test]
fn branded_rendering_is_complete_borderless_and_respects_terminal_cells() {
    for width in [36, 45, 70, 80, 93, 94, 120, 132] {
        let text = pretty(&snapshot(false), width, false, false, &[]);
        assert!(text.contains("FREDRIR   WORKSTATION"));
        assert!(text.contains("HARDWARE"));
        assert!(text.contains('█'));
        assert!(!text.contains("SOFTWARE"));
        assert!(!text.contains("SYSTEM\n"));
        assert!(!text.contains("PRIVATE"));
        assert!(!text.starts_with('╭'));
        assert!(
            text.lines()
                .all(|line| unicode_width::UnicodeWidthStr::width(line) <= width),
            "{width}: {text}"
        );
    }
    let text = pretty(&snapshot(false), 120, false, false, &[]);
    for expected in [
        "AMD",
        "NVIDIA",
        "CORSAIR",
        "ASUS TUF",
        "KINGSTON",
        "WESTERN DIGITAL",
        "ARCH LINUX",
        "KDE PLASMA",
        "KWIN",
        "WAYLAND",
        "8 cores / 16 threads",
        "96 MB L3",
        "120 W TDP",
        "PCIe 5.0 ×16",
        "NOCTUA",
        "ARCTIC",
        "POWER SUPPLY",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(text.lines().position(|line| line == "HARDWARE").unwrap() >= 12);
}
#[test]
fn pretty_full_and_health_flags_are_independent() {
    let issues = [HealthIssue {
        severity: Severity::Warning,
        title: "Synthetic warning".into(),
        detail: "Diagnostic text".into(),
        action: "Take action".into(),
    }];
    let full = pretty(&snapshot(false), 120, true, false, &issues);
    assert!(full.contains("SOFTWARE"));
    assert!(full.contains("SYSTEM"));
    assert!(full.contains("1 warning"));
    assert!(!full.contains("Diagnostic text"));
    let health = pretty(&snapshot(false), 120, false, true, &issues);
    assert!(health.contains("HEALTH"));
    assert!(health.contains("Diagnostic text"));
    assert!(health.contains("Take action"));
    assert!(!health.contains("SOFTWARE"));
}
#[test]
fn portable_pretty_uses_only_actual_components() {
    let text = pretty(&snapshot(true), 100, true, true, &[]);
    for absent in [
        "PRIVATE",
        "Apple Disk Image",
        "Virtual Interface",
        "CPU COOLING",
        "POWER SUPPLY",
        "no active warnings",
    ] {
        assert!(!text.contains(absent), "{text}");
    }
    assert!(text.contains("Internal battery"));
    assert!(text.contains("70 W"));
}
#[test]
fn brand_registry_matches_word_boundaries_specific_brands_and_classes() {
    for (kind, identity, key) in [
        ("cpu", "AuthenticAMD Ryzen 9 10950X3D", "amd"),
        ("gpu", "NVIDIA GeForce RTX 6090", "nvidia"),
        ("gpu", "Intel Arc B990", "intel"),
        ("memory", "G.Skill Trident Z5", "gskill"),
        ("memory", "SK hynix DDR5", "sk_hynix"),
        ("motherboard", "ASUSTeK TUF GAMING X990", "asus_tuf"),
        ("motherboard", "ASUS ROG STRIX X990", "asus_rog"),
        ("motherboard", "Gigabyte AORUS MASTER", "aorus"),
        ("storage", "ATA WDC WD20EZRZ", "western_digital"),
        ("hyprland", "KDE Plasma", "kde"),
        ("terminal", "Ghostty", "ghostty"),
    ] {
        assert_eq!(branding::resolve_brand(kind, &[identity]).key, key);
    }
    assert_eq!(
        branding::resolve_brand("gpu", &["Future Silicon Company", "Photon 9000"]).name,
        "GRAPHICS"
    );
    assert_eq!(branding::resolve_brand("cpu", &["charm"]).key, "cpu");
    let arts = [
        ("cpu", "AMD Ryzen"),
        ("gpu", "NVIDIA GeForce"),
        ("memory", "Corsair DDR5"),
        ("motherboard", "ASUS TUF GAMING"),
        ("storage", "KINGSTON SNVS2000G"),
        ("storage", "WDC WD20EZRZ"),
    ]
    .map(|(kind, name)| branding::illustration(branding::resolve_brand(kind, &[name]), kind));
    assert_eq!(arts.into_iter().collect::<HashSet<_>>().len(), 6);
}
#[test]
fn pretty_preserves_hostname_art_indentation_and_handles_tiny_terminals() {
    let machine = snapshot(false);
    let output = pretty(&machine, 70, false, false, &[]);
    let art = branding::block_text("archie");
    assert!(output.contains(&art.join("\n")));
    for width in [1, 2, 3, 10, 20] {
        let output = pretty(&machine, width, true, true, &[]);
        assert!(
            output
                .lines()
                .all(|line| unicode_width::UnicodeWidthStr::width(line) <= width)
        );
    }
}
#[test]
fn quantities_preserve_decimal_disks_binary_memory_and_rounding() {
    for (value, expected) in [
        (2_000_398_934_016.0, "2 TB"),
        (1_850_000_000_000.0, "1.9 TB"),
        (512_110_190_592.0, "512.1 GB"),
        (157_286_400.0, "157.3 MB"),
        (0.0, "unknown"),
    ] {
        assert_eq!(formatting::capacity(value), expected);
    }
    assert_eq!(formatting::memory_capacity(33_538_248_704.0), "32 GB");
    assert_eq!(formatting::memory_capacity(68_719_476_736.0), "64 GB");
    assert_eq!(
        formatting::configured_memory_bytes("Corsair 64 GB DDR5"),
        64.0 * formatting::GIB
    );
    assert_eq!(formatting::format_duration(93_784_000.0), "1d 2h 3m");
}
#[test]
fn nvidia_csv_supports_multiple_devices_missing_fields_and_quoted_names() {
    let devices=collect::parse_nvidia(b"0, NVIDIA GeForce RTX 5070 Ti, 16303, 2048, 8, 39, 32.5, 300, 2805, 610.43.03\n1, \"NVIDIA, T400\", 4096, 512, 2, 35, N/A, 30, 420, 610.43.03\nmalformed\n");
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0]["power_draw"], 32.5);
    assert_eq!(devices[0]["memory_total_mib"], 16303.0);
    assert!(devices[1]["power_draw"].is_null());
    assert_eq!(devices[1]["name"], "NVIDIA, T400");
}
#[test]
fn module_index_ignores_errors_and_unrecognized_terminals() {
    let modules = collect::index_modules(vec![
        json!({"type":"CPU","result":{"cpu":"Test"}}),
        json!({"type":"GPU","error":"absent"}),
        json!({"error":"bad"}),
    ]);
    assert_eq!(modules.len(), 1);
    assert!(collect::recognized_terminal(
        &json!({"prettyName":"GNOME Terminal"})
    ));
    assert!(!collect::recognized_terminal(&json!({"prettyName":"tmux"})));
}
#[test]
fn palette_schema_colors_and_missing_roles_fail_actionably() {
    let mut palette = json!({"version":2});
    assert!(
        presentation::Colors::from_palette(&palette)
            .unwrap_err()
            .contains("unsupported")
    );
    palette["version"] = json!(1);
    assert!(
        presentation::Colors::from_palette(&palette)
            .unwrap_err()
            .contains("missing")
    );
    palette["colors"] = json!({"fg":"#GG0000"});
    assert!(
        presentation::Colors::from_palette(&palette)
            .unwrap_err()
            .contains("invalid")
    );
}
fn inventory_context(root: &std::path::Path) -> inventory::InventoryContext {
    inventory::InventoryContext {
        root: root.into(),
        host: None,
        config: None,
        state_file: root.join("host-pin"),
    }
}
#[test]
fn inventory_keeps_whole_line_comments_and_hardware_part_hashes() {
    let hosts=inventory::parse_hosts("# machines\narchie {\n host names = other\n HOSTNAMES = Archie, ARCHIE.local\n ROLE = hyprland\n CPU_COOLER = Model # 2\n}\nmacie {\n role = laptop\n}\n").unwrap();
    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0].hostnames, ["Archie", "ARCHIE.local"]);
    assert_eq!(hosts[0].hardware["cpu_cooler"], "Model # 2");
    assert_eq!(hosts[0].resolved_hardware()["case"], "not set");
    assert_eq!(
        inventory::match_hostname(&hosts, &["ARCHIE.LOCAL".into()]),
        "archie"
    );
    assert_eq!(
        inventory::parse_hosts(&inventory::render_host(&hosts[0])).unwrap(),
        vec![hosts[0].clone()]
    );
}
#[test]
fn inventory_rejects_unsafe_names_and_malformed_blocks() {
    for source in [
        "../escape {\n}\n",
        "-option {\n}\n",
        "archie {\n malformed field\n}\n",
        "archie {\n nested {\n}\n",
        "archie {\n",
    ] {
        assert!(inventory::parse_hosts(source).is_err(), "{source}");
    }
}
#[test]
fn inventory_precedence_is_explicit_environment_pin_hostname() {
    let temporary = tempfile::tempdir().unwrap();
    let mut context = inventory_context(temporary.path());
    let hosts = inventory::parse_hosts("archie {\n hostnames = machine.local\n}\n").unwrap();
    std::fs::write(&context.state_file, "pinned\n").unwrap();
    context.host = Some("environment".into());
    let names = vec!["MACHINE.local".into()];
    assert_eq!(
        inventory::resolve_with(&context, &hosts, "explicit", &names),
        "explicit"
    );
    assert_eq!(
        inventory::resolve_with(&context, &hosts, "", &names),
        "environment"
    );
    context.host = None;
    assert_eq!(
        inventory::resolve_with(&context, &hosts, "", &names),
        "pinned"
    );
    context.state_file = PathBuf::from("/unavailable-host-pin");
    assert_eq!(
        inventory::resolve_with(&context, &hosts, "", &names),
        "archie"
    );
}
#[test]
fn injected_hostnames_keep_platform_precedence_and_fallbacks() {
    let mut commands = Vec::new();
    let names = inventory::local_hostnames_with(Some("machine.example.test"), |args| {
        commands.push(args.join(" "));
        match args {
            ["scutil", "--get", "LocalHostName"] => Some(" local-name \n".into()),
            ["scutil", "--get", "ComputerName"] => Some("local-name".into()),
            _ => panic!("unexpected hostname probe: {args:?}"),
        }
    });
    if cfg!(target_os = "macos") {
        assert_eq!(names, ["local-name", "machine.example.test", "machine"]);
        assert_eq!(commands.len(), 2);
    } else {
        assert_eq!(names, ["machine.example.test", "machine"]);
        assert!(commands.is_empty());
    }
    let names = inventory::local_hostnames_with(None, |args| {
        (args == ["hostname"]).then(|| "fallback.local\n".into())
    });
    assert_eq!(names, ["fallback.local", "fallback"]);
    assert!(inventory::local_hostnames_with(None, |_| None).is_empty());
}
#[test]
fn cli_parser_accepts_all_existing_flag_combinations_and_new_report_modes() {
    workstation_sysinfo::cli::command().debug_assert();
    for args in [
        vec![],
        vec!["-p"],
        vec!["-f"],
        vec!["-hh"],
        vec!["-p", "-f"],
        vec!["-p", "-hh"],
        vec!["-f", "-hh"],
        vec!["-p", "-f", "-hh"],
        vec!["-pf"],
        vec!["--json", "--timings"],
    ] {
        let args = std::iter::once("sysinfo")
            .chain(args)
            .map(std::ffi::OsString::from);
        assert!(
            workstation_sysinfo::cli::command()
                .try_get_matches_from(workstation_sysinfo::cli::normalize_arguments(args))
                .is_ok()
        );
    }
}
