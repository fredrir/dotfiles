use std::process::ExitCode;
use std::time::Duration;

use crate::cpu;
use crate::stress::monitor::Monitor;
use crate::stress::state::{self, PerCore, Recovered};
use crate::stress::{self, Context, Profile};
use crate::table;
use crate::time;

pub struct CpuOptions {
    pub profile: Profile,
    pub minutes: u64,
    pub cores: Option<String>,
    pub offset: Option<i32>,
}

const GRACE: Duration = Duration::from_secs(60);

pub fn args(profile: Profile, threads: usize, minutes: u64, core: Option<u32>) -> Vec<String> {
    let timeout = format!("{minutes}m");
    let mut args = match profile {
        Profile::AllCore => vec![
            "--cpu".to_string(),
            threads.to_string(),
            "--cpu-method".into(),
            "matrixprod".into(),
        ],
        Profile::Light => vec![
            "--cpu".to_string(),
            threads.to_string(),
            "--cpu-load".into(),
            "10".into(),
        ],
        Profile::PerCore => vec![
            "--cpu".to_string(),
            "1".into(),
            "--taskset".into(),
            core.unwrap_or(0).to_string(),
            "--cpu-method".into(),
            "matrixprod".into(),
        ],
    };
    args.extend(["--timeout".to_string(), timeout, "--metrics-brief".into()]);
    args
}

fn budget(minutes: u64) -> Duration {
    Duration::from_secs(minutes * 60) + GRACE
}

pub fn run(options: CpuOptions, context: &Context) -> Result<ExitCode, String> {
    match options.profile {
        Profile::PerCore => per_core(options, context),
        profile => whole(profile, options, context),
    }
}

fn whole(profile: Profile, options: CpuOptions, context: &Context) -> Result<ExitCode, String> {
    let session = time::session_id(profile.name());
    let threads = cpu::logical_count(context.sys)?;
    let log = stress::session_log(&session)?;
    let args = args(profile, threads, options.minutes, None);
    println!(
        "{} {} for {} min on {threads} threads",
        context.style.bold("stress-ng"),
        profile.name(),
        options.minutes
    );
    let mut child = stress::spawn("stress-ng", &args, &log)?;
    let mut monitor = Monitor::start(&session, context.sys, false)?;
    let finish = monitor.run(budget(options.minutes), Some(&mut child))?;
    let status = stress::describe_status(finish.status, finish.timed_out);
    println!("  stress-ng {status}");
    let passed =
        finish.status.is_some_and(|status| status.success()) && monitor.journal.total() == 0;
    let mut keys = stress::keys(&[
        ("profile", profile.name().to_string()),
        ("minutes", options.minutes.to_string()),
        ("threads", threads.to_string()),
        ("bios", context.bios_sha()),
        ("result", if passed { "pass".into() } else { "fail".into() }),
        ("stress", status.clone()),
        ("journal", monitor.journal.summary()),
    ]);
    keys.extend(monitor.peaks.keys());
    report(context, &session, &keys, &monitor, passed)?;
    if !passed {
        for line in stress::tail(&log, 5) {
            println!("  {}", context.style.dim(&line));
        }
    }
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn per_core(options: CpuOptions, context: &Context) -> Result<ExitCode, String> {
    let cores = match &options.cores {
        Some(list) => cpu::parse_cores(list)?,
        None => cpu::physical_cores(context.sys)?,
    };
    let session = time::session_id(Profile::PerCore.name());
    let state_path = state::path()?;
    let boot_id = state::boot_id();
    let mut results: Vec<(u32, String)> = Vec::new();
    if let Some(previous) = state::take(&state_path)? {
        match state::verdict(&previous, &boot_id) {
            Recovered::Rebooted => {
                println!(
                    "  {} core {} was under test when the machine rebooted (session {})",
                    context.style.red("recovered"),
                    previous.core,
                    previous.session
                );
                results.push((previous.core, "fail (rebooted)".into()));
            }
            Recovered::Interrupted => println!(
                "  {} core {} was interrupted in this boot; testing it again",
                context.style.code("33", "recovered"),
                previous.core
            ),
        }
    }
    let log = stress::session_log(&session)?;
    let mut monitor = Monitor::start(&session, context.sys, false)?;
    println!(
        "{} per-core, {} min on cores {}",
        context.style.bold("stress-ng"),
        options.minutes,
        cores
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    for core in &cores {
        if results.iter().any(|(done, _)| done == core) {
            continue;
        }
        state::write(
            &state_path,
            &PerCore {
                session: session.clone(),
                core: *core,
                started: time::now_iso(),
                offset: options.offset,
                boot_id: boot_id.clone(),
                pid: std::process::id(),
            },
        )?;
        println!("  core {core}");
        let args = args(Profile::PerCore, 1, options.minutes, Some(*core));
        let before = monitor.journal.total();
        let mut child = stress::spawn("stress-ng", &args, &log)?;
        let finish = monitor.run(budget(options.minutes), Some(&mut child))?;
        state::clear(&state_path);
        let clean = monitor.journal.total() == before;
        let verdict = match finish.status {
            Some(status) if status.success() && clean => "pass".to_string(),
            Some(status) if status.success() => "fail (journal errors)".to_string(),
            other => format!(
                "fail ({})",
                stress::describe_status(other, finish.timed_out)
            ),
        };
        println!("    {verdict}");
        results.push((*core, verdict));
    }
    results.sort();
    let passed = results.iter().all(|(_, verdict)| verdict == "pass");
    let mut keys = stress::keys(&[
        ("profile", Profile::PerCore.name().to_string()),
        ("minutes", options.minutes.to_string()),
        (
            "cores",
            results
                .iter()
                .map(|(core, _)| core.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
    ]);
    if let Some(offset) = options.offset {
        keys.push(("offset".into(), offset.to_string()));
    }
    keys.extend(stress::keys(&[
        ("bios", context.bios_sha()),
        ("result", if passed { "pass".into() } else { "fail".into() }),
        ("journal", monitor.journal.summary()),
    ]));
    keys.extend(monitor.peaks.keys());
    keys.extend(
        results
            .iter()
            .map(|(core, verdict)| (format!("core{core}"), verdict.clone())),
    );
    report(context, &session, &keys, &monitor, passed)?;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn report(
    context: &Context,
    session: &str,
    keys: &[(String, String)],
    monitor: &Monitor,
    passed: bool,
) -> Result<(), String> {
    println!();
    print!(
        "{}",
        table::render(&["peak", "value"], &monitor.peaks.rows())
    );
    println!(
        "\n  {}  samples {}  csv {}",
        if passed {
            context.style.green("pass")
        } else {
            context.style.red("fail")
        },
        monitor.peaks.samples,
        monitor.csv_path().display()
    );
    if let Some(file) = context.record(&time::now_iso(), keys)? {
        println!("  logged to {}", file.display());
    }
    let _ = session;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/stress/cpu_tests.rs"]
mod tests;
