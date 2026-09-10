use super::*;

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn catch_up_returns_one_latest_occurrence_and_stops_when_completed() -> Result<()> {
    let schedule = Schedule::default();
    let now = utc("2026-09-09T12:00:00Z");
    let occurrence = due(&schedule, Some(utc("2026-08-01T00:00:00Z")), now)?.unwrap();
    assert_eq!(occurrence, utc("2026-09-06T01:00:00Z"));
    assert!(due(&schedule, Some(occurrence), now)?.is_none());
    Ok(())
}

#[test]
fn spring_dst_gap_runs_at_the_first_valid_instant() -> Result<()> {
    let schedule = Schedule {
        hour: 2,
        minute: 30,
        ..Schedule::default()
    };
    let now = utc("2026-03-29T01:05:00Z");
    assert_eq!(
        due(&schedule, None, now)?,
        Some(utc("2026-03-29T01:00:00Z"))
    );
    Ok(())
}

#[test]
fn autumn_dst_fold_is_one_occurrence() -> Result<()> {
    let schedule = Schedule {
        hour: 2,
        minute: 30,
        ..Schedule::default()
    };
    let first = utc("2026-10-25T00:30:00Z");
    assert_eq!(
        due(&schedule, None, utc("2026-10-25T01:40:00Z"))?,
        Some(first)
    );
    assert!(due(&schedule, Some(first), utc("2026-10-25T01:40:00Z"))?.is_none());
    assert_eq!(next(&schedule, first)?, Some(utc("2026-11-01T01:30:00Z")));
    Ok(())
}

#[test]
fn disabling_catch_up_expires_old_occurrences() -> Result<()> {
    let schedule = Schedule {
        catch_up: false,
        ..Schedule::default()
    };
    assert!(due(&schedule, None, utc("2026-09-06T01:14:59Z"))?.is_some());
    assert!(due(&schedule, None, utc("2026-09-06T01:15:00Z"))?.is_none());
    assert!(due(&schedule, None, utc("2026-09-07T01:00:00Z"))?.is_none());
    Ok(())
}

#[test]
fn calendar_supports_multiple_weekdays_and_rejects_invalid_values() -> Result<()> {
    let mut schedule = Schedule {
        weekdays: vec![1, 3, 5],
        ..Schedule::default()
    };
    assert_eq!(
        next(&schedule, utc("2026-09-09T02:00:00Z"))?,
        Some(utc("2026-09-11T01:00:00Z"))
    );
    schedule.weekdays = vec![0];
    assert!(due(&schedule, None, Utc::now()).is_err());
    schedule.enabled = false;
    assert!(due(&schedule, None, Utc::now())?.is_none());
    Ok(())
}

#[test]
fn scheduler_files_escape_arguments_without_shell_interpretation() -> Result<()> {
    let executable = Path::new("/opt/tool name/dcloud");
    let config = Path::new("/home/fred/A&B$C%20\"config.toml");
    let plist = launchd(executable, config, "dev.dcloud")?
        .remove(0)
        .contents;
    assert!(plist.contains("A&amp;B$C%20&quot;config.toml"));
    assert!(plist.contains("<string>/opt/tool name/dcloud</string>"));
    let files = systemd(executable, config, "dcloud")?;
    assert!(files[0].contents.contains("\"/opt/tool name/dcloud\""));
    assert!(files[0].contents.contains("A&B$$C%%20\\\"config.toml"));
    assert!(launchd(executable, config, "../../bad").is_err());
    assert!(systemd(Path::new("relative"), config, "dcloud").is_err());
    Ok(())
}

#[test]
fn scheduler_supplies_tool_path_and_persistent_launchd_logs() -> Result<()> {
    let executable = Path::new("/home/fred/$bin/dcloud");
    let config = Path::new("/home/fred/config.toml");
    let plist = launchd_with_logs(
        executable,
        config,
        "dev.dcloud",
        Path::new("/home/fred/Logs&Audit"),
    )?
    .remove(0)
    .contents;
    assert!(plist.contains("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"));
    assert!(plist.contains("<key>StandardErrorPath</key>"));
    assert!(plist.contains("Logs&amp;Audit/dev.dcloud.stderr.log"));
    let service = systemd(executable, config, "dcloud")?.remove(0).contents;
    assert!(service.contains("Environment=\"PATH=/home/fred/$bin:"));
    assert!(service.contains("ExecStart=\"/home/fred/$$bin/dcloud\""));
    Ok(())
}
