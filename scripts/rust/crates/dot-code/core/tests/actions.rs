#![forbid(unsafe_code)]

use dot_code_core::{Edit, Request, Trigger, apply, contains};

#[test]
fn kinds_contain_their_dotted_descendants_only() {
    assert!(contains("refactor", "refactor.extract.interface"));
    assert!(contains(
        "refactor.extract.interface",
        "refactor.extract.interface"
    ));
    assert!(contains("", "quickfix"));
    assert!(!contains("refactor.ex", "refactor.extract"));
    assert!(!contains("refactor.extract.interface", "refactor.extract"));
}

#[test]
fn requests_without_a_kind_filter_want_everything() {
    let only = ["refactor.rewrite".to_string()];
    let request = |only: &'static [String]| Request {
        source: "",
        language: "typescript",
        selection: 0..0,
        trigger: Trigger::Invoked,
        only,
    };
    assert!(request(&[]).wants("refactor.extract.type"));
    let filtered = Request {
        only: &only,
        ..request(&[])
    };
    assert!(!filtered.wants("refactor.extract.type"));
    assert!(filtered.wants("refactor.rewrite.typeDefinition.type"));
    assert!(filtered.explains());
}

#[test]
fn edits_apply_in_any_order_against_the_original_text() {
    let edits = [
        Edit {
            range: 6..11,
            text: "there".into(),
        },
        Edit::insert(0, ">> ".into()),
    ];
    assert_eq!(apply("hello world", &edits), ">> hello there");
}
