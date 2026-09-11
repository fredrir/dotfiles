use std::process::ExitCode;
use std::time::Duration;

use clap::ValueEnum;

use crate::gpu;
use crate::stress::monitor::Monitor;
use crate::stress::{self, Context};
use crate::time;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum GpuTool {
    Vkmark,
    Glmark2,
}

pub struct GpuOptions {
    pub minutes: u64,
    pub tool: GpuTool,
}

pub fn command(tool: GpuTool) -> (String, Vec<String>) {
    match tool {
        GpuTool::Vkmark => ("vkmark".into(), vec!["--run-forever".into()]),
        GpuTool::Glmark2 => ("glmark2".into(), vec!["--run-forever".into()]),
    }
}

pub fn has_display() -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

pub fn run(options: GpuOptions, context: &Context) -> Result<ExitCode, String> {
    if !has_display() {
        return Err("gpu stress needs WAYLAND_DISPLAY or DISPLAY".into());
    }
    let before = gpu::query()?;
    let session = time::session_id("gpu");
    let provenance = context.provenance();
    let log = stress::session_log(&session)?;
    let (program, args) = command(options.tool);
    println!(
        "{} {} for {} min (cap {:.0} W)",
        context.style.bold(&program),
        args.join(" "),
        options.minutes,
        before.power_cap_w
    );
    let mut monitor = Monitor::start(&session, context.sys, false)?;
    let mut child = stress::spawn(&program, &args, &log)?;
    let finish = monitor.run(Duration::from_secs(options.minutes * 60), Some(&mut child))?;
    let ended_early = finish.status.is_some();
    let result = monitor
        .evidence
        .verdict("gpu", !ended_early, monitor.journal.total());
    let passed = result == "pass";
    let after = gpu::query().unwrap_or(before.clone());
    let mut keys = stress::keys(&[
        ("profile", "gpu".to_string()),
        ("tool", program.clone()),
        ("minutes", options.minutes.to_string()),
        ("power_cap", format!("{:.0}", before.power_cap_w)),
        ("bios", context.bios_sha()),
        ("result", result.into()),
        (
            "stress",
            if ended_early {
                stress::describe_status(finish.status, finish.timed_out)
            } else {
                "ran to time".into()
            },
        ),
        ("journal", monitor.journal.summary()),
        ("sm_mhz_end", after.sm_mhz.to_string()),
        ("mem_mhz_end", after.mem_mhz.to_string()),
    ]);
    keys.extend(monitor.peaks.keys());
    stress::cpu::report(context, &session, &keys, &monitor, passed, &provenance)?;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
#[path = "../../tests/unit/stress/gpu_tests.rs"]
mod tests;
