use std::time::Duration;

pub const TICK: Duration = Duration::from_secs(5);
// The monotonic clock stops while macie sleeps; the wall clock does not
const SLEEP_GAP: Duration = Duration::from_secs(30);

pub fn slept(wall: Duration, monotonic: Duration) -> bool {
    wall.saturating_sub(monotonic) > SLEEP_GAP
}

#[cfg(test)]
#[path = "../tests/unit/presence_tests.rs"]
mod tests;
