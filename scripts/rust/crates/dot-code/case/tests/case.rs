#![forbid(unsafe_code)]

use dot_code_case::{Case, Memory, Params, Replacement, Target};
use serde_json::json;

fn selection(text: &str) -> Target {
    Target::Selection { text: text.into() }
}

fn cursor(before: &str, after: &str) -> Target {
    Target::Cursor {
        before: before.into(),
        after: after.into(),
    }
}

fn file(name: &str) -> Target {
    Target::FileName { name: name.into() }
}

fn replace(memory: &mut Memory, case: Case, target: Target) -> Replacement {
    memory
        .transform(&Params {
            case,
            targets: vec![target],
        })
        .remove(0)
}

fn convert(text: &str, case: Case) -> String {
    replace(&mut Memory::default(), case, selection(text)).text
}

fn around_cursor(before: &str, after: &str, case: Case) -> Replacement {
    replace(&mut Memory::default(), case, cursor(before, after))
}

fn file_name(name: &str, case: Case) -> String {
    replace(&mut Memory::default(), case, file(name)).text
}

fn replacement(text: &str, before: usize, after: usize) -> Replacement {
    Replacement {
        text: text.into(),
        before,
        after,
    }
}

#[test]
fn every_case_splits_words_on_case_and_separators() {
    let text = "userProfile-card_item";
    let expected = [
        (Case::Camel, "userProfileCardItem"),
        (Case::Kebab, "user-profile-card-item"),
        (Case::Lower, "userprofile-card_item"),
        (Case::Pascal, "UserProfileCardItem"),
        (Case::Snake, "user_profile_card_item"),
        (Case::Title, "User Profile Card Item"),
        (Case::Upper, "USERPROFILE-CARD_ITEM"),
    ];
    for (case, converted) in expected {
        assert_eq!(convert(text, case), converted, "{case:?}");
    }
}

#[test]
fn punctuation_and_outer_separators_stay_in_place() {
    assert_eq!(convert("UserCard.test", Case::Kebab), "user-card.test");
    assert_eq!(convert("@scope/my-pkg", Case::Camel), "@scope/myPkg");
    assert_eq!(convert("[...slug]", Case::Pascal), "[...Slug]");
    assert_eq!(convert("__tests__", Case::Upper), "__TESTS__");
    assert_eq!(convert("_app", Case::Snake), "_app");
    assert_eq!(convert("  some text ", Case::Camel), "  someText ");
    assert_eq!(convert("call(myArg, 2)", Case::Snake), "call(my_arg, 2)");
    assert_eq!(
        convert("first line\r\n  second line", Case::Camel),
        "firstLine\r\n  secondLine"
    );
}

#[test]
fn empty_and_separator_only_text_is_unchanged() {
    assert_eq!(convert("", Case::Camel), "");
    assert_eq!(convert(" - _ ", Case::Pascal), " - _ ");
}

#[test]
fn cursor_beside_a_word_converts_only_that_word() {
    assert_eq!(
        around_cursor("some text", " written", Case::Upper),
        replacement("TEXT", 4, 0)
    );
    assert_eq!(
        around_cursor("some ", "text written", Case::Upper),
        replacement("TEXT", 0, 4)
    );
    assert_eq!(
        around_cursor("some te", "xt written", Case::Upper),
        replacement("TEXT", 2, 2)
    );
    assert_eq!(
        around_cursor("open my-file", ".ts", Case::Camel),
        replacement("myFile", 7, 0)
    );
}

#[test]
fn cursor_without_a_word_converts_its_line() {
    assert_eq!(
        around_cursor("some text writen ", "", Case::Camel),
        replacement("someTextWriten ", 17, 0)
    );
    assert_eq!(
        around_cursor("first line\nsome text ", "\nlast line", Case::Kebab),
        replacement("some-text ", 10, 0)
    );
    assert_eq!(around_cursor("", "", Case::Camel), replacement("", 0, 0));
}

#[test]
fn cursor_spans_are_counted_in_utf16_units() {
    assert_eq!(
        around_cursor("x café", "s", Case::Upper),
        replacement("CAFÉS", 4, 1)
    );
    assert_eq!(
        around_cursor("x 𝒜b", "", Case::Upper),
        replacement("𝒜B", 3, 0)
    );
}

#[test]
fn file_names_keep_their_extensions() {
    assert_eq!(
        file_name("some-file-or-text-input.rs", Case::Camel),
        "someFileOrTextInput.rs"
    );
    assert_eq!(
        file_name("user-card.test.tsx", Case::Pascal),
        "UserCard.test.tsx"
    );
    assert_eq!(file_name("my_folder", Case::Kebab), "my-folder");
    assert_eq!(file_name(".eslint-rc.json", Case::Snake), ".eslint_rc.json");
    assert_eq!(file_name("README", Case::Lower), "readme");
}

#[test]
fn lowered_text_keeps_its_words_for_the_next_transform() {
    let mut memory = Memory::default();
    let chain = [
        ("someThing", Case::Lower, "something"),
        ("something", Case::Pascal, "SomeThing"),
        ("SomeThing", Case::Upper, "SOMETHING"),
        ("SOMETHING", Case::Kebab, "some-thing"),
        ("some-thing", Case::Lower, "some-thing"),
        ("some-thing", Case::Camel, "someThing"),
    ];
    for (text, case, converted) in chain {
        assert_eq!(
            replace(&mut memory, case, selection(text)).text,
            converted,
            "{text} to {case:?}"
        );
    }
}

#[test]
fn only_the_last_transform_is_remembered() {
    let mut memory = Memory::default();
    replace(&mut memory, Case::Lower, selection("someThing"));
    replace(&mut memory, Case::Lower, selection("fooBar"));
    assert_eq!(
        replace(&mut memory, Case::Pascal, selection("something")).text,
        "Something"
    );
}

#[test]
fn memory_follows_cursor_and_file_name_targets() {
    let mut memory = Memory::default();
    replace(&mut memory, Case::Lower, cursor("rename someThing", ""));
    assert_eq!(
        replace(&mut memory, Case::Pascal, cursor("rename something", "")),
        replacement("SomeThing", 9, 0)
    );

    replace(&mut memory, Case::Upper, file("userCard.test.ts"));
    assert_eq!(
        replace(&mut memory, Case::Kebab, file("USERCARD.test.ts")).text,
        "user-card.test.ts"
    );
}

#[test]
fn one_transform_remembers_every_target() {
    let mut memory = Memory::default();
    let params = |case, texts: [&str; 2]| Params {
        case,
        targets: texts.into_iter().map(selection).collect(),
    };
    memory.transform(&params(Case::Upper, ["fooBar", "bazQux"]));
    let pascal = memory.transform(&params(Case::Pascal, ["FOOBAR", "BAZQUX"]));
    assert_eq!(
        pascal,
        [replacement("FooBar", 0, 0), replacement("BazQux", 0, 0)]
    );
}

#[test]
fn params_select_the_target() {
    let params: Params = serde_json::from_value(json!({
        "case": "snake",
        "targets": [
            { "scope": "selection", "text": "myFile.ts" },
            { "scope": "cursor", "before": "a myVar", "after": " b" },
            { "scope": "fileName", "name": "user-card.tsx" },
        ],
    }))
    .expect("valid params");
    assert_eq!(
        Memory::default().transform(&params),
        [
            replacement("my_file.ts", 0, 0),
            replacement("my_var", 5, 0),
            replacement("user_card.tsx", 0, 0),
        ]
    );
}
