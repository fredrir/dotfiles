use super::*;

#[test]
fn any_value_becomes_exactly_one_shell_word() {
    assert_eq!(quote(""), "''");
    assert_eq!(quote("plain"), "'plain'");
    assert_eq!(quote("a b"), "'a b'");
}

#[test]
fn an_apostrophe_closes_and_reopens_the_quoting() {
    assert_eq!(quote("it's $HOME"), "'it'\\''s $HOME'");
    assert_eq!(quote("'"), "''\\'''");
    assert_eq!(quote("''"), "''\\'''\\'''");
    assert_eq!(quote("a'b; touch nope"), "'a'\\''b; touch nope'");
}

#[test]
fn nothing_inside_the_quotes_is_left_for_the_shell_to_read() {
    for value in [
        "$HOME",
        "$(reboot)",
        "`reboot`",
        "a; reboot",
        "a && reboot",
        "*",
        "~",
        "\\",
        "a\nb",
        "--flag",
    ] {
        assert_eq!(quote(value), format!("'{value}'"));
    }
}

#[test]
fn a_quoted_path_is_the_quoted_string_of_its_text() {
    assert_eq!(
        quote_path(Path::new("/home/fred rir/a'b")).unwrap(),
        "'/home/fred rir/a'\\''b'"
    );
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_named_rather_than_mangled() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let error = quote_path(Path::new(OsStr::from_bytes(b"/tmp/\xff"))).unwrap_err();
    assert!(error.starts_with("path is not valid UTF-8:"), "{error}");
}
