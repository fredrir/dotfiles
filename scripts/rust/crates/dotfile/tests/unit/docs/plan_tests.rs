use super::*;

#[test]
fn changed_output_aborts_the_batch_and_preserves_new_occupants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("a.md"), "first original").unwrap();
    fs::write(root.join("b.md"), "second original").unwrap();
    let modified = fs::metadata(root.join("a.md")).unwrap().modified().unwrap();
    let plan = Plan::new(
        root,
        vec![
            Output::text("a.md", "first replacement".into()),
            Output::text("b.md", "second replacement".into()),
        ],
    )
    .unwrap();
    fs::remove_file(root.join("b.md")).unwrap();
    fs::create_dir(root.join("b.md")).unwrap();
    assert!(plan.apply(root).is_err());
    assert_eq!(
        fs::read_to_string(root.join("a.md")).unwrap(),
        "first original"
    );
    assert_eq!(
        fs::metadata(root.join("a.md")).unwrap().modified().unwrap(),
        modified
    );
    assert!(root.join("b.md").is_dir());
    assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}

#[test]
fn edits_after_planning_abort_before_the_first_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("a.md"), "first original").unwrap();
    fs::write(root.join("b.md"), "second original").unwrap();
    let plan = Plan::new(
        root,
        vec![
            Output::text("a.md", "first replacement".into()),
            Output::text("b.md", "second replacement".into()),
        ],
    )
    .unwrap();
    fs::write(root.join("b.md"), "authored after planning").unwrap();
    assert!(
        plan.apply(root)
            .unwrap_err()
            .contains("changed after planning")
    );
    assert_eq!(
        fs::read_to_string(root.join("a.md")).unwrap(),
        "first original"
    );
    assert_eq!(
        fs::read_to_string(root.join("b.md")).unwrap(),
        "authored after planning"
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn write_failure_rolls_back_preceding_replacements() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(root.join("a.md"), "original").unwrap();
    fs::write(locked.join("b.md"), "original").unwrap();
    let modified = fs::metadata(root.join("a.md")).unwrap().modified().unwrap();
    let plan = Plan::new(
        root,
        vec![
            Output::text("a.md", "replacement".into()),
            Output::text("locked/b.md", "replacement".into()),
        ],
    )
    .unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let result = plan.apply(root);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(root.join("a.md")).unwrap(), "original");
    assert_eq!(fs::read_to_string(locked.join("b.md")).unwrap(), "original");
    assert_eq!(
        fs::metadata(root.join("a.md")).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}
