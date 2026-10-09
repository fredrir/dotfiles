use super::*;

fn dump(dir: &Path, count: usize, functions: &[&str], revision: &str) -> PathBuf {
    let path = dir.join(".zcompdump-host-5.9");
    let text = format!(
        "#files: {count}\tversion: 5.9\n\n_comps=(\n)\n\nautoload -Uz {} \\\n            _tail\nautoload -Uz +X _call_program\n\n#omz revision: {revision}\n",
        functions.join(" ")
    );
    fs::write(&path, text).unwrap();
    path
}

fn setup(files: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let completions = root.path().join("completions");
    fs::create_dir_all(&completions).unwrap();
    for file in files {
        fs::write(completions.join(file), "#compdef x\n").unwrap();
    }
    fs::write(
        root.path().join(".zcompdump-host-5.9.fpath"),
        format!("{}\n", completions.display()),
    )
    .unwrap();
    (root, completions)
}

#[test]
fn a_dump_matching_its_functions_is_kept() {
    let (root, _) = setup(&["_a", "_tail", "_call_program", "_a.zwc", "_b~", "x"]);
    let path = dump(root.path(), 3, &["_a"], "abc");
    assert!(refresh(root.path(), None, false).unwrap().is_empty());
    assert!(path.is_file());
}

#[test]
fn an_added_completion_file_refreshes_the_dump() {
    let (root, completions) = setup(&["_a", "_tail", "_call_program"]);
    let path = dump(root.path(), 3, &["_a"], "abc");
    fs::write(completions.join("_new"), "").unwrap();
    fs::write(root.path().join(".zcompdump-host-5.9.zwc"), "").unwrap();
    assert_eq!(
        refresh(root.path(), None, false).unwrap(),
        vec![path.clone()]
    );
    assert!(!path.exists());
    assert!(!root.path().join(".zcompdump-host-5.9.zwc").exists());
    assert!(!root.path().join(".zcompdump-host-5.9.fpath").exists());
}

#[test]
fn a_renamed_function_refreshes_the_dump() {
    let (root, _) = setup(&["_renamed", "_tail", "_call_program"]);
    dump(root.path(), 3, &["_a"], "abc");
    assert_eq!(refresh(root.path(), None, true).unwrap().len(), 1);
    assert!(root.path().join(".zcompdump-host-5.9").is_file());
}

#[test]
fn a_new_oh_my_zsh_revision_refreshes_the_dump() {
    let (root, _) = setup(&["_a", "_tail", "_call_program"]);
    dump(root.path(), 3, &["_a"], "abc");
    let omz = root.path().join("omz");
    fs::create_dir_all(omz.join(".git/refs/heads")).unwrap();
    fs::write(omz.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
    fs::write(omz.join(".git/refs/heads/master"), "abc\n").unwrap();
    assert!(refresh(root.path(), Some(&omz), true).unwrap().is_empty());
    fs::write(omz.join(".git/refs/heads/master"), "def\n").unwrap();
    assert_eq!(refresh(root.path(), Some(&omz), true).unwrap().len(), 1);
}

#[test]
fn a_stamp_without_its_dump_is_removed_quietly() {
    let (root, _) = setup(&["_a"]);
    assert!(refresh(root.path(), None, false).unwrap().is_empty());
    assert!(!root.path().join(".zcompdump-host-5.9.fpath").exists());
}
