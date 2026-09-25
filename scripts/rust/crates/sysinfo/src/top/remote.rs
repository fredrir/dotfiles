//! `--target`: the peer samples itself and returns its report as JSON.

use super::{Options, Report, SCHEMA};
use hostkit::Host;
use hostkit::process::CaptureLimits;
use hostkit::ssh::Session;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);

pub fn script(options: Options) -> String {
    let mut script = format!(
        r#"exec "${{DOTFILES_COMPILED:-$HOME/dotfiles/.bin}}/sysinfo" --system --json --number {}"#,
        options.count
    );
    if let Some(flag) = options.sort.flag() {
        script.push(' ');
        script.push_str(flag);
    }
    if options.split {
        script.push_str(" --split");
    }
    script
}

pub fn fetch(host: Host, options: Options) -> Result<Report, String> {
    let name = host.name();
    let output = Session::new(name)
        .batch()
        .script(&script(options))
        .output_bounded(CaptureLimits::default(), TIMEOUT)
        .map_err(|error| format!("{name}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| format!("ssh exited {}", output.status), str::to_string);
        return Err(format!("{name}: {reason}"));
    }
    parse(name, &output.stdout)
}

pub fn parse(host: &str, bytes: &[u8]) -> Result<Report, String> {
    let report: Report = serde_json::from_slice(bytes)
        .map_err(|error| format!("{host}: invalid process report: {error}"))?;
    if report.schema != SCHEMA {
        return Err(format!(
            "{host}: process report schema {} unsupported",
            report.schema
        ));
    }
    Ok(report)
}

#[cfg(test)]
#[path = "../../tests/unit/top/remote.rs"]
mod tests;
