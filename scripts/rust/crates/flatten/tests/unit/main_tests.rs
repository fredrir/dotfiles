use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use testkit::tree;

fn listing(root: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut pending = vec![PathBuf::from(root)];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                found.insert(format!("{}/", relative.display()));
                pending.push(entry.path());
            } else {
                found.insert(relative.display().to_string());
            }
        }
    }
    found.into_iter().collect()
}

fn silent() -> impl FnMut(&str, &str) {
    |_, _| {}
}

fn collapse(root: &Path) -> apply::Done {
    match plan::collapse(root).unwrap() {
        Plan::Collapse(plan) => apply::collapse(root, &plan, &mut silent()).unwrap(),
        _ => panic!("expected a collapse"),
    }
}

fn deep(root: &Path, answer: impl Fn(usize) -> bool) -> apply::Done {
    let Plan::Deep(mut plan) = plan::deep(root).unwrap() else {
        panic!("expected a deep flatten");
    };
    for (nth, spot) in plan.collisions.clone().into_iter().enumerate() {
        if answer(nth) {
            plan.accept(spot);
        }
    }
    plan.refuse_shadowed();
    apply::deep(root, &plan, &mut silent()).unwrap()
}

#[test]
fn the_folder_inside_a_folder_is_undone() {
    let root = tree(&[
        "documents/documents/doc_1.txt",
        "documents/documents/doc_2.txt",
        "documents/documents/.env",
    ]);
    let target = root.path().join("documents");
    let done = collapse(&target);
    assert_eq!(done.moved, 3);
    assert_eq!(done.removed, 1);
    assert!(done.failures.is_empty());
    assert_eq!(listing(&target), [".env", "doc_1.txt", "doc_2.txt"]);
}

#[test]
fn a_whole_chain_of_wrappers_goes_in_one_move() {
    let root = tree(&["a/b/c/f.txt", "a/b/c/g.txt"]);
    let done = collapse(root.path());
    assert_eq!((done.moved, done.removed), (2, 3));
    assert_eq!(listing(root.path()), ["f.txt", "g.txt"]);
}

#[test]
fn a_wrapper_holding_its_own_name_is_moved_aside_first() {
    let root = tree(&["documents/x.txt", "documents/documents/"]);
    let done = collapse(root.path());
    assert_eq!((done.moved, done.removed), (2, 1));
    assert!(done.failures.is_empty());
    assert_eq!(listing(root.path()), ["documents/", "x.txt"]);
}

#[test]
fn a_collapse_lifts_directories_too_and_keeps_what_is_under_them() {
    let root = tree(&["wrap/sub/deep.txt", "wrap/f.txt"]);
    collapse(root.path());
    assert_eq!(listing(root.path()), ["f.txt", "sub/", "sub/deep.txt"]);
}

#[test]
fn a_collapse_stops_at_the_first_real_fork() {
    let root = tree(&["README", "sub/a.txt"]);
    assert!(matches!(
        plan::collapse(root.path()).unwrap(),
        Plan::Nothing
    ));
}

#[test]
fn a_collapse_has_nothing_to_do_twice() {
    let root = tree(&["wrap/f.txt"]);
    collapse(root.path());
    assert!(matches!(
        plan::collapse(root.path()).unwrap(),
        Plan::Nothing
    ));
}

#[test]
fn an_empty_directory_collapses_to_nothing_at_all() {
    let root = tree(&[]);
    assert!(matches!(
        plan::collapse(root.path()).unwrap(),
        Plan::Nothing
    ));
    assert!(matches!(plan::deep(root.path()).unwrap(), Plan::Nothing));
}

#[test]
fn an_empty_wrapper_is_removed_and_leaves_an_empty_target() {
    let root = tree(&["wrap/"]);
    let done = collapse(root.path());
    assert_eq!((done.moved, done.removed), (0, 1));
    assert!(listing(root.path()).is_empty());
}

#[test]
fn a_deep_flatten_brings_up_every_file_and_removes_every_directory() {
    let root = tree(&["README", "sub/a.txt", "sub/x/b.txt", "empty/"]);
    let done = deep(root.path(), |_| false);
    assert_eq!((done.moved, done.removed), (2, 3));
    assert_eq!(listing(root.path()), ["README", "a.txt", "b.txt"]);
}

#[test]
fn the_shallowest_entry_keeps_the_name_and_the_rest_are_asked_about() {
    let root = tree(&["notes.txt=top", "a/notes.txt=from-a", "b/notes.txt=from-b"]);
    let Plan::Deep(plan) = plan::deep(root.path()).unwrap() else {
        panic!("expected a deep flatten")
    };
    // The entry already in the target holds the name without moving, so both
    // of the buried ones have to ask.
    assert_eq!(plan.collisions.len(), 2);
    assert_eq!(plan.moves().count(), 0);
    assert_eq!(plan.holder(plan.collisions[0]), "notes.txt");
}

#[test]
fn declining_leaves_the_entry_and_its_directory_alone() {
    let root = tree(&["notes.txt=top", "a/notes.txt=from-a", "a/other.txt"]);
    deep(root.path(), |_| false);
    assert_eq!(
        listing(root.path()),
        ["a/", "a/notes.txt", "notes.txt", "other.txt"]
    );
    assert_eq!(
        fs::read_to_string(root.path().join("notes.txt")).unwrap(),
        "top"
    );
}

#[test]
fn accepting_replaces_what_was_there_and_empties_the_directory() {
    let root = tree(&["notes.txt=top", "a/notes.txt=from-a"]);
    deep(root.path(), |_| true);
    assert_eq!(listing(root.path()), ["notes.txt"]);
    assert_eq!(
        fs::read_to_string(root.path().join("notes.txt")).unwrap(),
        "from-a"
    );
}

#[test]
fn the_last_answer_is_the_one_left_there() {
    let root = tree(&["notes.txt=top", "a/notes.txt=from-a", "b/notes.txt=from-b"]);
    deep(root.path(), |_| true);
    assert_eq!(listing(root.path()), ["notes.txt"]);
    assert_eq!(
        fs::read_to_string(root.path().join("notes.txt")).unwrap(),
        "from-b"
    );
}

#[test]
fn an_entry_named_after_a_directory_on_its_way_out_gets_the_name() {
    let root = tree(&["sub/inner/keep.txt", "other/sub=iam-a-file"]);
    let done = deep(root.path(), |_| false);
    assert!(done.failures.is_empty());
    assert_eq!(listing(root.path()), ["keep.txt", "sub"]);
    assert_eq!(
        fs::read_to_string(root.path().join("sub")).unwrap(),
        "iam-a-file"
    );
}

#[test]
fn an_entry_inside_the_directory_it_is_named_after_still_gets_the_name() {
    let root = tree(&["sub/sub=nested"]);
    deep(root.path(), |_| false);
    assert_eq!(listing(root.path()), ["sub"]);
    assert_eq!(
        fs::read_to_string(root.path().join("sub")).unwrap(),
        "nested"
    );
}

#[test]
fn an_entry_named_after_a_directory_that_stays_is_refused() {
    let root = tree(&["keep.txt=top", "sub/keep.txt=buried", "z/sub=iam-a-file"]);
    let Plan::Deep(mut plan) = plan::deep(root.path()).unwrap() else {
        panic!("expected a deep flatten")
    };
    let refusals = plan.refuse_shadowed();
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].source, "z/sub");
    let done = apply::deep(root.path(), &plan, &mut silent()).unwrap();
    assert!(done.failures.is_empty());
    assert_eq!(
        listing(root.path()),
        ["keep.txt", "sub/", "sub/keep.txt", "z/", "z/sub"]
    );
}

#[cfg(unix)]
#[test]
fn a_linked_directory_is_moved_as_itself_and_never_walked_into() {
    let root = tree(&["wrap/real/deep.txt", "wrap/f.txt"]);
    std::os::unix::fs::symlink("real", root.path().join("wrap/link")).unwrap();
    deep(root.path(), |_| false);
    assert_eq!(listing(root.path()), ["deep.txt", "f.txt", "link"]);
    assert!(
        fs::symlink_metadata(root.path().join("link"))
            .unwrap()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn a_link_that_loops_back_is_not_a_walk_that_never_ends() {
    let root = tree(&["a/f.txt"]);
    std::os::unix::fs::symlink("../..", root.path().join("a/up")).unwrap();
    deep(root.path(), |_| false);
    assert_eq!(listing(root.path()), ["f.txt", "up"]);
}

#[test]
fn nothing_below_the_target_is_nothing_to_do() {
    let root = tree(&["a.txt", "b.txt"]);
    assert!(matches!(plan::deep(root.path()).unwrap(), Plan::Nothing));
}

#[test]
fn a_file_is_not_a_directory() {
    let root = tree(&["a.txt"]);
    assert!(path::require_directory(&root.path().join("a.txt")).is_err());
    assert!(path::require_directory(&root.path().join("missing")).is_err());
    assert!(path::require_directory(root.path()).is_ok());
}

#[test]
fn a_long_path_is_shown_from_the_end_that_names_the_file() {
    assert_eq!(truncate_front("sub/x/report.txt", 40), "sub/x/report.txt");
    assert_eq!(truncate_front("sub/x/report.txt", 12), "…/report.txt");
    // The mark counts towards the room it was given, so a row stays a row.
    assert_eq!(truncate_front("sub/x/report.txt", 12).chars().count(), 12);
}
