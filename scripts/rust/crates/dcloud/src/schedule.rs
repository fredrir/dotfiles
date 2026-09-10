use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::config::Schedule;

pub fn due(
    schedule: &Schedule,
    last_completed: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    if !schedule.enabled {
        return Ok(None);
    }
    ensure!(
        schedule.hour < 24
            && schedule.minute < 60
            && !schedule.weekdays.is_empty()
            && schedule.weekdays.iter().all(|day| (1..=7).contains(day)),
        "invalid calendar schedule"
    );
    let zone: Tz = schedule
        .timezone
        .parse()
        .context("invalid schedule timezone")?;
    let today = now.with_timezone(&zone).date_naive();
    let mut latest = None;
    for days_back in 0..=8 {
        let date = today
            .checked_sub_signed(Duration::days(days_back))
            .context("schedule date out of range")?;
        if !schedule
            .weekdays
            .contains(&date.weekday().number_from_monday())
        {
            continue;
        }
        let local = date
            .and_hms_opt(schedule.hour, schedule.minute, 0)
            .context("invalid calendar time")?;
        let candidate = resolve_local(zone, local)?;
        if candidate <= now && latest.is_none_or(|previous| candidate > previous) {
            latest = Some(candidate);
        }
    }
    let Some(latest) = latest else {
        return Ok(None);
    };
    if last_completed.is_some_and(|previous| latest <= previous) {
        return Ok(None);
    }
    if !schedule.catch_up && now - latest >= Duration::minutes(15) {
        return Ok(None);
    }
    Ok(Some(latest))
}

pub fn next(schedule: &Schedule, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    if !schedule.enabled {
        return Ok(None);
    }
    ensure!(
        schedule.hour < 24
            && schedule.minute < 60
            && !schedule.weekdays.is_empty()
            && schedule.weekdays.iter().all(|day| (1..=7).contains(day)),
        "invalid calendar schedule"
    );
    let zone: Tz = schedule
        .timezone
        .parse()
        .context("invalid schedule timezone")?;
    let today = after.with_timezone(&zone).date_naive();
    let mut next = None;
    for days_forward in 0..=8 {
        let date = today
            .checked_add_signed(Duration::days(days_forward))
            .context("schedule date out of range")?;
        if !schedule
            .weekdays
            .contains(&date.weekday().number_from_monday())
        {
            continue;
        }
        let local = date
            .and_hms_opt(schedule.hour, schedule.minute, 0)
            .context("invalid calendar time")?;
        let candidate = resolve_local(zone, local)?;
        if candidate > after && next.is_none_or(|previous| candidate < previous) {
            next = Some(candidate);
        }
    }
    Ok(next)
}

fn resolve_local(zone: Tz, local: NaiveDateTime) -> Result<DateTime<Utc>> {
    for skipped_minutes in 0..=1440 {
        let adjusted = local
            .checked_add_signed(Duration::minutes(skipped_minutes))
            .context("schedule time out of range")?;
        match zone.from_local_datetime(&adjusted) {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    bail!("timezone has no valid time within one day of the scheduled occurrence")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerFile {
    pub name: String,
    pub contents: String,
}

pub fn launchd(executable: &Path, config: &Path, label: &str) -> Result<Vec<SchedulerFile>> {
    launchd_render(executable, config, label, None)
}

pub fn launchd_with_logs(
    executable: &Path,
    config: &Path,
    label: &str,
    logs: &Path,
) -> Result<Vec<SchedulerFile>> {
    path_text(logs)?;
    launchd_render(executable, config, label, Some(logs))
}

fn launchd_render(
    executable: &Path,
    config: &Path,
    label: &str,
    logs: Option<&Path>,
) -> Result<Vec<SchedulerFile>> {
    validate_label(label)?;
    let path = xml(&scheduler_path(executable)?);
    let log_entries = match logs {
        Some(directory) => format!(
            "  <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n",
            xml(path_text(&directory.join(format!("{label}.stdout.log")))?),
            xml(path_text(&directory.join(format!("{label}.stderr.log")))?)
        ),
        None => String::new(),
    };
    let executable = path_text(executable)?;
    let config = path_text(config)?;
    let args = [executable, "--config", config, "run-due"]
        .into_iter()
        .map(|argument| format!("    <string>{}</string>", xml(argument)))
        .collect::<Vec<_>>()
        .join("\n");
    let contents = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n{args}\n  </array>\n  <key>EnvironmentVariables</key>\n  <dict><key>PATH</key><string>{path}</string></dict>\n{log_entries}  <key>StartInterval</key>\n  <integer>900</integer>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Background</string>\n</dict>\n</plist>\n"
    );
    Ok(vec![SchedulerFile {
        name: format!("{label}.plist"),
        contents,
    }])
}

pub fn systemd(executable: &Path, config: &Path, label: &str) -> Result<Vec<SchedulerFile>> {
    validate_label(label)?;
    let environment = systemd_quoted(&format!("PATH={}", scheduler_path(executable)?));
    let args = [
        path_text(executable)?,
        "--config",
        path_text(config)?,
        "run-due",
    ]
    .into_iter()
    .map(systemd_argument)
    .collect::<Vec<_>>()
    .join(" ");
    Ok(vec![
        SchedulerFile {
            name: format!("{label}.service"),
            contents: format!(
                "[Unit]\nDescription=Dcloud due jobs\n\n[Service]\nType=oneshot\nExecStart={args}\nEnvironment={environment}\nUMask=0077\nTimeoutStartSec=infinity\n"
            ),
        },
        SchedulerFile {
            name: format!("{label}.timer"),
            contents: format!(
                "[Unit]\nDescription=Check for due dcloud jobs\n\n[Timer]\nOnCalendar=*:0/15\nPersistent=true\nRandomizedDelaySec=30\nUnit={label}.service\n\n[Install]\nWantedBy=timers.target\n"
            ),
        },
    ])
}

fn scheduler_path(executable: &Path) -> Result<String> {
    let parent = executable
        .parent()
        .context("scheduler executable needs a parent directory")?;
    let parent = path_text(parent)?;
    ensure!(
        !parent.contains(':'),
        "scheduler executable directory cannot contain a PATH separator"
    );
    Ok(format!(
        "{parent}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    ))
}

fn path_text(path: &Path) -> Result<&str> {
    ensure!(
        path.is_absolute(),
        "scheduler paths must be absolute: {}",
        path.display()
    );
    let value = path.to_str().context("scheduler paths must be UTF-8")?;
    ensure!(
        !value.chars().any(char::is_control),
        "scheduler paths cannot contain control characters"
    );
    Ok(value)
}

fn validate_label(label: &str) -> Result<()> {
    ensure!(
        !label.is_empty()
            && label.len() <= 128
            && label
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-' | b'_')),
        "invalid scheduler label"
    );
    Ok(())
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd_argument(value: &str) -> String {
    systemd_quoted(&value.replace('$', "$$"))
}

fn systemd_quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

#[cfg(test)]
#[path = "../tests/unit/schedule_tests.rs"]
mod tests;
