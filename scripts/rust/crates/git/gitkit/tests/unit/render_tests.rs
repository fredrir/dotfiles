use super::*;

#[test]
fn a_column_pads_outside_its_colour() {
    assert_eq!(cell("ab", 5, Align::Left, str::to_string), "ab   ");
    assert_eq!(cell("ab", 5, Align::Right, str::to_string), "   ab");
    assert_eq!(cell("", 3, Align::Right, |_| "painted".into()), "   ");
    assert_eq!(cell("toolong", 3, Align::Left, str::to_string), "toolong");
}

#[test]
fn a_long_path_keeps_its_end() {
    assert_eq!(shorten_front("short", 10), "short");
    assert_eq!(shorten_front("0123456789", 5), "…6789");
    assert_eq!(shorten_front("0123456789", 10), "0123456789");
}

#[test]
fn counts_of_nothing_leave_their_column_empty() {
    assert_eq!(count(0, '+'), "");
    assert_eq!(count(12, '+'), "+12");
    assert_eq!(count(3, '-'), "-3");
}

#[test]
#[allow(unsafe_code)]
fn home_becomes_a_tilde_only_at_a_boundary() {
    // SAFETY: the tests in this module do not read HOME concurrently.
    unsafe { std::env::set_var("HOME", "/home/someone") };
    assert_eq!(shorten_home(Path::new("/home/someone/work")), "~/work");
    assert_eq!(shorten_home(Path::new("/home/someone")), "~");
    assert_eq!(
        shorten_home(Path::new("/home/someone-else/work")),
        "/home/someone-else/work"
    );
}
