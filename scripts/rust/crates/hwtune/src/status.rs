use std::process::ExitCode;

use workstation::Style;

use crate::bios::{export, live};
use crate::cpu;
use crate::env::{self, Sysfs};
use crate::gpu;
use crate::hwmon::{self, Hwmon};
use crate::journal;
use crate::paths::Paths;
use crate::services;
use crate::table;

fn line(style: &Style, label: &str, value: &str) {
    println!("  {:<9}  {value}", style.bold(label));
}

fn idle_line(sys: &Sysfs) -> String {
    let cpuidle = sys.sys.join("devices/system/cpu/cpuidle");
    let read = |path: std::path::PathBuf| env::read_text(&path).unwrap_or_else(|_| "?".into());
    let zswap = match env::read_text(&sys.sys.join("module/zswap/parameters/enabled")).as_deref() {
        Ok("Y") => "on",
        Ok("N") => "off",
        _ => "?",
    };
    let rapl = match crate::power::Rapl::discover(sys) {
        Ok(_) => "readable",
        Err(_) if sys.sys.join("class/powercap/intel-rapl:0").exists() => "root-only",
        Err(_) => "absent",
    };
    format!(
        "cpuidle {}/{}  zswap {zswap}  rapl {rapl}",
        read(cpuidle.join("current_driver")),
        read(cpuidle.join("current_governor"))
    )
}

pub fn link(device: &std::path::Path) -> Option<String> {
    let speed = |name: &str| {
        env::read_text(&device.join(name))
            .ok()
            .and_then(|text| text.split_whitespace().next().map(str::to_owned))
    };
    Some(format!(
        "{}/{} GT/s x{}",
        speed("current_link_speed")?,
        speed("max_link_speed")?,
        env::read_text(&device.join("current_link_width")).ok()?
    ))
}

fn link_line(paths: Option<&Paths>, sys: &Sysfs) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(gpu) = paths
        .and_then(|paths| crate::bios::spec::load(&paths.spec_file()).ok())
        .and_then(|spec| spec.live.gpu)
        && let Some(text) = link(&sys.sys.join("bus/pci/devices").join(gpu))
    {
        parts.push(format!("gpu {text}"));
    }
    if let Ok(entries) = std::fs::read_dir(sys.sys.join("class/nvme")) {
        let mut names = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            if let Some(text) = link(&sys.sys.join("class/nvme").join(&name).join("device")) {
                parts.push(format!("{name} {text}"));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("  "))
}

pub fn run(paths: Option<&Paths>, sys: &Sysfs, style: &Style) -> Result<ExitCode, String> {
    let units = services::all()
        .into_iter()
        .map(|state| {
            let mark = if state.active == "active" {
                style.green(&state.active)
            } else {
                style.red(&state.active)
            };
            format!("{} {mark}/{}", state.unit, state.enabled)
        })
        .collect::<Vec<_>>()
        .join("  ");
    line(style, "services", &units);

    match cpu::cpufreq(sys) {
        Ok(freq) => line(
            style,
            "cpu",
            &format!(
                "boost {}  governor {}  epp {}  ceiling {} MHz",
                freq.boost.map_or("?".to_string(), |on| if on {
                    "on".into()
                } else {
                    "off".into()
                }),
                freq.governor,
                freq.epp,
                freq.max_khz / 1000
            ),
        ),
        Err(e) => line(style, "cpu", &style.dim(&e)),
    }
    line(style, "idle", &idle_line(sys));
    if let Ok(cores) = cpu::physical_cores(sys) {
        let ranking = cpu::prefcore_ranking(sys, &cores)
            .into_iter()
            .map(|(core, rank)| format!("{core}:{}", rank.map_or("?".into(), |r| r.to_string())))
            .collect::<Vec<_>>()
            .join(" ");
        line(style, "cores", &format!("prefcore {ranking}"));
    }

    match Hwmon::find(sys, hwmon::CHIP) {
        Ok(chip) => {
            let rows = hwmon::CHANNELS
                .iter()
                .map(|(name, channel)| {
                    vec![
                        name.to_string(),
                        chip.pwm(*channel)
                            .map_or("?".into(), |pwm| format!("{}%", hwmon::duty_pct(pwm))),
                        chip.rpm(*channel)
                            .map_or("?".into(), |rpm| format!("{rpm} rpm")),
                    ]
                })
                .collect::<Vec<_>>();
            println!("  {}", style.bold("fans"));
            for row in table::render(&["channel", "duty", "speed"], &rows).lines() {
                println!("             {row}");
            }
            let tctl = Hwmon::find(sys, hwmon::CPU_SENSOR)
                .and_then(|cpu| cpu.temp_c(1))
                .map_or("?".into(), |t| format!("{t:.1}°C"));
            let vrm = chip
                .temp_c(hwmon::VRM_TEMP_CHANNEL)
                .map_or("?".into(), |t| format!("{t:.1}°C"));
            line(style, "temps", &format!("tctl {tctl}  vrm {vrm}"));
        }
        Err(e) => line(style, "fans", &style.dim(&e)),
    }

    match gpu::query() {
        Ok(stats) => line(
            style,
            "gpu",
            &format!(
                "{:.0}°C  {:.0}/{:.0} W  sm {} MHz  mem {} MHz  fan {}%",
                stats.temp_c,
                stats.power_w,
                stats.power_cap_w,
                stats.sm_mhz,
                stats.mem_mhz,
                stats.fan_pct
            ),
        ),
        Err(e) => line(style, "gpu", &style.dim(&e)),
    }

    if let Some(links) = link_line(paths, sys) {
        line(style, "links", &links);
    }
    match journal::errors_since(None) {
        Ok(lines) => {
            let counts = journal::counts(&lines);
            let summary = counts.summary();
            line(
                style,
                "journal",
                &if counts.total() == 0 {
                    style.green(&summary)
                } else {
                    style.red(&summary)
                },
            );
        }
        Err(e) => line(style, "journal", &style.dim(&e)),
    }

    let dimms = live::dimms(sys);
    if !dimms.is_empty() {
        line(style, "dimms", &format!("{} × {}", dimms.len(), dimms[0]));
    }

    if let Some(paths) = paths {
        match export::latest(&paths.exports_dir(), &paths.host)? {
            Some(path) => {
                let (text, _) = export::load(&path)?;
                line(
                    style,
                    "bios",
                    &format!(
                        "{} sha {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        export::sha8(&text)
                    ),
                );
            }
            None => line(style, "bios", &style.dim("no export imported")),
        }
    }
    Ok(ExitCode::SUCCESS)
}
