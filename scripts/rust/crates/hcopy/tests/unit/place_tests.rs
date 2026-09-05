use super::*;

// Nowhere real, so that resolving never reaches this machine's own
// filesystem and the lexical rules are what is under test.
fn home_path() -> PathBuf {
    PathBuf::from("/nowhere/fredrir")
}

#[test]
fn a_tilde_is_this_home() {
    assert_eq!(expand("~", &home_path()), home_path());
    assert_eq!(
        expand("~/projects/go", &home_path()),
        PathBuf::from("/nowhere/fredrir/projects/go")
    );
}

#[test]
fn a_tilde_only_leads_a_path() {
    assert_eq!(expand("a~b", &home_path()), PathBuf::from("a~b"));
    assert_eq!(expand("./~", &home_path()), PathBuf::from("./~"));
}

#[test]
fn tidy_removes_the_steps_that_go_nowhere() {
    assert_eq!(
        tidy(Path::new("/Users/fredrir/./projects/../go")),
        PathBuf::from("/Users/fredrir/go")
    );
}

#[test]
fn home_itself_is_not_a_path_to_copy() {
    let error = resolve("~", &home_path()).unwrap_err();
    assert!(error.contains("whole home directory"), "{error}");
}

#[test]
fn a_path_outside_home_is_refused() {
    let error = resolve("/etc/hosts", &home_path()).unwrap_err();
    assert!(error.contains("inside your home directory"));
}

#[test]
fn a_symlink_is_followed_to_what_it_points_at() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    std::fs::create_dir_all(home.join("dotfiles/tmux")).unwrap();
    std::fs::write(home.join("dotfiles/tmux/tmux.conf"), "").unwrap();
    std::os::unix::fs::symlink(
        home.join("dotfiles/tmux/tmux.conf"),
        home.join(".tmux.conf"),
    )
    .unwrap();

    let local = resolve(&home.join(".tmux.conf").to_string_lossy(), &home).unwrap();
    assert_eq!(local.relative, "dotfiles/tmux/tmux.conf");
    assert_eq!(local.name, "tmux.conf");
}

#[test]
fn a_path_that_is_not_there_yet_is_still_resolved() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let local = resolve(&home.join("not/here/yet").to_string_lossy(), &home).unwrap();
    assert_eq!(local.relative, "not/here/yet");
    assert_eq!(local.name, "yet");
}

#[test]
fn a_resolved_path_knows_its_own_shape() {
    let local = resolve("~/projects/my-app", &home_path()).unwrap();
    assert_eq!(local.relative, "projects/my-app");
    assert_eq!(local.name, "my-app");
    assert_eq!(local.parent(), "projects");
    assert_eq!(local.display(), "~/projects/my-app");
}

#[test]
fn a_path_directly_in_home_has_no_parent_below_it() {
    let local = resolve("~/.tmux.conf", &home_path()).unwrap();
    assert_eq!(local.relative, ".tmux.conf");
    assert_eq!(local.parent(), "");
}

#[test]
fn remote_paths_are_joined_without_doubling_the_separator() {
    assert_eq!(join("/home/f", "go"), "/home/f/go");
    assert_eq!(join("/home/f/", "go"), "/home/f/go");
    assert_eq!(join("/", "go"), "/go");
}

#[test]
fn a_remote_path_can_be_taken_apart() {
    assert_eq!(parent_of("/home/f/projects/go"), "/home/f/projects");
    assert_eq!(name_of("/home/f/projects/go"), "go");
    assert_eq!(parent_of("/home"), "/");
    assert_eq!(parent_of("/"), "/");
    assert_eq!(name_of("/home/f/go/"), "go");
}

#[test]
fn a_typed_remote_path_is_resolved_against_the_remote_home() {
    assert_eq!(expand_remote("~/go", "/home/f"), "/home/f/go");
    assert_eq!(expand_remote("~", "/home/f"), "/home/f");
    assert_eq!(expand_remote("/etc", "/home/f"), "/etc");
    assert_eq!(expand_remote("go", "/home/f"), "/home/f/go");
}

#[test]
fn a_pulled_path_from_that_home_lands_in_the_same_place_under_this_one() {
    let (landed, shown) = landing(
        "/home/fredrir/projects/my-app",
        "/home/fredrir",
        Path::new("/Users/fredrir"),
        Path::new("/Users/fredrir/somewhere"),
    )
    .unwrap();
    assert_eq!(landed, PathBuf::from("/Users/fredrir/projects/my-app"));
    assert_eq!(shown, "~/projects/my-app");
}

#[test]
fn a_pulled_path_from_outside_that_home_lands_where_it_was_asked_for() {
    let (landed, shown) = landing(
        "/etc/ssh/sshd_config",
        "/home/fredrir",
        Path::new("/Users/fredrir"),
        Path::new("/Users/fredrir/notes"),
    )
    .unwrap();
    assert_eq!(landed, PathBuf::from("/Users/fredrir/notes/sshd_config"));
    assert_eq!(shown, "~/notes/sshd_config");
}

#[test]
fn a_pull_into_somewhere_outside_this_home_is_refused() {
    let error = landing(
        "/etc/ssh/sshd_config",
        "/home/fredrir",
        Path::new("/Users/fredrir"),
        Path::new("/tmp"),
    )
    .unwrap_err();
    assert!(error.contains("outside this one"));
}

#[test]
fn a_home_that_only_shares_a_prefix_is_not_that_home() {
    let (landed, _) = landing(
        "/home/fredrir2/notes",
        "/home/fredrir",
        Path::new("/Users/fredrir"),
        Path::new("/Users/fredrir"),
    )
    .unwrap();
    assert_eq!(landed, PathBuf::from("/Users/fredrir/notes"));
}
