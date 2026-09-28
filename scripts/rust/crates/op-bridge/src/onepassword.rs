use std::process::{Command, Stdio};
use std::time::Duration;

use hostkit::process::CaptureLimits;
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(60);

pub fn read(reference: &str) -> Result<Zeroizing<String>, String> {
    let mut command = Command::new("op");
    command
        .args(["read", "--no-newline", reference])
        .stdin(Stdio::null());
    let output = hostkit::process::output(&mut command, CaptureLimits::default(), TIMEOUT)
        .map_err(|error| format!("op: {error}"))?;
    let stdout = Zeroizing::new(output.stdout);
    if !output.status.success() {
        return Err(hostkit::ssh::stderr_reason(
            &output.stderr,
            &format!("op read exited with {}", output.status),
        ));
    }
    if output.stdout_truncated {
        return Err("op read: value too large".to_string());
    }
    let text = std::str::from_utf8(&stdout).map_err(|_| "op read: value is not UTF-8")?;
    Ok(Zeroizing::new(text.to_owned()))
}
