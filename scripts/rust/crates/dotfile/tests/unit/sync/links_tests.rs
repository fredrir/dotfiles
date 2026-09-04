use std::os::unix::fs::symlink;

use super::{Operation, apply};
use crate::event::VecSink;

#[test]
fn managed_directory_replacement_preserves_a_late_file() {
    let temporary = tempfile::tempdir().unwrap();
    let repository = temporary.path().join("repo");
    let destination = temporary.path().join("destination");
    let target = repository.join("old");
    let replacement = repository.join("new");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(&target, "old\n").unwrap();
    std::fs::write(&replacement, "new\n").unwrap();
    let child = destination.join("managed");
    symlink(&target, &child).unwrap();
    let operations = vec![
        Operation::RemoveManagedLink {
            path: child,
            target,
        },
        Operation::RemoveDirectory(destination.clone()),
        Operation::Symlink {
            source: replacement,
            destination: destination.clone(),
        },
    ];
    let late = destination.join("late");
    std::fs::write(&late, "mine\n").unwrap();
    assert!(apply(&operations, &VecSink::default()).is_err());
    assert_eq!(std::fs::read_to_string(late).unwrap(), "mine\n");
    assert!(destination.is_dir());
}

#[test]
fn managed_directory_replacement_preserves_a_changed_leaf() {
    let temporary = tempfile::tempdir().unwrap();
    let repository = temporary.path().join("repo");
    let destination = temporary.path().join("destination");
    let target = repository.join("old");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(&target, "old\n").unwrap();
    let child = destination.join("managed");
    symlink(&target, &child).unwrap();
    let operations = [Operation::RemoveManagedLink {
        path: child.clone(),
        target,
    }];
    std::fs::remove_file(&child).unwrap();
    std::fs::write(&child, "mine\n").unwrap();
    assert!(apply(&operations, &VecSink::default()).is_err());
    assert_eq!(std::fs::read_to_string(child).unwrap(), "mine\n");
}
