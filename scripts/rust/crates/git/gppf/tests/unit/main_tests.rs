use super::*;

#[test]
fn git_statuses_pass_through() {
    assert_eq!(byte(0), 0);
    assert_eq!(byte(1), 1);
    assert_eq!(byte(128), 128);
}

#[test]
fn statuses_outside_a_byte_still_fail() {
    assert_eq!(byte(-1), 1);
    assert_eq!(byte(300), 1);
}
