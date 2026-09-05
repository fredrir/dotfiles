use super::*;

#[test]
fn a_column_pads_outside_its_colour() {
    assert_eq!(cell("ab", 5, Align::Left, str::to_string), "ab   ");
    assert_eq!(cell("ab", 5, Align::Right, str::to_string), "   ab");
    assert_eq!(cell("", 3, Align::Right, |_| "painted".into()), "   ");
    assert_eq!(cell("toolong", 3, Align::Left, str::to_string), "toolong");
}

#[test]
fn counts_of_nothing_leave_their_column_empty() {
    assert_eq!(count(0, '+'), "");
    assert_eq!(count(12, '+'), "+12");
    assert_eq!(count(3, '-'), "-3");
}
