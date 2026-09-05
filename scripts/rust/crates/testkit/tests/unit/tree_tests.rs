use super::*;

#[test]
fn a_spec_line_splits_on_the_first_equals() {
    let root = tree(&["a=one", "sub/b=two=three", "bare"]);
    assert_eq!(fs::read_to_string(root.path().join("a")).unwrap(), "one");
    assert_eq!(
        fs::read_to_string(root.path().join("sub/b")).unwrap(),
        "two=three"
    );
    assert_eq!(fs::read_to_string(root.path().join("bare")).unwrap(), "");
}

#[test]
fn a_trailing_slash_makes_an_empty_directory() {
    let root = tree(&[".git/", "sub/file.txt"]);
    assert!(root.path().join(".git").is_dir());
    assert!(names(&root.path().join(".git")).is_empty());
    assert!(root.path().join("sub/file.txt").is_file());
}

#[test]
fn pairs_keep_an_equals_sign_in_the_contents() {
    let root = tree_pairs(&[("host.conf", "a = 1\n"), ("deep/nested/file", "x")]);
    assert_eq!(
        fs::read_to_string(root.path().join("host.conf")).unwrap(),
        "a = 1\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("deep/nested/file")).unwrap(),
        "x"
    );
}

#[test]
fn names_are_sorted_and_include_hidden_entries() {
    let root = tree(&["b", "a", ".hidden", "sub/c"]);
    assert_eq!(names(root.path()), [".hidden", "a", "b", "sub"]);
}

#[test]
fn at_spells_out_a_path_inside_the_tree() {
    let root = tree(&["a"]);
    assert_eq!(at(&root, "a"), root.path().join("a").display().to_string());
}

#[cfg(unix)]
#[test]
fn an_executable_file_is_written_verbatim_and_can_be_run() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("stub.sh");
    executable(&script, "#!/bin/sh\nprintf ran\n");
    assert_eq!(
        fs::read_to_string(&script).unwrap(),
        "#!/bin/sh\nprintf ran\n"
    );
    assert_eq!(
        fs::metadata(&script).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(crate::Bin::new(&script).run().stdout, "ran");
}
