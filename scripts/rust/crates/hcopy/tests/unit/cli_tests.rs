use super::*;
use clap::CommandFactory;

#[test]
fn each_direction_names_itself() {
    assert_eq!(Direction::Push.program(), "hpush");
    assert_eq!(Direction::Pull.program(), "hpull");
    assert_eq!(Direction::Push.verb(), "push");
}

#[test]
fn both_parsers_are_well_formed() {
    Push::command().debug_assert();
    Pull::command().debug_assert();
}

#[test]
fn a_bare_command_asks_for_nothing() {
    let request: Request = Push::try_parse_from(["hpush"]).unwrap().into();
    assert!(request.path.is_none());
    assert!(request.remote.is_none());
    assert!(!request.yes);
}

#[test]
fn each_direction_reads_its_own_remote_flag() {
    let push: Request = Push::try_parse_from(["hpush", "go", "--to", "~/x"])
        .unwrap()
        .into();
    assert_eq!(push.direction, Direction::Push);
    assert_eq!(push.remote.as_deref(), Some("~/x"));
    assert_eq!(push.path.as_deref(), Some("go"));

    let pull: Request = Pull::try_parse_from(["hpull", "--from", "~/y"])
        .unwrap()
        .into();
    assert_eq!(pull.direction, Direction::Pull);
    assert_eq!(pull.remote.as_deref(), Some("~/y"));
}

#[test]
fn neither_direction_answers_the_other_ones_flag() {
    assert!(Push::try_parse_from(["hpush", "--from", "~/x"]).is_err());
    assert!(Pull::try_parse_from(["hpull", "--to", "~/x"]).is_err());
}

#[test]
fn the_old_spelling_of_all_still_works() {
    let request: Request = Push::try_parse_from(["hpush", "--no-excludes"])
        .unwrap()
        .into();
    assert!(request.all);
}

#[test]
fn the_short_flags_keep_their_old_meanings() {
    let request: Request = Push::try_parse_from(["hpush", "-n", "-c", "go"])
        .unwrap()
        .into();
    assert!(request.dry_run);
    assert!(request.checksum);
    assert_eq!(request.path.as_deref(), Some("go"));
}
