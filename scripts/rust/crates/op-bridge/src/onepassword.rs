use std::process::{Command, Stdio};
use std::time::Duration;

use hostkit::process::CaptureLimits;
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(60);

pub fn read(reference: &str) -> Result<Zeroizing<String>, String> {
    let output = op(&["read", "--no-newline", reference])?;
    let text = std::str::from_utf8(&output).map_err(|_| "op read: value is not UTF-8")?;
    Ok(Zeroizing::new(text.to_owned()))
}

// Unlocks 1Password once, so a batch of reads costs one prompt
pub fn authorize(vault: &str) -> Result<(), String> {
    op(&["vault", "get", vault, "--format", "json"]).map(drop)
}

// Ends the daemon's session, so 1Password prompts for the next read
pub fn sign_out() -> Result<(), String> {
    op(&["signout", "--all"]).map(drop)
}

fn op(args: &[&str]) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut command = Command::new("op");
    command.args(args).stdin(Stdio::null());
    let output = hostkit::process::output(&mut command, CaptureLimits::default(), TIMEOUT)
        .map_err(|error| format!("op: {error}"))?;
    let stdout = Zeroizing::new(output.stdout);
    if !output.status.success() {
        return Err(hostkit::ssh::stderr_reason(
            &output.stderr,
            &format!("op {} exited with {}", args[0], output.status),
        ));
    }
    if output.stdout_truncated {
        return Err(format!("op {}: output too large", args[0]));
    }
    Ok(stdout)
}
