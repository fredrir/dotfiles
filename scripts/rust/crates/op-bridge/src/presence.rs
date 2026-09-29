use std::process::{Command, Stdio};
use std::time::Duration;

pub const TICK: Duration = Duration::from_secs(5);
// The monotonic clock stops while macie sleeps; the wall clock does not
const SLEEP_GAP: Duration = Duration::from_secs(30);

pub fn slept(wall: Duration, monotonic: Duration) -> bool {
    wall.saturating_sub(monotonic) > SLEEP_GAP
}

pub fn screen_locked(ioreg: &str) -> bool {
    ioreg.contains("\"CGSSessionScreenIsLocked\"=Yes")
}

pub fn locked() -> bool {
    Command::new("ioreg")
        .args(["-n", "Root", "-d1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| screen_locked(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
#[path = "../tests/unit/presence_tests.rs"]
mod tests;
