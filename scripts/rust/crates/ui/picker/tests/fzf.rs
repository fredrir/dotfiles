use ui_picker::fzf::{self, Row};

#[test]
fn row_identity_survives_unsafe_text_and_invalid_results_are_rejected() {
    let rows = [
        Row {
            label: "a\tb\n\u{1b}[2J",
            copy: "",
        },
        Row {
            label: "界",
            copy: "copied value",
        },
    ];
    let input = fzf::input(&rows);
    assert_eq!(input.lines().count(), 2);
    assert_eq!(input.lines().next().unwrap().split('\t').count(), 3);
    assert!(!input.contains('\u{1b}'));
    assert_eq!(
        fzf::selection(0, "1\tcopied value\t界", 2).unwrap(),
        Some(1)
    );
    assert!(fzf::selection(0, "2\tunknown", 2).is_err());
    assert_eq!(fzf::selection(130, "", 2).unwrap(), None);
}

#[test]
fn plain_search_keeps_absolute_row_numbers_and_handles_eof() {
    let rows = [
        Row {
            label: "one",
            copy: "",
        },
        Row {
            label: "two",
            copy: "",
        },
    ];
    let mut output = Vec::new();
    assert_eq!(
        fzf::choose_plain("Choose", &rows, &mut &b"two\n2\n"[..], &mut output).unwrap(),
        Some(1)
    );
    assert!(String::from_utf8(output).unwrap().contains("  2  two"));
    assert_eq!(
        fzf::choose_plain("Choose", &rows, &mut &b""[..], &mut Vec::new()).unwrap(),
        None
    );
}
