use std::process::Command;

use super::*;

const WAIT: Duration = Duration::from_secs(10);

fn local() -> Shell {
    Shell::open(&mut Command::new("sh")).unwrap()
}

#[test]
fn a_batch_answers_in_order() {
    let mut shell = local();
    let replies = shell
        .ask(&["printf one".into(), "printf two".into()], WAIT)
        .unwrap();
    assert_eq!(replies, [b"one".to_vec(), b"two".to_vec()]);
}

#[test]
fn a_question_leaves_no_state_behind() {
    let mut shell = local();
    shell.ask(&["cd /; kept=yes".into()], WAIT).unwrap();
    let replies = shell
        .ask(
            &[
                "printf '%s ' \"${kept:-no}\"; [ \"$PWD\" = / ] && printf moved || printf stayed"
                    .into(),
            ],
            WAIT,
        )
        .unwrap();
    assert_eq!(replies[0], b"no stayed");
}

#[test]
fn silence_and_nul_bytes_are_replies_too() {
    let mut shell = local();
    let replies = shell
        .ask(
            &[
                "true".into(),
                "printf 'a\\000\\000b\\000'".into(),
                "exit 3".into(),
            ],
            WAIT,
        )
        .unwrap();
    assert_eq!(replies[0], b"");
    assert_eq!(replies[1], b"a\0\0b\0");
    assert_eq!(replies[2], b"");
}

#[test]
fn nothing_a_question_reads_is_taken_from_the_next_one() {
    let mut shell = local();
    let replies = shell
        .ask(&["cat".into(), "printf after".into()], WAIT)
        .unwrap();
    assert_eq!(replies, [b"".to_vec(), b"after".to_vec()]);
}

#[test]
fn a_shell_that_dies_reports_what_it_said() {
    let mut shell = Shell::open(Command::new("sh").args(["-c", "echo gone for good >&2"])).unwrap();
    assert_eq!(shell.ask(&["true".into()], WAIT), Err(Failure::Lost));
    assert_eq!(shell.complaint().as_deref(), Some("gone for good"));
}

#[test]
fn a_question_left_unanswered_times_out() {
    let mut shell = local();
    let failure = shell.ask(&["sleep 5".into()], Duration::from_millis(100));
    assert_eq!(failure, Err(Failure::TimedOut));
}

#[test]
fn a_reply_past_the_limit_is_refused_without_being_kept() {
    let mut shell = local();
    let flood = format!("head -c {} /dev/zero | tr '\\000' a", REPLY_LIMIT + 1);
    assert_eq!(shell.ask(&[flood], WAIT), Err(Failure::TooLarge));
}
