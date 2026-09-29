use super::*;

#[test]
fn a_wall_clock_far_ahead_of_the_monotonic_clock_means_a_sleep() {
    assert!(slept(Duration::from_secs(3600), Duration::from_secs(5)));
}

#[test]
fn ordinary_scheduling_jitter_is_not_a_sleep() {
    assert!(!slept(Duration::from_secs(6), Duration::from_secs(5)));
    assert!(!slept(Duration::from_secs(4), Duration::from_secs(5)));
}

#[test]
fn the_screen_is_locked_only_when_ioreg_says_so() {
    let locked = r#"  |   "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,"CGSSessionScreenIsLocked"=Yes,"kCGSSessionUserNameKey"="fredrir"})"#;
    let unlocked = r#"  |   "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,"kCGSSessionUserNameKey"="fredrir"})"#;
    assert!(screen_locked(locked));
    assert!(!screen_locked(unlocked));
}
