use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn read(reference: &str, newline: bool) -> Option<Read> {
    Some(Read {
        reference: reference.to_string(),
        newline,
    })
}

#[test]
fn a_plain_read_goes_to_the_bridge() {
    assert_eq!(
        Read::parse(&args(&["read", "op://Dev/pi/credential"])),
        read("op://Dev/pi/credential", true)
    );
}

#[test]
fn no_newline_is_understood_in_either_spelling_and_position() {
    for values in [
        ["read", "-n", "op://Dev/pi/credential"],
        ["read", "--no-newline", "op://Dev/pi/credential"],
        ["read", "op://Dev/pi/credential", "-n"],
    ] {
        assert_eq!(
            Read::parse(&args(&values)),
            read("op://Dev/pi/credential", false),
            "{values:?}"
        );
    }
}

#[test]
fn anything_the_bridge_does_not_understand_goes_to_the_real_op() {
    for values in [
        &["item", "get", "pi"][..],
        &["read"][..],
        &["read", "--account", "x", "op://Dev/pi/credential"][..],
        &["read", "op://Dev/a/b", "op://Dev/c/d"][..],
        &[][..],
    ] {
        assert_eq!(Read::parse(&args(values)), None, "{values:?}");
    }
}
