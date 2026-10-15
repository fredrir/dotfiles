#![forbid(unsafe_code)]
#![cfg(target_os = "macos")]

use serde_json::Value;
use testkit::{Bin, stderr, stdout};

fn dspell() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dspell")).plain()
}

fn typo(before: &str, after: &str) -> Value {
    let output = dspell().args(["--", before, after]).output();
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

fn guesses(typo: &Value) -> &Vec<Value> {
    typo["guesses"].as_array().unwrap()
}

#[test]
fn the_last_typo_before_the_cursor_is_found_with_its_guesses() {
    let typo = typo("teh quick borwn fox", "");
    assert_eq!(typo["word"], "borwn");
    assert_eq!(typo["length"], 5);
    assert_eq!(typo["offset"], -4);
    assert!(guesses(&typo).contains(&"brown".into()));
}

#[test]
fn a_typo_around_the_cursor_is_checked_whole() {
    let typo = typo("and not more diffu", "cult.");
    assert_eq!(typo["word"], "diffucult");
    assert_eq!(typo["offset"], 4);
    assert!(guesses(&typo).contains(&"difficult".into()));
}

#[test]
fn a_typo_after_the_cursor_is_ignored() {
    let typo = typo("teh", " borwn");
    assert_eq!(typo["word"], "teh");
    assert_eq!(typo["offset"], 0);
}

#[test]
fn a_norwegian_text_gets_norwegian_guesses() {
    let typo = typo("jeg har ikek tid", "");
    assert_eq!(typo["word"], "ikek");
    assert!(guesses(&typo).contains(&"ikke".into()));
}

#[test]
fn text_between_the_typo_and_the_cursor_is_counted_in_graphemes() {
    let typo = typo("teh 👍🏽 ø", "");
    assert_eq!(typo["word"], "teh");
    assert_eq!(typo["offset"], -4);
}

#[test]
fn a_word_known_in_any_language_is_not_a_typo() {
    let output = dspell().args(["--", "hello verden"]).output();
    assert!(!output.status.success());
    assert_eq!(stderr(&output).trim(), "dspell: no typo found");
}

#[test]
fn an_unknown_language_is_rejected() {
    let output = dspell().args(["--lang", "xx", "teh"]).output();
    assert!(!output.status.success());
    assert_eq!(stderr(&output).trim(), "dspell: unknown language: xx");
}
