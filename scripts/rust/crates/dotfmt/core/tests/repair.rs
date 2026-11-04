#![forbid(unsafe_code)]

use dotfmt_core::repair::{Edit, Failure, Fix, Locator, Repair, Repairs, apply, separator, settle};

#[test]
fn edits_apply_in_offset_order_with_insertions_first() {
    let edits = [
        Edit::delete(4, 5),
        Edit::insert(2, ","),
        Edit::replace(0, 1, "["),
        Edit::insert(5, "]"),
        Edit::insert(5, "}"),
    ];
    assert_eq!(apply(b"(1 2,", &edits), b"[1, 2]}");
}

#[test]
fn locations_are_one_based_byte_columns_in_any_order() {
    let source = "ab\nçd\n\nx".as_bytes();
    let mut locator = Locator::new(source);
    assert_eq!(locator.locate(0), (1, 1));
    assert_eq!(locator.locate(5), (2, 3));
    assert_eq!(locator.locate(8), (4, 1));
    assert_eq!(locator.locate(1), (1, 2));
    assert_eq!(locator.locate(100), (4, 2));
}

#[test]
fn repairs_count_by_kind_and_keep_their_order() {
    let fixes = [
        Fix {
            repair: Repair::Comma,
            line: 1,
            column: 2,
        },
        Fix {
            repair: Repair::Quote,
            line: 2,
            column: 1,
        },
        Fix {
            repair: Repair::Comma,
            line: 3,
            column: 4,
        },
    ];
    let repairs: Repairs = fixes.into_iter().collect();
    assert_eq!(repairs.of(Repair::Comma), 2);
    assert_eq!(repairs.of(Repair::Key), 0);
    assert_eq!(repairs.total(), 3);
    assert_eq!(repairs.fixes(), fixes);
    assert!(Repairs::default().is_empty());
}

fn unbalanced(text: &str) -> Option<Failure> {
    let mut open = 0usize;
    let failed = |at| {
        Some(Failure {
            at,
            message: String::new(),
        })
    };
    for (at, byte) in text.bytes().enumerate() {
        match byte {
            b'(' => open += 1,
            b')' if open == 0 => return failed(at),
            b')' => open -= 1,
            b'!' => return failed(at),
            _ => {}
        }
    }
    if open > 0 { failed(text.len()) } else { None }
}

fn close_or_drop(text: &str, failure: &Failure) -> Vec<(Repair, Edit)> {
    let at = failure.at;
    match text.as_bytes().get(at) {
        None => vec![(Repair::Bracket, Edit::insert(at, ")"))],
        Some(b')') => vec![(Repair::Bracket, Edit::delete(at, at + 1))],
        _ => Vec::new(),
    }
}

#[test]
fn settling_fixes_each_error_until_the_source_reads() {
    let repaired = settle("(a))\n(b))\n(c", unbalanced, close_or_drop);
    assert_eq!(repaired.text, "(a)\n(b)\n(c)");
    assert_eq!(
        repaired.repairs.fixes(),
        [
            Fix {
                repair: Repair::Bracket,
                line: 1,
                column: 4
            },
            Fix {
                repair: Repair::Bracket,
                line: 2,
                column: 4
            },
            Fix {
                repair: Repair::Bracket,
                line: 3,
                column: 3
            },
        ]
    );
}

#[test]
fn settling_keeps_what_read_furthest_and_stops_at_what_it_cannot_fix() {
    let repaired = settle("(a)) ! (", unbalanced, close_or_drop);
    assert_eq!(repaired.text, "(a) ! (");
    assert_eq!(repaired.repairs.total(), 1);
    let valid = settle("(a)", unbalanced, close_or_drop);
    assert_eq!(valid.text, "(a)");
    assert!(valid.repairs.is_empty());
}

#[test]
fn settling_refuses_a_fix_that_does_not_move_the_error_forward() {
    let repaired = settle("a)", unbalanced, |_, failure| {
        vec![(Repair::Comma, Edit::insert(failure.at, " "))]
    });
    assert_eq!(repaired.text, "a)");
    assert!(repaired.repairs.is_empty());
}

#[test]
fn settling_locates_every_fix_a_step_makes() {
    let repaired = settle(
        "!a\n!b",
        |text| {
            text.find('!').map(|at| Failure {
                at,
                message: String::new(),
            })
        },
        |text, _| {
            text.match_indices('!')
                .map(|(at, _)| (Repair::Indent, Edit::replace(at, at + 1, " ")))
                .collect()
        },
    );
    assert_eq!(repaired.text, " a\n b");
    assert_eq!(
        repaired.repairs.fixes(),
        [
            Fix {
                repair: Repair::Indent,
                line: 1,
                column: 1
            },
            Fix {
                repair: Repair::Indent,
                line: 2,
                column: 1
            },
        ]
    );
}

#[test]
fn a_missing_separator_goes_after_the_token_before_it_unless_a_comment_may_intervene() {
    assert_eq!(separator("[1 2]", 3, "#"), 2);
    assert_eq!(separator("[\n  1\n  2\n]", 8, "#"), 5);
    assert_eq!(separator("[\n  1 # one\n  2\n]", 14, "#"), 14);
    assert_eq!(separator("2", 0, "#"), 0);
}
