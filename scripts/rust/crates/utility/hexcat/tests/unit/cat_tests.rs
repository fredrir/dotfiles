use super::*;
use clap::Parser;

#[derive(Parser)]
#[command(args_override_self = true)]
struct Probe {
    #[command(flatten)]
    cat: CatFlags,
}

fn flags(arguments: &[&str]) -> CatFlags {
    Probe::parse_from(std::iter::once("hexcat").chain(arguments.iter().copied())).cat
}

fn letters(arguments: &[&str]) -> String {
    flags(arguments).letters()
}

fn arguments(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn bundled_and_repeated_flags_become_one_set_of_letters() {
    assert_eq!(letters(&["-nb", "-n"]), "bn");
}

#[test]
fn long_flags_become_their_letters() {
    assert_eq!(
        letters(&["--show-all", "--number", "--squeeze-blank", "--show-tabs"]),
        "AnsT"
    );
}

#[test]
fn every_cat_letter_is_known() {
    assert_eq!(letters(&["-AbeElnstTuv"]), "AbeElnstTuv");
}

#[test]
fn bat_highlights_with_the_flags_it_shares_with_cat() {
    assert_eq!(flags(&[]).highlight(), Some(Highlight::default()));
    assert_eq!(
        flags(&["-Ansu"]).highlight(),
        Some(Highlight {
            show_all: true,
            number: true,
            squeeze_blank: true
        })
    );
}

#[test]
fn flags_only_cat_has_leave_highlighting_to_cat() {
    for flag in ["-b", "-e", "-E", "-l", "-t", "-T", "-v"] {
        assert_eq!(flags(&["-n", flag]).highlight(), None, "{flag}");
    }
}

#[test]
fn cat_gets_the_letters_then_the_files() {
    let command = cat("ns", &["a".into(), "-b".into()]);
    assert_eq!(command.get_program(), "cat");
    assert_eq!(arguments(&command), ["-ns", "--", "a", "-b"]);
}

#[test]
fn cat_without_letters_gets_only_the_files() {
    assert_eq!(arguments(&cat("", &[])), ["--"]);
}
