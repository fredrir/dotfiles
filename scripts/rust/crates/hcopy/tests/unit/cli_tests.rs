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
    assert!(request.target.is_none());
    assert!(request.remote.is_none());
    assert!(!request.yes);
}

#[test]
fn target_argument_is_read_in_both_directions() {
    let push: Request = Push::try_parse_from(["hpush", "folder_1", "12.3456.34"])
        .unwrap()
        .into();
    assert_eq!(push.direction, Direction::Push);
    assert_eq!(push.path.as_deref(), Some("folder_1"));
    assert_eq!(push.target.as_deref(), Some("12.3456.34"));

    let push_dot: Request = Push::try_parse_from(["hpush", ".", "ntnu"])
        .unwrap()
        .into();
    assert_eq!(push_dot.path.as_deref(), Some("."));
    assert_eq!(push_dot.target.as_deref(), Some("ntnu"));

    let pull: Request = Pull::try_parse_from(["hpull", "notes.md", "ntnu"])
        .unwrap()
        .into();
    assert_eq!(pull.direction, Direction::Pull);
    assert_eq!(pull.path.as_deref(), Some("notes.md"));
    assert_eq!(pull.target.as_deref(), Some("ntnu"));

    let pull_dot: Request = Pull::try_parse_from(["hpull", ".", "ntnu"])
        .unwrap()
        .into();
    assert_eq!(pull_dot.path.as_deref(), Some("."));
    assert_eq!(pull_dot.target.as_deref(), Some("ntnu"));
}

#[test]
fn pull_with_from_and_single_positional_interprets_it_as_target() {
    let pull: Request = Pull::try_parse_from(["hpull", "--from", "~/scratch/go", "ntnu"])
        .unwrap()
        .into();
    assert_eq!(pull.direction, Direction::Pull);
    assert_eq!(pull.remote.as_deref(), Some("~/scratch/go"));
    assert_eq!(pull.target.as_deref(), Some("ntnu"));
    assert!(pull.path.is_none());
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
fn neither_direction_accepts_more_than_two_positionals() {
    assert!(Push::try_parse_from(["hpush", "one", "two", "three"]).is_err());
    assert!(Pull::try_parse_from(["hpull", "one", "two", "three"]).is_err());
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
