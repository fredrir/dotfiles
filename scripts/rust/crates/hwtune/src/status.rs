use std::process::ExitCode;

use workstation::Style;

use crate::bios::{export, live};
use crate::cpu;
use crate::env::Sysfs;
use crate::gpu;
use crate::hwmon::{self, Hwmon};
use crate::journal;
use crate::paths::Paths;
use crate::services;
use crate::table;

fn line(style: &Style, label: &str, value: &str) {
    println!("  {:<9}  {value}", style.bold(label));
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
