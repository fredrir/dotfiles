use super::*;

fn plan(direction: Direction) -> Plan {
    Plan {
        direction,
        host: "archie".into(),
        local: PathBuf::from("/Users/fredrir/projects/my-app"),
        local_display: "~/projects/my-app".into(),
        remote: "/home/fredrir/projects/my-app".into(),
        remote_display: "~/projects/my-app".into(),
        route: Some(Route::Cable),
        dry_run: false,
        checksum: false,
        all: false,
    }
}

#[test]
fn an_old_push_client_shell_quotes_the_remote_directory() {
    let plan = plan(Direction::Push);
    assert_eq!(
        plan.source(RemoteArguments::ShellQuoted),
        "/Users/fredrir/projects/my-app"
    );
    assert_eq!(
        plan.destination(RemoteArguments::ShellQuoted),
        "archie:'/home/fredrir/projects'/"
    );
}

#[test]
fn a_modern_pull_client_leaves_the_remote_path_for_rsync_to_protect() {
    let plan = plan(Direction::Pull);
    assert_eq!(
        plan.source(RemoteArguments::Protected),
        "archie:/home/fredrir/projects/my-app"
    );
    assert_eq!(
        plan.destination(RemoteArguments::Protected),
        "/Users/fredrir/projects/"
    );
}

#[test]
fn both_argument_styles_preserve_a_remote_path_that_needs_quoting() {
    let mut plan = plan(Direction::Pull);
    plan.remote = "/home/fredrir/odd path/it's here".into();
    assert_eq!(
        plan.source(RemoteArguments::Protected),
        "archie:/home/fredrir/odd path/it's here"
    );
    assert_eq!(
        plan.source(RemoteArguments::ShellQuoted),
        "archie:'/home/fredrir/odd path/it'\\''s here'"
    );
}

#[test]
fn the_client_help_selects_the_argument_contract_it_supports() {
    assert_eq!(
        remote_arguments_from_help(b"--old-args  disable modern protection\n", b""),
        RemoteArguments::Protected
    );
    assert_eq!(
        remote_arguments_from_help(b"openrsync options\n", b""),
        RemoteArguments::ShellQuoted
    );
    assert_eq!(
        remote_arguments_from_help(b"", b"usage: rsync --old-args\n"),
        RemoteArguments::Protected
    );
}

#[test]
fn a_modern_client_is_pinned_to_the_detected_contract() {
    let modern = plan(Direction::Pull).arguments(RemoteArguments::Protected);
    let legacy = plan(Direction::Pull).arguments(RemoteArguments::ShellQuoted);
    assert!(modern.contains(&"--no-old-args".to_string()));
    assert!(!legacy.contains(&"--no-old-args".to_string()));
}

#[test]
fn the_default_transfer_skips_what_git_was_told_to_skip() {
    let arguments = plan(Direction::Push).arguments(RemoteArguments::Protected);
    assert!(arguments.contains(&GITIGNORE.to_string()));
    assert!(arguments.contains(&"--exclude=.git/".to_string()));
}

#[test]
fn all_turns_every_filter_off_at_once() {
    let mut plan = plan(Direction::Push);
    plan.all = true;
    let arguments = plan.arguments(RemoteArguments::Protected);
    assert!(!arguments.contains(&GITIGNORE.to_string()));
    assert!(!arguments.contains(&"--exclude=.git/".to_string()));
    assert!(
        !arguments
            .iter()
            .any(|arg| arg.starts_with("--exclude-from="))
    );
}

#[test]
fn a_fast_route_sends_whole_files_and_a_slow_one_does_not() {
    for route in [Route::Cable, Route::Wifi, Route::Lan] {
        let mut plan = plan(Direction::Push);
        plan.route = Some(route);
        assert!(
            plan.arguments(RemoteArguments::Protected)
                .contains(&"-W".to_string())
        );
    }
    let mut plan = plan(Direction::Push);
    plan.route = Some(Route::Tailscale);
    assert!(
        !plan
            .arguments(RemoteArguments::Protected)
            .contains(&"-W".to_string())
    );
    plan.route = None;
    assert!(
        !plan
            .arguments(RemoteArguments::Protected)
            .contains(&"-W".to_string())
    );
}

#[test]
fn the_paths_are_the_last_two_arguments_and_nothing_reads_them_as_flags() {
    let arguments = plan(Direction::Push).arguments(RemoteArguments::ShellQuoted);
    let end = arguments.len();
    assert_eq!(arguments[end - 3], "--");
    assert_eq!(arguments[end - 2], "/Users/fredrir/projects/my-app");
    assert_eq!(arguments[end - 1], "archie:'/home/fredrir/projects'/");
}

#[test]
fn dry_run_and_checksum_reach_the_command() {
    let mut plan = plan(Direction::Push);
    plan.dry_run = true;
    plan.checksum = true;
    let arguments = plan.arguments(RemoteArguments::Protected);
    assert!(arguments.contains(&"-n".to_string()));
    assert!(arguments.contains(&"-c".to_string()));
}

#[test]
fn a_transferred_file_is_counted_with_its_size() {
    let mut outcome = Outcome::default();
    absorb(&mut outcome, ">f+++++++|projects/my-app/main.rs|1024");
    absorb(&mut outcome, ">f..t....|projects/my-app/lib.rs|2048");
    assert_eq!(outcome.files, 2);
    assert_eq!(outcome.created, 1);
    assert_eq!(outcome.bytes, 3072);
}

#[test]
fn an_unchanged_file_is_not_a_transfer() {
    let mut outcome = Outcome::default();
    absorb(&mut outcome, ".f........|projects/my-app/main.rs|1024");
    assert_eq!(outcome, Outcome::default());
    assert!(outcome.quiet());
}

#[test]
fn a_new_directory_counts_once_and_carries_no_bytes() {
    let mut outcome = Outcome::default();
    absorb(&mut outcome, "cd+++++++|projects/my-app/|128");
    assert_eq!(outcome.files, 0);
    assert_eq!(outcome.created, 1);
    assert_eq!(outcome.bytes, 0);
    assert!(!outcome.quiet());
}

#[test]
fn a_name_containing_a_bar_is_not_cut_in_half() {
    let mut outcome = Outcome::default();
    absorb(&mut outcome, ">f+++++++|odd|name.txt|64");
    assert_eq!(outcome.files, 1);
    assert_eq!(outcome.bytes, 64);
    assert_eq!(outcome.lines, [">f+++++++ odd|name.txt"]);
}

#[test]
fn anything_that_is_not_a_formatted_line_is_ignored() {
    let mut outcome = Outcome::default();
    absorb(&mut outcome, "sent 147 bytes  received 38 bytes");
    absorb(&mut outcome, "");
    absorb(&mut outcome, "created directory /tmp/x");
    assert_eq!(outcome, Outcome::default());
}

#[test]
fn a_failure_is_reported_by_its_cause_rather_than_its_code() {
    let stderr = "rsync: link_stat \"/x\" failed: No such file or directory (2)\n\
                      rsync error: some files could not be transferred (code 23)\n";
    assert!(explain(stderr, Some(23)).contains("No such file or directory"));
    assert_eq!(explain("", Some(23)), "rsync exited with status 23");
    assert_eq!(explain("", None), "rsync was killed");
}

#[test]
fn an_error_with_only_a_summary_line_still_says_something() {
    let stderr = "rsync error: unexplained error (code 255)\n";
    assert!(explain(stderr, Some(255)).contains("unexplained error"));
}
