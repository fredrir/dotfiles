use super::*;

fn repository(head: &str) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let git = root.path().join(".git");
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), head).unwrap();
    (root, git)
}

#[test]
fn head_names_a_branch_loose_or_packed_or_a_detached_commit() {
    let (_root, git) = repository("ref: refs/heads/main\n");
    assert_eq!(revision(&git), None);
    fs::write(
        git.join("packed-refs"),
        "# pack-refs with: peeled fully-peeled sorted\n\
         111 refs/heads/x/refs/heads/main\n\
         222 refs/heads/main\n\
         ^333\n",
    )
    .unwrap();
    assert_eq!(revision(&git).as_deref(), Some("222"));
    fs::write(git.join("refs/heads/main"), "444\n").unwrap();
    assert_eq!(revision(&git).as_deref(), Some("444"));
    fs::write(git.join("HEAD"), "555\n").unwrap();
    assert_eq!(revision(&git).as_deref(), Some("555"));
}

#[test]
fn the_nearest_git_directory_holds_a_path() {
    let (root, git) = repository("ref: refs/heads/main\n");
    let nested = root.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    assert_eq!(dir(&nested), Some(git.clone()));
    assert_eq!(dir(root.path()), Some(git));

    let worktree = root.path().join("a");
    fs::write(worktree.join(".git"), "gitdir: /elsewhere\n").unwrap();
    assert_eq!(dir(&nested), None);
}
