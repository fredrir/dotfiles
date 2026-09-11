use std::process::ExitCode;
use std::time::Duration;

use clap::ValueEnum;

use crate::env;
use crate::stress::monitor::Monitor;
use crate::stress::{self, Context};
use crate::time;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum MemTool {
    StressNg,
    Memtester,
}

pub struct MemOptions {
    pub minutes: u64,
    pub percent: u8,
    pub tool: MemTool,
}

pub fn available_bytes(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

pub fn command(tool: MemTool, minutes: u64, percent: u8, available: u64) -> (String, Vec<String>) {
    let bytes = available * u64::from(percent) / 100;
    match tool {
        MemTool::StressNg => (
            "stress-ng".into(),
            vec![
                "--vm".into(),
                "4".into(),
                "--vm-bytes".into(),
                bytes.to_string(),
                "--vm-method".into(),
                "all".into(),
                "--verify".into(),
                "--timeout".into(),
                format!("{minutes}m"),
                "--metrics-brief".into(),
            ],
        ),
        MemTool::Memtester => (
            "memtester".into(),
            vec![format!("{}M", bytes / (1 << 20)), "1".into()],
        ),
    }
}

pub fn run(options: MemOptions, context: &Context) -> Result<ExitCode, String> {
    let meminfo = env::read_text(std::path::Path::new("/proc/meminfo"))?;
    let available = available_bytes(&meminfo).ok_or("MemAvailable missing from /proc/meminfo")?;
    let session = time::session_id("mem");
    let log = stress::session_log(&session)?;
    let (program, args) = command(options.tool, options.minutes, options.percent, available);
    println!(
        "{} {} for {} min with {}% of available memory",
        context.style.bold(&program),
        args.join(" "),
        options.minutes,
        options.percent
    );
    let mut monitor = Monitor::start(&session, context.sys, false)?;
    let budget = Duration::from_secs(options.minutes * 60 + 60);
    let mut passes = 0usize;
    let mut failure = None;
    loop {
        let mut child = stress::spawn(&program, &args, &log)?;
        let finish = monitor.run(budget, Some(&mut child))?;
        match finish.status {
            Some(status) if status.success() => passes += 1,
            other => {
                failure = Some(stress::describe_status(other, finish.timed_out));
                break;
            }
        }
        if options.tool == MemTool::StressNg
            || finish.elapsed >= Duration::from_secs(options.minutes * 60)
        {
            break;
        }
    }
    let passed = failure.is_none() && monitor.journal.total() == 0;
    let mut keys = stress::keys(&[
        ("profile", "mem".to_string()),
        ("tool", program.clone()),
        ("minutes", options.minutes.to_string()),
        ("percent", options.percent.to_string()),
        ("passes", passes.to_string()),
        ("bios", context.bios_sha()),
        ("result", if passed { "pass".into() } else { "fail".into() }),
        ("stress", failure.clone().unwrap_or_else(|| "exit 0".into())),
        ("journal", monitor.journal.summary()),
    ]);
    keys.extend(monitor.peaks.keys());
    stress::cpu::report(context, &session, &keys, &monitor, passed)?;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
#[path = "../../tests/unit/stress/mem_tests.rs"]
mod tests;
