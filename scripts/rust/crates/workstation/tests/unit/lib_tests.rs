use super::*;

#[derive(clap::Parser)]
struct Nested {
    #[command(flatten)]
    completions: Completions,
}

#[derive(clap::Parser)]
struct Cli {
    #[command(flatten)]
    common: Nested,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.common.completions
    }
}

#[test]
fn a_completable_parser_hands_back_the_flag_it_flattened() {
    use clap::Parser;

    let asked = Cli::try_parse_from(["tool", "--completions", "zsh"]).expect("a parse");
    assert!(asked.completions().is_zsh());

    let bare = Cli::try_parse_from(["tool"]).expect("a parse");
    assert!(!bare.completions().is_zsh());
    assert!(!bare.completions().dump);
}

#[test]
fn git_statuses_pass_through_as_exit_bytes() {
    assert_eq!(exit_byte(0), 0);
    assert_eq!(exit_byte(1), 1);
    assert_eq!(exit_byte(128), 128);
}

#[test]
fn statuses_outside_a_byte_still_fail() {
    assert_eq!(exit_byte(-1), 1);
    assert_eq!(exit_byte(300), 1);
}
