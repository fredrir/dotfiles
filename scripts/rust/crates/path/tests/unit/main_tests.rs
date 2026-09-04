use super::*;

fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::create_dir_all(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/file.txt"), "").unwrap();
    root
}

fn described(target: &str, root: Option<&str>, home: Option<&str>) -> String {
    describe(Path::new(target), root.map(Path::new), home.map(Path::new))
}

#[test]
fn inside_a_repository_the_root_is_the_top() {
    assert_eq!(described("/w/repo", Some("/w/repo"), None), "/");
    assert_eq!(
        described("/w/repo/sub/file", Some("/w/repo"), None),
        "/sub/file"
    );
}

#[test]
fn a_repository_wins_over_the_home_directory() {
    assert_eq!(
        described("/home/u/repo/src", Some("/home/u/repo"), Some("/home/u")),
        "/src"
    );
}

#[test]
fn outside_a_repository_the_home_directory_is_a_tilde() {
    assert_eq!(described("/home/u", None, Some("/home/u")), "~");
    assert_eq!(described("/home/u/docs", None, Some("/home/u")), "~/docs");
}

#[test]
fn outside_both_the_path_is_left_alone() {
    assert_eq!(described("/usr/share", None, Some("/home/u")), "/usr/share");
    assert_eq!(described("/usr/share", None, None), "/usr/share");
}

#[test]
fn a_shared_prefix_is_not_a_shared_directory() {
    assert_eq!(
        described("/w/repository/src", Some("/w/repo"), None),
        "/w/repository/src"
    );
    assert_eq!(
        described("/home/user2/x", None, Some("/home/user")),
        "/home/user2/x"
    );
}

#[test]
fn the_root_is_the_nearest_ancestor_holding_git() {
    let root = repo();
    let real = fs::canonicalize(root.path()).unwrap();
    assert_eq!(
        repository_root(&real.join("sub/file.txt")),
        Some(real.clone())
    );
    assert_eq!(repository_root(&real.join("does/not/exist")), Some(real));
}

#[test]
fn a_git_file_marks_a_worktree_root() {
    let root = tempfile::tempdir().unwrap();
    let real = fs::canonicalize(root.path()).unwrap();
    fs::write(real.join(".git"), "gitdir: /elsewhere\n").unwrap();
    assert_eq!(repository_root(&real.join("sub")), Some(real));
}

#[test]
fn a_directory_without_git_has_no_root() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    assert!(repository_root(&nested).is_none_or(|found| !found.starts_with(root.path())));
}

#[test]
fn existing_targets_resolve_the_way_canonicalize_does() {
    let root = repo();
    let file = root.path().join("sub/file.txt");
    assert_eq!(real_path(&file), fs::canonicalize(&file).unwrap());
}

#[test]
fn missing_targets_keep_the_part_that_is_missing() {
    let root = repo();
    let real = fs::canonicalize(root.path()).unwrap();
    assert_eq!(
        real_path(&root.path().join("missing/deep.txt")),
        real.join("missing/deep.txt")
    );
}

#[test]
fn dots_fold_away_even_where_nothing_exists() {
    let root = repo();
    let real = fs::canonicalize(root.path()).unwrap();
    assert_eq!(
        real_path(&root.path().join("missing/../other")),
        real.join("other")
    );
    assert_eq!(
        real_path(&root.path().join("./sub/../sub")),
        real.join("sub")
    );
}

#[cfg(unix)]
#[test]
fn symlinks_in_the_existing_part_are_followed() {
    let root = repo();
    let real = fs::canonicalize(root.path()).unwrap();
    std::os::unix::fs::symlink(real.join("sub"), real.join("link")).unwrap();
    assert_eq!(
        real_path(&real.join("link/new.txt")),
        real.join("sub/new.txt")
    );
}
