use super::{facts, joined};
use crate::formatting::*;
use crate::model::{Fact, Snapshot, SoftwareBadge};

fn badge(kind: &str, vendor: &str, label: &str, identifiers: Vec<String>) -> SoftwareBadge {
    SoftwareBadge {
        kind: kind.into(),
        vendor: vendor.into(),
        label: label.into(),
        identifiers,
    }
}
pub fn software_badges(snapshot: &Snapshot) -> (SoftwareBadge, Vec<SoftwareBadge>) {
    let os = snapshot.result("OS");
    let de = snapshot.result("DE");
    let wm = snapshot.result("WM");
    let os_name = os["prettyName"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| os["name"].as_str())
        .unwrap_or("Unknown system");
    let platform = badge(
        "os",
        os["id"].as_str().unwrap_or(os_name),
        os_name,
        vec![text(&os["id"]), os_name.into()],
    );
    let mut badges = Vec::new();
    if !snapshot.de_display.is_empty() && snapshot.de_display != "unknown" {
        badges.push(badge(
            "hyprland",
            de["prettyName"].as_str().unwrap_or(&snapshot.de_display),
            &snapshot.de_display,
            vec![text(&de["processName"])],
        ));
    }
    let wm_name = string(wm, "prettyName");
    if !wm_name.is_empty() {
        badges.push(badge(
            "wm",
            wm_name,
            wm_name,
            vec![text(&wm["processName"])],
        ));
    }
    let protocol = string(wm, "protocolName");
    if !protocol.is_empty() {
        badges.push(badge("session", protocol, protocol, Vec::new()));
    }
    for (kind, value) in [
        ("terminal", &snapshot.terminal_display),
        ("shell", &snapshot.shell_display),
    ] {
        if !value.is_empty() && value != "unknown" {
            badges.push(badge(kind, value, value, Vec::new()));
        }
    }
    (platform, badges)
}
fn grouped(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 && (text.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
pub fn system_facts(snapshot: &Snapshot) -> Vec<Fact> {
    let kernel = snapshot.result("Kernel");
    let host = snapshot.result("Host");
    let uptime = snapshot.result("Uptime");
    let bios = snapshot.result("BIOS");
    let boot = snapshot.result("Bootmgr");
    let init = snapshot.result("InitSystem");
    let packages = snapshot.result("Packages");
    let mut managers = packages
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter(|(k, _)| k.as_str() != "all")
        .filter_map(|(k, v)| {
            v.as_u64()
                .filter(|v| *v > 0)
                .map(|v| format!("{} {k}", grouped(v)))
        })
        .collect::<Vec<_>>();
    if managers.is_empty()
        && let Some(total) = packages["all"].as_u64()
    {
        managers.push(grouped(total));
    }
    let mut values = facts([
        (
            "Host",
            joined(&[
                string(host, "vendor"),
                string(host, "family"),
                string(host, "name"),
            ]),
        ),
        (
            "Kernel",
            joined(&[string(kernel, "release"), string(kernel, "architecture")]),
        ),
        (
            "Uptime",
            if uptime.is_null() {
                String::new()
            } else {
                format_duration(number(&uptime["uptime"]))
            },
        ),
        ("Packages", managers.join(", ")),
        (
            "Firmware",
            joined(&[string(bios, "type"), string(bios, "version")]),
        ),
        ("Boot manager", text(&boot["name"])),
        (
            "Secure Boot",
            if boot.is_null() {
                ""
            } else if boot["secureBoot"].as_bool() == Some(true) {
                "On"
            } else {
                "Off"
            }
            .into(),
        ),
        (
            "Init",
            joined(&[string(init, "name"), string(init, "version")]),
        ),
    ]);
    let opencl = snapshot.result("OpenCL");
    let vulkan = snapshot.result("Vulkan");
    let theme = snapshot.result("Theme")["theme1"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| text(snapshot.result("WMTheme")));
    let font = snapshot.result("TerminalFont");
    values.extend(facts([
        (
            "OpenCL",
            joined(&[string(opencl, "name"), string(opencl, "version")]),
        ),
        (
            "Vulkan",
            joined(&[string(vulkan, "apiVersion"), string(vulkan, "driver")]),
        ),
        ("Theme", theme),
        (
            "Terminal font",
            font["font"]
                .as_str()
                .or_else(|| font["name"].as_str())
                .unwrap_or("")
                .into(),
        ),
    ]));
    for disk in list(snapshot.result("Disk"))
        .iter()
        .filter(|d| is_actionable_filesystem(d))
    {
        let total = number(&disk["bytes"]["total"]);
        if total == 0.0 {
            continue;
        }
        let used = number(&disk["bytes"]["used"]);
        let name = disk["mountpoint"]
            .as_str()
            .or_else(|| disk["name"].as_str())
            .unwrap_or("Filesystem");
        values.push(Fact {
            label: format!("Filesystem {name}"),
            value: joined(&[
                &format!("{} / {}", format_bytes(used), format_bytes(total)),
                &format!("{:.0}%", percentage(used, total)),
                string(disk, "filesystem"),
            ]),
        });
    }
    for display in list(snapshot.result("Display")) {
        let output = &display["output"];
        let scaled = &display["scaled"];
        let mut parts = Vec::new();
        if number(&output["width"]) > 0.0 && number(&output["height"]) > 0.0 {
            let mut resolution = format!("{}×{}", text(&output["width"]), text(&output["height"]));
            if let Some(refresh) = output["refreshRate"].as_f64().filter(|v| *v != 0.0) {
                resolution.push_str(&format!(" @ {refresh:.0} Hz"));
            }
            parts.push(resolution);
        }
        if !scaled.is_null()
            && (scaled["width"] != output["width"] || scaled["height"] != output["height"])
        {
            parts.push(format!(
                "scaled {}×{}",
                text(&scaled["width"]),
                text(&scaled["height"])
            ));
        }
        if display["hdrStatus"] == "Supported" {
            parts.push("HDR".into());
        }
        if !parts.is_empty() {
            values.push(Fact {
                label: format!("Display {}", string(display, "name")).trim().into(),
                value: parts.join("  "),
            });
        }
    }
    values
}
