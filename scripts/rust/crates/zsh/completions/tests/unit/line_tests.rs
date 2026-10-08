use super::*;
use crate::help::Value;

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(String::from).collect()
}

fn help_with_value_flag() -> Help {
    Help {
        flags: vec![
            Flag {
                names: vec!["-w".into(), "--workspace".into()],
                value: Some(Value {
                    placeholder: "name".into(),
                    optional: false,
                    choices: Vec::new(),
                }),
                description: String::new(),
            },
            Flag {
                names: vec!["-D".into()],
                ..Flag::default()
            },
        ],
        ..Help::default()
    }
}

#[test]
fn the_cursor_word_defaults_to_the_word_itself() {
    let line = Line::new(words("npm i rea"), 3, None);
    assert_eq!(line.current, 2);
    assert_eq!(line.prefix, "rea");
    assert_eq!(line.before(), ["i"]);
    assert_eq!(line.previous(), Some("i"));
}

#[test]
fn a_cursor_past_the_last_word_starts_an_empty_one() {
    let line = Line::new(words("npm i"), 3, Some(String::new()));
    assert_eq!(line.words, ["npm", "i", ""]);
    assert_eq!(line.before(), ["i"]);
    let clamped = Line::new(Vec::new(), 0, None);
    assert_eq!(clamped.words, [""]);
    assert!(clamped.before().is_empty());
}

#[test]
fn flag_values_are_read_in_both_spellings() {
    let line = Line::new(words("pi --provider openai --session-dir=/tmp x"), 5, None);
    assert_eq!(line.flag_value(&["--provider"]), Some("openai"));
    assert_eq!(line.flag_value(&["--session-dir"]), Some("/tmp"));
    assert!(line.has_flag(&["--session-dir"]));
    assert!(!line.has_flag(&["--model"]));
}

#[test]
fn the_word_under_the_cursor_is_not_a_flag_on_the_line() {
    let line = Line::new(words("npm i -g"), 3, None);
    assert!(!line.has_flag(&["-g"]));
}

#[test]
fn a_flag_that_takes_a_value_swallows_the_next_word() {
    let help = help_with_value_flag();
    let words = words("add -w web -D react");
    let scanned = scan(&words, &[&help]);
    assert_eq!(scanned.positionals, ["add", "react"]);
    assert_eq!(scanned.flags, ["-w", "-D"]);
    assert!(scanned.pending.is_none());
}

#[test]
fn a_trailing_flag_waits_for_its_value() {
    let help = help_with_value_flag();
    let words = words("add --workspace");
    let pending = scan(&words, &[&help])
        .pending
        .map(|flag| flag.names.clone());
    assert_eq!(
        pending,
        Some(vec!["-w".to_string(), "--workspace".to_string()])
    );
    let inline = self::words("add --workspace=web");
    assert!(scan(&inline, &[&help]).pending.is_none());
}

#[test]
fn everything_after_a_separator_is_positional() {
    let words = words("-- -D x");
    let scanned = scan(&words, &[]);
    assert!(scanned.after_separator);
    assert_eq!(scanned.positionals, ["-D", "x"]);
}
