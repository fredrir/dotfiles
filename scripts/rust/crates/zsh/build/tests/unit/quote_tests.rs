use super::*;

#[test]
fn plain_values_stay_bare() {
    assert_eq!(word("/opt/homebrew/bin"), "/opt/homebrew/bin");
}

#[test]
fn values_with_shell_syntax_are_single_quoted() {
    assert_eq!(word("a b"), "'a b'");
    assert_eq!(word("it's"), r"'it'\''s'");
    assert_eq!(word(""), "''");
    assert_eq!(word("~x"), "'~x'");
}

#[test]
fn control_characters_use_ansi_quoting() {
    assert_eq!(word("a\nb"), r"$'a\nb'");
    assert_eq!(ansi("\x1b[0m\\'"), r"$'\x1b[0m\\\''");
}

#[test]
fn plain_paths_are_absolute_and_safe() {
    assert!(is_plain_path("/a/b-c_d.zsh"));
    assert!(!is_plain_path("relative"));
    assert!(!is_plain_path("/a b"));
}
